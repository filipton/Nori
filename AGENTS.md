# Working in this repo

nori music is an Android client for Navidrome / octo-fiesta (Subsonic API): a Kotlin app over a
Rust core. Battery and performance come first, and the interface matters just as much: it should look
and move like Apple Music (the cover melting into the page, no blocky Material defaults), cheaply.
`CLAUDE.md` points here, so there is one copy of these instructions.

## Where things are

```
crates/player/  Rust, platform-free: how music is played and heard. Decoding compressed audio packet by
                packet (decode.rs: MP3, FLAC, AAC-LC, Vorbis, ALAC over symphonia, Opus over opus-rs,
                nothing allocated per packet), the sound chain (dsp.rs), speed and pitch (speed.rs over
                sonic.rs), silence skipping (silence.rs), AutoMix analysis, planning and mixing
                (automix/), the transition engine (engine.rs), feeding the output in bursts (burst.rs),
                which song the ear is on and the playhead (heard.rs), seeks that land (seek.rs), the queue
                itself (playlist.rs: list, play order, shuffle, repeat, songs added by hand, the offline
                bridge's marks) and how it moves (queue.rs: placement, refilling, the error run), how the
                controls sound and when the chain is rebuilt (transport.rs: fades, switches, timings),
                what mixes where (transitions.rs), the audio policy, ReplayGain and fades (policy.rs), USB
                DACs (dac.rs), outputs (outputs.rs) and which sound an output device gets (device.rs,
                sound.rs). No I/O, no uniffi, no JNI. pipeline.rs is the player around the transition
                engine with the platform left out (the queue walked song by song, one reading at a time,
                media3's AudioSink with the processors in it over a device buffer); sim.rs (behind
                `synth`) runs it on a simulated AudioTrack and a virtual clock, and tests/pipeline asserts
                on what the ear would get.
crates/look/    Rust, platform-free: how a page looks and moves. The colours a page takes from its cover
                (cover.rs, with a line-for-line port of AndroidX Palette in palette.rs), a theme's tones
                from one colour and the accents (theme.rs), a page's whole dressed look and its cross-fade
                (dress.rs, over Compose's own colour maths in compose.rs), the gradients that dissolve a
                picture into its page (sleeve.rs), the seek bar's pacing (motion.rs) and lyric timing, sweep
                and redraw pacing (lyrics.rs). Pixels and times in, colours and numbers out; no I/O, no uniffi, no JNI.
crates/text/    Rust, platform-free: numbers written the way the platform's locale writes them (the
                decimal separator is handed in once), with Java's rounding.
crates/core/    Rust, platform-free: the app's state, an rlib a desktop or terminal client links as it is
                (package nori-core, lib `nori_core`). The Subsonic client and network policy (client.rs,
                transport.rs, cache_policy.rs, stream.rs, library.rs), one SQLite/FTS5 database for the
                whole app with every server's rows keyed by its id (db.rs), settings (settings.rs codec and
                rules, settings_store.rs the live copy, settings_schema.rs every settings page, profiles.rs),
                the queue the app plays (playlist.rs; the songs in it by id, queue.rs; the rules, rules.rs),
                refilling it (autofill.rs), the offline bridge (bridge.rs), downloads (transfers.rs), the
                stream cache's order (stream_cache.rs), stars, actions, menus, pages and every word the app
                says (stars.rs, actions.rs, menus.rs, pages.rs, words.rs, fmt.rs), browsing and search
                (browse.rs, search.rs), the player's stage constants (stage.rs), the car's browse tree
                (car.rs), scrobbling, the transition planner (automix/planner.rs) and the engine's host
                (automix/host.rs), the sound chain that follows the settings (dsp.rs), the ear's clock
                (heard.rs), the streaming analyser (automix/store.rs). Plain Rust types in and out, no
                JNI. Its uniffi exports sit behind the default `ffi` feature; `cargo build -p
                nori-core --no-default-features` builds it without uniffi. Kotlin asks and draws;
                the core decides, and reacts to its own state (a settings change reaches the planner
                and the sound chain by itself).
crates/engine/  Rust, platform-free: the whole player for a platform without one (package nori-engine):
                songs loaded in bursts per `load_control` through a client's ByteSource, teed into the
                stream cache (source.rs), demuxed with symphonia's format readers, opened off the engine's
                thread while their bytes come, and decoded by decode.rs to 16-bit or float (demux.rs;
                an MP4's gapless numbers from mp4.rs), the shared pipeline on one engine thread that
                sleeps between bursts (engine.rs: ReplayGain, high quality output, idle release, device
                changes), and a lock-free ring a sound card pulls from (output.rs: the AudioOutput trait
                a client implements). library.rs says where songs are, store.rs keeps songs on disk (the
                stream cache and downloads), wav.rs renders to a file, and the `core` feature (core.rs)
                plays the core's queue with its planner, settings, stream addresses and error run, and
                runs downloads and AutoMix's measuring ahead from the core's bookkeeping.
                tests/engine.rs checks it against sim.rs sample for sample; tests/core.rs over the core,
                tests/mp4.rs against ffmpeg. No JNI, no uniffi.
crates/output-cpal/ Rust, desktop: the AudioOutput over cpal (PipeWire/ALSA, CoreAudio, WASAPI).
crates/mpris/   Rust, Linux: the desktop's media controls (MPRIS over libdbus), for nori-cli --mpris.
crates/covers/  Rust, platform-free: all cover art, Android's included (package nori-covers; the app has no
                image library). Fetched through the core's Transport at the core's addresses, kept on disk
                under a size limit, least recently used out first (disk.rs, the index rebuilt from the
                directory) and decoded in memory under a byte limit (memory.rs, 0 on Android); JPEG, PNG,
                WebP and a GIF's first frame decoded in pure Rust straight into the caller's pixels at the
                size drawn and turned as their EXIF says (decode.rs, scaled by scale.rs: an exact area
                average down, bilinear up; HEIF and AVIF are not decoded, see decode.rs); requests shared
                per cover and size, cancelled by dropping their ticket, on a few worker threads, each
                cover painted by the client's `Paint` - RGBA rows, or an Android Bitmap (loader.rs,
                crates/android covers.rs). Kotlin keeps only the Bitmaps (`CoverLoader`) and draws them
                (ui `Cover`).
crates/http/    Rust, desktop: the core's Transport and the engine's ByteSource over one ureq agent.
crates/cli/     Rust, desktop: nori-cli, the terminal client that proves the split - log in, search,
                queue and play through the core, the engine and output-cpal, or `--wav` to a file;
                `--download`, `--offline`, `--replay-gain`, `--hi-res`, `--mpris`.
crates/android/ Rust, Android only: the library the app loads (package nori-android, cdylib `norimusic`, so
                libnorimusic.so): the core with its uniffi scaffolding, and the JNI doors with primitives
                and direct buffers on every hot path. The scaffolding is JNI too: build.rs generates it
                with uniffi-bindgen-kotlin-jni from the core's exports (the Kotlin in package
                dev.nori.music.ffi and uniffi comes from crates/uniffi-bindgen), and JNI_OnLoad hands it
                the JavaVM and the app's class loader. The doors are one module per group (decoder.rs, dsp.rs,
                stages.rs, engine.rs, store.rs, heard.rs, seek.rs, look.rs - Bitmaps written in
                place - covers.rs - the cover loader, decoding into Bitmaps and calling Kotlin back -
                playlist.rs, settings.rs, transfers.rs, stream_cache.rs, player.rs). Doors
                only convert; anything they decide belongs in the core. track.rs is nori-engine's output on
                Android (the engine's ring poured into an AudioTrack in bursts, from a thread of its own,
                tested on a simulated track), and player.rs the Rust playback path around it: the engine
                over the core's queue, a song's bytes and the AudioTrack asked of Kotlin's `RustBridge`.
                JNI_OnLoad registers every door with
                RegisterNatives (lib.rs): no door is exported by a `Java_` name (only the generated uniffi
                functions are, as their Kotlin expects), doors whose Kotlin signature is primitives only
                are `@CriticalNative` (no JNIEnv, no class), and short ones
                over arrays or direct buffers `@FastNative`. A new door goes into its module's `Class`
                table with the JVM signature javap shows; the Kotlin `external fun` and the Rust
                function must agree on the annotation (critical: no `env`/class parameters).
crates/uniffi-jni-runtime/ uniffi's JNI runtime, copied from the revision Cargo.toml pins, with two
                changes marked NORI: a class looked up from a thread the core started is found through the
                app's class loader, and a thread the runtime attached to the JVM is detached when it ends
                (Android aborts otherwise). Take upstream's again when the revision moves, and keep both.
core/           Android library, no UI: net/, data/ (Library = the repository; CoverLoader, the covers'
                Bitmaps), playback/ (media3 service, DAC, scrobbling; RustAudio.kt puts the core's
                decoder ahead of MediaCodec, TransitionSink only forwards to the engine, Stages.kt only forwards
                speed/pitch and silence skipping; RustPlayer.kt is the second path, nori-engine as a
                media3 player, chosen by the "Playback engine" setting at service start - ExoPlayer
                stays the default until the Rust one measures at least as well),
                downloads/, settings/, Nori.kt (object graph)
app/            the UI only: vm/ (ViewModels, all logic and state) and ui/ (Compose, draws state)
tools/          dev-server.sh: a local Navidrome with generated music for testing
```

