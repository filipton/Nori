# Handoff: where the UI work stopped

Written at the end of a long session that ran into its usage limit mid-task. Everything described as
done is committed and was checked on a device; everything under "Not done" was either started and
abandoned or identified and not begun. Read `AGENTS.md` first — it holds the house rules; this file
only covers what is in flight.

## The one-paragraph summary

The app works and is tested end to end against the owner's real server (`tools/audio-e2e.sh`, 17
checks; `tools/feature-e2e.sh`, 11 checks — both passing at `7233eb0`). The remaining work is visual:
the owner wants the interface to follow Apple Music closely, and a round of research into Apple's
design language was under way when the session ended. Two research agents died mid-flight to a rate
limit, so their findings are lost; what I had already established from the screenshots myself is
written down below so nobody has to rediscover it.

## Not done, in priority order

### 1. The full-screen player does not match Apple Music

I compared our player against Apple's own App Store screenshots (see *Reference material*) and found
five concrete differences. None of them are implemented yet; `PlayerScreen.kt` is untouched by this
comparison.

| Part | Apple Music (iOS 26) | Ours today |
|---|---|---|
| Artwork | Full-bleed square, touches both screen edges, no corner radius, roughly the top 45 % of the screen | Inset ~26 dp each side, 18 dp radius, centred, with a shadow |
| Play/pause | A plain white glyph, no container | Sits in a filled circle |
| Transport | Exactly three controls: back, play/pause, forward | Five: shuffle, previous, play/pause, next, repeat |
| Volume | A slider under the transport, with small speaker glyphs at each end | Absent |
| Title row | Title and artist on the left; a **favourite (star)** circle and a **⋯** circle on the right of the same row | Title, artist and a ⋯ button; no favourite |

Apple's seek row also carries a centre status label between the elapsed and remaining times
(theirs reads "Mixing" during an AutoMix transition, "Sing" in the lyrics view). We have an AutoMix
feature that knows when a transition is planned, so that slot is worth using rather than leaving
empty.

Where shuffle and repeat should go: Apple puts them in the queue view, not the player. Moving ours
into the queue panel's header keeps the transport at three controls.

### 2. The lyrics view should restructure, not just swap panels

In Apple Music, opening lyrics shrinks the artwork to a small rounded thumbnail in a header row
alongside the title, artist, favourite and ⋯ — the lyrics then get the whole middle of the screen.
Ours replaces the artwork area with the lyrics list but keeps the full title block underneath, which
wastes the space the lyrics want. Screenshot `appstore_w3.png` shows Apple's layout clearly.

### 3. The Apple research itself was never finished

Two agents were dispatched and both died to the session limit:

- **Human Interface Guidelines**: fetch and read the typography, layout, materials, tab-bars,
  navigation-bars, lists-and-tables, buttons, color and motion pages under
  `https://developer.apple.com/design/human-interface-guidelines/`. The goal is numbers we can put
  into `Design.kt` — the iOS type scale (sizes, weights, tracking at large sizes), standard side
  margins, separator insets, minimum tap targets, tab bar and row heights, button corner radii and
  heights. One agent got as far as the Dynamic Type table before it was cut off. Do not invent
  numbers: a wrong one is worse than a missing one, because it will be implemented literally.
- **Measuring Apple Music screenshots**: measure artwork width as a percentage of screen, corner
  radii, gaps, row heights, seek-bar height and inset, transport icon sizes, and the mini player and
  tab bar geometry, then list where our numbers differ. PIL is available; measure by cropping and
  inspecting pixels rather than by eye.

### 4. Smaller things noticed and not addressed

- The page title on an album page (`HeroPage`) is centred; Apple centres it too, but the metadata
  line under it uses small capitals in our app and sentence case in theirs. Unverified which reads
  better here; worth one screenshot comparison.
- We show a format/bitrate caption under the player's artist line. Apple shows nothing there. The
  owner's audience probably wants it, but it should be quieter than it is.
- The album page's track rows show a download tick; Apple shows a cloud/download glyph and the ⋯
  menu. Ours is close enough that it was left alone.

## Reference material

The screenshots used are Apple's own App Store assets (iOS 26 era). They were in a session scratch
directory that is now gone; re-fetch them with a browser User-Agent. Each base URL takes a trailing
size segment, so request them large:

```
https://is1-ssl.mzstatic.com/image/thumb/PurpleSource221/v4/09/d2/62/09d262c5-7fb2-0e48-6545-4a3e16dffb14/iPhone6p9-iOS26-USEN-Music-Wrapper1.png/1290x2796bb.png
.../PurpleSource211/v4/62/61/29/626129da-b55a-c00a-7629-da095e947ba9/iPhone6p9-iOS26-USEN-Music-Wrapper2.png/1290x2796bb.png
.../PurpleSource221/v4/66/f9/e7/66f9e733-0174-ee61-1963-0ce4ceb518a6/iPhone6p9-iOS26-USEN-Music-Wrapper3.png/1290x2796bb.png
.../PurpleSource221/v4/c5/f0/46/c5f046ff-4e33-2395-2c4f-9ae04149801b/iPhone6p9-iOS26-USEN-Music-Wrapper4.png/1290x2796bb.png
.../PurpleSource221/v4/c7/aa/89/c7aa89f7-b2c0-58de-bc20-8a710dc90671/iPhone6p9-iOS26-USEN-Music-Wrapper5.png/1290x2796bb.png
.../PurpleSource221/v4/67/04/a6/6704a634-d243-e850-2b93-fd38c1915e62/iPhone6p9-iOS26-USEN-Music-Wrapper6.png/1290x2796bb.png
```

Wrapper 3 is the lyrics view, 4 is the player, 5 and 6 show the mini player and tab bar. They are
listed on `https://apps.apple.com/us/app/apple-music/id1108187390`, so the list can be rebuilt if
those URLs rot. 9to5Mac's article on the iOS 26.4 album redesign has the album and playlist pages.

The images are not committed: they are ~29 MB of someone else's copyrighted marketing material, and
they are one curl away.

## How to work on this app

`tools/app.sh` drives a **debug** build over adb without touching the screen, which is the only
reliable way to test this app — see the warning below. `open <route>`, `play "search:…"`,
`do "download album:<id>"`, `set limiter true`, `state` (one JSON line: route, playback, DSP,
downloads, lyrics). `tools/audio-e2e.sh` and `tools/feature-e2e.sh` build on it. `tools/apk.sh`
builds a release APK for a phone (arm64 by default; it lands in `build/`, and **never** copy it into
the user's home directory — they asked for that explicitly).

### Testing this app is full of traps

Every one of these produced a wrong conclusion during the session that wrote this file:

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
   and keeps the values it captured. Read `MutableState` or a view model at event time.
4. **Sentinels used in arithmetic.** `AudioSink.getCurrentPositionUs` returns
   `CURRENT_POSITION_NOT_SET` (`Long.MIN_VALUE`) when stopped; subtracting it produced an enormous
   "already buffered" figure and the sink refused audio for ever, which is what made playback silent
   after a background pause.
5. **Player or `DownloadManager` touched off the main thread.** Both throw. `PlayerConnection.with {}`
   now posts to the main looper; new code that reaches a media3 object directly must do the same.
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
