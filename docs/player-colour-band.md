# The band under the cover, and the colours of the player's page

Written after a long night of failed attempts on it, so the next person does not repeat them. The
short version: the soft strip where the sleeve meets the page is the hardest thing on that screen to
get right, every fix that changed *when* its colours move was wrong, and the answer is almost
certainly to stop giving it colours of its own.

## What is actually drawn there

Six things are stacked where the sleeve meets the page, all of them taken from the cover:

| Layer | What it is |
|---|---|
| 1 | the page's blur (`drawSleeveWash`), in the colours the page had |
| 2 | the same, in the colours it is settling into, faded in over `washFade` |
| 3 | the same, in the arriving record's colours, faded in with the record's travel |
| 4 | the record itself, sliding, lifted, rounded |
| 5 | the soft bottom (`drawSleeveMelt`), in the colours the page had |
| 6 | the same two steps again, over it |

Layers 5 and 6 are the "band". `drawSleeveMelt` draws the *cover's own blurred bottom rows* over the
last 19 % of the sleeve with a rising alpha, so the picture goes soft instead of ending on a line.

Colour-coding all six in a debug build (each layer a flat primary) is the only reliable way to see
which one is misbehaving; screenshots of the real thing tell you almost nothing, because five of the
six are nearly the same colour at any moment. Do that first, and ask the owner which colour he sees:
his eyes on the emulator found in one message what frame-by-frame sampling had got wrong for hours.

## Why the band reads as "old colour, still, always"

It is not frozen and it is not late. It cross-fades at exactly the same rate as the page. But:

- The band is made of the cover's *bottom rows*, which on a strong sleeve are far brighter than the
  page (Amnesiac: the band's source is about `(128,35,34)`, the page under it settles near `(10,10,9)`
  once the floor gradient has darkened it).
- So half way through a change, the page looks finished and the band still looks like the old record.
  Measured in the owner's own screenshot: page `(10,10,9)`, band `(45,17,16)` - a third of the old
  red, i.e. exactly half way through a fade, not stuck.
- It is also the only thing in that area that does not move with the record. A record slides and
  turns its corners; the band lies still across the page while records come and go over it.

Both of those make it read as a slab of the last record's colour under the cover coming in, which is
what he kept seeing and what no amount of re-timing could fix.

## What was tried, and what it cost

| Attempt | Result |
|---|---|
| Cross-fade the band's colours with the page (three layers, same alphas) | Correct on paper, still read as stale: see above |
| Split the band at the gap between the two records, each side from its own record | A hard seam straight down the middle of the blur. Far worse. Reverted |
| Fade the band out while a record travels, back as it settles | A hard edge under a lifted card where a soft one belonged; "appears out of nowhere" |
| Scale the band and the page with the lift | A band at the wrong scale under a shrunken card |
| Finish the colour crossover early (a twentieth of the travel) | "Finishes too early"; the change no longer belongs to the record's movement |
| Give each record its own band inside its own layer | The band travels and turns with the record, which he did not want, and left a rectangle of the old colour for a frame after the switch |
| Draw the band from the current interpolated tint, no picture at all | No stale colour anywhere, but the band becomes visible as a flat wash - "now that band is visible" |
| Blur what is behind it (`BlurEffect` on a recorded `GraphicsLayer`, Android 12+) | Closest to what he asked for: no colours of its own, so nothing to be stale. Needs the page's own tint over the last rows, where the blur has nothing left to sample and goes dark |

The last one is the direction to finish: a colourless blur of whatever is behind the strip, fixed in
place, with the page's current tint taking over the last rows. It cannot hold a record's colours
because it has none.

## Things worth knowing whatever is done next

- `PageShift` carries how far the record has travelled and which cover it is heading to. Write it in
  the same breath as the record's own offset (`tell()` in `SleeveCarousel`); an earlier version
  sampled it through a `snapshotFlow`, which lagged a frame and skipped frames entirely when the
  phone dropped them.
- `rememberCoverTint` must read the palette cache *during composition*. It used `produceState`, whose
  block is a coroutine that runs after the frame, so even a cover measured long ago handed its colours
  over a frame late and everything drawn from them was the last record's until it did.
- The now playing bar measures both neighbours' palettes ahead (`warmCoverPalette`), so by the time a
  record is swiped to, its colours are a map lookup.
- The page's floor must end on the page's own colour. Darkening it a third of the way to black was
  invisible while every page was dark and split the screen in two once bright records got bright
  pages.
- This emulator drops most frames of a 200 ms animation and its `screencap` lands wherever it lands.
  Sampling boxes drawn on a lifted card overlap the card, not the page. Almost every wrong conclusion
  in this round came from trusting those numbers.

## Fixes from the same night that are worth keeping

These were in the commits scrapped with the band work, and were each approved on sight:

- The page colour takes the hue family that covers most of the sleeve (Amnesiac comes out red, not
  black; the Black Album still black).
- The page keeps the record's own brightness, limited only by what the text needs, instead of a flat
  dark ceiling.
- The page's colour runs to the bottom edge instead of settling onto a darkened floor.
- The heart and ⋯ discs carry their own contrast, so a white glyph never sits on a pale disc.
- Search results swipe like every other list of songs.
- A panel no longer blinks at full strength for a frame before fading in.
- The status bar's shade travels with the flying cover.
