<h1 align="center">Nori</h1>

<p align="center">
  A native Android client for Navidrome and octo-fiesta.
  Battery life first, features second, looks last.
</p>

<p align="center">
  <img alt="version 0.2.0" src="https://img.shields.io/badge/version-0.2.0-2b7fff?style=flat-square">
  <img alt="Android 8.0+" src="https://img.shields.io/badge/Android-8.0%2B-3ddc84?style=flat-square&logo=android&logoColor=white">
  <img alt="arm64 · x86_64" src="https://img.shields.io/badge/arm64%20%C2%B7%20x86__64-555?style=flat-square">
</p>

<table align="center">
  <tr>
    <td width="33%"><img src="docs/screenshots/home.png" width="100%" alt="Listen now: shelves of favourites, recent and new albums"></td>
    <td width="33%"><img src="docs/screenshots/player.png" width="100%" alt="The player: cover colour bleeding into the page"></td>
    <td width="33%"><img src="docs/screenshots/lyrics.png" width="100%" alt="Synced lyrics with a word-by-word sweep"></td>
  </tr>
  <tr>
    <td align="center"><sub>Listen now</sub></td>
    <td align="center"><sub>The player</sub></td>
    <td align="center"><sub>Synced lyrics</sub></td>
  </tr>
</table>

<details>
<summary align="center"><b>More: the library, search, the equalizer</b></summary>
<br>
<table align="center">
  <tr>
    <td width="33%"><img src="docs/screenshots/library.png" width="100%" alt="The library: an album grid"></td>
    <td width="33%"><img src="docs/screenshots/search.png" width="100%" alt="Search across artists, albums and songs"></td>
    <td width="33%"><img src="docs/screenshots/equalizer.png" width="100%" alt="The parametric equalizer"></td>
  </tr>
  <tr>
    <td align="center"><sub>Library</sub></td>
    <td align="center"><sub>Search</sub></td>
    <td align="center"><sub>Equalizer</sub></td>
  </tr>
</table>
</details>

## What it does

- **Playback** that sips battery: gapless, hardware audio offload, 10 s burst
  buffering, ReplayGain (track/album, pre-amp), a queue that survives process
  death, shuffle/repeat, sleep timer, internet radio.
- **Sound** shaped in Rust: a parametric equalizer (peak, shelf, pass, notch,
  all-pass, per-channel bands, presets, Equalizer APO paste-in, 8850 measured
  headphones searchable on-device), crossfeed, balance, mono, a look-ahead
  limiter, profiles that load per output — plus crossfade, speed and
  skip-silence in the one player.
- **Bit-perfect USB** on Android 14+: the track's own sample rate straight to
  the DAC, hi-res float output, DAC state in settings.
- **Offline** everything: downloads down to the whole library, stars, ratings,
  playlist edits and plays queued offline and replayed, rolling stream cache,
  cached browse screens, offline search.
- **Live search** answered from the on-device FTS index as you type; the server
  (and through octo-fiesta, the providers) asked once typing pauses.
- **Library** with home shelves, 6 album sort orders, bios, similar artists,
  playlists, favourites, ratings, genres, song radio and a queue shared with
  your other devices.
- **Integration**: event-driven home-screen widget, share links, media
  notification, headset buttons, Android Auto, scrobbling.

Not there yet: casting, smart playlists, multiple servers, formats the platform
cannot decode (DSD, APE, WavPack), integer 24/32-bit bit-perfect output.

## Measured

Same emulator, same server, same tracks, screen off — Nori release against
Play releases. Full table: [BENCHMARKS.md](BENCHMARKS.md).

| | nori | Symfonium 15.0.1 | musly 2.0.2 | Navic alpha55 |
|---|---|---|---|---|
| Screen off, CPU (MP3 / FLAC / EQ on) | **1.18 / 1.24 / 1.33 %** | 6.66 / 7.41 / 6.73 % | 2.78 / 3.76 / no EQ | 2.78 / 4.44 / 2.81 % |
| Screen off, seconds asleep (of 90) | **76 / 75 / 73** | 1 / 0 / 1 | 0 / 0 / 0 | 1 / 0 / 0 |
| Cold start | **~280 ms** | ~390 ms | ~760 ms | ~570 ms |
| Memory playing / paused | **108 / 97 MB** | 117 / 105 MB | 152 / 150 MB | 140 / 127 MB |

With the screen off nothing of ours runs: no timers, no polling, no position
updates. Decoded audio is buffered in 10 s bursts, so the process sleeps ~80 %
of every playing minute; off-by-default features cost nothing until switched
on. Reproduce it with `tools/perf-suite.sh <serial> <server-url>`.

## Install

```sh
tools/apk.sh           # release APK for a phone (arm64) -> build/nori-music-<version>-<abi>.apk
tools/apk.sh x86_64    # for an emulator
tools/apk.sh --install # straight to the connected device
```

Signed with the Android debug key: installs and updates on your own devices,
cannot be published. It is not on Google Play.

## Build

```sh
./gradlew :app:assembleDebug -PrustTargets=x86_64   # fast build for an emulator
./gradlew :app:assembleRelease                       # arm64-v8a + x86_64
cargo test
tools/dev-server.sh                                  # local Navidrome with generated music
```

You need the Android SDK + NDK, Rust with the Android targets, and
`cargo-ndk`. `tools/app.sh` drives a debug build over adb without touching the
screen; `tools/audio-e2e.sh` and `tools/feature-e2e.sh` check playback and the
rest of the app against a real server.

## How it is put together

- **Rust core** in `crates/`: request signing, response parsing, the
  SQLite/FTS5 index and caches (uniffi), and the equalizer DSP (raw JNI).
- **Kotlin core** in `core/`: networking, the library repository, the media3
  playback service, downloads and settings. It never knows a UI exists.
- **The UI** in `app/`: ViewModels hold all logic and state, Compose only
  draws it. The UI layer may be thrown away and rewritten.

The boundary that matters: `ui/` reads ViewModel state and calls ViewModel
functions; it never touches networking, media3 or the FFI.

## More

- [Battery shootout in full](BENCHMARKS.md), raw traces in `perf-shootout.md`
- [What's planned](docs/features.md), [where the work stopped](docs/handoff.md)
- [How to work in this repo](AGENTS.md)

Provider items (`ext-…`) are shown with a cloud icon and are never indexed or
auto-queued: asking the proxy to stream one makes it download the track, which
can take a minute before the first byte. Tapping a search result plays that one
song only, for the same reason.