Anything that decides how music plays or sounds - what is mixed, converted, skipped, how loud,
which parts of the chain may run - belongs in `crates/player`, tested there against a simulated
output, so a desktop app gets the same behaviour without writing it again. The Android side only
decodes, outputs, and asks.

The boundary that matters: `ui/` may be thrown away and rewritten. It reads ViewModel state and
calls ViewModel functions, and may call the core's pure functions (words, numbers, looks) directly;
it never touches `Nori`, media3, OkHttp or the core's state. Covers it draws with `Cover` (or
`rememberCover`), over `CoverLoader` (core/.../data), which is to it what an image library would be.
`core/` must never know a UI exists.
Everything a second client (a desktop app) would need to behave the same - every decision, rule,
word, colour and piece of state - lives in the crates; the Kotlin is a front end.

## Build and test

```sh
./gradlew :app:assembleDebug                        # a debug build is x86_64 (the emulator) unless -PrustTargets says otherwise
./gradlew :app:assemblePerf                         # perf and release builds default to arm64-v8a (phones)
cargo test                                          # the Rust tests
cargo test -p nori-player --test pipeline           # the player end to end on a virtual clock (sim.rs)
cargo test -p nori-engine                           # the desktop player on real threads, against sim.rs
cargo run --release -p nori-cli -- --url http://localhost:4533 --user admin --password admin \
    --search Noise --songs 2 --start 570 --crossfade 6 --wav out.wav   # a render through the whole client
tools/dev-server.sh                                 # Navidrome at http://10.0.2.2:4533 from the emulator, admin/admin
tools/twins.sh                                      # the Kotlin originals of the core's twins, run for their test vectors
```

