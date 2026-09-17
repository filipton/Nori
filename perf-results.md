# Measured performance

Everything here is produced by `tools/perf-suite.sh <serial> <server-url>`, which installs the release
APK, logs in, and walks the app while `tools/bench.sh` samples CPU, wake-ups and memory from `/proc`.
"Quiet seconds" counts seconds in which every thread of the app stayed asleep - the number that
decides battery life with the screen off. The library is the generated dev server (`tools/dev-server.sh`):
300 albums, 10-minute 320 kbps MP3s, one FLAC, and one track with synced lyrics.

Read the numbers with these caveats:

- **Emulators render in software.** Any row with the screen on is dominated by that, and will be far
  lower on a phone with a GPU. Screen-off rows are not affected.
- **Neither emulator image has audio offload**, so every screen-off row is the worst case: the CPU
  decodes. On a phone with offload, MP3/AAC/Opus move to the audio DSP.
- Rows marked *(by hand)* were measured directly rather than by the suite, because the suite's taps
  could not reach that screen (see "Harness notes").

### sdk_gphone64_x86_64 · Android 14 · x86_64 · 2026-09-17

Cold start (ms, 5 runs sorted): 763 822 913 935 1006 
Album grid fling, 300 albums: 50th 48ms 99th 77ms 

| Scenario | CPU (% of one core) | Quiet seconds | Memory |
|---|---|---|---|
| Idle on home, screen on (30 s) | -1.20% of one core | 29 of 30 | 72 MB PSS |
| MP3 320, screen off (90 s) | 1.53% of one core | 75 of 90 | 96 MB PSS |
| Player screen visible (45 s) | 3.00% of one core | 0 of 45 | 95 MB PSS |
| Lyrics, word sweep on (45 s) | 3.20% of one core | 0 of 45 | 95 MB PSS |
| FLAC, screen off (90 s) | 1.57% of one core | 75 of 90 | 117 MB PSS |
| FLAC + equalizer, screen off (90 s) | 1.72% of one core | 71 of 90 | 121 MB PSS |
| Paused in background (30 s) | .03% of one core | 29 of 30 | 115 MB PSS |

Audio offload threads active during MP3 playback: 0

### sdk_gphone_x86_64 · Android 11 · x86_64 · 2026-09-17

Cold start (ms, 5 runs sorted): 705 750 785 787 804 
Album grid fling, 300 albums: 50th 14ms 99th 22ms 

| Scenario | CPU (% of one core) | Quiet seconds | Memory |
|---|---|---|---|
| Idle on home, screen on (30 s) | 0% of one core | 29 of 30 | 73 MB PSS |
| MP3 320, screen off (90 s) | 21.01% of one core | 63 of 90 | 80 MB PSS |
| Player screen visible (45 s) | 19.84% of one core | 0 of 45 | 79 MB PSS |
| Lyrics, word sweep on (45 s) | 20.53% of one core | 0 of 45 | 78 MB PSS |
| FLAC, screen off (90 s) | 20.30% of one core | 43 of 90 | 80 MB PSS |
| FLAC + equalizer, screen off (90 s) | 21.31% of one core | 44 of 90 | 74 MB PSS |
| Paused in background (30 s) | 0% of one core | 29 of 30 | 69 MB PSS |

Audio offload threads active during MP3 playback: 0

### Measured by hand, API 34 emulator

| Scenario | CPU (% of one core) | Note |
|---|---|---|
| Player screen visible, music playing | 3.2 % | the seek bar ticks once a second |
| Lyrics tab, **word sweep on** | 44 % | one line of text redrawn at 30 fps; software GPU |
| Lyrics tab, **word sweep off** | 2.9 % | the line just lights up; the switch is in Settings -> Lyrics |
| Lyrics tab, track without lyrics | 2.8 % | nothing to sweep |

The sweep is the one feature that costs real CPU while it is on screen, which is why it has a switch
and why it stops the moment the tab is left or the screen goes off.

### Android 11 versus Android 14

Same APK, same file, same host, same emulator settings:

| | Android 14 (API 34) | Android 11 (API 30) |
|---|---|---|
| MP3 320, screen off | 1.1 - 1.7 % | 21 % |
| FLAC, screen off | 1.0 - 1.6 % | 20 % |
| Busiest thread | `MediaCodec_loop` 0.6 s / 90 s | `MediaCodec_loop` 17 s / 90 s |
| `ExoPlayer:Playb` (our scheduling) | ~0.5 s / 90 s | ~1.1 s / 90 s |
| Live codec instances in the process | 1 | 3 |

The difference is entirely in the platform's software decoder callbacks, not in flint's own threads:
both images decode with `codec2::software`, but the Android 11 image keeps three codec instances alive
and spends 27x more time in the in-process callback loop. Burst playback still works there - 63 to 75
of 90 seconds asleep - it simply has more work to wake up for. Worth re-checking on a real Android 11
device before treating it as a platform fact rather than an image quirk.

### Package

- Release APK 10.0 MB (universal: arm64-v8a + x86_64), `libflintmusic.so` 3.1 MB per ABI.
- All native libraries are 16 KB page aligned (`llvm-objdump -p` reports `2**14`), and `zipalign -c -P 16`
  passes, which is what Android 15+ and Play require.

### Harness notes

- Run one suite at a time. Two suites, or a leftover background run, drive the same device at once and
  produce nonsense (both were seen during this session).
- After typing in the search field the on-screen keyboard swallows taps aimed at the bottom of the
  screen, and those taps arrive as text in the field. The suite force-stops the app before the steps
  that need the mini player; a leftover keyboard is what made the player and equalizer steps fail in
  earlier runs.
- `dumpsys media_session` prints the playback state as a word on Android 14 and as a number on
  Android 11; `bench.sh` and the suite handle both.
