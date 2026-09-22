# Changelog

What changed in each release, newest first. Sections are grouped from the commit log by
`tools/changelog.py` — `feat:` becomes Added, `fix:` becomes Fixed, and so on — then read over by
hand. Versions follow [semantic versioning](https://semver.org).

## [Unreleased]

## [0.3.2] - 2026-09-22

### Added

- Album and playlist pages pause their own queue, and Shuffle shows when it is on
- The player's cover blurs as it melts into the page
- Settings pages are split into short named sections, with plainer descriptions

### Fixed

- Page colours follow what the cover really is: a big field of colour beats a black or white border, and a dark strip at the bottom no longer leaves a band
- White covers keep a white page
- Controls stay readable on white pages, and the colours change smoothly between songs
- The player's controls follow the cover's colour while you swipe, with no jump halfway

### Performance

- Snappier page push and pop, and a quicker finish to the back gesture