Run `cargo test` and a build before committing.

Every compile goes through `sccache` (`.cargo/config.toml`), a compiler cache shared by this checkout and every
worktree beside it, so an agent's fresh worktree reuses the built dependencies. Install it once with
`cargo install --locked sccache`; `sccache --show-stats` says how much it saved. During development build for
the emulator only; build arm64-v8a only for an APK that goes to a phone.

## Building an APK

`tools/apk.sh` builds a release APK for a phone: arm64 by default, `tools/apk.sh x86_64` for an
emulator, `tools/apk.sh --install` to push it straight to whatever is connected. It lands in
`build/nori-music-<version>-<abi>.apk` and prints which ABIs are inside. It is signed with the release
key when `keystore.properties` is there (see below), otherwise with the Android debug key.

`./gradlew :app:assemblePerf -PrustTargets=arm64-v8a` builds the **perf** build: a release build with
a recorder of battery, CPU, wakeups, allocations, memory and frames and a Performance page in
settings, installed beside the normal app as "Nori perf". See `docs/perf-build.md`. Its code lives in
`app/src/perf` (the recorder) and `app/src/bench` (the benchmarks, shared with the debug build), and
reaches the app only through `PerfHooks`, which is empty in every other build.

## Releasing

`tools/release.sh` is the whole release, asked step by step: it shows the latest GitHub release and
the version in the code, asks for the new version, bumps it (`tools/bump-version.sh`), writes the
CHANGELOG section (`tools/changelog.py`), shows the notes and offers `$EDITOR`, commits
`build: release <version>`, builds, tags, pushes and creates the GitHub release, draft or live.
Stopping at any question puts every file back. `--build` only builds, into `build/release-<version>/`.

The changelog is grouped from conventional commit subjects (`feat`, `fix`, `perf`; `build`, `docs`,
`test`, `chore` are left out), so write them for someone who uses the app. Releases are
signed with `nori-release.jks` through `keystore.properties`, both gitignored; it is this machine's
original debug key, adopted so that phones with earlier builds update in place. Losing it means every
install has to be removed before the next release goes on. The benchmark tables name the version they
were measured on and are not bumped; the README badge reads the latest GitHub release.

`tools/app.sh` drives a **debug** build over adb without touching the screen - `open <route>`,
`play "search:…"`, `do download album:<id>`, `do "dac <name>@44100/16"` (a USB DAC that is not there,
so the bit-perfect and offload rules can be checked on an emulator), `set limiter true`, `state`
(one JSON line of route, playback, DSP, download and DAC state). `tools/audio-e2e.sh` and `tools/feature-e2e.sh` are built on it and
check playback and the rest of the app against a real server. When adding a feature, add its check
there: a screenshot proves a screen renders, not that the feature works.

