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
| Server API, sync, the one app database (`nori.db`), settings with their schema and effects | `norimusic-core`: `api`, `client`, `db`, `settings_store`, `settings_schema` |
| Library, search, browse, mixes, smart lists, stats, history, favourites, playlists, M3U | `library`, `search`, `browse`, `mixes`, `smart`, `history`, `stars`, `m3u` |
| What each page shows, its menus and buttons, and every word on screen, pluralised and localised | `pages`, `menus`, `actions`, `words`, `fmt`, `nori-text` |
| The queue: order, shuffle, autofill, radio, the offline bridge, error runs | `nori-player::playlist`, `queue`, `autofill`, `bridge`, `rules` |
| Decoding MP3, FLAC, AAC-LC, Vorbis, ALAC and Opus, gapless, without allocating per packet | `nori-player::decode` |
| The sound: EQ, pre-amp, limiter, balance, mono, crossfeed, ReplayGain, speed and pitch, silence skipping | `nori-player::dsp`, `sonic`, `speed`, `silence`, `stages` |
| Transitions: crossfades, AutoMix analysis, planning and mixing | `nori-player::engine`, `transitions`, `automix` |
| The playhead, seeks, fades on play/pause/skip, and when the player may sleep | `nori-player::heard`, `seek`, `transport`, `burst` |
| **The whole player** for a platform without one: loading in bursts, demuxing, decoding, the sound chain, transitions, gapless, seeks, fades, the queue walked, events for a screen | `nori-engine` (over `nori-player::pipeline`); with its `core` feature it plays the core's queue, planner and settings |
| A desktop sound card; HTTP on the desktop | `nori-output-cpal`; `nori-http` (the core's `Transport` and the engine's `ByteSource`) |
| Output devices: naming, ranking, per-device sound profiles, AutoEQ curves, bit-perfect decisions | `nori-player::outputs`, `device`, `dac`; `profiles`, `autoeq` |
| Downloads and stream cache bookkeeping: what is stored, what to fetch next, what to evict | `transfers`, `stream_cache`, `cache_policy` |
| Scrobbling decisions, lyrics (server and LRCLIB) with parsing and the current line | `scrobble`, `lrclib`, `lyrics`, `nori-look::lyrics` |
| Cover colours: palette, theme, the page's colour scheme, the wash and melt behind the player | `nori-look::cover`, `palette`, `theme`, `dress` |
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
  `Feed::pull` for every buffer it plays (lock-free, allocation-free) - or use `nori-output-cpal`
  (PipeWire/ALSA, CoreAudio, WASAPI). `WavOutput` renders to a file instead, on a clock of its own.
- **Bytes:** a `ByteSource` (a GET from a byte offset) for streamed songs; `nori-http` has one. Local
  files need nothing.
- **Driving it:** `Engine::start(CoreLibrary, CoreApp, CoreQueue, output, ..)` with the `core` feature
  plays the core's queue with the core's planner, analysis store, settings (`core::settings`) and stream
  addresses. Edit the queue through the core's `playlist_*` calls and tell the engine
  (`queue_changed`); the controls are `play_at`, `play`, `pause`, `next`, `previous`, `seek`,
  `set_settings`, `replan`, `set_repeat`. A screen follows `Event`s (state, the song heard - through a
  mix, the moment the next song is audible - errors, and positions only when asked for) and reads
  `Status` at any time without waking the engine.

The engine does, from the core's decisions, everything ExoPlayer does around the Rust on Android: it
loads each song in bursts per `load_control` (the whole song in one request when it fits, the next one
fetched as the one before starts, the network left alone in between), demuxes with symphonia's format
readers, decodes with `nori-player::decode`, cuts the encoder's delay and padding so songs join sample
for sample, runs the sound chain, speed and pitch and silence skipping, holds and mixes endings through
the transition engine, fades on play, pause and switches, walks the queue (next, previous, repeat, the
error run) and keeps the ear's playhead. It is `nori-player::pipeline`, the code the simulated player
runs, on one thread that sleeps between bursts (its wakeups are listed in `crates/engine/src/engine.rs`).

Not in the engine yet, compared with the Android player: audio offload and bit-perfect output (it always
plays 16-bit through the chain), ReplayGain (the core's `playlist_gain` is there to apply as volume),
the stream cache and downloads on disk, measuring the songs ahead for AutoMix (songs are measured as they
play), gapless trims from MP4 edit lists, and output device changes reported to the core.

Android keeps ExoPlayer for loading, demuxing and output, around the same Rust:
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
- Media controls and "now playing": the Android media session and notification, MPRIS on Linux,
  System Media Transport Controls on Windows, MPNowPlayingInfoCenter on macOS.
- Headset buttons, audio focus and ducking, sleep and idle release. The core gives the timings
  (`rules.rs`); the client wires the events.
- Car integration (Android Auto: `PlaybackService.kt` browse), widgets (`app/PlayerWidget.kt`).
- File storage for downloads and the stream cache. Android uses media3's `DownloadManager` and
  `SimpleCache`; the core decides what goes in and out.

### 4. Pictures
- Load and cache cover images (Android: Coil). Hand decoded pixels to `nori-look::cover::derive`, which
  returns the page's colours and the wash picture. Draw them; never recompute them.

### 5. The interface
- Every screen, its layout and text rendering. Take the words and page contents from the core.
- **Animations, gestures and transitions.** Their feel is per platform: easing, springs, flick
  thresholds, swipe maths. Android's are in `app/ui/` (`PlayerScreen.kt`, `Chrome.kt`, `Components.kt`),
  described in `docs/motion.md`.
- Locale: pass the platform's decimal and grouping separators to `fmt_set_locale` once.
- Drawing cost: redraw only when something visible changes. The Android player draws the seek bar once
  per pixel and the times from pre-laid-out glyphs; a paused player draws nothing.

## Calling the core cheaply
- `crates/cli` (nori-cli) is a whole terminal client in a few hundred lines: arguments, commands and
  printing, and nothing else.
- Rust clients call the crates directly. The core (`crates/core`, package `norimusic-core`, lib
  `norimusic`) is a plain rlib with no JNI in it, and its uniffi exports sit behind the default `ffi`
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
  attaches it once, detaches it when it ends, and finds the app's classes from it through the class loader
  `JNI_OnLoad` handed over, since `FindClass` on such a thread only sees the system's.
- On Android every door is registered in `JNI_OnLoad` with `RegisterNatives` (none is looked up by a
  `Java_` symbol). A door whose Kotlin signature is primitives only is `@CriticalNative` - its Rust
  function takes no `JNIEnv` and no class, and Android 8 to 11 only honour that for registered methods - and
  a short door over arrays or direct buffers that allocates no Java objects and calls nothing back is
  `@FastNative`. Every raw pointer (a direct buffer's address, array elements, Bitmap pixels) is checked
  for null and against its length before a slice is made of it.
- Measure before moving work across the boundary. The Android debug build's `bench` test command prints
  what each kind of crossing costs next to the same work in Kotlin.
