# Writing a nori client

The Rust core (`crates/`) is meant to be reused whole: a desktop app or a terminal client links it and
writes only what is specific to its platform. This page lists what the core already does, and what every
client has to build itself. The Android app (`app/`, `core/`) is the reference for each item.

One rule decides where a piece of work goes, and it outranks the split below: **speed, CPU, memory,
wakeups and battery come first.** Something that measures faster on the client side (animation maths
called every frame, for example) belongs in the client even when it looks like "logic".

## What the core does

Link these and call them; do not reimplement them.

| Area | Crate / module |
|---|---|
| Server API, sync, the one app database (`nori.db`), settings with their schema and effects | `nori-core`: `api`, `client`, `db`, `settings_store`, `settings_schema` |
| Library, search, browse, mixes, smart lists, stats, history, favourites, playlists, M3U | `library`, `search`, `browse`, `mixes`, `smart`, `history`, `stars`, `m3u` |
| What each page shows, its menus and buttons, and every word on screen, pluralised and localised | `pages`, `menus`, `actions`, `words`, `fmt`, `nori-text` |
| The queue: order, shuffle, autofill, radio, the offline bridge, error runs | `nori-player::playlist`, `queue`, `autofill`, `bridge`, `rules` |
| Decoding MP3, FLAC, AAC-LC, Vorbis, ALAC and Opus, gapless, without allocating per packet | `nori-player::decode` |
| The sound: EQ, pre-amp, limiter, balance, mono, crossfeed, ReplayGain, speed and pitch, silence skipping | `nori-player::dsp`, `sonic`, `speed`, `silence`, `stages` |
| Transitions: crossfades, AutoMix analysis, planning and mixing | `nori-player::engine`, `transitions`, `automix` |
| The playhead, seeks, fades on play/pause/skip, and when the player may sleep | `nori-player::heard`, `seek`, `transport`, `burst` |
| **The whole player** for a platform without one: loading in bursts, demuxing, decoding, the sound chain, transitions, gapless, seeks, fades, the queue walked, events for a screen | `nori-engine` (over `nori-player::pipeline`); with its `core` feature it plays the core's queue, planner and settings |
| A desktop sound card; HTTP on the desktop; the Linux desktop's media controls | `nori-output-cpal`; `nori-http` (the core's `Transport` and the engine's `ByteSource`); `nori-mpris` |
| The stream cache and downloads on disk, and measuring the songs ahead for AutoMix, for a client without a platform player | `nori-engine::store` (`Store`), `nori-engine::core` (`CoreOrder`, `Downloader`, `Measurer`) |
| Output devices: naming, ranking, per-device sound profiles, AutoEQ curves, bit-perfect decisions | `nori-player::outputs`, `device`, `dac`; `profiles`, `autoeq` |
| Downloads and stream cache bookkeeping: what is stored, what to fetch next, what to evict | `transfers`, `stream_cache`, `cache_policy` |
| Scrobbling decisions, lyrics (server and LRCLIB) with parsing and the current line | `scrobble`, `lrclib`, `lyrics`, `nori-look::lyrics` |
| What a song that will not play says, the credits (the core's crates, Android's libraries, the typeface and the third parties' data), About's lines | `words::words_playback_error`, `settings_schema::core_credits`, `android_credits`, `data_credits`, `words::words_about_android` |
| A perf recorder's bookkeeping: the state a stretch is filed under, what two readings of the counters make (the threads that woke most among them), the stretches kept, their sums by state, the page's figures, the audio output's line and the shared report | `perf_log` |
| Cover colours: palette, theme, the page's colour scheme, the wash and melt behind the player | `nori-look::cover`, `palette`, `theme`, `dress` |
| Cover art: fetched through the `Transport`, kept on disk and in memory, JPEG, PNG, WebP and a GIF's first frame decoded straight to the size drawn and turned as their EXIF says (Android's Bitmaps included) | `nori-covers` |
| The car browse tree | `car` |

## What each client builds