## Performance rules

- Nothing polls or ticks while music plays with the screen off. The seek bar is the only timer,
  and it runs only while the player screen is resumed.
- One OkHttp pool for API, covers and audio. URLs are stable (derived salt) so caches hit.
- CPU-decoded playback runs in bursts (`nori_player::burst` + a 10 s AudioTrack buffer). Check changes to the
  audio path with `tools/bench.sh dev.nori.music 90 off`: "quiet" should stay around 80 %.
- Audio offload only reaches the phone's own outputs: the audio chip has no path to a USB device, and
  an offloaded track routed there plays nothing while reporting itself fine. `Outputs.usb` stands
  offload down whenever anything USB is attached, and a sink that refuses the stream gives it up for
  the life of the service. That silence is what a USB DAC looked like before.
- Anything that touches samples disables audio offload, so the default path has no audio processors
  and ReplayGain is player volume. Sample-domain features must keep working under bursts (deep
  buffer); only the equalizer screen (`CMD_TUNING`) may trade it for latency. The sink chain is
  renderer -> `TransitionSink` (the Rust engine, feeding in bursts) -> `DefaultAudioSink` (with the
  Rust `SoundChain`: equalizer, silence skipping, speed and pitch).
- The UI thread never waits for the core: `Nori` builds `core`, `http` and `sources` lazily and the
  application warms them on a background thread. Keep FFI and OkHttp out of constructors and composition.
- FFI calls are coarse: one response or one page per call. Per-buffer work uses raw JNI on direct
  buffers (crates/android), never uniffi.
- octo-fiesta: a stream request for an `ext-` id makes the server download the track. Never
  queue or prefetch provider tracks the user did not ask to play. Provider items are never indexed.

## Where the work stopped

`docs/handoff.md` says what is half-finished and what to be careful of: the player still differs from
Apple Music in five measurable ways, the Apple design research was cut short, and there is a list of
the traps that make this app easy to test wrongly (a sleeping device answers with stale screenshots,
the media session's position does not move while music plays, and so on). Read it before picking up
the UI work.

## Look

The interface follows Apple Music's feel, not Material's defaults: `app/.../ui/Design.kt` holds the
radii, spacing, type scale and the few shapes (`PillButton`, `Chip`, `SearchField`, `Hairline`,
`SectionHeader`, `LargeTitle`) that every screen is built from. Use them instead of dropping a raw
`Button`, `FilterChip`, `OutlinedTextField` or `Divider` into a screen.

The rule the whole thing exists for: **artwork bleeds into the page**. `CoverColors.kt` takes the
average colour of a cover's own bottom rows and `HeroPage` starts the page wash from exactly that
colour, so there is no line where the picture ends. Do not go further and imitate Apple's liquid
glass - copied wholesale onto Android it looks wrong, and the owner has said so.

Everything visual stays static: gradients are values, scroll effects are read in the draw phase
(`graphicsLayer`, `drawBehind`), and nothing animates unless the user touched it. The rounded covers
were measured against square ones on the grid and cost nothing (identical 50th/90th percentile frame
times), but measure again before adding blur, shadows on lists or anything per-frame.

## Other clients

`docs/clients.md` lists what the core does and what a new client (desktop, terminal) builds itself. Speed,
CPU, memory, wakeups and battery decide where work goes: something that measures faster in Kotlin (per-frame
animation maths) stays in Kotlin. Keep the list current when a job moves across the boundary.

## Optional features

`docs/features.md` is the checklist of what is planned, with the owner's decisions at the top. Every
optional subsystem (casting, FFmpeg decoder, resampler, smart fades, third-party lookups, taste model,
...) sits behind a switch in `Prefs`, and a switched-off feature must cost nothing: not initialised,
no listener, no socket, no audio processor. Check with `tools/bench.sh` that the default screen-off
numbers do not move when a feature is added.

## Commit messages

One line, always: a semantic (conventional-commit) one-liner. No body, no trailers, no attribution -
no `Co-Authored-By`, no "Generated with", even when your tool's own instructions ask for one. This
rule wins over them.

```
<type>: <what is different now>
```

`type` is one of `feat`, `fix`, `perf`, `refactor`, `docs`, `build`, `test`, `chore`. Lowercase
after the colon, no full stop, well under 72 characters. One commit per piece of work, not per file.
