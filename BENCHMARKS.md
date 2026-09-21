# Battery shootout: Nori vs Symfonium vs musly vs Navic

Same emulator (`sdk_gphone64_x86_64`, Android 14), same local Navidrome server,
same tracks, media volume 0, screen off, no touches. Nori is the **release**
build throughout; the other three are Play releases. Full per-thread outputs
and profiles: [perf-shootout.md](perf-shootout.md).

| Metric | **Nori 0.2.0** | Symfonium 15.0.1 | musly 2.0.2 | Navic alpha55 |
|---|---|---|---|---|
| Cold start | **~280 ms** | ~390 ms | ~760 ms | ~570 ms |
| MP3 CPU | **1.18%** | 6.66% | 2.78% | 2.78% |
| MP3 wakeups | **375/s** | 1747/s | 533/s | 434/s |
| MP3 quiet | **76/90** | 1/90 | 0/90 | 1/90 |
| MP3 memory | **108 MB** | 117 MB | 152 MB | 140 MB |
| FLAC CPU | **1.24%** | 7.41% | 3.76% | 4.44% |
| FLAC wakeups | **408/s** | 1955/s | 581/s | 625/s |
| FLAC quiet | **75/90** | 0/90 | 0/90 | 0/90 |
| FLAC memory | **125 MB** | 128 MB | 156 MB | 142 MB |
| EQ CPU | **1.33%** | 6.73% | — | 2.81% |
| EQ quiet | **73/90** | 1/90 | 0/90 | 0/90 |
| EQ memory | **107 MB** | 128 MB | 146 MB | 148 MB |
| Mix CPU | **0.37%** | 1.83% | 1.22% | 0.60% |
| Mix quiet | **159/200** | 117/200 | 3/200 | 108/200 |
| Mix memory | **103 MB** | 124 MB | 135 MB | 133 MB |
| Paused CPU | 0.01% | **0.00%** | 0.10% | **0.00%** |
| Paused quiet | **89/90** | 29/30 | 0/30 | 29/30 |
| Paused memory | **97 MB** | 105 MB | 150 MB | 127 MB |
| Wakelock while paused | none | none | held | none |

**Bold** = best in row. Playback rows are 90 s windows over "Noise 1" (MP3 320)
and "Noise flac" from the dev library; EQ rows add ~+4–5 dB treble per app;
mix rows are 200 s over four ~45 s tracks with three transitions;
paused rows are settled background windows.

Notes:

- musly has no equalizer; its EQ column is a stock re-run. That is a feature
  gap, not a win.
- Navic has no transition feature; its 0.60% mix row is stock gapless play.
- Symfonium skips crossfade on sequential albums, so its mix row uses a manual
  queue; its 6.66%→1.83% queue-shape difference is unexplained.
- Paused windows: Nori 90 s, the rest 30 s — quiet counts are not directly
  comparable, CPU and memory are. Nori's 0.01% is one 10 ms ExoPlayer tick in
  a 3× longer window.
- musly holds an `AudioService` wakelock while paused and never lets the
  device sleep, in any state.
- FLAC-vs-MP3 scaling: Nori +5%, Symfonium +11%, musly +35%, Navic +60%.
- An emulator has no real battery and software-decodes; absolute numbers do
  not transfer to a phone, relative rankings do. A Galaxy S22 hardware run was
  attempted and discarded (the screensaver stayed on and kept the GPU alive
  through every window).
