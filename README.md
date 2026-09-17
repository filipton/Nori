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
| Equalizer | 10-band, in Rust over raw JNI; system audio-effects panel as the alternative |
| Offline | downloads (song/album/playlist), rolling stream cache keyed by song+quality, cached browse screens, offline search, scrobbles queued while offline |
| Quality | separate transcode settings for Wi-Fi, mobile data and downloads, decided when a track is opened |
| Library | home shelves, albums (6 sort orders, paged), artists (bio, top songs, similar), playlists (create/add/remove/delete), favourites, ratings, genres |
| Queue | play next, add to queue, song radio, auto-continue with similar songs, resume the queue saved on the server by another device |
| Integration | media notification, Bluetooth/headset buttons, Android Auto browsing and voice search, scrobbling + now-playing |

Not there yet: casting (Chromecast/UPnP), crossfade, widgets, smart playlists, multiple servers,
a userspace USB driver (exclusive DAC access below Android 14, DSD), integer 24/32-bit bit-perfect
output (media3's sink emits 16-bit or float only; needs a custom `AudioOutputProvider`).

## Why it is cheap to run

- With the screen off nothing of ours runs: no timers, no polling, no position updates. Measured
  on the emulator (no offload hardware, debug build): app main thread ~20 ms CPU per 40 s of
  playback; everything else is the decoder, which a real phone moves to the audio DSP.
- Offload by default; the only features that need the CPU in the audio path (equalizer) are opt-in
  and say so. ReplayGain is applied as volume, so it keeps offload.
- The network is used in bursts: up to ten minutes / 48 MB buffered at once, then silence until a
  minute is left. One HTTP/2 connection pool serves API, covers and audio.
- Every URL is stable (derived salt), covers are requested in three fixed sizes: HTTP, image and
  media caches actually hit. Browse screens paint from the cached response and only redraw if the
  server's answer differs.
- Rust owns the SQLite/FTS5 index and writes to it straight from response bytes: a library sync
  moves three integers per page across the FFI, not 500 objects.

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
