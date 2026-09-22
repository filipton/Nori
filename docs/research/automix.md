# AutoMix-style transitions for an Android player: research and design

Labels used: **[verified]** = stated by a cited source; **[inferred]** = my reasoning or estimate, not confirmed by a source.

## 1. What Apple's AutoMix does

**Apple's own wording.** Apple's WWDC25 services release says AutoMix uses AI "to analyze audio features" and "crafts unique transitions between songs with time stretching and beat matching" ([Apple Newsroom](https://www.apple.com/newsroom/2025/06/apple-services-deliver-powerful-features-and-intelligent-updates-to-users-this-fall/), [MusicTech](https://musictech.com/news/gear/apple-music-automix-ai/)). Press coverage adds that it looks at tempo and key ([MacRumors](https://www.macrumors.com/how-to/ios-enable-automix-feature-apple-music/)). No WWDC technical session covers the DSP.

**Limits and behaviour, from Apple Support** ([support.apple.com/105067](https://support.apple.com/en-us/105067)) **[verified]:**
- It only works with Apple Music subscription content. It does not work with uploaded or matched library files or iTunes purchases, even when the same song is in the catalogue.
- It is not available with hi-res lossless. On Mac it needs Apple silicon.
- It may not transition when an album plays in order, or when "the genres or tempos are incompatible". In those cases it falls back to a plain transition.
- Crossfade uses a fixed 1–12 s. AutoMix picks its own transition points and length.

**Server-side or on-device? [inferred]** The catalogue-only rule, with a clean copy of the same song being refused, strongly suggests the transition metadata is computed by Apple on the server for each catalogue track: beat grid, cue points and compatibility. The rendering (time-stretch, filters, mixing) happens on the device, which fits the Apple-silicon and no-hi-res requirements. Apple has not confirmed this. A third-party developer notes that MusicKit never exposes decoded protected audio to apps ([primuse #117](https://github.com/chenqi92/primuse/issues/117)), so a client could not analyse protected audio itself anyway.

**What it sounds like.** In iOS 26 the transitions were described as predictable, with a characteristic "underwater" sound ([9to5Mac](https://9to5mac.com/2026/07/16/ios-27-makes-one-of-my-favorite-apple-music-features-even-better/)). **[inferred]** That is almost certainly a low-pass filter sweep on the outgoing track, so the effect is DJ-style filtering and not only volume. In iOS 27 Apple reworked the intro and outro sections so the tempos line up, and it loops or repeats parts of intros and outros to bridge the two songs ([MacRumors](https://www.macrumors.com/2026/06/09/apple-music-gains-automix-upgrades-and-more-in-ios-27/), [RouteNote](https://routenote.com/radar/apple-music-automix-gets-a-major-upgrade-in-ios-27/)). Apple has not published how long transitions are or whether it uses EQ swaps.

**Complaints** ([How-To Geek](https://www.howtogeek.com/i-disabled-apple-music-automix/), [BGR](https://www.bgr.com/2059450/how-to-turn-off-automix-apple-music-worst-feature/)):
- The transition does not always happen.
- It sometimes cuts off final chords.
- It sometimes skipped "as much as a full minute" to line up the tempos.
- Volume spikes and "broken record" stutters.
- It made odd choices, such as jumping into the middle of the next song.
- Album listeners dislike it, and it suits house, techno and pop better than other genres.

**Lesson for us:** never throw away a lot of the song, keep the transition window bounded, and make fallbacks conservative.

## 2. Comparable features

- **Spotify Automix.** It looks at tempo, key, energy and rhythmic structure. Spotify, not the user, chooses the start and end points, and the user cannot change the overlap length ([Spotify Community](https://community.spotify.com/t5/FAQs/Automix-Overview/ta-p/5257278)). Its newer Mix feature for playlists shows waveform, key and BPM and offers transition presets such as "Fade" and "Rise", with EQ, effects and cue-point editing ([MusicRadar](https://www.musicradar.com/music-tech/spotify-responds-to-apple-musics-new-automix-feature-by-letting-you-turn-your-playlists-into-ready-made-dj-sets-with-seamless-transitions)). Spotify computed bars, beats, sections, key and loudness on its servers for years. Its Web API exposed this as `/audio-analysis` until access was cut on 27 Nov 2024 ([Music Ally](https://musically.com/2024/11/28/spotify-removes-features-from-web-api-citing-security-issues/)). This supports the idea that server-side analysis is standard in the industry.
- **Symfonium Smart Fades.** Experimental, and it "requires waveform extraction" ([Symfonium 12.3.0](https://symfonium.app/news/version-1230/)). The developer says that "smart fades with settings are crossfade" ([forum](https://support.symfonium.app/t/smart-fade-tuning/12114)). Users complain that songs with long tails still get faded, and that tracks with a loud start still get a fade-in ([feedback thread](https://support.symfonium.app/t/smart-fades-feedback-thread/7900)). **[inferred]** It seems to pick fade points from where the amplitude envelope crosses thresholds. It does not beat-match.
- **Plexamp Sweet Fades.** Based on MPD's MixRamp. The server measures EBU R128 loudness and works out how far two songs should overlap from the loudness ramps at the end of one and the start of the next ([Plex Labs](https://medium.com/plexlabs/plexamp-v3-9af3b10063b4)). In MPD, MixRamp tags store how loudness changes over time at each end of the song. Overlap is set where both songs sit at `mixrampdb` (e.g. −17 dB), and MPD can also analyse songs on the fly ([MPD docs](https://mpd.readthedocs.io/en/stable/user.html)). This is the cheapest "smart" transition and makes an ideal fallback.
- **Poweramp.** A fixed-length crossfade in milliseconds, with separate settings for automatic and manual track changes, plus fades on seek, play and pause ([guide](https://caninfotech.com/poweramp-music-player/poweramp-music-player-how-to-crossfade-between-two-tracks/)). No analysis.
- **djay Automix AI.** It "identifies rhythmic patterns and the best intro and outro sections", "calculates optimal fade durations and automatically applies parameter changes to EQs and filters". Neural Mix adds stem separation ([Algoriddim](https://help.algoriddim.com/user-manual/djay-pro-windows/mixing-basics/automix)). Rekordbox and Serato work from a beat grid computed offline, with cue points and phrase analysis (Rekordbox). **[not re-verified here]**
- **Open-source reference.** kumone PR #51 is a full AutoMix design built on vDSP ([kumone #51](https://github.com/missuo/kumone/pull/51)):
  - Analysis: spectral-flux onsets, BPM by autocorrelation with a log-normal prior, Ellis DP beat tracking, downbeat voting, phrase boundaries, RMS-based intro and outro landmarks, BS.1770 loudness, Krumhansl key, and how much vocals are present over time.
  - Five checks decide whether a pair may be mixed: loudness gap, timbre distance, tempo stability, key distance and vocal clash. Each pair then gets a transition type, from a short fade up to a beat-matched mix with EQ hand-over or a beat-synced echo-out.
  - It checks alignment at bar level with a 3 % tolerance, because beat-level checks failed: onset timing jitters by 5–13 %.
  - Offline rendering runs at 100–300× realtime.
  - It also falls back to plain whole-mix blending when rendering fails or is late.

  This is the closest public blueprint for what we want.

## 3. Algorithms

### Tempo and beat tracking
Standard pipeline, which suits mobile:
1. Downmix to mono and resample to about 22 kHz.
2. STFT (window 1024–2048, hop 512), then mel or log-magnitude bands.
3. Half-wave-rectified spectral flux gives the onset-strength envelope (about 43 frames/s).
4. Autocorrelate the envelope, or run a comb filterbank, over 60–200 BPM. Weight by a log-normal prior centred near 120 BPM.
5. Ellis (2007) dynamic programming finds the beat sequence. Each beat's score is its onset strength plus the best earlier score, minus a penalty for straying from the target beat period. Backtracking gives the path.

Ellis reported just under 60 % beat accuracy on MIREX-06 development data ([paper](https://www.ee.columbia.edu/~dpwe/pubs/Ellis07-beattrack.pdf)). Tempo is usually scored as Acc1 (within 4 % of the true tempo) and Acc2 (also counting ×2, ×3, ½ and ⅓ as correct). **Half and double tempo errors are the main failure mode** ([Hörschläger et al.](https://www.ifs.tuwien.ac.at/~knees/publications/hoerschlaeger_etal_smc_2015.pdf)).

For mixing, most octave errors do no harm. When comparing two tracks, compare BPM after folding ×½ and ×2, and beat-match at whichever level gives the smallest ratio.

Rust crates, both MIT/Apache:
- **`beat-track-rs`** is exactly Ellis 2007: mel spectral flux, autocorrelation with a log-normal prior, then DP. It uses rustfft and ndarray ([docs.rs](https://docs.rs/beat-track-rs)).
- **`stratum-dsp`** covers BPM, key and HMM beat grids, with optional ONNX ([docs.rs](https://docs.rs/stratum-dsp)).

aubio is GPL and Essentia is AGPL, so avoid both. Recommendation: implement it in our own Rust core (about 500 lines), using `beat-track-rs` as a reference or dependency. We already have FFT/DSP code.

**CPU [inferred estimate]:** a 4-min track at 22 kHz gives about 10k frames. rustfft on one Cortex-A7x core takes roughly 0.1 s. Onset detection, autocorrelation and DP add less than 50 ms. **Decoding dominates:** about 0.2–1 s per track for MP3, AAC or Opus, less for FLAC. Total: about 0.3–1.5 CPU-seconds per track, roughly 0.2–0.5 % of the track's duration.

### Downbeats and phrases (lightweight)
- **Downbeat phase:** test the 4 possible bar starts (assume 4/4). For each one, add up bass-band onset strength (kick) and chroma change (chords tend to change on the "1") at every 4th beat, then take the phase with the highest total. This is the "downbeat voting" approach.
- **Phrases:** use beat-synchronous features (RMS, bass energy, chroma). Compute a Foote novelty curve from the self-similarity matrix, or simply look for energy jumps. Keep only candidates that fall on multiples of 8 or 16 bars from the first downbeat.
- **Checking:** check the grid at bar level, not beat level (see kumone). Flag tracks with drifting tempo (live recordings, older music). The DP beat intervals show this through their variance, and such tracks should not be beat-matched.

### Intro and outro regions
- Compute the RMS and loudness envelope at about 10 Hz, per beat. Trim silence where the level stays below −50 to −60 dBFS.
- The outro candidate is the last phrase boundary before the energy drops, or the last 16–32 bars when the ending is steady. The intro candidate is the region before the first large energy or bass jump.
- Also store MixRamp-style points: when the end of the track falls below −17 dB relative to track loudness, and when the start rises above it. These are the fallback.
- **Vocal activity heuristic [inferred, rough]:** high energy in the 300 Hz–3.4 kHz band relative to the whole spectrum, together with the spectral flatness and centroid patterns vocals produce, smoothed per beat. It is enough to avoid overlapping two vocal sections, but not to detect lyrics. Stem separation is too heavy for a battery-first design.

### Time-stretching

| Library | Licence | Quality at ±2–8 % | Notes |
|---|---|---|---|
| **Signalsmith Stretch** | MIT | Very good; rated alongside Rubber Band R3 ([KVR](https://www.kvraudio.com/forum/viewtopic.php?t=623537)) | C++11, header-only. Rust crates `signalsmith-stretch` ([lib.rs](https://lib.rs/crates/signalsmith-stretch)) and `ssstretch`. It has a cheaper preset. Build it with optimisation on, because it is about 10× slower without ([docs](https://signalsmith-audio.co.uk/code/stretch/)). |
| **Bungee** | MPL-2.0 | Good (adaptive phase vocoder) | Supports Android. Rust bindings `bungee-rs` ([GitHub](https://github.com/bungee-audio-stretch/bungee)). |
| Rubber Band | GPL, or paid commercial licence | R3 is excellent, R2 is fine | R3 uses a lot of CPU ([licence](https://breakfastquay.com/rubberband/license.html)). |
| SoundTouch | LGPL-2.1 | OK for small changes (WSOLA), tuned for pop/rock | About 100 ms latency. `soundtouch` crate ([lib.rs](https://lib.rs/crates/soundtouch)). |
| media3 Sonic | Apache-2.0 | Poor for music | Based on PICOLA and aimed at speech; its author says music quality is "pretty poor" ([Sonic docs](https://github.com/waywardgeek/sonic/blob/master/doc/index.md)). Fine as a last resort for ≤2 %. |
| Resampling ("vinyl") | – | Pitch moves 0.34 semitone per 2 % | The cheapest option. Many DJs accept it at ±2 %. |

Recommendation: **Signalsmith Stretch** (MIT) inside the Rust core, linked statically through its crate. Use varispeed resampling for changes of 2 % or less when the user enables that mode.

**CPU [inferred]:** Signalsmith at the default preset on 44.1/48 kHz stereo probably uses a single-digit percentage of one big mobile core, and only during the 10–30 s window. Measure it on device.

### Key detection
Take chroma from the STFT, averaged over the track (better: weighted towards the intro and outro windows that will actually overlap). Correlate it with Krumhansl or Temperley profiles for all 24 keys. Expect about 70–85 % accuracy on tonal Western pop, with relative-key and fifth errors being common ([summary](https://github.com/Corentin-Lcs/music-key-finder)).

**Verdict:** do not reorder the queue by key, because users of a library player expect the queue to be respected. Use key distance as one input to the pair score: Camelot distance ≤1 allows a long harmonic overlap, a clashing pair gets a short overlap, drums only, or an echo-out. The cost is minimal because the chroma comes from the same STFT.

### Transition shaping
- **Equal-power curve** (cos/sin) for uncorrelated material. Linear or sine-squared for beat-matched, phase-locked content, where the two tracks add coherently.
- **Bass swap:** the incoming track starts with its lows cut (high-pass or low-shelf at about 150–200 Hz, −20 to −inf dB). On a downbeat at a phrase boundary, swap in one move, over about 1 beat: cut the outgoing lows and restore the incoming lows. Only one track ever carries the bass ([vibesdj](https://vibesdj.io/learn/techniques/eq-swapping), [Club Ready DJ School](https://www.clubreadydjschool.com/tribe-talk/getting-started/bass-swapping-dont-make-this-common-mistake)). Use a gradual swap when the incoming intro is sparse.
- **Filter sweep:** a low-pass on the outgoing track, from 20 kHz down to about 300 Hz over 4–8 bars (the "underwater" sound). Or a high-pass on the outgoing track as the incoming track comes in.
- **Echo-out:** feedback delay synced to the beat on the outgoing track, then cut. Use it for clashing pairs.
- All of this is biquads plus gains. Per-sample cost is negligible.

## 4. Recommended design (battery-first)

**When to analyse:**
1. **During normal playback, for free:** tap the PCM already passing through our audio chain. Decoding is already paid for, so feed a streaming analyser in the Rust core (STFT, onset envelope, chroma and RMS accumulators). At track end, run tempo, DP, downbeat, phrase and key, which takes tens of ms, and store the record.
2. **When a track finishes downloading or caching:** analyse it on a low-priority thread, or queue it.
3. **Backfill the library** with WorkManager, constrained to charging + unmetered + battery-not-low (+ device idle). The work is batched and can resume.
4. **Just in time (first play, no record):** about 20 s before the outro window, decode only the last ~45 s of the current track and the first ~45 s of the next. That is about 10 % of a full analysis. Grid confidence is lower, so use a more conservative transition.

**Storage:** one SQLite row per track, about 200–500 bytes. Key it on server id + file hash or duration, and store an `analysis_version`.

```
bpm REAL, bpm_confidence REAL, beat_offset_ms INT, tempo_stable BOOL,
downbeat_phase INT(0-3), first_downbeat_ms INT,
intro_end_ms INT, outro_start_ms INT,                -- phrase-aligned cues
cue_candidates BLOB  -- few (ms, bars, energy, vocal) tuples
lead_silence_ms INT, trail_silence_ms INT,
mixramp_start_ms INT, mixramp_end_ms INT,
loudness_lufs REAL, key INT(0-23), key_confidence REAL,
vocal_end_ms INT, vocal_start_ms INT
```

Beat times are not stored. The grid is rebuilt as `offset + n·60/bpm`. For tracks with drifting tempo, beat matching is simply turned off.

**Planning at playback (Kotlin):**
1. Take the records for the current track A and next track B, and compute the tempo ratio after folding ×½ and ×2.
2. Beat-match only if both grids are confident and stable and the ratio is within the user's max (default ±6 %).
3. Choose A's outro cue and B's intro cue on phrase boundaries, avoiding vocal-on-vocal overlap. Never skip more than about 15 s of either track. This directly addresses Apple's "skipped a minute" complaint.

**Rendering (Rust):**
- Run both decks through the existing chain. Only B is stretched during the window, locked to A's tempo.
- After the swap, ramp B back to its native tempo over 4–8 bars, then bypass the stretcher.
- Apply the filters and bass swap, with loudness matching from LUFS or ReplayGain.
- **Outside the window the extra cost is zero.** During it, one stretcher plus about 6 biquads run for 10–30 s.

**Navidrome:** OpenSubsonic `Child` exposes `bpm` and `replayGain` ([OpenSubsonic Child](https://opensubsonic.netlify.app/docs/responses/child/), [Navidrome PR #2597](https://github.com/navidrome/navidrome/pull/2597)). Use `bpm` as a prior to settle half/double tempo, and as a prefilter to skip pairs that cannot be matched without decoding anything. It has no beat phase, so it cannot drive beat matching alone. Tag quality varies.

**Fallback ladder:**
1. Full analysis: beat-matched mix with bass swap.
2. Grid missing or unreliable: phrase-less crossfade at MixRamp or silence-trimmed points, with a filter sweep.
3. Nothing is known: a fixed equal-power crossfade.
4. Same album, played in order (or gapless-tagged): gapless, with no mixing.

## 5. Settings

- AutoMix on/off (separate from Crossfade).
- Style: Smart fade only / DJ mix.
- Transition length: auto, or a maximum in bars/seconds (e.g. 4–32 bars).
- Beat matching on/off.
- Allow tempo change on/off, with a maximum change of 2/4/6/8 %.
- Keep pitch (time-stretch) vs varispeed.
- Bass swap on/off. Filter effects on/off.
- Skip transitions within albums (default on), and respect gapless.
- Also transition on manual skip.
- Loudness matching.
- Analyse library only on charger + Wi-Fi (default on).
- Per-track exclusion ("never mix this track").

## 6. Competitive audit (2026) and what we ship

| Capability | Apple Music AutoMix | DJ.Studio Harmonize | Symfonium / Plexamp | **nori** |
|---|---|---|---|---|
| Beat match + time-stretch | yes (catalogue AI) | yes (offline edit) | no / MixRamp only | **yes, on-device** |
| Bass swap | not documented | yes | no | **yes** |
| LPF / filter sweep | yes (iOS 26 “underwater”; iOS 27 softer) | yes + HPF presets | no | **yes; Camelot-softened** |
| Echo-out for clashes | simple fade fallback | yes | no | **yes** |
| Camelot-aware length | inferred (key+tempo) | yes (bars 4–32) | no | **yes (≤1 long, 2 short, ≥4 echo)** |
| Loudness match | yes (catalogue) | yes | MixRamp / RG | **LUFS when RG off** |
| Max skip bound | criticised (≤1 min) | n/a (edit) | n/a | **15 s hard cap** |
| Album-in-order gapless | yes | n/a | yes | **yes** |
| Works on self-hosted library | **no** (catalogue only) | yes (files) | yes | **yes** |
| Hi-res / USB DAC path | blocked on hi-res | n/a | varies | **offload-aware** |
| Intro/outro loop remix | **iOS 27** | loop effects | no | **outro loop remix** (intro live-loop deferred) |
| Reorder playlist by key | no (queue respected) | **yes (Harmonize)** | no | **no** (by design: library player) |
| Stem separation | no | optional | no | **no** (battery) |
| Tag BPM half/double prior | inferred | yes | n/a | **yes** |
| DJ filter-open (HPF) | soft in iOS 27 | yes | no | **yes** (Camelot stretch pairs) |

**Verdict.** For a library player that respects queue order, we match or beat Apple on self-hosted music: on-device analysis, bass swap, clash echo-out, MixRamp fallback, hard skip cap, Camelot-scaled length/filters, LUFS match, tag-BPM octave correction, and outro loop remix when the ending is too short for the target overlap. Still behind Apple’s catalogue intro looping (needs a second decode source) and DJ.Studio’s playlist reordering — deliberate non-goals for a queue-respecting library client.
