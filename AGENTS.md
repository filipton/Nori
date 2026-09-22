# Working in this repo

nori music is an Android client for Navidrome / octo-fiesta (Subsonic API): a Kotlin app over a
Rust core. Priorities, in order: battery and performance, functionality, and only then looks.
`CLAUDE.md` points here, so there is one copy of these instructions.

## Where things are

```
crates/core/    Rust: request signing, response parsing, SQLite/FTS5 index + caches (uniffi),
                and the equalizer DSP (raw JNI, see dsp.rs)
core/           Android library, no UI: net/, data/ (Library = the repository), playback/
                (media3 service, DAC, equalizer, scrobbling), downloads/, settings/, Nori.kt (object graph)
app/            the UI only: vm/ (ViewModels, all logic and state) and ui/ (Compose, draws state)
tools/          dev-server.sh: a local Navidrome with generated music for testing
```

The boundary that matters: `ui/` may be thrown away and rewritten. It must only read ViewModel
state and call ViewModel functions; it never touches `Nori`, media3, OkHttp or the FFI. `core/`
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
`build/nori-music-<version>-<abi>.apk` and prints which ABIs are inside. It is signed with the release
key when `keystore.properties` is there (see below), otherwise with the Android debug key.

## Releasing

`tools/release.sh` builds a release into `build/release-<version>/` (APK, SHA256SUMS, RELEASE.txt);
`--publish` tags, pushes and creates a draft GitHub release with the CHANGELOG section as its notes.
The steps, in order:

    tools/bump-version.sh 0.3.3          # versionName/Code, Rust workspace, docs/features.md
    tools/changelog.py --update          # commits since the last tag into CHANGELOG [Unreleased]
    tools/changelog.py --release 0.3.3   # [Unreleased] becomes [0.3.3] - <date>
    git commit -am "build: release 0.3.3"
    tools/release.sh --publish

The changelog is grouped from conventional commit subjects, so write them for a reader. Releases are
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
- CPU-decoded playback runs in bursts (`BurstSink` + a 10 s AudioTrack buffer). Check changes to the
  audio path with `tools/bench.sh dev.nori.music 90 off`: "quiet" should stay around 80 %.
- Audio offload only reaches the phone's own outputs: the audio chip has no path to a USB device, and
  an offloaded track routed there plays nothing while reporting itself fine. `Outputs.usb` stands
  offload down whenever anything USB is attached, and a sink that refuses the stream gives it up for
  the life of the service. That silence is what a USB DAC looked like before.
- Anything that touches samples disables audio offload, so the default path has no audio processors
  and ReplayGain is player volume. Sample-domain features must keep working under `BurstSink` (deep
  buffer); only the equalizer screen (`CMD_TUNING`) may trade it for latency. The sink chain is
  renderer -> `CrossfadeSink` -> `BurstSink` -> `DefaultAudioSink` (with the Rust `Equalizer` processor).
- The UI thread never waits for the core: `Nori` builds `core`, `http` and `sources` lazily and the
  application warms them on a background thread. Keep FFI and OkHttp out of constructors and composition.
- FFI calls are coarse: one response or one page per call. Per-buffer work uses raw JNI on direct
  buffers, never uniffi.
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
