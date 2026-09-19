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
- **The player against Apple's.** The sleeve runs to all three edges — **including up under the
  status bar**, which is the point: Apple's artwork has no top edge, and stopping ours below the
  handle drew a line across the screen. The handle and the close button float over it, with the same
  shade under the status bar the album page uses. Three plain transport glyphs (double triangles, as
  Apple draws them), a volume slider with no knob, a favourite and a ⋯ on the title row, and the
  column's spare height split three ways as Apple's is: about 7 % of the screen under the sleeve,
  10 % over the volume slider, 11 % under the bottom icons. Every row of the control stack now lands
  within a percent or two of `w4`; measure a change against `/tmp/crops/w4_screen.png` the same way.
- **The lyrics view.** One title block, not two: the artwork shrinks to a thumbnail in a header row
  and the words take the whole middle of the screen.
- **The album page.** Sentence case under the title, as Apple writes it, and a track by the album's
  own artist no longer repeats that artist on every row.
- **The seek row's centre label.** Apple puts a word between the elapsed and remaining times while a
  transition is running; ours now reads "Mixing" for exactly as long as `TransitionSink` is out of
  `Phase.PASS`. Nothing new watches it - the seek bar is the only thing that ticks while the player is
  open, so it asks on the same beat. Note while testing this: the media session's position pins at the
  outgoing track's duration while the mixed tail plays, which looks like a stall and is not one.
- **Favourites, search and the mini player.** A favourite flips under the finger instead of after a
  round trip to the server; tapping Search raises the keyboard even when the screen is already open;
  the mini player rises with the finger and hands over to the full player part-way through the drag.

## The page colour

Apple's player is not painted one flat colour. Sample across their screenshot and it varies both
ways — at `y=1100`: `118,27,25  99,29,25  84,16,34  88,28,27  78,19,18`; at `y=2000` it is still
red but darker. The background *is* the artwork, enormously enlarged and blurred, which is why it
matches the sleeve so exactly.

One average of the cover's bottom rows cannot do that. *In Rainbows* is vivid everywhere and near
black along its bottom edge, so the page came out brown mud next to a rainbow. `CoverColors.washOf`
now shrinks the cover to 16 px a side, smooths it once off the main thread, pulls every pixel to
within 0.05 of the page colour's own lightness (0.035 in light mode) and holds its saturation back —
then `Design.drawPageWash` draws that stretched over the page and lets the GPU's bilinear filter do
the enlarging. The hues vary the way the record's do; the contrast text needs does not move.

It is still static: one 4 kB texture per cover, uploaded once, drawn as one quad. Neither fill covers
the whole page — the seam gradient is opaque down to 42 % and gone by 68 %, so each is clipped to
where it shows. Sixteen pixels a side held the colour but no shape, and the sleeve read as stopping
dead where the sharp artwork ended; thirty-two keeps enough of the record's forms that the picture
seems to carry on behind the words, for the same one quad.

**The player only.** It was tried on the album page and taken out again. That page has a list
scrolling over it and an artwork that fades under the parallax, and every edge those give the wash is
one more thing for it to disagree with: stretched over the header alone it squashed the whole cover
into a few hundred pixels (visible as bands of the record's colours behind the title) and ended on
the sleeve's dark bottom rows with a line across the page; drawn on the page instead it stayed put
while the list scrolled into it, so the top rows always sat on colour. The album page keeps
`pageBrush`, which has none of those problems.

## Animation

There is one, and it is the exception to "nothing animates unless the user touched it": the four bars
where a playing track's number would be (`Components.PlayingBars`). The owner asked for it. They move
only while the music sounds, freeze into a fixed shape when it is paused, and stop entirely when the
screen goes off or the row leaves the composition. The phase is read in the draw phase, so a frame
invalidates that 16 dp box and nothing else. The screen-off benchmark below is unchanged by it.

## The album page's seam

`HeroPage` draws the cover's edge colour under the artwork so the picture runs out rather than
stopping. The artwork above it is in a parallax layer that does two things as the page scrolls — it
is *drawn* 0.4 of the scroll lower than it is laid out, and it fades to half — and the gradient has
to follow both, or the picture goes pale and shifts while the colour it melts into does not. That is
a hard line straight across the page, and it was there before the wash work went anywhere near it;
the gradient now starts where the picture actually ends and fades by the same amount.

Measure this, do not eyeball it: sample a column down the right edge and look for a step of more than
about 15. A one- or two-pixel step is a row divider and is meant to be there.

## What the audio path costs

`tools/bench.sh dev.flint.music 90 off`, same album, fresh install, on an x86_64 emulator, before
this work and after it:

| | `2670679` (before) | `c9c6910` (after) |
|---|---|---|
| CPU | 3.04 % of one core | 2.82 % of one core |
| wakeups | 411 /s | 425 /s |
| quiet seconds | 75 of 90 | 72 of 90 |
| PSS | 239 MB | 225 MB |

Within the noise of a debug build on an emulator, and "quiet" stays around the 80 % the house rules
ask for. The work added nothing per buffer or per frame: the audio track provider runs once when a
track is opened, and `Outputs.usb` only emits when something is plugged in. With the page wash and
the playing bars on top of it the numbers are the same again — 72 of 90 quiet seconds, 392 wakeups a
second, and no UI thread anywhere in the busiest list, because neither runs while the screen is off.

## What the page costs to scroll

`tools/scroll.sh dev.flint.music 12` on an album page, music playing. The emulator renders in
software, so the absolute numbers are dreadful and only the comparison means anything.

| | 50th | 90th |
|---|---|---|
| flat page colour | 73 ms | 93 ms |
| page wash on the album page | 77 ms | 97 ms |
| as shipped (wash on the player only) | 69 ms | 93 ms |

Measured on the same build by making `derive` hand back a null wash, which is the only honest way to
compare. While the album page carried the wash it cost about 4 ms a frame to scroll, roughly 5 % —
the emulator's software rasterizer, where a full-screen textured fill is expensive and a gradient is
not; on a GPU a second full-screen quad is nothing. It was taken off that page for how it looked
rather than what it cost, and scrolling is back at the baseline. The player never scrolls.

Two traps this measurement fell into, both worth knowing:

- **Wake the device first.** `tools/scroll.sh` straight after `tools/bench.sh` swipes at a black
  screen and reports `Total frames rendered: 0`.
- **Do not compare runs with different frame counts.** A run with the playing bars animating rendered
  647 frames against 366, and the extra cheap animation frames pulled the percentiles down to 65/85 —
  which read as "the wash made scrolling faster" and was nothing of the sort. Scroll an album whose
  track is *not* the one playing, so the frame population is the same on both sides.

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
