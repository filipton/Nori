# Motion: every animation in the app, and the state of each

The owner asked for every animation to be looked at and polished, one at a time, starting with the
page transitions ("all of them just animate from the top of the page, it doesn't seem natural; back
needs to be gesture-natural"). This file is the list, so the work can be picked up by anyone: what
each animation is, where it lives, what it does now, what it should do, how to check it, and whether
it is done. Update the status line when an item changes.

The rules these sit under (from `AGENTS.md`): nothing animates unless the user touched it; everything
is read in the draw phase (`graphicsLayer`, `drawBehind`), so a running animation redraws a layer and
recomposes nothing; `reduceMotion()` / `AppMotion.reduce` must turn every one of these into a snap or
a short fade. The feel is Apple Music's, not Material's.

## How to look at one

The emulator drops frames and `screencap` lands wherever it lands, so record and tile:

```sh
(adb shell screenrecord --time-limit 5 --bit-rate 8000000 /sdcard/m.mp4 &); sleep 1.2
adb shell input tap X Y            # or a swipe: adb shell input swipe x0 y0 x1 y1 ms
sleep 3.5; adb pull /sdcard/m.mp4 /tmp/m.mp4
ffmpeg -y -i /tmp/m.mp4 -vf "fps=30,scale=216:-1,select='between(n\,30\,53)',tile=8x3" -frames:v 1 /tmp/tile.png
```

`tools/app.sh open <route>` opens a page without touching the screen; `tools/app.sh open player`
opens the sheet. The back *gesture* is `adb shell input swipe 5 1200 500 1200 350` (from the left
edge); the back *button* is `adb shell input keyevent BACK`.

## The list

Status is one of: **todo**, **doing**, **done (commit)**, **leave** (looked at, fine as it is).

### 1. Page push and pop — `App.kt` `PageMotion`, `NavHost` enter/exit/popEnter/popExit

- Now: every page change, either direction, is the same: the new page fades in while dropping from a
  twelfth of the screen *above* its place (220 ms, decelerate); the old one fades out lifting up. Both
  pages fade, so for a few frames neither is opaque and the window's black shows through - a
  fade-through-black, and the "from the top of the page" the owner means.
- Should: a stack, the way iOS pushes. Push: the new page slides in from the right edge, opaque, the
  whole width, over the old page, which moves a third of the way left underneath and darkens under a
  scrim. Pop: the reverse. No fade on the pages themselves (the pages are opaque; a fade is what made
  the earlier sideways slide read as "flying out of the top left corner" - see handoff.md, "Page
  transitions"). About 350 ms, decelerating hard (most of the travel in the first half).
- Check: home → tap an album → back button. Both pages must stay opaque throughout; the album must
  arrive from the right, not from above; the home page must be visibly *under* it, shifted, and come
  back to place on pop.
- Done as: `PageMotion` (push/pop slides, 200 ms tap / 140 ms gesture finish, sharp `Settle` ease; tab roots cross-fade) and `Page`
  (paints the page's background while it moves, and the scrim on the page underneath, as a child of
  the page's own transition so the gesture scrubs it). Every route goes through `page(route)`.
  Two things the old fades had been hiding had to go with it: the home page replayed its sections'
  staggered arrival on every return (now once per process, `HomeScreen.arrived`), and `LoadBox` faded
  a page's content in from the page's background even when the data came within a quarter second
  (now a snap inside `QUICK_LOAD_MS`; the fade is only for a page that really showed its loader).
- Known: the first ~100 ms of a pop are lost to composing the page that is coming back (Compose
  Navigation does not keep it); on a debug build on the emulator that is most of the slide. Judge
  the pop on a release build (`tools/apk.sh x86_64 --install`). The push shows the page's bare
  background for as long as its data takes to arrive - see item 16.
- Status: **done**

### 2. The back gesture — `App.kt` `PredictiveBack`, `NavHost` predictivePop*

- Now: a linear fade-through scrubbed by the finger: the page leaving fades out and lifts a tenth of
  the height; the one underneath fades in and drops from above over the second half.
- Should: the pop of item 1, scrubbed: the page follows the finger to the right, the page underneath
  slides from a third left back to place and its scrim lifts. Let go, it finishes; cancelled, it runs
  back. Nothing fades.
- Check: album → swipe from the left edge slowly and hold: the album should sit part way off to the
  right with home visible and shifted under it. Release: it completes. Swipe and swipe back: cancels.
- Done as: the pop of item 1 with `LinearEasing` over 140 ms (`PageMotion.popEnter/popExit(scrubbed = true)`);
  `PageMotion.scrubbed` tells `Page` to run the scrim linear too. Finger scrub seeks; 140 ms is only
  the leftover after release / cancel.
- Status: **done**

### 3. Tab switches — `Nav.tab`, same `NavHost` transitions

- Now: Home → Library etc. use the same page transition as a push. Tabs are siblings, not a stack, so
  a direction is wrong here (and Apple Music does not animate a tab change at all).
- Should: no direction: a short cross-fade (about 150 ms) or nothing. Told apart from a push by the
  route: the four tab roots are `home`, `search`, `library`, `settings`, and a transition between two
  of them is a tab change; anything else is a push/pop.
- Check: tap Library then Home: nothing slides.
- Done as: `PageMotion.tab()` - both routes in `roots` - returns a 100 ms fade and sets `pop = null`
  so neither page is dimmed.
- Status: **done**

### 4. Previous / next buttons pressed quickly — `PlayerScreen.kt` `SleeveCarousel`, the `asks` channel

- Now: presses queue (a channel of four); each waits for the record before it to finish sliding,
  hurried up to three times as quick. Four fast presses are four full slides, so the records go on
  scrolling after the finger has stopped - it reads as lag, and the owner said so.
- Should: spammable with no backlog. A press while a record is in flight commits that change at once
  (the song changes now) and the record coming in carries straight on from where it is to become the
  one going out - one continuous scroll, the motion never more than one slide behind the thumb, and
  it stops when the presses stop. A press on a record at rest is the same full slide as now.
- Check: on the player, tap next five times in about a second. Five songs later, and the records
  stop moving within about a third of a second of the last tap. `tools/app.sh state` shows the index.
- Done as: the channel and `queued` are gone. `slide.run` cancels the slide in flight
  (`moving.cancelAndJoin`) and starts a new `land`; `land`'s cancel path sees `presses` has moved on
  and commits the change with the record left where it is (`arrive(offset - go * span)`), and the new
  slide starts from that offset with the speed the record had (`speed`, written by the animate
  callback) at 1.5x the button stiffness. The wait for the player to catch up before a slide
  (`stale`) is 250 ms, down from 500. `PlayPauseGlyph` shows pause while buffering with play-when-ready
  (it flashed the play arrow after every skip). Checked with adb: 5 taps 180 ms apart → 5 songs, last
  slide over ~400 ms after the last tap; swipe, tap-then-swipe, prev-then-next all count once each.
- Known: between two records with different covers the slide waits for the player to answer the
  change before it (the neighbour's cover is read from `state.index`); on the debug emulator that is
  100-250 ms of the record sitting lifted, on a phone much less. Removing it means the carousel
  looking further down the queue than one song.
- Status: **done**

### 5. Player sheet open / close — `PlayerSheet.kt`, `App.kt` `PlayerLayer`, `FlyingCover`

- Now: a critically damped spring (stiffness 420, 700 on a back-gesture close) carrying the finger's
  velocity; the page behind darkens to 45 %; the sheet's corners round in flight; the cover flies from
  the now playing bar's thumbnail into the sleeve. Recently tuned with the owner.
- Should: as is. Look only if something else changes under it.
- Status: **leave**

### 6. Player panel change (artwork ↔ lyrics ↔ queue) — `PlayerScreen.kt` `arrival`, `PanelFlight`, `PANEL_MS`

- Now: a 360 ms dissolve driven by one `Animatable` for the whole screen; the cover flies between the
  sleeve and the lyrics header's thumbnail; the transport is shared (not faded). Recently tuned.
- Should: as is.
- Status: **leave**

### 7. Record swipe on the sleeve — `SleeveCarousel` (finger), `land`

- Now: follows the finger, decides at a third of the width or a 1000 px/s flick, springs at 520 (a
  finger) or 950 (a button). Recently tuned; a same-album swipe bug was fixed in the last commit.
- Should: as is.
- Status: **leave**

### 8. Now playing bar swipe — `Chrome.kt` `SwipeCarousel`

- Now: the same as 7 at the bar's size, spring 560.
- Should: as is.
- Status: **leave**

### 9. Home page sections arriving — `HomeScreen.kt` `rememberArrival`, `Arrival`

- Now: on the first composition of the page the sections fade up and rise into place one after
  another over about a third of a second. Runs once per visit to the route.
- Should: with item 1 the page itself now slides in, and a second movement inside a moving page is
  busy. Keep it for the app's first page only (a cold start, when there is no push), and skip it when
  the page arrives by a push or a pop. Decide after seeing 1 on the device.
- Check: cold start → home: the sections stagger in. Album → back → home: they do not.
- Done as: `HomeScreen.arrived`, a process-wide flag; the run happens the first time only.
- Status: **done**

### 10. Album / artist / playlist page — `HeroPage.kt`

- Now: nothing animates on arrival; the cover parallaxes and fades with the scroll (draw phase).
- Should: as is; the push (item 1) is the arrival.
- Status: **leave**

### 11. Loaders — `Components.kt` `LoadBox`, `Design.kt` `LoadingDots`, `loadingSheen`

- Now: content fades in over the loader (260 ms after 60 ms); the dots stay invisible for the first
  quarter-second and never show for a fast load; the cover sheen only runs for a picture that is
  really on the way.
- Should: as is.
- Status: **leave**

### 12. Song menu — `SongMenu.kt` `ModalBottomSheet`, the "More" row

- Now: Material's sheet spring; "More" expands in place (`expandVertically`), its chevron turns.
- Should: as is unless the owner says otherwise; the sheet's own motion is the platform's.
- Status: **leave**

### 13. Lyrics — `LyricsView.kt` (line change, scroll)

- Now: the line arriving fades and rises 1/40 of its height over 420 ms after an 80 ms wait; the list
  scrolls with a tuned spring. Tuned with the owner on the phone (see handoff, "Animation and
  Android's animation setting").
- Should: as is.
- Status: **leave**

### 14. Seek bar, volume, switches, swipe-row actions, play/pause glyph, downloads screen

- `SeekBar` (one per-frame loop, eases to the position, done in the last two commits); `VolumeRow`
  (180 ms ease on an outside change); `NoriSwitch` (180 ms); `swipeActions` (spring back, colour and
  pop when armed); `PlayPauseGlyph` (fade + scale, 180/140 ms); `DownloadsScreen` (phase icons,
  progress ring, rows moving with `animateItem`).
- Should: as is; each answers a touch or a state change and does nothing otherwise.
- Status: **leave**

### 16. A pushed page shows its bare background until its data arrives — `AlbumScreen` and the other `LoadBox` pages

- Now: the page slides in at once (item 1) but everything in it is behind `LoadBox`, and until the
  server answers (100-300 ms on the emulator, longer on a slow connection) the card is the page's
  background colour with nothing on it. The old fade-through hid this; the slide shows it.
- Should: the page arrives with something on it: the hero's plate, the title if the row that was
  tapped knew it, the wash from a palette already measured for that cover (`CoverTint` cache). A
  skeleton, not a spinner.
- Check: home → tap an album with the network throttled (or a cold server): the card that slides in
  should not be a blank rectangle.
- Done as: `Nav.album(id, hint)` keeps the last few tapped `Album`s; `AlbumScreen` draws `HeroPage`
  from `nav.albumHint(id)` (cover, title, artist, year/count caption) as soon as it is composed, and
  fills in Play/shuffle/songs when `Load.Ready` lands. Deep links and "Go to album" from a song still
  have no hint and wait behind `LoadBox`. Call sites that have an `Album` pass it (home, search,
  library, artist). The same pattern is on artists and playlists (`Nav.artist` / `Nav.playlist`,
  `ArtistScreen` / `PlaylistScreen`); song-menu "Go to artist" passes a stub with the name it already
  knows.
- Status: **done**

### 15. Login → app

- Now: `App` composes `LoginScreen` or the app; the change is a cut.
- Should: a fade would do; once, on sign-in. Low priority.
- Done as: `Crossfade(prefs.loggedIn, …)` around the login and the signed-in tree, 280 ms (snap when
  reduce-motion). No direction - the two screens are unrelated.
- Status: **done**

### 17. Audit: nothing appears on one frame — hinted detail pages + player title

- Found (record tiles of home → album): with item 16 the hero is there from the slide, but
  Play / Shuffle / ⋯ still landed by replacing the heart-only row, and the tracklist appeared fully
  opaque on one frame once the server answered. Player title / artist / album also swapped hard
  when the song changed while the sleeve was still moving.
- Fixed: `HeroPage.awaitingPlay` keeps the transport row (disabled) and a 46 dp slot for ⋯ while
  the detail is on the wire; album / artist / playlist pass it from the hint path. Body content
  under the hero fades and rises once via `Arrive` (300 ms). Player title block cross-fades on
  song id (220 / 160 ms).
- Left: page content that is composed late mid-slide (Compose Navigation), player sheet contents
  that are already on the rising surface, Cover Coil crossfade, loaders, lyrics, downloads,
  switches, seek / volume - already eased or intentional snaps under `reduceMotion`.
- Status: **done**

## Decisions and traps

- The owner rejected a sideways slide once because the pages faded while they slid, which read as
  the page flying diagonally out of the top-left corner. The push in item 1 must keep both pages
  opaque: slide and scrim only, no `fadeIn`/`fadeOut` on the page.
- `NavHost`'s default predictive-back transition scales the leaving page to 70 % over a page that is
  already fully drawn; always give `predictivePopEnterTransition`/`predictivePopExitTransition`.
- Compose scales every animation by Android's animator duration scale; `Prefs.ignoreSystemMotion`
  makes the app ignore that. Test with the emulator's scale at 1 (Developer options).
- Nothing here may tick while music plays with the screen off (`tools/bench.sh`); a transition that
  is over is over, and `PlayerLayer` skips a fully covered page.
