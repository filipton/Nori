# Working in this repo

flint music is an Android client for Navidrome / octo-fiesta (Subsonic API): a Kotlin app over a
Rust core. Priorities, in order: battery and performance, functionality, and only then looks.
`CLAUDE.md` points here, so there is one copy of these instructions.

## Where things are

```
crates/core/    Rust: request signing, response parsing, SQLite/FTS5 index + caches (uniffi),
                and the equalizer DSP (raw JNI, see dsp.rs)
core/           Android library, no UI: net/, data/ (Library = the repository), playback/
                (media3 service, DAC, equalizer, scrobbling), downloads/, settings/, Flint.kt (object graph)
app/            the UI only: vm/ (ViewModels, all logic and state) and ui/ (Compose, draws state)
tools/          dev-server.sh: a local Navidrome with generated music for testing
```

The boundary that matters: `ui/` may be thrown away and rewritten. It must only read ViewModel
state and call ViewModel functions; it never touches `Flint`, media3, OkHttp or the FFI. `core/`
must never know a UI exists.

## Build and test

```sh
./gradlew :app:assembleDebug -PrustTargets=x86_64   # fast build for an emulator
cargo test                                          # the Rust tests
tools/dev-server.sh                                 # Navidrome at http://10.0.2.2:4533 from the emulator, admin/admin
```

Run `cargo test` and a build before committing.

## Building an APK

`tools/apk.sh` builds a release APK for a phone: arm64 by default, `tools/apk.sh x86_64` for an
emulator, `tools/apk.sh --install` to push it straight to whatever is connected. It lands in
`build/flint-music-<version>-<abi>.apk` and prints which ABIs are inside. The signature is the Android
debug key, which installs and updates on your own device but cannot be published.

`tools/app.sh` drives a **debug** build over adb without touching the screen - `open <route>`,
`play "search:…"`, `do download album:<id>`, `set limiter true`, `state` (one JSON line of route,
playback, DSP and download state). `tools/audio-e2e.sh` and `tools/feature-e2e.sh` are built on it and
check playback and the rest of the app against a real server. When adding a feature, add its check
there: a screenshot proves a screen renders, not that the feature works.

## Performance rules

- Nothing polls or ticks while music plays with the screen off. The seek bar is the only timer,
  and it runs only while the player screen is resumed.
- One OkHttp pool for API, covers and audio. URLs are stable (derived salt) so caches hit.
- CPU-decoded playback runs in bursts (`BurstSink` + a 10 s AudioTrack buffer). Check changes to the
  audio path with `tools/bench.sh dev.flint.music 90 off`: "quiet" should stay around 80 %.
- Anything that touches samples disables audio offload, so the default path has no audio processors
  and ReplayGain is player volume. Sample-domain features must keep working under `BurstSink` (deep
  buffer); only the equalizer screen (`CMD_TUNING`) may trade it for latency. The sink chain is
  renderer -> `CrossfadeSink` -> `BurstSink` -> `DefaultAudioSink` (with the Rust `Equalizer` processor).
- The UI thread never waits for the core: `Flint` builds `core`, `http` and `sources` lazily and the
  application warms them on a background thread. Keep FFI and OkHttp out of constructors and composition.
- FFI calls are coarse: one response or one page per call. Per-buffer work uses raw JNI on direct
  buffers, never uniffi.
- octo-fiesta: a stream request for an `ext-` id makes the server download the track. Never
  queue or prefetch provider tracks the user did not ask to play. Provider items are never indexed.

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

## Optional features

`docs/features.md` is the checklist of what is planned, with the owner's decisions at the top. Every
optional subsystem (casting, FFmpeg decoder, resampler, smart fades, third-party lookups, taste model,
...) sits behind a switch in `Prefs`, and a switched-off feature must cost nothing: not initialised,
no listener, no socket, no audio processor. Check with `tools/bench.sh` that the default screen-off
numbers do not move when a feature is added.

## Commit messages

One line, always. No body, no trailers, no attribution, no `Co-Authored-By`.

```
<type>: <what is different now>
```

`type` is one of `feat`, `fix`, `perf`, `refactor`, `docs`, `build`, `test`, `chore`. Lowercase
after the colon, no full stop, well under 72 characters. One commit per piece of work, not per file.
