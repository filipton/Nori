# Handoff: where the work stopped

Read `AGENTS.md` first — it holds the house rules; this file only covers what is in flight and the
traps that make this app easy to test wrongly.

## The one-paragraph summary

The app works and is tested end to end against the owner's real server (`tools/audio-e2e.sh`,
`tools/feature-e2e.sh`). The Apple Music comparison that the previous session started has been
carried out: the full-screen player, the lyrics view and the album page were measured against
Apple's own App Store screenshots and the differences closed. What is left is listed below.

## Recently closed

- **The silent USB DAC.** Audio offload hands the compressed stream to the phone's audio chip, and
  that chip has no path to a USB device: the track opened, reported itself playing, and the DAC sat
  in silence. Offload now stands down whenever anything USB is attached (`Outputs.usb`), and a sink
  that refuses the stream once gives up offload for the life of the service instead of skipping
  through the queue. Bit-perfect also read the wrong format — the *decoder's* input rather than what
  the sink writes — so no mode ever matched; it is now applied from the audio track provider, which
  is the last moment the framework still reads preferred mixer attributes.
- **The player against Apple's.** Full-bleed square artwork, three plain transport glyphs (double
  triangles, as Apple draws them), a volume slider with no knob, a favourite and a ⋯ on the title
  row, and the column's spare height split the way Apple's is — a quarter under the sleeve, the rest
  above the volume row.
- **The lyrics view.** One title block, not two: the artwork shrinks to a thumbnail in a header row
  and the words take the whole middle of the screen.
- **The album page.** Sentence case under the title, as Apple writes it, and a track by the album's
  own artist no longer repeats that artist on every row.
- **Favourites, search and the mini player.** A favourite flips under the finger instead of after a
  round trip to the server; tapping Search raises the keyboard even when the screen is already open;
  the mini player rises with the finger and hands over to the full player part-way through the drag.

## Not done

- **Bit-perfect at 24 bit.** media3's sink writes 16-bit or float and nothing else, so a DAC that
  only offers bit-perfect modes at 24 or 32 bit is told so rather than driven. Feeding one needs an
  integer output path: an audio processor at the end of the chain that widens to
  `ENCODING_PCM_24BIT_PACKED`, and the sink opening the track at that encoding. `DacState.blockedBy`
  already says this to the user.
- **Apple's Human Interface Guidelines were never read.** The numbers in `Design.kt` were measured
  off screenshots, not taken from the type scale, standard margins, separator insets and row heights
  under `https://developer.apple.com/design/human-interface-guidelines/`. Do not invent numbers: a
  wrong one is worse than a missing one, because it gets implemented literally.
- **The album page's Play pill** is a solid fill of the page accent. Apple's is a translucent
  capsule with the accent as its content colour. Unverified against a real screenshot of the album
  page — the App Store set below does not include one.

## Reference material

Apple's own App Store assets (iOS 26 era). Fetch with a browser User-Agent; each base URL takes a
trailing size segment, so request them large.

```
https://is1-ssl.mzstatic.com/image/thumb/PurpleSource221/v4/09/d2/62/09d262c5-7fb2-0e48-6545-4a3e16dffb14/iPhone6p9-iOS26-USEN-Music-Wrapper1.png/1290x2796bb.png
.../PurpleSource211/v4/62/61/29/626129da-b55a-c00a-7629-da095e947ba9/iPhone6p9-iOS26-USEN-Music-Wrapper2.png/1290x2796bb.png
.../PurpleSource221/v4/66/f9/e7/66f9e733-0174-ee61-1963-0ce4ceb518a6/iPhone6p9-iOS26-USEN-Music-Wrapper3.png/1290x2796bb.png
.../PurpleSource221/v4/c5/f0/46/c5f046ff-4e33-2395-2c4f-9ae04149801b/iPhone6p9-iOS26-USEN-Music-Wrapper4.png/1290x2796bb.png
.../PurpleSource221/v4/c7/aa/89/c7aa89f7-b2c0-58de-bc20-8a710dc90671/iPhone6p9-iOS26-USEN-Music-Wrapper5.png/1290x2796bb.png
.../PurpleSource221/v4/67/04/a6/6704a634-d243-e850-2b93-fd38c1915e62/iPhone6p9-iOS26-USEN-Music-Wrapper6.png/1290x2796bb.png
```

Wrapper 3 is the lyrics view, 4 is the player, 2 shows a playlist, 5 and 6 the mini player and tab
bar. They are listed on `https://apps.apple.com/us/app/apple-music/id1108187390`, so the list can be
rebuilt if those URLs rot. The images are not committed: ~29 MB of someone else's copyrighted
marketing material, and one curl away.

## How to work on this app

