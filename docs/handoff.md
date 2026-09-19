# Handoff: where the work stopped

Read `AGENTS.md` first — it holds the house rules; this file only covers what is in flight and the
traps that make this app easy to test wrongly.

## The one-paragraph summary

The app works and is tested end to end against the owner's real server (`tools/audio-e2e.sh`,
`tools/feature-e2e.sh`). The Apple Music comparison that the previous session started has been
carried out: the full-screen player, the lyrics view and the album page were measured against
Apple's own App Store screenshots and the differences closed. What is left is listed below.

## Recently closed

- **The record change, properly.** Three faults sat on top of each other in SleeveCarousel, all from
  the same root: a `pointerInput` keyed on `Unit` is created once and never replaced, so the gesture
  closed over the first composition's addresses and painters - back then there was no queue at all.
  The landing was filed under an address the sleeve could never match, so the record sat in the
  middle at its lifted size, over the whole change, until the timeout let go of it (that is the
  "static smaller cover covering the animation"); a painter caught that early has no picture in it,
  so a record could land with nothing to draw and the cover being left stayed put for a few frames
  (the blink). Everything a gesture or the button queue reads now goes through `rememberUpdatedState`.
  The landed record also travels with the drag instead of sitting in the middle, so a swipe during a
  change no longer has a second cover pinned over it, and a landing that is cancelled leaves the
  offset alone if a finger has taken the record over - putting it back wiped the new drag's first
  half, which is why a swipe straight after a swipe went nowhere.
- **Buttons make the same move.** Next and previous lift the record, send it out one side and settle
  the new one into the sleeve, exactly as a thumb does, with a stiffer spring (`BUTTON_STIFFNESS`)
  and a quicker settle. Presses queue rather than interrupt (a small channel), and the record stays
  up between them, so four quick presses are four songs and four changes. Springs animate to within a
  pixel now, not a hundredth of one: the default threshold made a quarter-second move take half a
  second.
- **A suite flake.** The downloads test picked a random album and expected songs to start
  downloading; an album an earlier run had already fetched has nothing to do and failed it. It now
  tries up to four candidates until one has work left.
- **The transport and the record.** A skip asked for while the music is paused starts it playing
  (`Controls.andPlay` in PlaybackService, so the notification and a headset do it too); the service's
  own skips - an explicit track, a track that will not play - go to the player underneath and leave a
  paused queue paused. The player's times refresh on a track change even while paused (`position`
  takes the song as a key), instead of leaving the last song's 2:50 under the new song's title. The
  transport's skip buttons now send the record across exactly as a swipe does, a little quicker
  (`SleeveSlide`, `BUTTON_STIFFNESS`); a previous press that only rewinds the current song - media3's
  three-second rule, which the button repeats so the sleeve and the sound agree - does not, because
  there is no other record to show.
- **Flicked records.** Both carousels kept their offset in an `Animatable` and snapped to it from a
  coroutine per pointer event. On a flick several of those were still queued when the finger left and
  landed on top of the settle that had already started, dragging the record back mid-change - the
  jerk you could see when a swipe was let go early with momentum. The offset is now plain state
  written straight from the drag, with one cancellable job for the settle or the landing, and a
  landing that is cancelled still changes the song so a quick second press is not dropped.
- **Back gesture on the player.** `PlayerSheet.isOpen` is what the sheet was last asked to do, not
  where it is: it came from the Animatable's target, so the first pixel of a drag or a back gesture
  read as "closed" and switched the back handler off underneath the finger - the player never sank
  and then vanished in one frame instead of settling onto the now playing bar. The back gesture's
  close is a little quicker than the close button's (spring stiffness 700 against 420), and the page
  back gesture lets go of the page sooner too (`PredictiveBack.MS`, 320 -> 240).
- **Scrubbing.** The seek bar owns the pointer from touch-down and consumes every move, so the player
  sheet's vertical drag can no longer take a scrub that runs a few degrees off level - that was the
  bug where the bar followed the finger, the time changed, and the song never moved, because the
  gesture ended in `onDragCancel`. The strip is 34 dp tall (26 dp was easy to miss with a thumb), the
  bar thickens and grows a dot while held, and the seek happens once, on release. `build/seek.sh
  <slant px per step> <end x>` scrubs it on the emulator and prints where it landed.