### 1. Talking to the network
- **HTTP transport:** implement the core's `Transport` trait (`crates/core/src/transport.rs`), or on a
  desktop link `nori-http`, which implements it (and the engine's `ByteSource`) over ureq. It covers
  TLS, self-signed servers, client certificates and the headers the profile asks for. Ask
  `request_policy` once per host (cache it) to learn which requests go to the server.
  Android: `net/Http.kt` on OkHttp. Keep API calls, covers and audio on one HTTP/2 connection, so the
  radio wakes once.
- **Network state:** tell the core when the network changes (metered or not, gone or back), for the offline
  bridge and the "unmetered only" setting. Android: `playback/OfflineBridge.kt`.

### 2. Playing audio
A platform without a player of its own (a desktop app, a terminal client) links `nori-engine` and writes
only what touches the hardware:
- **Output:** implement `nori_engine::AudioOutput` - open a device, and from its own thread call
  `Feed::pull` for every buffer it plays (lock-free, allocation-free), say whether it plays float
  (`takes_float`, for high quality output) and, where the platform can tell, which device the music goes
  to (`watch`) - or use `nori-output-cpal` (PipeWire/ALSA, CoreAudio, WASAPI; device changes on
  PipeWire, CoreAudio and WASAPI). `WavOutput` renders to a file instead (16-bit, or float with
  `in_float`), on a clock of its own. An output whose device holds seconds of music (Android's
  AudioTrack) also runs the fades at its own volume (`ramp`), empties the device when the ring is
  flushed (`flush`, with `Feed::flushed` saying which pull starts the new music) and says what it still
  holds (`holding`), so the music is not over while it plays out.
- **Bytes:** a `ByteSource` (a GET from a byte offset) for streamed songs; `nori-http` has one. Local
  files need nothing.
- **Disk:** a directory for `nori_engine::Store`: the stream cache (written while a song loads, read
  back when it is whole, held to the "space for streamed music" setting in the core's eviction order
  through `CoreOrder`) and downloads (fetched by `core::Downloader` from the core's download queue, with
  its progress and phases in `transfers`). `CoreLibrary` with the store plays a download or a whole
  cached copy from the disk before the network is asked.
- **Driving it:** `Engine::start(CoreLibrary, CoreApp, CoreQueue, output, ..)` with the `core` feature
  plays the core's queue with the core's planner, analysis store, settings (`core::settings`), stream
  addresses, ReplayGain, error run and "skip explicit songs". `CoreApp::measuring(Measurer)` measures the
  songs ahead for AutoMix, and `per_device(core)` gives each output device its own sound. Edit the queue
  through the core's `playlist_*` calls and tell the engine (`queue_changed`); the controls are
  `play_at`, `play`, `pause`, `next`, `previous`, `seek`, `go_to`, `set_settings`, `replan`,
  `set_repeat`, `gain_changed`, `set_tuning` (the equalizer screen's shallow buffer) and `pause_at_end`
  (the sleep timer's "end of this song"). The player's own rules come with them: a seek or a `go_to`
  while paused is held until play and fetches nothing, and a skip button while paused is a request for
  music (`nori_player::transport::skip_plays`). A screen follows `Event`s (state, the song heard -
  through a mix, the moment the next song is the louder - errors, the output device, a network stall
  heard as buffering, playback stopping by itself, and positions only when asked for) and reads
  `Status` at any time without waking the engine, the sound chain and the limiter's meter included.

The engine does, from the core's decisions, everything ExoPlayer does around the Rust on Android: it
loads each song in bursts per `load_control` (the whole song in one request when it fits, the next one
fetched as the one before starts, the network left alone in between), demuxes with symphonia's format
readers, decodes with `nori-player::decode`, cuts the encoder's delay and padding so songs join sample
for sample (an MP4's from its `iTunSMPB` or edit list, as media3 reads them), runs the sound chain, speed
and pitch and silence skipping, holds and mixes endings through the transition engine, fades on play,
pause and switches, plays each song at its ReplayGain volume (changing on the song's first sample), walks
the queue (next, previous, repeat, explicit songs skipped, the error run, a failed connection reported)
and keeps the ear's playhead. High quality output carries float from the decoder to a device that plays
float, with nothing touching the samples, as Android's does. A song still on its way is opened off the
engine's thread, and a long pause lets the output and the song's bytes go (the core's idle release) and
opens them again where it was. It is `nori-player::pipeline`, the code the simulated player runs, on one
thread that sleeps between bursts (its wakeups are listed in `crates/engine/src/engine.rs`).

Not in the engine yet, compared with the Android player: audio offload and bit-perfect output (a USB
DAC opened at the file's own format), the precacher (the next song is fetched as the one before starts,
but not the ones after it), the offline bridge, AutoEQ curves offered for a new device (the core's
`DeviceArrival` names one; fetching and offering it is the client's), the network's metered state (the
engine streams the unmetered quality unless told), and symphonia's readers still allocate a buffer per
packet (their API has no way to read into one kept).

Android can play through the engine too, as a second path to measure against ExoPlayer ("Playback
engine" in the sound settings, read when the playback service starts; `tools/app.sh engine rust`).
`crates/android/src/track.rs` is the engine's output there: a thread of its own pours the ring into an
AudioTrack whose buffer holds one of the engine's bursts and a second and a half more, in the
power-saving mode, waking when a second is left and taking everything the ring has, so the engine is
woken in the same moment - both about every ten seconds while music plays, like the ExoPlayer path's
deep buffer; the engine keeps no timer of its own for the ring then (`AudioOutput::bursts`). Fades
run at the track's volume and a flush empties the track, since seconds of music sit in it
(`AudioOutput::ramp`, `flush`, `holding`); a track that dies is opened again, and one that will not
open is the engine's to hear of (`AudioOutput::failed`). A song's address and cache key are the core's
(`stream::resolve_now`, over the network state Kotlin tells it, `network_metered`), and its bytes come
through media3's data sources on the app's OkHttp client (`RustBridge.open`), so the profile's TLS,
certificates and headers apply and the downloads, the stream cache and the precacher are the ExoPlayer
path's; the queue is the core's (`CoreQueue`, `CoreApp`), and `EnginePlayer` (`RustPlayer.kt`) is a
media3 player over it for the session, the notification, Android Auto and the widget. Not on that path
yet: audio offload, the offline bridge, internet radio, and bit-perfect output (a USB DAC gets its mixer
attributes at the song's rate, but ReplayGain and the sound chain still touch the samples).

By default Android keeps ExoPlayer for loading, demuxing and output, around the same Rust:
- **Loading and buffering:** fetch bytes (through the stream cache or a download), demux the container into
  packets, and keep a buffer ahead. Follow the core's `load_control` numbers: fill in one burst, then leave
  the network alone. Android: ExoPlayer (`playback/MediaSources.kt`, `PlaybackService.kt`).
- **Feeding the core:** hand packets to `nori-player::decode`, PCM to the stages and DSP chain, and let the
  transition engine hold and mix track ends. Android: `playback/RustAudio.kt`, `Stages.kt`, `Equalizer.kt`,
  `TransitionSink.kt`, over direct buffers.
- **Output:** a platform audio sink (AudioTrack on Android). Report output device changes so the core can
  pick the device's sound (`playback/Outputs.kt`, `DeviceSound.kt`), and, where the platform can, open the
  device bit-perfect (`playback/BitPerfect.kt`).
- **Mirroring the queue:** the core owns the queue. If the platform player keeps its own list (ExoPlayer
  does), route every edit through the core and apply the `QueueEdit` it answers. Never edit the platform
  list directly. Android: the `Controls` forwarding player in `PlaybackService.kt`.

Every client schedules the background work - the precacher, AutoMix measuring and downloads - on the
platform's threads or jobs, when the core says to (`Precacher.kt`, `AutoMixPrefetch.kt`, `downloads/`).

### 3. The operating system around the player
- Media controls and "now playing": the Android media session and notification, MPRIS on Linux
  (`nori-mpris`: a client says what the controls do and when something changed), System Media Transport
  Controls on Windows, MPNowPlayingInfoCenter on macOS.
- Headset buttons, audio focus and ducking, sleep and idle release. The core gives the timings
  (`rules.rs`); the client wires the events.
- Car integration (Android Auto: `PlaybackService.kt` browse), widgets (`app/PlayerWidget.kt`).
- File storage for downloads and the stream cache. Android uses media3's `DownloadManager` and
  `SimpleCache`; a client over `nori-engine` gives `Store` a directory. The core decides what goes in and
  out.

### 4. Pictures
- Draw cover images. Fetching, caching and decoding are `nori-covers`', on every client, Android
  included: a client asks for a cover at a view's size and draws what comes back.
- Hand decoded pixels to `nori-look::cover::derive`, which returns the page's colours and the wash
  picture. Draw them; never recompute them. Android has the core do both in one call (below).

`nori-covers` (`crates/covers`) is platform-free:
- **`Loader`** (`Loader::new(Config, Arc<dyn Transport>)` for RGBA, `Loader::with_paint(.., paint)` for a
  platform's own pictures): `request(url, width, height, done)` answers on one of a few worker threads
  (or at once, from memory) with the cover decoded to fill `width` x `height` the way a cover is drawn
  (the middle kept, the overhang cut); 0 x 0 is the file's own size, at most 2048 a side (a larger file
  is shrunk in its shape; one past 16 MP is not decoded at all). The `Ticket` it returns cancels the
  request when dropped (a row scrolled away); `detach` lets it run on. Views asking for one cover at one
  size while it is on its way share one fetch and one decode, and the newest request is served first, so
  the covers on screen now come before the rows flung past. `warm(url)` fetches onto the disk only,
  behind every view's request (a download's covers); `read(url, bytes)` hands the file itself over on the
  calling thread (a page's colours). `cached(url, w, h)` is the memory cache alone, for drawing at once;
  `load` waits on the calling thread. Workers start with the first requests, at most `Config::workers`,
  and sleep when idle; nothing touches the disk on the thread that asks, not even opening the cache.
- **`Paint`**: what a cover's file becomes, on the worker that fetched it. `Rgba` is tight RGBA rows at
  exactly the size asked for (an `Arc<Image>`); Android's paints a Bitmap (below).
- Addresses are the core's (`cover_url_into`, or `Core.coverUrl`), so every client asks the server for
  the same renditions. Provider covers (`is_provider_cover`) are never written to disk.
- **`DiskCache`**: the server's bytes, one file per address (its MD5) in the directory the client
  names, under `Config::disk_bytes` (the core's `cover_rules`), least recently used out first. The index
  is rebuilt from the directory when it opens, and a read sets the file's modification time, so the order
  survives a restart without being state in the app's database. A file that does not decode is deleted
  and fetched again next time; one in a format the decoder does not know is kept.
- **`MemoryCache`**: decoded covers by address and size, under a limit in bytes, least recently drawn
  out first. `trim_memory` lets them all go. 0 bytes keeps none, for a client that keeps its own.
- **`Decoder`**: `decode_into(bytes, Target { px, width, height, stride }, alpha)` writes RGBA straight
  into the caller's rows (padded rows allowed, as a Bitmap or a texture upload buffer has them), straight
  or premultiplied, turned or mirrored the way the file's EXIF says (JPEG APP1, PNG eXIf, WebP EXIF,
  found by walking the container, nothing decoded). A picture already that size is decoded into them with
  no copy between; anything else is decoded whole into a buffer the decoder keeps and filtered in: an
  exact area average shrinking, bilinear growing, in fixed point, two passes, nothing allocated once the
  buffers have grown. JPEG is zune-jpeg (the fastest pure-Rust decoder, SIMD on x86 and NEON), except
  that a JPEG at least twice the size drawn may go to jpeg-decoder, whose IDCT decodes at 1/2, 1/4 or 1/8
  of the size (what Android's `inSampleSize` does in libjpeg-turbo) - about a third faster on a desktop,
  with the pixels a step or so from the exact average; `set_idct_scaling(false)` decodes those whole
  instead. PNG is the `png` crate, WebP `image-webp`, GIF the `gif` crate: its first frame, on its screen,
  with what the frame leaves uncovered transparent (a cover is a still picture). Pictures over 16384
  pixels a side are refused. `header(bytes)` reads a file's format, its size as shown and its
  orientation from the headers alone; `Header::fill` is the size to decode to for a view (never grown
  past the file's pixels).
- **Not decoded: HEIF/HEIC and AVIF.** HEVC has no decoder in pure Rust. AV1 has one (rav1d), but
  without its assembly (which would need nasm and the NDK's assemblers in the build) it added about
  1.5 MB to the library and decoded a 320 px cover in 6.3 ms and an 800 px one in 38 ms on a desktop,
  six times a JPEG's time, for a format a Subsonic server does not send when it is asked for a size (it
  resizes to JPEG or PNG). Such a cover is `DecodeError::Unknown`: its view keeps its placeholder.

Android has no image library: Coil is gone, and there is no fallback to Android's decoders. The core
fetches (through the app's own `Transport`, so covers ride the API's HTTP/2 connection), keeps the files
on disk (`cacheDir/art`, `cover_rules`' size) and decodes; Kotlin keeps the Bitmaps it is handed and
draws them.

- **The door** (`dev.nori.music.look.CoverPixels`, crates/android/src/covers.rs): `open` makes a loader
  (cheap: the directory is read by its first thread) whose `Paint` decodes each cover straight into a
  Bitmap made at the size `Header::fill` says, from a loader thread attached to the JVM: RGB_565 for a
  JPEG (the core packs it), ARGB_8888 premultiplied otherwise, `setHasAlpha(false)` for a JPEG so drawing
  skips blending it. From Android 9 the screens get hardware Bitmaps, as they did from Coil: the picture
  is decoded into a software Bitmap the thread keeps for the next cover (reconfigured, not made again;
  let go past 512x512) and copied to the GPU there, so no frame pays the upload. `request(loader, url, w,
  h, waiter)` answers a handle; the core calls `waiter.done(bitmap, status)` once, on the loader thread
  that finished it, and `cancel(handle)` (`@FastNative`) drops the core's ticket, after which it is not
  called for a cover finished later (one finished as it is cancelled may still arrive, and Kotlin drops
  it: `cancel` does not wait for a call back under way, which would be a `@FastNative` door waiting on
  Java). `warm` and `clear` are the loader's; `colours` is the page's colours, below. The transport is
  the one the app hands the core (`set_cover_transport`, where `Nori` builds it on the warm-up thread);
  a cover that reaches the network before that waits for it on its loader thread, never on the main one.
- **`CoverLoader`** (core/.../data): the app's one loader, and the Bitmaps' memory cache. That cache
  has to be Kotlin's: a Bitmap is a Java object, and the core holding a reference to every one would keep
  it from the collector without knowing when a view has let it go. It is an LRU by bytes
  (`allocationByteCount`), per cover address, sized by `cover_rules`' share of the memory class as Coil's
  was, trimmed as Coil's was when the system asks. Each address keeps its largest picture, and whether it
  is the file's whole picture: a row's thumbnail is drawn from the grid's larger one rather than decoded
  again, and a view that needs more is shown what is kept while the larger one comes. A request posts
  its call back to the main thread (`Handler.post` of the request itself: nothing else allocated) and
  keeps the picture there. `prefetch` decodes at the file's own size (at most 2048 a side) into memory
  (the library's next screenful, the player's neighbours); `warm` is disk only (a download's covers, hundreds of them, which
  would push the screen's covers out of memory). Provider covers are never kept.
- **`Cover` and `rememberCover`** (app/.../ui): the one component every screen draws a cover with. It
  asks at the view's own pixel size (a view sized by its layout asks once measured), draws the kept
  picture at once, otherwise the plate, its sheen and then the picture faded in over 260 ms; a picture
  that never comes leaves the note glyph, faded in. The picture is drawn in the draw phase, the middle of
  it in the view's shape as `ContentScale.Crop` did, and the fade is read there too, so it recomposes
  nothing. Leaving composition cancels the request. The player's sleeve and its neighbours take the same
  `CoverImage` as a painter.
- **The page's colours** (`CoverLoader.colours`, the door's `colours`): the core reads the cover's file
  (disk or network), decodes the whole picture to fit 320 px in straight colours and hands the pixels to
  `nori_look::cover::derive`, writing the look into an int array and the wash into a Bitmap: no Bitmap
  of the cover in between, and no round trip through premultiplied pixels. One call works out the page
  in the plain theme and on AMOLED black from the one decode, where the bar is black and the player
  keeps the record's colours.

The debug build measures the core's covers (the perf build runs the same from its Performance page,
docs/perf-build.md):

```sh
adb shell am broadcast -a dev.nori.music.TEST --es cmd coverbench --es arg 40
adb logcat -s noritest   # one line when done
```

`coverbench` takes up to that many covers from the disk cache (`cacheDir/art`) and decodes each to
300x300 and 1080x1080 into one reused Bitmap: ARGB_8888 with the IDCT shrinking and without, and RGB_565.
Then it loads up to that many covers the app has shown (the addresses kept in memory) from the disk
through a loader of its own at 300 px, all asked for at once as a screenful is, once into software
Bitmaps and once into hardware ones, so the GPU copy's cost is there to see. For each: total and
per-cover time, the Java heap allocated per cover (`art.gc.bytes-allocated`), GCs, and the Java and
native heaps before and after.

### 5. The interface
- Every screen, its layout and text rendering. Take the words and page contents from the core.
- **Where things sit on the screen.** The core gives the gradients' stops, the timings and the colours;
  the boxes are the platform's. Android's player sleeve is 0.74 wide for 1 tall and runs 9.5 % of its
  height under the title (`PlayerScreen.kt`), tuned for a phone held upright; the interface is scaled
  to a phone 411 dp wide when Android's display size is set large (`Theme.uiScale`). A desktop window
  lays its player out and sizes its text otherwise.
- **Animations, gestures and transitions.** Their feel is per platform: easing, springs, flick
  thresholds, swipe maths. Android's are in `app/ui/` (`PlayerScreen.kt`, `Chrome.kt`, `Components.kt`),
  described in `docs/motion.md`.
- **A perf recorder** (optional): read the platform's counters (CPU time, every thread's name, CPU time and
  context switches, heap, memory, the battery's counter and gauge, frames, the network bytes, and the audio
  output as the platform describes it against what was asked of it) at each change of state, and hand them
  to `perf_log` (`perf_state`, `perf_stretch`, `perf_log_add`, `perf_page`, `perf_report`). Android's is
  the perf build (`app/src/perf`, docs/perf-build.md).
- Locale: pass the platform's decimal and grouping separators to `fmt_set_locale` once.
- Drawing cost: redraw only when something visible changes. The Android player draws the seek bar once
  per pixel and the times from pre-laid-out glyphs; a paused player draws nothing.

## Calling the core cheaply
- `crates/cli` (nori-cli) is a whole terminal client in a few hundred lines: arguments, commands and
  printing, and nothing else.
- Rust clients call the crates directly. The core (`crates/core`, package `nori-core`, lib
  `nori_core`) is a plain rlib with no JNI in it, and its uniffi exports sit behind the default `ffi`
  feature: depend on it with `default-features = false` and nothing of uniffi is built. Everything the
  Android doors call is ordinary Rust there - `dsp::SoundChain` over sample slices, `heard::HeardClock`,
  `automix::store::AnalysisStream`, `automix::host::CoreHost` for the transition engine, the download
  tracker's functions in `transfers`, words written into a buffer or lent to a closure.
- Other languages go through uniffi bindings for calls made on user actions. For anything called per
  frame, per buffer or per list row, use a thin native door with primitives in and out and no allocation.
  Keep those doors in a crate of their own, as Android does: `crates/android` builds libnorimusic.so from
  the core (with its uniffi scaffolding) and the JNI doors, which only convert arguments and call the core.
- Android's uniffi bindings are uniffi's JNI generator (`uniffi-bindgen-kotlin-jni`), not its JNA one: a
  JNA call measured 10-25 µs and 1.5-4 KB of garbage in a release-like build, where a JNI door costs about
  8 ns. `crates/android/build.rs` generates the scaffolding into the library, and
  `cargo run -p uniffi-bindgen -- bindings src:nori-android <dir>` writes the Kotlin. A thread the core
  starts may call into Kotlin (a callback, a future it wakes): the runtime (`crates/uniffi-jni-runtime`)
  attaches it once, under the thread's own name (attached without one, the JVM renames it "Thread-NN"),
  detaches it when it ends, and finds the app's classes from it through the class loader
  `JNI_OnLoad` handed over, since `FindClass` on such a thread only sees the system's. crates/android's
  own doors attach their threads the same way (`attached` in lib.rs).
- On Android every door is registered in `JNI_OnLoad` with `RegisterNatives` (none is looked up by a
  `Java_` symbol). A door whose Kotlin signature is primitives only is `@CriticalNative` - its Rust
  function takes no `JNIEnv` and no class, and Android 8 to 11 only honour that for registered methods - and
  a short door over arrays or direct buffers, or one that answers a short string (the equalizer's
  figures, a row's download line, the Rust player's `eventText`), that calls nothing back and waits on no
  lock another thread holds for long is `@FastNative`. Every raw pointer (a direct buffer's address, array elements, Bitmap pixels) is checked
  for null and against its length before a slice is made of it.
- Measure before moving work across the boundary. The Android debug build's `bench` test command prints
  what each kind of crossing costs next to the same work in Kotlin.

## Measured: what stays where

Where a job lives is decided by measurement, never by where it looks like it belongs. Measured on the
Android emulator, non-debuggable build (a debuggable build turns off the fast JNI paths and reads 5-10x
slower), with the debug bridge's `bench` command:

| Work | Kotlin | Rust (crossing included) | Where it lives |
|---|---|---|---|
| Seek bar step (easing, once-a-pixel pacing) | 14-20 ns | 8 ns (`@CriticalNative`) | Rust (`nori_look::motion`) |
| One cover-colour blend frame, all 52 colours | 42 µs (Compose `lerp`) | 16 µs | Rust (`nori_look::dress::mix`) |
| A time label ("3:07") | - | 0.14 µs, one 24-byte string | Rust over JNI, cached per second |
| A small uniffi call | - | 0.1-0.3 µs, no garbage (uniffi's JNI bindings; JNA was 10-25 µs and 1.5-4 KB) | Rust |
| Decoding a packet | MediaCodec: a hop to the codec process, framework buffer objects per packet | in-process, nothing allocated | Rust (`nori_player::decode`) |
| Decoding a cover to 300 px (40 real covers, Galaxy S22, arm64, Android 16) | BitmapFactory as Coil drove it: 1.71 ms | `nori-covers`: 0.93 ms decoded whole (0.98 ms with the IDCT shrinking) | Rust (`nori-covers` fetches, keeps and decodes; Kotlin keeps the Bitmaps and draws) |
| Decoding a cover to 1080 px (the player), same phone | 9.48 ms, native heap up 14-178 MB during the run | 2.80 ms either way, native heap flat | Rust; the pixels are practically BitmapFactory's (mean difference under 0.25/255) |

Kept in the client (Kotlin on Android), because crossing would cost more than the work:
- **Per-frame gesture and animation maths**: drag offsets, spring and easing values, flick velocity
  tracking, sleeve lift and scale. Each is a few multiplications Compose already does inline (about
  1 ns); a crossing costs 8 ns at best, and the numbers are consumed by Compose on the same frame. The
  decisions a gesture ends in (which way a swipe turns, where the sheet settles, the flick speeds, how
  far a drag must go, the give towards a record that is not there, how far back takes the player, how
  small a held record gets) stay beside it too (Chrome.kt, PlayerScreen.kt, PlayerSheet.kt,
  Components.kt): gestures are mobile UI, and a desktop or terminal client has other input.
- **Drawing**: layout, text measurement, the glyph-cached seek times, bitmaps. The core says what to draw
  and with which colours; the platform draws it. Phone-only layout lives here too, with no twin: the
  sleeve's box (`PlayerScreen.SLEEVE`, `SLEEVE_UNDER_TEXT`) and the interface's scale for a large display
  size (`Theme.uiScale`), both of which the core carried once.
- **Threading of the platform player**: decoding runs synchronously on media3's playback thread
  (`RustAudioDecoder`), because a decoder thread of its own cost two wakeups a packet (24 wakeups/s
  screen-off, now 5).

Anything moved across the boundary must come with a measurement showing it is at least as fast, and this
table updated.

## Twins: logic Android keeps in Kotlin, and the core has too

Where Android keeps a piece of logic in Kotlin - because it runs on the platform's own objects, or where a
crossing costs more than the work (string building per list row, keys read while drawing) - the core
carries a twin with the same answers, so a desktop or terminal client gets the rule without porting it.
A twin is never deleted for being slower on Android, and nothing a client calls per row, frame or buffer
allocates (`crates/core/tests/twins.rs` and `crates/look/src/no_alloc.rs` count).

Each twin is held to its Kotlin original by test vectors: `tools/twins.sh` compiles the generators in
`crates/<crate>/testdata/twins/*.kt` - the original functions copied as they are, with the Android or
media3 class they lean on written out (AOSP's `Uri.encode`, `MutableTransitionState` as two booleans) -
with the Kotlin compiler the app builds with, runs them on the JVM and writes the `.tsv` beside them;
`crates/<crate>/tests/twins.rs` must match every row exactly (floats go in as their bits). When an original
changes, copy it into its generator again and run `tools/twins.sh`.

None of these is called by Android: each Kotlin original stays, since it is either glue around a
platform object or cheaper than a crossing. They are for other clients.

| Kotlin original | Rust twin |
|---|---|
| `Covers.isProvider` (data/Library.kt) | `nori_core::covers::is_provider_cover` |
| `Library.coverUrl` with `Uri.encode` (data/Library.kt) | `covers::cover_url_into` (into a kept buffer) |
| `SearchViewModel`'s live search debounce, `isBlank` | `search::live_delay_ms`, `search::kotlin_whitespace` |
| AutoEQ fetches: `Http.get(..).decodeToString()` in `SettingsViewModel.downloadAutoEqIndex`, `DeviceSound.adopt` | `autoeq::fetch_text`, `autoeq::text` (the JVM's UTF-8 repair) |
| `AutoMixPrefetch.onDevice`, `AutoMixPrefetch.update` | `automix::ahead::whole_on_device`, `automix::ahead::plan` |
| `ResizableEvictor.trimLocked` (playback/MediaSources.kt) | `stream_cache::trim` |
| `PlayerConnection.publish`'s heard row, `PlayerConnection.read` | `heard::shown_index`, `heard::HeardAt::unpack` |
| `PlayerViewModel.setVolumeFraction`, `volumeFraction` | `rules::volume_step`, `rules::volume_fraction` |
| The car browser's paging (`PlaybackService.onGetChildren`, `onGetSearchResult`) | `car::page` |
| `downloadEntry`'s missing songs (ui/DetailScreens.kt) | `menus::download_missing` |
| `AnimatedRows` (ui/DevicesSection.kt): rows kept, new, leaving in place | `rows::merge_rows` |
| `setupData` (AutoMixPrefetch.kt, RustAudio.kt) | `nori_player::packets::join_setup` |
| AAC `codecs`, packet buffers (`AutoMixPrefetch.inCore`, `RustAudioDecoder`) | `packets::aac_codecs`, `packets::packet_bytes`, `packets::grown` |
| `TransitionSink.configure`: the encoding, the formats kept by token | `nori_player::sink::sample_encoding`, `sink::formats_below` |
| `paletteKey` (ui/CoverColors.kt) | `nori_look::cover::palette_key` (into a kept buffer) |
| `BandEffect.of`'s key (ui/PlayerScreen.kt) | `nori_look::sleeve::band_key` |

What stays in the client with no twin:
- **Touch gestures and motion**: sleeve and carousel drags, row swipes, the sheet and queue drags, the
  springs and easings Compose runs, flight transforms, the playing bars and loading dots. How a platform
  moves is its own; a desktop or terminal client has other input and its own animation.
- **Glue around platform objects**: media3's player, sink, renderer and processors (`Stages.kt`,
  `Equalizer.kt`, `RustAudio.kt`'s buffer queues), OkHttp (the TLS setup, exception kinds, the per-host
  cache of `request_policy` answers, which only saves crossings), `ConnectivityManager`, `AudioManager`,
  notifications, SharedPreferences carry-overs, the covers' Bitmap memory cache (its trim levels and
  "each address keeps its largest picture": Bitmaps are Java objects), `EnginePlayer`'s media3 item list
  (a mirror of the core's queue for the session; a client on nori-engine has the queue itself), the
  sorting of media3's error codes and exceptions into the core's kinds (`failureKind`, `isNetworkish`,
  the 5000s as the output), and the uniffi/JNI wrappers that unpack what the core packed (`LyricsClock`,
  `Stages.fadeVolume`).
- **The perf build's counters**: reading `/proc`, `BatteryManager`, `Debug.MemoryInfo`, `TrafficStats`,
  the AudioTrack the player opened (`PlaybackService.track`) and `FrameMetrics`, and counting frames as they
  are drawn. What a stretch is, which threads it names and how it is said are the core's (`perf_log`).
- **The debug build's tools**: the test bridge's verbs (`ActionsViewModel.testAction`, `TestBridge.kt`) and
  the benchmarks (`app/src/bench`), which measure Android's own crossings and Bitmaps.
- **Drawing**: layout, glyph widths, gradients' brushes, slider widget geometry, the player's sleeve box
  and the interface's scale.
