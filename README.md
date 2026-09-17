# flint music

A native Android client for Navidrome and [octo-fiesta](https://github.com/filipton/octo-fiesta)
(any Subsonic/OpenSubsonic server works). Kotlin + media3 for playback, a Rust core for the index
and DSP. Built for battery life first, features second, looks last: the UI is deliberately plain
and is the one layer meant to be replaced (see `AGENTS.md`).

## What it does

| | |
|---|---|
| Live search | every keystroke answered instantly from the offline FTS index; the server (and through octo-fiesta, the providers) asked once typing pauses, in-flight requests cancelled |
| Playback | gapless, hardware audio offload, large burst buffering, ReplayGain (track/album, pre-amp), queue survives process death, shuffle/repeat, sleep timer (timed / end of track), internet radio |
| DAC | Android 14+ bit-perfect USB output at the track's sample rate, hi-res float output, DAC state shown in settings |
| Sound | parametric equalizer (peaking and shelving bands, any number, AutoEQ / Equalizer APO import, automatic or manual pre-amp) and headphone crossfeed, in Rust over raw JNI; crossfade inside the one player; speed; skip silence; system audio-effects panel as the alternative |
| Offline | downloads (song/album/playlist/whole library), stars, ratings, playlist edits and plays made offline are queued and replayed, rolling stream cache keyed by song+quality, cached browse screens, offline search |
| Quality | separate transcode settings for Wi-Fi, mobile data and downloads, decided when a track is opened |
| Library | internet radio management, configurable scrobble threshold, home shelves, albums (6 sort orders, paged), artists (bio, top songs, similar), playlists (create/add/remove/delete), favourites, ratings, genres |
| Queue | play next, add to queue, song radio, auto-continue with similar songs, resume the queue saved on the server by another device |
| Integration | home-screen widget (event-driven, never polls), share links, media notification, Bluetooth/headset buttons, Android Auto browsing and voice search, scrobbling + now-playing |

Not there yet: casting (Chromecast/UPnP), smart playlists, multiple servers, formats the platform cannot decode (DSD, APE, WavPack),
a userspace USB driver (exclusive DAC access below Android 14, DSD), integer 24/32-bit bit-perfect
output (media3's sink emits 16-bit or float only; needs a custom `AudioOutputProvider`).

## Why it is cheap to run

- With the screen off nothing of ours runs: no timers, no polling, no position updates.
- Audio the DSP cannot take (FLAC on most phones) is decoded on the CPU in bursts: a 10 s
  AudioTrack buffer is refilled in one go when it drains to 2 s (`BurstSink`), so the process is
  completely asleep for ~80 % of every playing minute instead of waking every 10 ms.
- Radio discipline: the Wi-Fi lock is held only while a track is being fetched, idle connections close
  after 20 s (inside the radio's own tail, not minutes later), a cached browse response younger than two
  minutes is not re-asked, there is no network callback listening to signal-strength changes, and the
  media session does not broadcast the playhead.
- Offload by default. Features that need decoded samples (equalizer, crossfeed, crossfade, speed,
  skip silence) pause offload but not burst playback: the DSP runs inside the bursts, and only the
  equalizer screen itself switches to a shallow buffer so a moved slider is heard at once. Measured
  with a 4-band curve, crossfeed and a 6 s crossfade all on: 1.55 % CPU and 75 of 90 seconds asleep,
  whole-system 2.8 % against 5.0 % (0 of 90) for Navic with its 5-band system equalizer on.
  ReplayGain is applied as volume, so it keeps offload.
- The network is used in bursts: up to ten minutes / 48 MB buffered at once, then silence until a
  minute is left. One HTTP/2 connection pool serves API, covers and audio.
- Every URL is stable (derived salt), covers are requested in three fixed sizes: HTTP, image and
  media caches actually hit. Browse screens paint from the cached response and only redraw if the
  server's answer differs.
- Rust owns the SQLite/FTS5 index and writes to it straight from response bytes: a library sync
  moves three integers per page across the FFI, not 500 objects.

## Measured

Full results, including an Android 11 versus Android 14 comparison and the package checks, are in
[perf-results.md](perf-results.md); reproduce them with `tools/perf-suite.sh <serial> <server-url>`.
The short version, API 34 emulator (software GPU, no audio offload, so these are worst cases):

| | flint | Navic alpha55 | Musly 2.0.2 | Symfonium 15.0.1 |
|---|---|---|---|---|
| Screen off, CPU | **1.1-1.7 %** of a core | 2.4-3.3 % | 4.6 % | 10.5 % |
| Screen off, seconds asleep (of 90) | **70-75** | 1 | 0 | 0 |
| Player visible, CPU | **3.2 %** | 90 % | 146 % | 185 % |
| Cold start | **0.74-1.0 s** | ~1.9 s | - | - |
| Memory | 72-121 MB | 120-137 MB | 132-148 MB | 119-136 MB |

Equalizer, crossfeed and crossfade cost nothing extra with the screen off (1.7 % with all three on),
because the DSP runs inside the same bursts. The word-by-word lyric sweep is the one exception at
44 % while that tab is open on this emulator, which is why it has a switch (2.9 % off).

Musly refuses to start on an emulator, so it was built from its v2.0.2 tag with the check removed;
Symfonium is the official universal APK in trial mode. Both were measured on the same emulator and
track as the rows above.

## Build

Needs the Android SDK + NDK, Rust with the Android targets, and `cargo-ndk`.

```sh
./gradlew :app:assembleDebug -PrustTargets=x86_64     # emulator
./gradlew :app:assembleRelease                         # arm64-v8a + x86_64
cargo test
tools/dev-server.sh                                    # local Navidrome with generated music
```

## octo-fiesta notes

Provider items (`ext-…`, `isExternal`) are shown with a cloud icon and are never indexed or
auto-queued: asking the proxy to stream one makes it download the track, which can take a minute
before the first byte (the stream client waits up to four). Tapping a search result plays that one
song only, for the same reason.