- **Long titles read themselves out.** `Modifier.readable()` in PlayerScreen (`basicMarquee`) walks a
  title that does not fit, after a 2.6 s pause, in the full player, the lyrics header and the now
  playing bar. The last one passes `iterations = 2` - it reads itself out when the song comes on and
  then settles back - because that bar is on screen for as long as the app is, which is the same
  reason it has no progress bar. On the full player it runs while you are looking at it and not
  otherwise (`LocalPlayerShown`): the player stays composed behind the rest of the app, and a title
  scrolling down there would hold the frame clock awake. Measured on the emulator: 0 frames in 5 s
  with the player closed, ~37 fps while it is open and a title walks, and the now playing bar goes
  quiet (0 frames per 10 s) about 40 s after a long title starts.
  A marquee lays its text out unbounded, so there is no ellipsis; the line goes soft over its last
  20 dp instead (an offscreen layer and a `DstIn` gradient in `readable`).
- **Up Next.** Play next and Add to queue (the default right swipe) work as in Apple Music: the songs
  go right after the playing one, "last" ones after the songs added by hand before them, in order,
  and then the queue carries on. Items carry `queued` = "next"/"last" in their extras
  (`MediaItem.queued`); the service's `Controls.addMediaItems` sends marked items to `upNext`, which
  places them in the list and, under shuffle, rebuilds the `DefaultShuffleOrder` so they are not
  scattered. Turning shuffle on puts the playing song first, keeps the hand-added run after it and
  shuffles only the rest (`shuffleAroundCurrent`). The queue panel lists songs in play order
  (`PlayerState.order`), marks hand-added ones, and hides reordering under shuffle. Checked with
  `build/upnext.sh`-style runs via `do enqueue|playnext|shuffle` and the `upNext` state field.
- **Swipes.** A song row moves only in a direction that has an action. Right adds to the queue, left
  favourites (or unfavourites) by default; the left one is stored under a new key, `swipeLeft3`, so
  old installs get the new default too. The drag uncovers the
  action's icon and words; past 30% of the width the strip turns accent, the phone ticks and the row
  goes heavier, and letting go acts. 5-star ratings are gone (nobody used them).
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
- **The output switcher.** The middle glyph at the bottom of the player is where the sound is going,
  as Apple's AirPlay mark is: a cast glyph on the phone's speaker, headphones or Bluetooth in the
  accent colour when something else carries it. It opens Android's own output picker: on 14 and
  later through the public `MediaRouter2.showSystemOutputSwitcher()`; on 11-13 through SystemUI's
  `LAUNCH_MEDIA_OUTPUT_DIALOG`, which is a **broadcast** — the first version started it as an
  activity, which cannot resolve, so on a real phone the button only named the output; on 10 through
  the Settings panel. Cast speakers will not appear in it until the app implements casting
  (`docs/features.md`, build step 8); USB, Bluetooth and wired outputs do. The sleep timer
  that used to sit there is on the player's ⋯ instead — `LocalPlayerMenu`, the same song menu with
  the playback-wide entry added, so a row's menu in a list does not grow one.
- **Favourites, search and the mini player.** A favourite flips under the finger instead of after a
  round trip to the server; tapping Search raises the keyboard even when the screen is already open;
  the mini player rises with the finger and hands over to the full player part-way through the drag.

