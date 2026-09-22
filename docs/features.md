# Feature checklist

Living inventory for **Nori 0.3.3**. Every distinct feature found in Symfonium (S), Musly (M) and
Navic (N), merged. The raw, sourced inventories are in `docs/research/` (Symfonium: all 67 release
posts, the docs site, the Play listing and the APK's strings; Musly: changelog, 892 l10n keys,
source at v2.0.2; Navic: releases alpha19-55, 836 commits, source). Provider-specific items that
cannot apply to a Subsonic server (Plex auth, Jellyfin Quick Connect, ...) are left in the raw files.

nori column: `yes` have it, `part` partly. Plan column: **add** = clean win, will be built;
**ask** = costs battery/CPU, a dependency, privacy or a lot of scope, the owner decides;
**skip** = see reason. "cost" notes say what a feature costs *while music plays with the screen off*,
because that is the budget this app protects.

## Decisions (2026-09-17)

The owner answered the **ask** rows:

- Sources: Subsonic only. No Jellyfin/Emby, local files or yt-dlp.
- Audio engine: bundled FFmpeg decoder, USB exclusive driver + DSD output, resampler / fixed output
  rate (+ compressor), smart fades + waveform bar: **all yes, each behind a switch that is off by default.**
- Network: UPnP/DLNA, Chromecast, third-party lookups (LRCLIB, AutoEQ database, update check), mTLS:
  **all yes, each can be disabled.**
- Extras: on-device taste model + mixes + Wrapped: yes. Word-by-word lyrics that scroll smoothly: yes,
  explicitly wanted. Wear OS / Android TV, audiobook mode, Bluetooth lyrics: no.

The rule that follows: **an optional subsystem that is switched off costs nothing** - it is not
initialised, holds no listener, opens no socket and adds no audio processor. Settings has one
"Features" page listing them all.