`tools/app.sh` drives a **debug** build over adb without touching the screen, which is the only
reliable way to test this app — see the warnings below. `open <route>`, `play "search:…"`,
`do "download album:<id>"`, `do "dac <spec>"`, `set limiter true`, `state` (one JSON line: route,
playback, DSP, downloads, lyrics, DAC). `tools/audio-e2e.sh` and `tools/feature-e2e.sh` build on it.
`tools/apk.sh` builds a release APK for a phone (arm64 by default; it lands in `build/`, and
**never** copy it into the user's home directory — they asked for that explicitly).

### Testing a USB DAC without a USB DAC

One cannot be attached to an emulator, so `app.sh do "dac Topping E30@44100/16,96000/24"` points the
app at a fake one offering exactly those bit-perfect modes, and tells `Outputs` a USB device is
attached. `dac off` hands it back to the audio system. The state dump then answers `dac`,
`bitPerfect`, `dacModes`, `dacBlocked`, `dacTrack` and `offloadWanted`, which is enough to check the
whole decision — including the part that was actually broken, offload standing down. The checks at
the end of `tools/feature-e2e.sh` do this.

What a mock cannot prove is that a real DAC makes a sound. When the hardware is to hand, plug it in,
play something, and read Settings → Audio: the line under the toggle now says what the AudioTrack
was opened with and whether it was offloaded.

### Testing this app is full of traps

Every one of these produced a wrong conclusion in an earlier session:

- **The media session's position does not move while music plays.** Periodic updates are switched
  off deliberately to save wakeups. A test that watches it passes in silence.
- **A `MediaController` in the background reports a stale position**, so the app's own numbers lie
  too while it is not on screen.
- **`BurstSink.bytesWritten` is honest but bursty** — ten seconds of audio are written at once, then
  nothing for about eight. A three-second sampling window sees zero and calls it silence.
- What can be trusted: `adb shell dumpsys audio` showing our `AudioTrack` as `state:started`, plus
  sink bytes measured over a full buffer cycle. `tools/audio-e2e.sh` does exactly this.
- **A sleeping device answers `uiautomator dump` and `screencap` with stale content.** Taps go
  nowhere and the screenshots look plausible. Check `dumpsys power | grep mWakefulness` first.
- **The on-screen keyboard eats automation.** A fling across it is glide typing. `tools/ui.sh kb off`
  disables every IME for a run; `input text` does not need one.
- **`do star` with no argument does nothing** — the verb needs a song, as in `do "star song:<id>"`.
- **The emulator's own settings are not the defaults.** Crossfade and crossfeed left switched on
  there mean offload can never be asked for, so a check of the offload rules measures nothing until
  they are turned off.
- **`tools/perf-suite.sh` installs `app-release.apk`** — it now builds it first, but if you change
  that, remember two of its runs once measured a day-old binary.

### Bug classes this codebase keeps producing

Check for these before believing a screen is fine — each has bitten more than once:

1. **`Surface` with a computed colour and no `contentColor`.** Material resolves it to
   `Color.Unspecified` and text inside renders almost black, so a title ends up dimmer than its own
   subtitle. Always pass `contentColor`.
2. **Gestures keyed on something that changes while dragging.** `pointerInput(list)` restarts when
   the list reorders and the drag dies; tracking a dragged item by index has the same effect. Key on
   a stable identity, or do not reorder until the finger lifts (the queue does the latter).
3. **State read at composition time inside a `pointerInput(Unit)` block.** The block is created once
   and keeps the values it captured. Read `MutableState` or a view model at event time, or keep the
   flag inside the gesture block itself (the mini player's `riseToPlayer` does).
4. **Sentinels used in arithmetic.** `AudioSink.getCurrentPositionUs` returns
   `CURRENT_POSITION_NOT_SET` (`Long.MIN_VALUE`) when stopped; subtracting it produced an enormous
   "already buffered" figure and the sink refused audio for ever, which is what made playback silent
   after a background pause.
5. **Player or `DownloadManager` touched off the main thread.** Both throw. `PlayerConnection.with {}`
   posts to the main looper; new code that reaches a media3 object directly must do the same. The
   audio track provider runs on the playback thread, so anything it triggers posts to `main` first.
6. **Scrolling screens under the floating chrome** need `LocalChromeInset.current` as bottom content
   padding, or their last row cannot be reached.
7. **Implementation detail leaking into user-visible text.** The settings copy has been cleaned once;
   new strings keep reintroducing threads, buffers and "Rust core".

## Standing instructions from the owner

- Commit messages: one line, no attribution lines, no `Co-Authored-By`.
- Do not run the long performance suite for small or UI-only changes — build, install, screenshot.
  Measure only when something can plausibly move CPU or battery.
- Build artefacts stay in `build/`. Never copy them to `~`.
- The look is Apple Music: artwork bleeding into the page, floating chrome, no Material defaults. No
  liquid glass, and nothing that reads as an Android navigation bar — a patch behind the selected tab
  was rejected three times, in circle and rectangle form. It is colour, weight and size only.
- Do not fake word-by-word lyric timing. The sweep runs only when the lyrics genuinely carry per-word
  timings; line-timed lyrics simply light up.
- Test features end to end rather than declaring them done. A screenshot proves a screen renders, not
  that a feature works.