- **Downloads.** Up to "Downloads at once" (Settings → Library, 1-10, default 5) run in parallel, in
  the order asked for; that is media3's own queue, `maxParallelDownloads` kept in step from the
  foreground-notification tick. Progress comes from wrapping media3's downloaders
  (`TrackedDownloaders`), not from polling, and passes a `ProgressGate` (4 a second, whole percents)
  into one flow per song. `Downloads.marks` changes only on a phase change, so song rows
  (`DownloadSlot`) recompose on those and one ring per downloading song recomposes on progress.
  `downloads` is a route; the notification opens it with `ACTION_OPEN_DOWNLOADS` (onNewIntent when
  running). Traps found the hard way:
  - `DefaultDownloaderFactory`'s executor runs the byte copying. A small pool there caps the
    downloads that move at the pool size while media3 still reports all of them as downloading (5
    rings, 2 filling). It is `Runnable::run`: each download copies on media3's own task thread. The
    stream `Dispatcher` (media3's OkHttp source enqueues on it) must also have room for 10 + playback.
  - Nothing about a download may live only in memory. media3's `DefaultDownloadIndex` survives a
    force stop; the manager only resumes it once something starts `DownloadWorker`. `Downloads.reconcile`
    (at launch, and again from `ActionsViewModel`) squares the Rust index with media3's and starts the
    service; the batch the notification counts (`DownloadBatch`) is fed only from the manager's
    callbacks, `onInitialized` included, so it is rebuilt the same way.
  - The notification's total is the batch's: everything queued since the queue was last empty. The
    result goes in its own id (1002), because the service takes 1001 away when it stops.

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

## The sleeve is not square

Album art is square, Apple's included — so how does their player's artwork touch the top edge of the
screen *and* reach down behind the title, which a full-width square cannot do? It is the square
scaled up and cropped at the left and right edges to fill a taller box. Crop `w4` across the row
where a full-width square would have ended (y = 977 of a 977-wide screen) and the flowers below that
line are exactly as sharp as the ones above it, with a strip of red tape crossing it unbroken. It is
the picture, not the blur behind it.

`PlayerScreen.SLEEVE` is that ratio and `Cover` already crops. Everything else follows from it: no top
edge because the picture starts at y = 0, and the picture's tail reaching the title because it ends
past half the screen. It is 0.80 rather than the 0.88 measured off `w4`, because this screen is 20:9
against the 19.5:9 that was measured; check it by sharpness rather than by the number. Row by row,
`w4` against ours: 44 % 9.9/9.8, 46 % 9.9/10.0, 48 % 8.2/9.1, 50 % 5.2/4.4, 54 % 1.1/3.2. It costs
about an eighth of the cover off each side, which is the price of the sleeve reaching both ends.

**The wash has to be quieter than Apple's, not equal to it.** Measured across the page below the
sleeve, Apple's colour varies *more* than ours ever did - channel spreads of 36/21/17 against our
8/5/5 now - but all of their variation is inside one red, because that cover is one hue. A cover that
is teal down one side and warm down the other gives the page teal and warm patches at the same
spread, and a patch reads as a fault where a glow does not. `CoverColors.MUTE` pulls every pixel most
of the way back to the flat page colour after the lightness clamp; that is what makes it subtle
without making it grey.

Below that, `Design.drawSleeveMelt` cross-fades the sharp sleeve into its own blur in slices, and
`drawSleeveWash` draws that blur behind and below it at the same scale, so the two are the same
picture and the join cannot be seen. **Nowhere in either is there a flat colour**, which is the whole
point — every earlier version faded the picture onto some computed colour, and that colour met the
page along a dead straight line every time.

Two bugs to know about if you touch the slicing: a band of 14.2 px drawn as `band.toInt()` = 14
leaves a fifth of a pixel behind on each slice, and by the last one that is six rows of raw, unmelted
cover lying across the bottom of the sleeve — a bright hairline. Round the *edges*, not the heights,
and pin the last slice to the sleeve's own bottom. `drawSleeveWash`'s three bands have the same trap.

## Sizes, measured

Everything on the player was measured against `w4` as a share of the screen's *width*, so a 977 px
iPhone and a 1080 px Android compare directly. Apple / ours after the change:

| | Apple | ours |
|---|---|---|
| title top, artist top | 56.5 %, 59.3 % of height | 56.3 %, 59.3 % |
| cover detail at 53 / 55 / 57 % of height | 5.0 / 1.9 / 0.8 | 4.5 / 2.4 / 0.7 |
| side margin (title, seek bar, discs) | 8.2 % | 8.3 % (`PLAYER_GUTTER`, 33 dp) |
| pause glyph height | 9.8 % | 10.0 % |
| skip glyph width | 9.7 % | 9.6 % |
| seek bar thickness | 1.64 % | 1.67 % |
| volume bar thickness | 1.84 % | 1.76 % |
| title-row disc | 7.9 %, glyph 60 % of it | 7.9 %, glyph 60 % |
| bottom icons | 5.4 × 5.1 % | 5.5 × 5.0 % |

The title sits over the sleeve's blurred tail, as Apple's does: the sleeve is laid out shorter than it
is drawn (`SLEEVE_UNDER_TEXT`), and its melt is quick-then-long (`1 - (1-t)³`) so a faint trace of
the cover is still there behind the title, which is what the numbers above show on theirs.

Settings and menus had two Material shapes left in them. The switch is now UISwitch's 51 × 31 pt
track with a 27 pt white thumb (`FlintSwitch`), and `FlintSlider` is UISlider's 4 pt track with a
28 pt white knob on a soft shadow. Neither of those was measured off a screenshot — there is no
settings screen in the App Store set — they are UIKit's own defaults.

## Gestures and the equalizer

- **The player is a sheet, not a route** (`PlayerSheet`). One number, `progress` 0..1, drives the
  whole transition: the sheet's top edge goes from the mini player's top to the screen's top, the
  page behind darkens, and the cover flies from the mini player's thumbnail into the sleeve
  (`FlyingCover`). A drag sets the number directly, so it follows the finger and holds where it is
  held; a release goes the way of a flick, else finishes once it has come 15 % of the way, else goes
  back. Up on the mini player opens it; down anywhere on the artwork (or on the handle, in lyrics
  and queue) closes it. Modelled on the reference recording of Apple's (owner's `otherappanim.mp4`).