Build order: 1 connection and servers, 2 library and browsing, 3 queue / playlists / smart playlists /
mixes / taste model, 4 playback behaviour, 5 downloads and storage, 6 lyrics, 7 DSP extras and
per-output profiles, 8 casting, 9 FFmpeg + waveform + smart fades, 10 USB exclusive + DSD (needs the
owner's DAC), 11 backup, automation API, shortcuts, widgets, Auto nodes, logs, Wrapped.

## Servers and connection

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Subsonic token auth | x | x | x | yes | |
| Legacy (plaintext/enc) auth for old servers, auto-detected | x | x | | yes | done |
| OpenSubsonic API-key auth | x | | | yes | done |
| Stable salt so URLs stay cacheable | | x | | yes | |
| Custom HTTP headers (reverse proxy, Cloudflare Access) | x | | x | yes | done |
| Basic-auth for reverse proxies | x | | | yes | done |
| Accept self-signed certificate / custom CA / mTLS client cert | x | x | x(user CA) | yes | done |
| Two addresses per server (LAN first, WAN fallback), bitrate cap on the second | x | x | | yes | done |
| "Wi-Fi only" per server | x | | | yes | done |
| Multiple saved servers / profiles, switcher | x | x | | yes | done |
| Music-folder (library) selection | x | x | | yes | done |
| URL help: prepend https, http/https chips, reverse-proxy subpath | x | | x | yes | done |
| Server type/version shown, octo-fiesta detected | | x | | part | add |
| Categorised login errors, retry, "open offline" | | x | | yes | done |
| Zstandard / gzip response compression | x | | | gzip (OkHttp) | skip: server side does not offer zstd |
| Other sources: Jellyfin/Emby, Plex, Kodi, Audiobookshelf | x | x(J/E) | | | ask |
| Local files on the device | x | x | | | ask |
| SMB / WebDAV / cloud drives | x | | | | skip: different product |
| YouTube Music through yt-dlp (embedded Python) | | x | | | ask (leaning no: +60 MB, legal grey) |

## Sync and offline index

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Whole library mirrored locally | x | x | x | optional "Sync all" | keep optional: live search is the point |
| Automatic / scheduled re-sync, differential | x | 6 h | 1 h | | add: on app open when older than N hours, Wi-Fi only option; never from the background |
| Sync status screen with counters | x | | x | part | add |
| Compatibility mode for servers without empty-query search3 | x | | | | add (walk getAlbumList2) |
| Fetch extra metadata (artist bios/images) during sync | x | | | on demand | skip: on demand is cheaper |

## Library and browsing

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Albums, artists, playlists, genres, favourites, radio, downloads | x | x | x | yes | |
| All-songs list with sorts | x | x | x | yes | done |
| Browse by year / decade | x | | | yes | done |
| Browse by folder (getIndexes / getMusicDirectory) | x | | | yes | done |
| Album sorts: name, artist, added, played, most played, starred | x | x | x | yes | |
| More sorts: year, random, release date; asc/desc | x | | x | yes | done |
| Sort + view remembered per list | x | | x | part | part: sort |
| Grid / list toggle, grid size | x | x | x | | add (UI-light) |
| Filters: starred only, downloaded only, quick text filter | x | x | x | part | part: artists, songs, album/playlist tracks |
| A-Z fast scroller | x | x | x | part | part: artists |
| Tracks grouped by disc, disc subtitles | x | | x | yes | done |
| Album: "more by artist", quality badge, in-album filter | x | x | x | yes | done |
| Artist: albums grouped album/EP/single, "appears on", top songs, similar, bio | x | x | x | part | part: no appears-on |
| Artist: play all, shuffle, queue artist, download all albums | x | x | x | yes | done |
| last.fm / MusicBrainz links (behind a confirmation) | | | x | yes | done |
| Multiple artists per track with artist picker | x | x | x | yes | done |
| Track info sheet (path, codec, rate, bits, channels, ReplayGain, MBID) | x | | x | yes | done |
| Fullscreen cover with save / share | | | x | part | part: view only |
| Swipe a row: queue / play next / favourite (configurable) | x | x | x | yes | done |
| Default tap action configurable (play list / play one / queue) | x | | | yes | done |
| Multi-select with batch actions | x | x | | yes | done |
| Drag and drop onto play/queue targets | x | | | | skip: UI-heavy, no function gained |
| Playing row highlighted | x | x | x | yes | |
| Explicit badge; explicit content: allow / skip | x | | x | yes | done |
| Listening history screen | x | x | | yes | done |
| Home: fixed shelves | | | x | yes | |
| Home: configurable rows / order / pinned playlists | x | | | yes | done |
| Tablet two-pane layout, landscape layouts | x | x | x | | skip for now: UI pass belongs to the UI rewrite |
| Open audio files sent from other apps | x | | | | skip: no local-file source |

## Search

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Live server search merged with instant offline index | | live opt. | local+server | yes | |
| Provider (octo-fiesta) results marked, never auto-queued | | detect only | badge | yes | |
| Recent searches | 15 | x | 10 | 20 | |
| Filter chips (all / songs / albums / artists / playlists), top result | x | x | x | | add |
| Playlists in results | x | x | x | | add |
| Accent-insensitive; transliteration | x | | | accent yes | skip transliteration (ICU tables, niche) |
| Favourites-only search; search inside queue / album / playlist | x | x | | | add |
| Play from search starts a similar-songs queue | | x | | | ask via setting "default tap action" (off for ext- items, always) |
| Voice search (Assistant, Android Auto) | x | x | | yes | |

## Playback

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Gapless | x | x | x | yes | |
| Hardware offload | x | | x | yes | |
| Burst playback from a deep buffer (CPU asleep ~80 %) | | | | yes | |
| Crossfade with real overlap | x | fade-out only | | yes | |
| Crossfade: separate in/out length, curves, "mix only", off for albums in order | x | | | part | part: off inside albums |
| A scrub into the mix stays on the song to hear the ending, and the mix still fires | | | | yes | |
| Smart fades (waveform-analysed fade points) | x | | | | ask (cost: decode-ahead analysis per track) |
| Fade on play / pause / seek / skip | x | x | | yes | done |
| Speed | x | x | x | yes | |
| Pitch control / preserve pitch | x | stub | | yes | done |
| Skip silence; only for audiobooks | x | | | yes | |
| ReplayGain track / album | x | x | x | yes | |
| ReplayGain automatic (album when queue is one album), fallback gain for untagged, clipping guard | x | x | x | yes | done |
| ReplayGain with positive gain (needs DSP path) | x | | x | | add: through the Rust chain with a limiter when DSP is already on |
| Loudness normalisation to LUFS target | x | | | | skip: needs R128 tags the server does not expose |
| Repeat, shuffle, shuffle order restored, previous follows history | x | x | x | part | add history-aware previous |
| Weighted shuffle (spread artists/albums) | x | | | yes | done |
| Queue + position survive process death | x | x | x | yes | |
| Server play-queue sync (other devices) | bookmarks | | | yes | |
| Bookmarks / resume points for long tracks | x | | | | add |
| Audiobook mode: chapters, rollback, mark played | x | | | | ask (scope) |
| Play / skip counts, thresholds | x | | x | yes | done (local history) |
| Audio focus: duck / pause / ignore; resume after call | x | x | | default | add |
| Auto-play on headset/Bluetooth connect; pause at volume 0 | x | | | | add (broadcast-driven, no polling) |
| Headset single/double/triple click mapping, long-press = next album | x | | | | add |
| "Previous" rewinds first toggle; seek step sizes | x | | | part | part: previous rule |
| Pre-cache next N tracks, separate Wi-Fi / mobile | x | x | | yes | done |
| Bitrate by network; metered Wi-Fi/VPN counts as mobile; re-transcode when Wi-Fi drops | x | x | x | part | add |
| Prefer original over cached transcode on Wi-Fi | x | | | | add |
| Keep skipping on server errors / faster skip offline | x | | | yes | done |
| Buffered position in seek bar | x | | | | add |
| Waveform seek bar | x | | | | ask (cost: decode whole track once, or server call) |
| Notification: shuffle / repeat / favourite / custom buttons | x | | x | | add |
| Media session options (queue exposure, explicit marks) | x | | | default | skip: niche |
| Sleep timer: minutes, end of track | x | x | x | yes | |
| Sleep timer: after N songs, end of queue, fade out, custom duration | x | x | x | part | part: after N songs |
| Internet radio playback, ICY now-playing titles | x | x | x | yes | done |
| Formats Android cannot decode (DSD, APE, WavPack, WMA, MPC, TTA) via bundled FFmpeg | x | | | | ask (cost: +5-8 MB, those formats decode on CPU) |
| CUE sheets | x | | | | skip: file providers only |

## Output and DAC

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Android 14+ bit-perfect USB | x | | | yes (untested on hardware) | |
| Hi-res float output | x | | | yes | |
| 24/32-bit integer output to the DAC | x | | | | add (custom AudioOutputProvider) - needs your DAC to verify |
| Settings per output device (speaker / wired / each BT device / each DAC): EQ, RG, offload | x | | | yes | done as sound profiles bound to outputs |
| Bypass all processing per output | x | | | bit-perfect only | add (falls out of per-output settings) |
| Max output sample rate / fixed output format / high-quality resampler | x | | | | ask (resampling on CPU) |
| USB exclusive mode (own USB stack), DAC volume, warm-up delay | x | | | | ask (large; needs your DAC) |
| DSD native / DoP / PCM-to-DSD | x | | | | ask (tied to FFmpeg + USB stack) |
| Vendor DAP routes (HiBy, FiiO, iBasso, Shanling) | x | | | | skip unless you own one |
| Show current output, rate, depth in player | x | | x | settings only | add |

## DSP

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Parametric EQ, peaking + shelves, AutoEQ/APO import, auto pre-amp | x | | | yes | |
| More filter types: low/high pass, band pass, notch; per-channel filters | x | | | yes | done |
| Graphic EQ 5/10/15/31 bands | x | gone | system 5-band | part | part: 10 bands plus any number of parametric ones |
| Built-in presets (bass/treble/vocal, loudness) and named profiles, per-output auto-apply | x | gone | | yes | done |
| AutoEQ database browser (download headphone list) | x | | | yes | done: 8850 headphones, index cached locally, behind the lookups switch |
| Crossfeed (levels, cutoff) | x | | | yes (level) | add cutoff |
| L/R balance, mono | x | | | yes | done |
| Limiter (so boosts and positive ReplayGain cannot clip) | x | | | yes | done: look-ahead, soft knee, transparent below the ceiling |
| Compressor / expander / noise gate | x | | | | ask (niche; cost small) |
| Bass boost, virtualizer, volume boost | x | | | | add bass boost + volume boost as EQ/limiter presets; virtualizer: skip |
| System equalizer / external EQ session broadcast | x | | x | panel only | add session broadcast |

## Queue

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Play next / add / remove / clear / jump | x | x | x | yes | |
| Drag to reorder | x | x | x | API only | add |
| Swipe to remove / play next; scroll to current | x | x | x | | add |
| Save queue as playlist; clear remaining | x | | | | add |
| Total / remaining time header | | | x | | add |
| Duplicate-in-queue warning | | x(playlist) | x | | add |
| Multiple saved queues (last 15) | x | | | | add |
| Auto-continue with similar songs | x | x | 1 random | yes | |
| Auto-continue modes: random, same genre, same artist, similar; how many | x | x | | similar only | add |
| Mixes: instant mix from track/artist, decade, genre; "exclude from mixes" | x | x | | yes | done: Discover + Discover Weekly (on-device taste, daily/weekly seed) |
| On-device taste model (plays, skips, completion, hour of day) feeding mixes | x | x | | yes | done |
| Live queue reshaping ("Smart Flow") | x | | | | skip: opaque behaviour, little demand |

## Playlists

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Create, delete, add, remove song | x | x | x | yes | |
| Rename, comment, public flag | x | | | | add |
| Reorder tracks, multi-select remove | x | x | | | add |
| Add to several playlists at once; "already in playlist" hint | x | x | x | | add |
| Remove duplicates / missing | x | | | | add |
| M3U/M3U8 import and export | x | | | yes | done |
| Pin playlist to home; launcher shortcuts (play / shuffle) | x | x | | part | part: pin |
| Smart playlists: rule groups (AND/OR, nested) over index fields, limit, sort, stable random | x | | | yes | done (editor: one group; nesting via the core's JSON) |
| Default smart playlists (most played, recently played, never played, ...) | x | | | yes | done |
| Composite 2x2 covers | | x | | server-made | skip: Navidrome already serves them |

## Downloads and caches

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Download song / album / playlist / whole library | x | x | x | yes | |
| Download all albums of an artist / all favourites | x | x | x | | add |
| Auto-download rules (playlist, smart playlist, artist, genre, favourites) | x | | | | add (evaluated on sync, Wi-Fi only option) |
| Download quality, Wi-Fi vs mobile; per-item "original" | x | | x | one quality | add |
| Parallel downloads 1-8, Wi-Fi only, pause/resume/cancel, low-space stop | x | x | x | 2, no UI | add |
| Download manager screen with per-item state, retry | x | x | x | count only | add |
| Covers and lyrics saved with downloads | x | x | x | covers via image cache | add |
| Storage location incl. SD card; export to Music/Downloads | x | x | | app-external | add |
| Rolling stream cache with size cap | x | x | | yes | |
| Promote played cache to permanent | x | | | | add |
| Storage screen: sizes, clear image cache / stream cache / downloads / index / pending | x | x | x | sizes, clears stream + covers | clear downloads and the index from here |
| Image cache: Wi-Fi-only, size, cover quality setting | x | x | x | fixed | add |
| Offline mode: auto / forced / "metered counts as offline"; hide unavailable | x | x | x | implicit | add |
| Bridge with downloads while offline (park online queue, resume when back) | | | | yes | done: Prefs `bridgeOffline` (off by default); network errors jump to a download still in the queue or park the rest and play smart picks from full downloads; network callback only while bridging |
| Offline write queue (stars, ratings, plays, playlist edits) | x | scrobbles | x | yes | |

## Lyrics

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Server synced + plain lyrics, tap to seek | x | x | x | yes | |
| Word-by-word (karaoke) cues from OpenSubsonic enhanced lyrics | x | stub | x | yes | done: server cues, inline LRC tags, or estimated per line; swept in the draw phase at 30 fps, only while visible; switchable |
| Translations | x | | | yes | done |
| LRCLIB / LyricsPlus fallback providers, ordered | | x | x | via octo-fiesta | ask (privacy: sends artist+title to a third party) |
| Offset adjust; keep screen on; text size / alignment | x | x | x | yes | done (alignment: no) |
| Lyrics cached with downloads | x | x | x | | add |
| Share lyrics as image / text | | | x | | add (text); image: skip |
| Lyric line under artwork / over cover | x | x | | | skip: UI rewrite |
| Lyrics over Bluetooth (AVRCP title field) | x | gone | | | ask (cost: a metadata update per line while playing) |

## Casting and remote

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Chromecast | x | x | | | ask (needs Google Play services Cast SDK) |
| UPnP / DLNA renderers, gapless, volume | x | x | | | ask (own SSDP+SOAP code, no dependency; discovery runs only while the picker is open) |
| Sonos groups, Kodi, Plex clients | x | | | | skip unless you own them |
| Subsonic jukebox (play on the server's sound card) | | x | | | add |
| Android output switcher integration | x | | | default | add |
| LAN remote control between two phones | | disabled | | | skip |

## Car, watch, TV, widgets

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Android Auto browse tree + voice search | x | x | session only | yes | |
| Android Auto: configurable tabs, more nodes (artists, genres, mixes) | x | x | | fixed | add |
| Widget 4x1 | x | | x | yes | |
| More widgets (2x2 with cover, resizable) | x | | x | | add, still event-driven |
| Launcher shortcuts (search, shuffle all, playlists) | x | | | | add |
| Wear OS app (browse, control, download to watch) | x | | | | ask |
| Android TV build (leanback, D-pad) | x | x | | | ask |
| Quick-settings tile / Assistant actions | | | | | skip |

## Ratings, scrobbling, stats

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Star songs/albums/artists | x | x | x | yes | |
| 1-5 star ratings | x | x | x | no | dropped: nobody used them, favourites cover it |
| Scrobble + now playing, threshold %, offline queue | x | x | x | yes | |
| Minimum duration to scrobble | | | x | 10 s fixed | add |
| OpenSubsonic playbackReport | x | | | | add (octo-fiesta handles it) |
| Year-in-review ("Wrapped") | | x | | yes | done as listening stats for any period |
| Direct Last.fm / ListenBrainz | | | | | skip: Navidrome does this server-side |

## Shares, radio, misc

| Feature | S | M | N | nori | Plan |
|---|---|---|---|---|---|
| Create share link | x | | x | yes | |
| Share with expiry + description; list / delete shares; hide sharing | | | x | | add |
| Radio: list, play, add, delete | x | x | add only | yes | |
| Radio: edit; homepage link; copy URL | | x | | | add |
| Settings backup / restore (encrypted zip) | x | | | | add |
| Automation API (broadcast intents: play/pause/next, play playlist, load EQ profile, set bitrate, sync) | x | | | | add |
| Settings PIN | x | | | | skip |
| In-app log viewer / send logs, crash screen | x | | x | | add log viewer |
| Update checker (GitHub releases) | | x | x | | ask (a network call to GitHub on open) |
| Discord presence, donation nags, easter eggs, emulator block, analytics | | x | | | skip |
| Translations | 20 | 27 | 28 | yes | done |

## Owner's list, 2026-09-18

The owner asked for these explicitly; they override the "UI later" note below.

- USB DAC handling like Symfonium, and earphone-model auto EQ -> AutoEQ browser done; when a Bluetooth or USB device
  connects whose name matches a measured headphone, the app offers its curve and remembers it for that device;
  integer/exclusive USB output needs the owner's DAC.
- Octo-fiesta-aware search, telling library and provider items apart, and the stale-cover bug (provider covers
  cached with their "not downloaded" badge) -> done: provider label, library/provider filter, provider covers never
  disk-cached, "add to library" (star makes octo-fiesta download it).
- Automatic synced lyrics for songs without an LRC -> done: server first, then LRCLIB (synced preferred, duration-matched,
  hits kept, misses retried weekly, failures never cached). LyricsPlus mirrors (Navic's karaoke source) were all
  down on 2026-09-18, so not shipped; the provider layer takes more sources.
- Apple Music AutoMix-style transitions with BPM/beat matching -> **done** (on-device analysis, Camelot-aware length/filters, outro loop remix, bass swap, echo-out, LUFS match; see docs/research/automix.md).
- Material You, AMOLED, adjustable -> done: wallpaper colours, accent colours, theme mode, true-black mode.
- The owner's friend: "take it from Apple, the album cover spills into the page; but no forced liquid glass" -> done:
  album, artist and playlist pages open with the cover edge to edge under the status bar, melting into a colour taken
  from it; buttons use the cover's accent; the player gets the same wash. Static tint (0 % CPU idle), switchable,
  and with AMOLED on it melts into black instead.
- Gestures -> done: swipe the mini player to skip, up to open; swipe the player header or artwork down to close;
  swipe the artwork to skip.

## Look and feel

Symfonium's style builders, Navic's five themes and Musly's artwork editor are all UI-layer work.
The owner's instruction is to leave UI for a later rewrite, so only function-bearing options are
planned here: theme mode (system/light/dark), dynamic colour, cover-based player colour (one palette
extraction per track; **ask** if even that is wanted), keep-screen-on for lyrics, configurable
mini-player buttons, tab order/hide.