- **The flying cover is laid out once** as the full square at the sleeve's height and moved only by
  a layer transform (scale plus a clip from square to the sleeve's window). Growing it by layout gave
  the image a new size per frame, and every size was a new decode and texture: 300 ms stalls. The
  sleeve and the flight share one painter; two requests for the same picture in one frame decoded
  two bitmaps and uploaded the second at the landing.
- **The player stays composed** once the app has been up 1.5 s, parked a screen below the bottom
  edge (off-screen, so it draws nothing and catches no touch meant for the mini player). Building it
  on the first frame of the drag stalled that frame. Anything in it that ticks or reaches outside
  (seek bar, lyric timing, status-bar icons, keep-screen-on) checks `LocalPlayerShown`.
- **Panels dissolve** (artwork, lyrics, queue): `AnimatedContent`, the old panel held opaque under
  the new one (`ExitTransition.KeepUntilTransitionsFinished`; a zero-length delayed fade-out was not
  held), and the seek bar, transport, volume and icons are shared elements so one copy moves rather
  than two showing.
- **Measuring on the emulator:** it renders with SwiftShader (CPU), so GPU time per frame is 30-60 ms
  even idle and first draws take hundreds. Judge smoothness on a phone; on the emulator, check that
  nothing recomposes per frame (log from the composables) and read the UI-thread columns of
  `dumpsys gfxinfo framestats`. Raw gestures: `adb shell input motionevent DOWN/MOVE/UP x y`, which
  can hold a drag mid-way for a screenshot.
- **The equalizer no longer drops the sound on entry.** Opening it sent `CMD_TUNING`, and the
  service rebuilt the sink (stop, prepare) to swap the 10 s buffer for a shallow one - an audible
  break, on a DAC or anywhere, and again on leaving. Now: nothing on opening; one rebuild on the
  first change to a band, only if the equalizer is in the chain; nothing on leaving; the deep
  buffer comes back at the next pause, where a rebuild is silent. Counted from the `AudioTrack` log
  line: 0 / 1 / 0 / 0 / 1 for open, first change, second change, leave, pause.

## Loading, and nothing appearing in one frame

The owner's rule: with animations on, nothing may appear or change in one frame, and waiting must
look like waiting. The pieces:

- `LoadingDots` (Design.kt): three breathing dots, Apple's lyric-interlude mark. Invisible for the
  first 250 ms, then fades in, so fast loads never show a loader. Used by `LoadBox` (every page's
  loading state, whose content now fades in over it) and by the lyrics, placed where the first line
  will be.
- `Modifier.loadingSheen`: a faint band crossing a cover's plate while it loads, same 250 ms grace.
  Every `Cover` uses it, fades its picture in (coil crossfade, which skips memory hits) and fades in
  the note glyph when there is no picture.
- The player's sleeve (`SleeveArt`) keeps the old cover while the next one loads and cross-fades;
  after 600 ms without it, the old one fades out to the sheen so it never stands under the wrong
  title. The page colours cross-fade with it and hold the old palette while the new one is worked out.
  `PlayerViewModel` warms the covers of the previous track and `Prefs.coversAhead` (default 3)
  upcoming ones, taking shuffle into account via `PlayerState.nextIndex` / `previousIndex`.
- A sideways swipe on the sleeve is a carousel (`SleeveCarousel`). Each record is the cover's whole
  square (wider than the screen, so at rest the screen crops it to the sleeve); held, it lifts - shrinks
  to 86 % of the width, rounds, casts a shadow - so its cropped sides come into view. The neighbour's picture waits off
  the edge and follows the finger in; on commit the old record goes all the way off, and the incoming
  picture stays drawn over the sleeve until `SleeveArt.shownUrl` matches it (`snapNext` makes that
  load skip its cross-fade), so the change has no second step. A swipe back is `previousItem()`
  (always the previous song), not `previous()` (which restarts the song past 3 s).
- Lyrics go back through the loader on every song (`PlayerViewModel.lyrics` starts each song from
  `Loading`), and an empty server answer is not emitted while LRCLIB may still answer. The lyrics
  loader sits centred, where "No lyrics" would be.
- `PlayPauseGlyph`: play, pause and the buffering spinner cross-fade, the spinner only after 300 ms.

## Animation and Android's animation setting

Compose scales every animation by Android's animator duration scale. Plenty of people switch that off
for speed (and GrapheneOS users often do), and at 0 every tween finishes on its first frame - the
owner's lyrics jumped from line to line on the phone while gliding on the emulator. `reduceMotion()`
also followed that switch. `Prefs.ignoreSystemMotion` ("Animate even when Android's are off") makes
`reduceMotion()` ignore it. The speed itself is app-wide: `MainActivity` builds the window's
recomposer with `AppMotion` (a `MotionDurationScale`) in its context, and every animation in the
composition, ours and the libraries' (page transitions, sheets, fades, the lyrics), reads its scale
from there - the system's normally, 1 when the switch is on. It replaced a per-animation override
that only the few animations wrapped in it obeyed. Measured with the system scale at 0: the sheet
opens through intermediate frames with the switch on and in one frame with it off; earlier, off, a
lyric line change was over in 66 ms with 60 % of the movement in one frame, on, 600-730 ms with no
frame over 20 %.

Measure motion with `glide3.py`-style frame differencing (share of a change in its biggest frame),
not by matching vertical shifts: lyric lines are evenly spaced, so "moved one line" and "did not
move" look the same to a shift search.

## The back gesture

Predictive back is on (`enableOnBackInvokedCallback`), so Navigation Compose scrubs a page transition
with the finger. Unless `NavHost` is given `predictivePopEnterTransition` / `predictivePopExitTransition`
it uses its own defaults - the page being left scales to 70 % with no fade over the page underneath,
already fully drawn - which is what the owner saw as the animation "breaking" on a back swipe.
`PredictiveBack` in App.kt is a linear fade-through: the page leaving is gone by 60 % of the way and
slides a fifth of the width towards the edge the finger moves to; the page underneath fades in over the
second half. Cancelled, it runs back. The player sheet takes the gesture itself (`SheetBack`,
`PredictiveBackHandler`): it sinks up to a fifth with the finger and closes on a stiffer spring than a
tap-close (`PlayerSheet.backClose`).

## Interface size

Every size above was measured on a phone 411 dp wide. The owner's phone is about 358 dp wide
(measured off a screenshot: the 64 dp lyrics thumbnail is 17.9 % of its width against 15.6 % on the
emulator), which is Android's display-size setting, and at that width every dp-sized thing is a
seventh larger - the whole app looked zoomed in. `Prefs.uiScale` defaults to automatic, which scales
the app's density so it lays out as if the screen were at least 411 dp wide (`Theme.uiScale`). It
only ever shrinks, and it leaves the system font scale alone, since that one is the reader's.
Checked with `adb shell wm density 483`: pause glyph and title margin come out at the same share of
the width as at 411 dp. `wm density reset` afterwards.

## The album page's seam

An album page has no wash (see above) and no separate gradient under its artwork either. It used to:
the picture faded onto the cover's edge colour and a second gradient below carried that on to the
page colour. But the parallax slides the picture *down* over that gradient as the page scrolls —
`translationY = 0.4 * scroll` — squeezing it into a few dozen pixels, and a colour ramp that steep
across the full width is a line. The dissolve now happens entirely inside the artwork, which has its
own height to do it in, and finishes on the page colour, so there is nothing left to hand over to.
Because the artwork's layer is alpha-faded by the same parallax, its last row is the page colour at
any scroll and at any fade.

Measure this, do not eyeball it. `edges.py` in a scratch directory is twenty lines: sample only the
far left and right margins, where no text or control ever sits, average 14 rows either side of each
candidate, and report a step of 10 or more. Averaging is what makes it useful — it ignores row
dividers and glyph edges, which are one or two pixels tall, and finds the things that are not. Run it
on the player and on the album page at three or four scroll positions; anything it reports inside the
artwork's own rows is the cover's own contrast, not a fault.

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
- **The offline check needs a long song.** `feature-e2e.sh` downloads a random song and samples the
  AudioTrack a dozen seconds after starting it. It once drew a thirteen-second interlude, found the
  track legitimately stopped, and reported that downloads do not play offline. It now picks one of at
  least 90 s. Before believing a failure there, look at the duration it printed.
- **The DAC mock names the output too.** `do "dac Topping E30@..."` makes `Outputs.current` read
  `USB: Topping E30` as well as setting the USB flag, so the player's output glyph and anything bound
  to that output can be checked on an emulator.
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
   flag inside the gesture block itself (`dragsSheet` keeps its velocity tracker there).
4. **Sentinels used in arithmetic.** `AudioSink.getCurrentPositionUs` returns
   `CURRENT_POSITION_NOT_SET` (`Long.MIN_VALUE`) when stopped; subtracting it produced an enormous
   "already buffered" figure and the sink refused audio for ever, which is what made playback silent
   after a background pause.
5. **Player or `DownloadManager` touched off the main thread.** Both throw. `PlayerConnection.with {}`
   posts to the main looper; new code that reaches a media3 object directly must do the same. The
   audio track provider runs on the playback thread, so anything it triggers posts to `main` first.
6. **Scrolling screens under the floating chrome** need `LocalChromeInset.current` as bottom content
   padding, or their last row cannot be reached.
7. **Two clocks in one subtraction.** The sleep label subtracted `System.currentTimeMillis()` from a
   deadline set with `SystemClock.elapsedRealtime()`, got a number about fifty years wide, and the
   `coerceAtLeast(1)` after it turned that into "1 min" for every timer ever set. Anything stored as
   a deadline in this codebase is elapsedRealtime; read it back the same way.
8. **Ranking with a catch-all that beats the default.** `Outputs.rank` gave unlisted device types 5
   and the built-in speaker 9, so a phone's telephony output — every phone has one — was reported as
   where the music was going. Unlisted types now rank below the speaker.
9. **Implementation detail leaking into user-visible text.** The settings copy has been cleaned once;
   new strings keep reintroducing threads, buffers and "Rust core".

## Standing instructions from the owner

- Commit messages: one line, no attribution lines, no `Co-Authored-By`.
- One emulator only: `battery-perf`. No extra AVDs, not even for subagents; they take turns on it.
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
