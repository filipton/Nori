//! Transition planning: a pure function from what is known about two tracks to a `TransitionPlan`.
//! The fallback ladder from the research, most to least informed:
//!
//! 1. clashing pairs (two vocals, far-apart keys): an echo-out, timed by the outgoing grid;
//! 2. both grids confident and stable, tempos within reach: beat-matched, bar-aligned, with bass swap;
//!    Camelot distance shapes length and filter: same key = long natural blend, neighbour = soft LPF,
//!    farther = short + classic sweep (Apple iOS 27 / DJ.Studio Harmonize);
//! 3. something analysed, no usable grid: overlap from the MixRamp points and trimmed silence, with a filter sweep;
//! 4. nothing known: a fixed equal-power crossfade;
//! 5. same album in order: gapless, no mixing.
//!
//! A loudness gap or a timbre mismatch shortens whatever the ladder picks. Every gate only demotes:
//! a clash shortens or reroutes a transition, never upgrades one.
//!
//! Hard rule for every plan: at most `MAX_SKIP_MS` of either track goes unplayed (Apple's AutoMix is criticised for
//! skipping up to a minute to line tempos up).

use super::structure::key_distance;
use super::tempo::{fold, match_ratio};
use crate::{AutoMixSettings, FadeCurve, TrackAnalysis, TransitionKind, TransitionPlan};

pub const MAX_SKIP_MS: i64 = 15_000;
pub const MIN_BPM_CONFIDENCE: f32 = 0.5;
pub const MIN_STABILITY: f32 = 0.6;
/// Varispeed moves the pitch 0.34 semitone per 2 %; beyond that it is audible as detuning.
pub const VARISPEED_MAX_PCT: f64 = 2.0;
/// Longest fixed crossfade.
const MAX_BLIND_FADE_MS: i64 = 12_000;
/// Shortest overlap that is still a fade rather than a click guard.
const MIN_FADE_MS: i64 = 300;
/// Shortest MixRamp overlap. MixRamp lays the songs together where the outgoing one has already gone
/// quiet, which on a song that fades out is its last nearly silent second or two - heard as no mix
/// at all ("MIX_RAMP_FADE 300 ms"). This much of the ending is always shared, fading as it goes.
const MIN_MIXRAMP_MS: i64 = 5_000;
const BASS_CUT_HZ: f32 = 180.0;
const SWEEP_FROM_HZ: f32 = 18_000.0;
/// Where the outgoing low-pass ends: beat-matched (lows already swapped out) and plain fades.
const SWEEP_TO_HZ_MATCHED: f32 = 400.0;
const SWEEP_TO_HZ_FADE: f32 = 500.0;
/// Camelot neighbour / relative: gentle muffling, not Apple iOS 26's "underwater" dump.
const SWEEP_TO_HZ_SOFT: f32 = 2_500.0;

fn blank(kind: TransitionKind, out_start: i64, in_start: i64, duration: i64, reason: String) -> TransitionPlan {
    TransitionPlan {
        kind,
        out_start_ms: out_start,
        in_start_ms: in_start,
        duration_ms: duration,
        tempo_ratio: 1.0,
        tempo_ramp_beats: 0,
        tempo_ramp_ms: 0,
        keep_pitch: true,
        fade_curve: FadeCurve::EqualPower,
        out_fade_start_ms: 0,
        out_fade_end_ms: duration,
        in_fade_start_ms: 0,
        in_fade_end_ms: duration,
        out_gain_db: 0.0,
        in_gain_db: 0.0,
        bass_swap_ms: -1,
        bass_swap_len_ms: 0,
        bass_cut_hz: BASS_CUT_HZ,
        filter_start_ms: -1,
        filter_end_ms: -1,
        filter_from_hz: 0.0,
        filter_to_hz: 0.0,
        echo_delay_ms: -1,
        echo_feedback: 0.5,
        echo_wet_db: -6.0,
        out_loop_ms: -1,
        in_loop_ms: -1,
        hp_start_ms: -1,
        hp_end_ms: -1,
        hp_from_hz: 0.0,
        hp_to_hz: 0.0,
        reason,
    }
}

/// An analysis only counts when it describes this file: same length within 3 s.
fn usable(a: Option<&TrackAnalysis>, duration_ms: i64) -> Option<&TrackAnalysis> {
    a.filter(|a| a.duration_ms <= 0 || duration_ms <= 0 || (a.duration_ms - duration_ms).abs() <= 3000)
}

fn grid_ok(a: &TrackAnalysis) -> bool {
    a.bpm > 0.0 && a.bpm.is_finite() && a.bpm_confidence >= MIN_BPM_CONFIDENCE && a.stability >= MIN_STABILITY
}

/// The song as the mix sees it at one end: its grid replaced by the one measured over that end alone.
/// A whole-song grid asks one tempo to fit four minutes, which a band without a click track never
/// does - every one of them scored no stability, so nothing was ever beat-matched - and a song that
/// changes tempo half way has the wrong answer at its end. The window's grid is used whenever it
/// holds; the whole song's only when it holds and the window's does not (too little music at that end).
fn with_grid(a: &TrackAnalysis, bpm: f64, confidence: f32, offset_ms: f64, stability: f32, phase: i32) -> TrackAnalysis {
    let w = TrackAnalysis { bpm, bpm_confidence: confidence, beat_offset_ms: offset_ms, stability, downbeat_phase: phase, ..a.clone() };
    let measured = bpm > 0.0 && bpm.is_finite();
    if measured && (grid_ok(&w) || !grid_ok(a)) { w } else { a.clone() }
}

/// The outgoing song, gridded over its last seconds.
fn at_end(a: &TrackAnalysis) -> TrackAnalysis {
    with_grid(a, a.outro_bpm, a.outro_bpm_confidence, a.outro_beat_offset_ms, a.outro_stability, a.outro_downbeat_phase)
}

/// The incoming song, gridded over its first seconds.
fn at_start(b: &TrackAnalysis) -> TrackAnalysis {
    with_grid(b, b.intro_bpm, b.intro_bpm_confidence, b.intro_beat_offset_ms, b.intro_stability, b.intro_downbeat_phase)
}

/// Prefer the analysis BPM folded toward a server/tag prior when that settles a half/double error.
fn bpm_with_tag(analysis: f64, tag: f32) -> f64 {
    if !(analysis > 0.0 && analysis.is_finite()) {
        return analysis;
    }
    let tag = tag as f64;
    if !(tag > 0.0 && tag.is_finite()) {
        return analysis;
    }
    let mut best = analysis;
    let mut best_err = (analysis - tag).abs() / tag;
    for c in [analysis, analysis * 2.0, analysis / 2.0] {
        let err = (c - tag).abs() / tag;
        if err < best_err {
            best = c;
            best_err = err;
        }
    }
    if best_err <= 0.04 {
        best
    } else {
        analysis
    }
}

fn music_end(a: &TrackAnalysis, duration: i64) -> i64 {
    if a.silence_end_ms > a.silence_start_ms && a.silence_end_ms <= duration {
        a.silence_end_ms
    } else {
        duration
    }
}

fn loudness_trim(a: Option<&TrackAnalysis>, b: Option<&TrackAnalysis>, s: &AutoMixSettings) -> f32 {
    match (a, b) {
        (Some(a), Some(b)) if s.match_loudness && a.lufs > -60.0 && b.lufs > -60.0 => (a.lufs - b.lufs).clamp(-9.0, 9.0),
        _ => 0.0,
    }
}

/// What the two tracks sound like next to each other, from the stored overlap windows.
#[derive(Clone, Copy)]
struct Verdict {
    /// Two vocals, or two confident keys a tritone or more apart: do not blend.
    clash: bool,
    cause: &'static str,
    /// A large loudness gap or far-apart brightness: keep any overlap short.
    shorten: bool,
    short_cause: &'static str,
}

/// Confident keys this far apart on the Camelot wheel do not blend.
const KEY_FAR: i32 = 4;
/// Mean voice-band share that counts as "sung" in an overlap window.
const VOCAL_MIN: f32 = 0.45;
/// Loudness gap that shortens an overlap, dB.
const LOUD_GAP_DB: f32 = 6.0;
/// Brightness ratio (octaves of centroid) that counts as a timbre mismatch.
const TIMBRE_OCTAVES: f64 = 1.0;

fn pair_gate(a: &TrackAnalysis, b: &TrackAnalysis) -> Verdict {
    let mut v = Verdict { clash: false, cause: "", shorten: false, short_cause: "" };
    let keys_known = a.key_confidence >= 0.4 && b.key_confidence >= 0.4;
    if keys_known && key_distance(a.key, b.key) >= KEY_FAR {
        v.clash = true;
        v.cause = "keys far apart";
    } else if a.outro_vocal >= VOCAL_MIN && b.intro_vocal >= VOCAL_MIN {
        // A v1 row reads back as 0 and can never trip this; only a measured window can.
        v.clash = true;
        v.cause = "vocals overlap";
    }
    if a.lufs > -60.0 && b.lufs > -60.0 && (a.lufs - b.lufs).abs() > LOUD_GAP_DB {
        v.shorten = true;
        v.short_cause = "loudness gap";
    } else if a.outro_centroid > 0.0
        && b.intro_centroid > 0.0
        && (a.outro_centroid as f64 / b.intro_centroid as f64).log2().abs() > TIMBRE_OCTAVES
    {
        v.shorten = true;
        v.short_cause = "timbre mismatch";
    }
    v
}

/// A clashing pair does not blend: the outgoing track exits into a beat-synced echo while the
/// incoming track fades in over its tail. Four outgoing beats, confined to the overlap the mixer
/// renders (the repeats decay inside it), starting on a downbeat. No tempo change, no bass swap:
/// the echo is the effect.
fn echo_out(a: &TrackAnalysis, b: &TrackAnalysis, out_dur: i64, in_dur: i64, max_len: i64, s: &AutoMixSettings, cause: &str) -> Option<TransitionPlan> {
    let beat = 60_000.0 / a.bpm;
    if !beat.is_finite() || beat <= 0.0 {
        return None;
    }
    // Slow tempos get a clamped delay rather than a cavern (the mixer caps at one second).
    let delay = beat.clamp(250.0, 1000.0).round() as i64;
    let beats = 4i64;
    let dur = (beats as f64 * beat).round() as i64;
    if dur > max_len || dur < MIN_FADE_MS {
        return None;
    }
    let end_a = music_end(a, out_dur).max(out_dur - MAX_SKIP_MS);
    let phase = a.downbeat_phase.rem_euclid(4) as i64;
    let mut n = ((end_a as f64 - dur as f64 - a.beat_offset_ms) / beat).floor() as i64;
    while n.rem_euclid(4) != phase {
        n -= 1;
    }
    let start = (a.beat_offset_ms + n as f64 * beat).round() as i64;
    if start < a.silence_start_ms || out_dur - (start + dur) > MAX_SKIP_MS {
        return None;
    }
    let in_start = b.silence_start_ms.clamp(0, MAX_SKIP_MS.min(in_dur / 3));
    if dur > in_dur - in_start {
        return None;
    }
    let beat_ms = beat.round() as i64;
    let mut p = blank(TransitionKind::EchoOut, start, in_start, dur, String::new());
    p.fade_curve = FadeCurve::SineSquared;
    // Outgoing dies in two beats; incoming rides the last three so the room is not empty
    // while the repeats decay (DJ.Studio-style echo-out into the next track).
    (p.out_fade_start_ms, p.out_fade_end_ms) = (0, (2 * beat_ms).min(dur));
    (p.in_fade_start_ms, p.in_fade_end_ms) = ((dur - 3 * beat_ms).max(0), dur);
    (p.echo_delay_ms, p.echo_feedback, p.echo_wet_db) = (delay, 0.45, -7.0);
    p.in_gain_db = loudness_trim(Some(a), Some(b), s);
    p.reason = format!("echo-out over {beats} beats, {cause}");
    Some(p)
}

/// Camelot distance when both keys are trusted, else -1.
fn camelot_dist(a: &TrackAnalysis, b: &TrackAnalysis) -> i32 {
    if a.key_confidence >= 0.4 && b.key_confidence >= 0.4 {
        key_distance(a.key, b.key)
    } else {
        -1
    }
}

/// Low-pass shaping for MixRamp / one-grid fades: soft when keys agree, classic when they do not.
fn apply_fade_filter(p: &mut TransitionPlan, a: Option<&TrackAnalysis>, b: Option<&TrackAnalysis>, s: &AutoMixSettings, dur: i64) {
    if !s.filter_effects || dur < 2000 {
        return;
    }
    let soft = a.zip(b).is_some_and(|(a, b)| {
        let d = camelot_dist(a, b);
        d == 0 || d == 1
    });
    if soft {
        (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) =
            (dur / 2, dur, SWEEP_FROM_HZ, SWEEP_TO_HZ_SOFT);
    } else {
        (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) =
            (0, dur, SWEEP_FROM_HZ, SWEEP_TO_HZ_FADE);
    }
}

pub fn plan(out: Option<&TrackAnalysis>, inc: Option<&TrackAnalysis>, out_duration_ms: i64, in_duration_ms: i64, s: &AutoMixSettings) -> TransitionPlan {
    let out_dur = if out_duration_ms > 0 { out_duration_ms } else { out.map_or(0, |a| a.duration_ms) };
    let in_dur = if in_duration_ms > 0 { in_duration_ms } else { inc.map_or(0, |a| a.duration_ms) };
    if s.same_album_in_order {
        return blank(TransitionKind::Gapless, out_dur.max(0), 0, 0, "same album in order: gapless".into());
    }
    let max_len = if s.max_transition_s.is_finite() { (s.max_transition_s.clamp(0.0, 60.0) * 1000.0) as i64 } else { 0 };
    if out_dur <= 0 || in_dur <= 0 || max_len < MIN_FADE_MS {
        return blank(TransitionKind::Gapless, out_dur.max(0), 0, 0, "no room for a transition: gapless".into());
    }
    // Never more than a third of either track.
    let max_len = max_len.min(out_dur / 3).min(in_dur / 3);
    if max_len < MIN_FADE_MS {
        return blank(TransitionKind::Gapless, out_dur, 0, 0, "tracks too short: gapless".into());
    }
    let (a, b) = (usable(out, out_dur).map(at_end), usable(inc, in_dur).map(at_start));
    let (a, b) = (a.as_ref(), b.as_ref());
    let mut why_not = String::new();
    // The gates only demote: a clash reroutes to an echo-out (or shortens when the echo cannot
    // run), a loudness gap or timbre mismatch caps an overlap at 8 bars. Never an upgrade.
    let verdict = a.zip(b).map(|(a, b)| pair_gate(a, b));
    // Why this pair is shortened, for the reason line; empty when it is not.
    let short_cause = verdict.map(|v| if v.clash { "clash" } else { v.short_cause }).unwrap_or("");
    let short = !short_cause.is_empty();
    if let (Some(a), Some(b)) = (a, b) {
        if !s.beat_match {
            why_not = "beat matching off".into();
        } else if !grid_ok(a) || !grid_ok(b) {
            why_not = format!(
                "no reliable beat grid (out {:.0} BPM conf {:.2} stab {:.2}, in {:.0} BPM conf {:.2} stab {:.2}; needs conf ≥ {:.1}, stab ≥ {:.1})",
                a.bpm, a.bpm_confidence, a.stability, b.bpm, b.bpm_confidence, b.stability,
                MIN_BPM_CONFIDENCE, MIN_STABILITY
            );
        } else if let Some(v) = verdict {
            if v.clash && s.echo_out {
                if let Some(p) = echo_out(a, b, out_dur, in_dur, max_len, s, v.cause) {
                    return p;
                }
            }
            match beat_matched(a, b, out_dur, in_dur, max_len, s, short_cause) {
                Ok(p) => return p,
                Err(e) => why_not = e,
            }
        } else {
            match beat_matched(a, b, out_dur, in_dur, max_len, s, "") {
                Ok(p) => return p,
                Err(e) => why_not = e,
            }
        }
    }
    // One grid survived (or the lock failed on two good ones): align to what exists rather than
    // fading blind. See one_grid.
    if s.beat_match && (a.is_some_and(grid_ok) || b.is_some_and(grid_ok)) {
        if let Some(p) = one_grid(a, b, out_dur, in_dur, max_len, s, short, &why_not) {
            return p;
        }
    }
    if a.is_some() || b.is_some() {
        return mixramp(a, b, out_dur, in_dur, max_len, s, &why_not);
    }
    let dur = max_len.min(MAX_BLIND_FADE_MS);
    blank(TransitionKind::EqualPowerFade, out_dur - dur, 0, dur, "not analysed: equal-power fade".into())
}

fn beat_matched(a: &TrackAnalysis, b: &TrackAnalysis, out_dur: i64, in_dur: i64, max_len: i64, s: &AutoMixSettings, short_cause: &str) -> Result<TransitionPlan, String> {
    let bpm_a = bpm_with_tag(a.bpm, s.out_tag_bpm);
    let bpm_b = bpm_with_tag(b.bpm, s.in_tag_bpm);
    let mut ratio = match_ratio(bpm_a, bpm_b);
    let pct = (ratio - 1.0).abs() * 100.0;
    let mut max_pct = (s.max_tempo_change_pct as f64).clamp(0.0, 12.0);
    if !s.keep_pitch {
        max_pct = max_pct.min(VARISPEED_MAX_PCT);
    }
    if pct > max_pct + 1e-9 {
        return Err(format!("tempo gap {pct:.1} % over the {max_pct:.1} % limit"));
    }
    if (ratio - 1.0).abs() < 0.0005 {
        ratio = 1.0;
    }
    let beat_a = 60_000.0 / bpm_a;
    let bar = 4.0 * beat_a;
    let b_bpm = fold(bpm_a, bpm_b);
    let beat_b = 60_000.0 / b_bpm;
    // Camelot: ≤1 = harmonic (long natural blend), 2 = workable but short, >2 = mild clash.
    // DJ.Studio Harmonize and Apple both lengthen compatible pairs and shorten the rest.
    let dist = camelot_dist(a, b);
    let mild_clash = dist > 2;
    let max_bars = if mild_clash || !short_cause.is_empty() || dist == 2 {
        8
    } else {
        16
    };
    // Non-trivial tempo change on a harmonic pair: try a longer unique bridge first (Apple remix runway).
    let want_extend = pct > 2.0 && dist >= 0 && dist <= 1 && short_cause.is_empty();

    // The incoming track starts on its first downbeat, at or just before the music.
    let phase_b = b.downbeat_phase.rem_euclid(4) as i64;
    let mut nb = (((b.silence_start_ms as f64 - b.beat_offset_ms) / beat_b) - 0.25).ceil() as i64;
    while nb.rem_euclid(4) != phase_b {
        nb += 1;
    }
    let in_start = (b.beat_offset_ms + nb as f64 * beat_b).round().max(0.0) as i64;
    if in_start > MAX_SKIP_MS {
        return Err(format!("incoming track starts at {} s", in_start / 1000));
    }

    let end_a = music_end(a, out_dur).max(out_dur - MAX_SKIP_MS);
    let phase_a = a.downbeat_phase.rem_euclid(4) as i64;
    let downbeat_before = |t: f64| -> f64 {
        let mut n = ((t - a.beat_offset_ms) / beat_a).floor() as i64;
        while n.rem_euclid(4) != phase_a {
            n -= 1;
        }
        a.beat_offset_ms + n as f64 * beat_a
    };

    // Longer first. When unique outro is short, loop a 4/8-bar slice to fill the target (iOS 27 remix).
    let mut try_bars: Vec<i64> = [16i64, 8, 4].into_iter().filter(|b| *b <= max_bars).collect();
    if want_extend && max_bars >= 8 && !try_bars.contains(&12) {
        // Prefer a 12-bar bridge between 8 and 16 when tempos need runway.
        try_bars.insert(1, 12);
    }
    for &bars in &try_bars {
        let dur = bars as f64 * bar;
        if dur > max_len as f64 || dur * ratio > (in_dur - in_start) as f64 / 2.0 {
            continue;
        }
        let latest = downbeat_before(end_a as f64 - dur);
        let phrase = if a.outro_start_ms > 0 && (a.outro_start_ms as f64) <= latest {
            let k = ((latest - a.outro_start_ms as f64) / (8.0 * bar) + 1e-6).floor();
            Some(a.outro_start_ms as f64 + k * 8.0 * bar)
        } else {
            None
        };
        let start_try = phrase.filter(|p| out_dur as f64 - (p + dur) <= MAX_SKIP_MS as f64).unwrap_or(latest);
        let fits_unique = start_try >= a.silence_start_ms as f64
            && end_a as f64 - start_try + 1.0 >= dur
            && out_dur as f64 - (start_try + dur) <= MAX_SKIP_MS as f64;
        let on_phrase = phrase.is_some_and(|p| (start_try - p).abs() < 1.0);

        let (hold_start, mix_dur, out_loop_ms, looped) = if fits_unique {
            (start_try, dur.round() as i64, -1i64, false)
        } else {
            // Capture a loopable slice and wrap to the target length.
            let loop_bars = [8i64, 4].into_iter().find(|&lb| {
                lb <= bars && (lb as f64 * bar) <= (end_a - a.silence_start_ms).max(0) as f64 + 1.0
            });
            match loop_bars {
                Some(lb) => {
                    let loop_ms = (lb as f64 * bar).round() as i64;
                    let loop_start = downbeat_before(end_a as f64 - lb as f64 * bar);
                    if loop_start < a.silence_start_ms as f64
                        || out_dur as f64 - (loop_start + lb as f64 * bar) > MAX_SKIP_MS as f64
                    {
                        continue;
                    }
                    (loop_start, dur.round() as i64, loop_ms, true)
                }
                None => continue,
            }
        };

        // Intro loop is planned only when the whole overlap fits inside the intro (no live-stream
        // discard after the mix). Longer intros stay one-pass; the outro remix covers the runway.
        let intro_len = (b.intro_end_ms - in_start).max(0);
        let in_loop = if looped && intro_len >= mix_dur && intro_len > 0 {
            // Rare: mix sits entirely in the intro — still one pass, no wrap needed.
            -1
        } else if !looped && intro_len > 0 && (intro_len as f64) >= 2.0 * bar && (intro_len as f64) < mix_dur as f64 * 0.5 {
            // Mark a soft intent for logs; the sink only wraps the outgoing hold today.
            -1
        } else {
            -1
        };
        let _ = in_loop;
        return Ok(finish_beat_matched(
            a, b, s, ratio, pct, bpm_a, bpm_b, beat_a, beat_b, bar, bars, hold_start, in_start, mix_dur,
            out_loop_ms, -1, dist, mild_clash, short_cause, looped, on_phrase && !looped,
        ));
    }
    Err("no bar-aligned window fits".into())
}

fn finish_beat_matched(
    a: &TrackAnalysis,
    b: &TrackAnalysis,
    s: &AutoMixSettings,
    ratio: f64,
    pct: f64,
    bpm_a: f64,
    bpm_b: f64,
    beat_a: f64,
    beat_b: f64,
    bar: f64,
    bars: i64,
    start: f64,
    in_start: i64,
    dur_ms: i64,
    out_loop_ms: i64,
    in_loop_ms: i64,
    dist: i32,
    mild_clash: bool,
    short_cause: &str,
    remix: bool,
    on_phrase: bool,
) -> TransitionPlan {
    let intro_wall = (b.intro_end_ms - in_start) as f64 / ratio;
    let swap_bars = if b.intro_end_ms > in_start && intro_wall >= bar && intro_wall <= dur_ms as f64 - bar {
        (intro_wall / bar).round()
    } else {
        (bars / 2) as f64
    };
    let swap = (swap_bars * bar).round() as i64;
    let beat_ms = beat_a.round() as i64;
    let mut p = blank(TransitionKind::BeatMatched, start.round() as i64, in_start, dur_ms, String::new());
    p.tempo_ratio = ratio;
    p.keep_pitch = s.keep_pitch;
    if ratio != 1.0 {
        p.tempo_ramp_beats = if pct <= 2.0 { 16 } else { 32 };
        p.tempo_ramp_ms = (p.tempo_ramp_beats as f64 * beat_b / ((1.0 + ratio) / 2.0)).round() as i64;
    }
    p.fade_curve = FadeCurve::SineSquared;
    (p.in_fade_start_ms, p.in_fade_end_ms) = (0, swap);
    (p.out_fade_start_ms, p.out_fade_end_ms) = ((swap + beat_ms).min(dur_ms), dur_ms);
    if s.bass_swap {
        (p.bass_swap_ms, p.bass_swap_len_ms) = (swap, beat_ms);
    }
    // Harmonic pairs: skip or soften the LPF (iOS 27 moved off the predictable underwater dump).
    // Stretched keys: DJ filter-open (HPF). Farther / unknown: classic LPF after the bass hand-over.
    if s.filter_effects {
        match dist {
            0 => {}
            1 => {
                let start_f = ((swap + dur_ms) / 2).max(swap);
                (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) =
                    (start_f, dur_ms, SWEEP_FROM_HZ, SWEEP_TO_HZ_SOFT);
            }
            2 => {
                (p.hp_start_ms, p.hp_end_ms, p.hp_from_hz, p.hp_to_hz) = (0, swap.max(beat_ms * 4), 40.0, 1_200.0);
            }
            _ => {
                (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) =
                    (swap, dur_ms, SWEEP_FROM_HZ, SWEEP_TO_HZ_MATCHED);
            }
        }
    }
    p.out_loop_ms = out_loop_ms;
    p.in_loop_ms = in_loop_ms;
    p.in_gain_db = loudness_trim(Some(a), Some(b), s);
    let mut bits = Vec::new();
    if on_phrase {
        bits.push("on the outro phrase".to_string());
    }
    if remix {
        bits.push("intro/outro remix".to_string());
    }
    if mild_clash {
        bits.push("keys clash: short".to_string());
    } else if dist == 2 {
        bits.push("keys stretch: filter-open".to_string());
    } else if !short_cause.is_empty() {
        bits.push(format!("short ({short_cause})"));
    } else if dist == 0 {
        bits.push("same key".to_string());
    } else if dist == 1 {
        bits.push("harmonic".to_string());
    }
    let extra = if bits.is_empty() { String::new() } else { format!(", {}", bits.join(", ")) };
    p.reason = format!(
        "beat-matched {bars} bars, {bpm_b:.1} -> {bpm_a:.1} BPM ({:+.1} %){extra}",
        (ratio - 1.0) * 100.0,
    );
    p
}

/// One side has a usable grid and the two could not be locked (the other grid is unreliable, the
/// tempos are too far apart, or no shared window fits): align to the grid that exists. An exit that
/// starts on the outgoing track's downbeats, or an entrance that lands on the incoming track's,
/// still sounds intentional where a blind fade sounds accidental. No tempo change: with only one
/// tempo known there is nothing to match to, so this is a well-placed fade, not a mix.
fn one_grid(a: Option<&TrackAnalysis>, b: Option<&TrackAnalysis>, out_dur: i64, in_dur: i64, max_len: i64, s: &AutoMixSettings, short: bool, why_not: &str) -> Option<TransitionPlan> {
    let tail = if why_not.is_empty() { String::new() } else { format!(" ({why_not})") };
    let end_a = a.map_or(out_dur, |x| music_end(x, out_dur)).max(out_dur - MAX_SKIP_MS);
    // The outgoing grid carries the exit: 8 bars of it, then 4 (4 only when the gates shortened it).
    let out_bars: &[i64] = if short { &[4] } else { &[8, 4] };
    if let Some(g) = a.filter(|x| grid_ok(x)) {
        let beat = 60_000.0 / g.bpm;
        let phase = g.downbeat_phase.rem_euclid(4) as i64;
        let in_start = b.map_or(0, |x| x.silence_start_ms.clamp(0, MAX_SKIP_MS.min(in_dur / 3)));
        for bars in out_bars {
            let dur = (*bars as f64 * 4.0 * beat).round() as i64;
            if dur > max_len || dur < MIN_FADE_MS {
                continue;
            }
            let mut n = ((end_a as f64 - dur as f64 - g.beat_offset_ms) / beat).floor() as i64;
            while n.rem_euclid(4) != phase {
                n -= 1;
            }
            let start = (g.beat_offset_ms + n as f64 * beat).round() as i64;
            if start < g.silence_start_ms || out_dur - (start + dur) > MAX_SKIP_MS {
                continue;
            }
            let mut p = blank(TransitionKind::MixRampFade, start, in_start, dur, String::new());
            p.fade_curve = FadeCurve::SineSquared;
            apply_fade_filter(&mut p, a, b, s, dur);
            p.in_gain_db = loudness_trim(a, b, s);
            p.reason = format!("downbeat-aligned fade, {bars} bars out{tail}");
            return Some(p);
        }
    }
    // Else the incoming grid carries the entrance: its first downbeat at or after its music starts.
    if let Some(g) = b.filter(|x| grid_ok(x)) {
        let beat = 60_000.0 / g.bpm;
        let phase = g.downbeat_phase.rem_euclid(4) as i64;
        let mut nb = (((g.silence_start_ms as f64 - g.beat_offset_ms) / beat) - 0.25).ceil() as i64;
        while nb.rem_euclid(4) != phase {
            nb += 1;
        }
        let in_start = (g.beat_offset_ms + nb as f64 * beat).round().max(0.0) as i64;
        if in_start > MAX_SKIP_MS {
            return None;
        }
        for bars in out_bars {
            let dur = (*bars as f64 * 4.0 * beat).round() as i64;
            if dur > max_len || dur < MIN_FADE_MS || dur > in_dur - in_start {
                continue;
            }
            let start = end_a - dur;
            if start < 0 || out_dur - (start + dur) > MAX_SKIP_MS {
                continue;
            }
            let mut p = blank(TransitionKind::MixRampFade, start, in_start, dur, String::new());
            p.fade_curve = FadeCurve::SineSquared;
            apply_fade_filter(&mut p, a, b, s, dur);
            p.in_gain_db = loudness_trim(a, b, s);
            p.reason = format!("downbeat-aligned fade, {bars} bars in{tail}");
            return Some(p);
        }
    }
    None
}

fn mixramp(a: Option<&TrackAnalysis>, b: Option<&TrackAnalysis>, out_dur: i64, in_dur: i64, max_len: i64, s: &AutoMixSettings, why_not: &str) -> TransitionPlan {
    let end_a = a.map_or(out_dur, |a| music_end(a, out_dur)).max(out_dur - MAX_SKIP_MS);
    let in_start = b.map_or(0, |b| b.silence_start_ms.clamp(0, MAX_SKIP_MS.min(in_dur / 3)));
    // How long the outgoing track is already quiet at its end, and the incoming one still quiet at its start.
    let tail = a.map(|a| if a.mixramp_end_ms > 0 { (end_a - a.mixramp_end_ms).max(0) } else { 0 });
    let head = b.map(|b| if b.mixramp_start_ms > 0 { (b.mixramp_start_ms - in_start).max(0) } else { 0 });
    let mut dur = tail.unwrap_or(0) + head.unwrap_or(0);
    if tail.is_none() || head.is_none() {
        // One side is unknown: give it an ordinary fade.
        dur = dur.max(max_len.min(4000));
    }
    let dur = dur.max(MIN_MIXRAMP_MS).clamp(MIN_FADE_MS.min(max_len), max_len);
    let mut p = blank(TransitionKind::MixRampFade, end_a - dur, in_start, dur, String::new());
    // A loud start comes in at once rather than being faded up; a quiet one rides its own ramp.
    p.in_fade_end_ms = head.map_or(dur, |h| h.clamp(MIN_FADE_MS.min(dur), dur));
    apply_fade_filter(&mut p, a, b, s, dur);
    p.in_gain_db = loudness_trim(a, b, s);
    p.reason = format!("mixramp fade {:.1} s{}", dur as f64 / 1000.0, if why_not.is_empty() { String::new() } else { format!(" ({why_not})") });
    p
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automix::structure::camelot;

    /// A 4-minute, 128 BPM track, 4/4 from 0, with an 8-bar intro, a 16-bar outro and trimmed silence at both ends.
    fn track(bpm: f64) -> TrackAnalysis {
        let beat = 60_000.0 / bpm;
        TrackAnalysis {
            song_id: "t".into(),
            analysis_version: 1,
            duration_ms: 240_000,
            bpm,
            bpm_confidence: 0.9,
            beat_offset_ms: 120.0,
            stability: 0.9,
            downbeat_phase: 0,
            downbeat_confidence: 0.8,
            lufs: -9.0,
            key: camelot(0, false),
            key_confidence: 0.8,
            silence_start_ms: 100,
            silence_end_ms: 238_500,
            mixramp_start_ms: 400,
            mixramp_end_ms: 236_000,
            intro_end_ms: (120.0 + 32.0 * beat) as i64,
            outro_start_ms: (120.0 + beat * 4.0 * (((238_500.0 - 120.0) / (4.0 * beat)).floor() - 16.0)) as i64,
            outro_vocal: 0.1,
            intro_vocal: 0.1,
            outro_centroid: 1200.0,
            intro_centroid: 1200.0,
            analysed_ms: 0,
            outro_bpm: bpm,
            outro_bpm_confidence: 0.9,
            outro_beat_offset_ms: 120.0,
            outro_stability: 0.9,
            outro_downbeat_phase: 0,
            intro_bpm: bpm,
            intro_bpm_confidence: 0.9,
            intro_beat_offset_ms: 120.0,
            intro_stability: 0.9,
            intro_downbeat_phase: 0,
        }
    }

    fn check_skip(p: &TransitionPlan, out_dur: i64) {
        assert!(p.in_start_ms <= MAX_SKIP_MS, "{p:?}");
        // Outro remix: only the captured loop must stay inside the track; duration may wrap past it.
        let heard_end = if p.out_loop_ms > 0 { p.out_start_ms + p.out_loop_ms } else { p.out_start_ms + p.duration_ms };
        assert!(out_dur - heard_end <= MAX_SKIP_MS, "{p:?}");
        assert!(p.out_start_ms >= 0 && p.duration_ms >= 0 && heard_end <= out_dur, "{p:?}");
    }

    #[test]
    fn same_album_in_order_is_gapless() {
        let s = AutoMixSettings { same_album_in_order: true, ..Default::default() };
        let p = plan(Some(&track(128.0)), Some(&track(128.0)), 240_000, 240_000, &s);
        assert_eq!(p.kind, TransitionKind::Gapless);
        assert_eq!((p.out_start_ms, p.in_start_ms, p.duration_ms), (240_000, 0, 0));
    }

    #[test]
    fn nothing_known_is_a_fixed_equal_power_fade() {
        let s = AutoMixSettings { max_transition_s: 8.0, ..Default::default() };
        let p = plan(None, None, 200_000, 180_000, &s);
        assert_eq!(p.kind, TransitionKind::EqualPowerFade);
        assert_eq!(p.fade_curve, FadeCurve::EqualPower);
        assert_eq!((p.out_start_ms, p.in_start_ms, p.duration_ms), (192_000, 0, 8000));
        assert_eq!((p.bass_swap_ms, p.filter_start_ms, p.tempo_ratio), (-1, -1, 1.0));
        check_skip(&p, 200_000);
        // The blind fade is capped, and never longer than a third of a short track.
        let p = plan(None, None, 200_000, 180_000, &AutoMixSettings { max_transition_s: 30.0, ..Default::default() });
        assert_eq!(p.duration_ms, 12_000);
        let p = plan(None, None, 200_000, 9_000, &AutoMixSettings { max_transition_s: 30.0, ..Default::default() });
        assert_eq!(p.duration_ms, 3_000);
        let p = plan(None, None, 200_000, 600, &s);
        assert_eq!(p.kind, TransitionKind::Gapless);
    }

    #[test]
    fn confident_grids_beat_match_on_bars_with_a_bass_swap() {
        let s = AutoMixSettings::default();
        let (a, b) = (track(128.0), track(124.0));
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &s);
        assert_eq!(p.kind, TransitionKind::BeatMatched, "{}", p.reason);
        check_skip(&p, 240_000);
        assert!((p.tempo_ratio - 128.0 / 124.0).abs() < 1e-9);
        assert!(p.keep_pitch);
        assert_eq!(p.tempo_ramp_beats, 32, "a 3 % change ramps back over 8 bars");
        let bar = 4.0 * 60_000.0 / 128.0;
        // 16 bars of 128 BPM is 30 s, over the 16 s default; 8 bars is 15 s.
        assert!((p.duration_ms as f64 - 8.0 * bar).abs() <= 1.0, "{}", p.duration_ms);
        // Starts on a downbeat of the outgoing track and on the first downbeat of the incoming one.
        let beats = (p.out_start_ms as f64 - a.beat_offset_ms) / (60_000.0 / 128.0);
        assert!((beats - beats.round()).abs() < 0.01 && (beats.round() as i64) % 4 == 0, "{beats}");
        assert_eq!(p.in_start_ms, 120);
        // The bass swap lands on a bar line inside the window, lasts a beat, and splits the fades.
        assert!(p.bass_swap_ms > 0 && p.bass_swap_ms < p.duration_ms);
        let swap_bars = p.bass_swap_ms as f64 / bar;
        assert!((swap_bars - swap_bars.round()).abs() < 0.01);
        assert_eq!(p.bass_swap_len_ms, (60_000.0f64 / 128.0).round() as i64);
        assert_eq!((p.in_fade_start_ms, p.in_fade_end_ms), (0, p.bass_swap_ms));
        assert_eq!(p.out_fade_end_ms, p.duration_ms);
        assert_eq!(p.fade_curve, FadeCurve::SineSquared);
        assert_eq!(p.filter_start_ms, -1, "same key: natural blend, no underwater LPF");
        assert_eq!(p.in_gain_db, 0.0, "loudness matching is off by default");
        assert!(p.reason.contains("same key"), "{}", p.reason);

        let long = AutoMixSettings { max_transition_s: 40.0, ..Default::default() };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &long);
        assert!((p.duration_ms as f64 - 16.0 * bar).abs() <= 1.0, "room for 16 bars: {}", p.reason);
        check_skip(&p, 240_000);
    }

    #[test]
    fn harmonic_neighbour_gets_a_soft_filter_and_long_mix() {
        // C major -> G major: Camelot distance 1.
        let a = track(128.0);
        let b = TrackAnalysis { key: camelot(7, false), ..track(124.0) };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings { max_transition_s: 40.0, ..Default::default() });
        assert_eq!(p.kind, TransitionKind::BeatMatched, "{}", p.reason);
        assert!(p.reason.contains("harmonic"), "{}", p.reason);
        let bar = 4.0 * 60_000.0 / 128.0;
        assert!((p.duration_ms as f64 - 16.0 * bar).abs() <= 1.0, "{}", p.duration_ms);
        assert!(p.filter_start_ms >= 0);
        assert!((p.filter_to_hz - SWEEP_TO_HZ_SOFT).abs() < 1.0, "soft, not underwater: {}", p.filter_to_hz);
    }

    #[test]
    fn stretched_keys_open_the_filter() {
        // C major -> D major: Camelot distance 2 (two fifths).
        let a = track(128.0);
        let b = TrackAnalysis { key: camelot(2, false), ..track(128.0) };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings { max_transition_s: 40.0, ..Default::default() });
        assert_eq!(p.kind, TransitionKind::BeatMatched, "{}", p.reason);
        assert!(p.reason.contains("filter-open"), "{}", p.reason);
        assert!((p.duration_ms as f64 - 8.0 * 4.0 * 60_000.0 / 128.0).abs() <= 1.0, "{}", p.duration_ms);
        assert!(p.hp_start_ms >= 0 && p.hp_to_hz > 500.0, "DJ filter-open HPF");
        assert_eq!(p.filter_start_ms, -1, "no underwater LPF on stretched keys");
    }

    #[test]
    fn short_outro_loops_to_fill_the_mix() {
        // Only five bars of music at the end: an 8-bar mix must loop a 4-bar slice.
        let beat = 60_000.0 / 128.0;
        let music = (5.0 * 4.0 * beat) as i64;
        let a = TrackAnalysis {
            silence_start_ms: 238_500 - music,
            silence_end_ms: 238_500,
            outro_start_ms: 238_500 - music,
            mixramp_end_ms: 238_500 - music / 2,
            ..track(128.0)
        };
        let b = track(128.0);
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings { max_transition_s: 40.0, ..Default::default() });
        assert_eq!(p.kind, TransitionKind::BeatMatched, "{}", p.reason);
        assert!(p.out_loop_ms > 0, "outro should loop: {}", p.reason);
        assert!(p.reason.contains("remix"), "{}", p.reason);
        assert!(p.duration_ms > p.out_loop_ms);
        check_skip(&p, 240_000);
    }

    #[test]
    fn tag_bpm_settles_a_half_double() {
        // Analysis reported half tempo; the server tag is the true 128.
        let a = TrackAnalysis { bpm: 64.0, ..track(128.0) };
        let b = track(128.0);
        let s = AutoMixSettings { out_tag_bpm: 128.0, max_transition_s: 40.0, ..Default::default() };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &s);
        assert_eq!(p.kind, TransitionKind::BeatMatched, "{}", p.reason);
        assert!((p.tempo_ratio - 1.0).abs() < 0.01, "folded to tag: ratio {}", p.tempo_ratio);
    }

    #[test]
    fn half_and_double_tempo_still_match() {
        let p = plan(Some(&track(174.0)), Some(&track(88.0)), 240_000, 240_000, &AutoMixSettings::default());
        assert_eq!(p.kind, TransitionKind::BeatMatched, "{}", p.reason);
        assert!((p.tempo_ratio - 174.0 / 176.0).abs() < 1e-9);
        assert_eq!(p.tempo_ramp_beats, 16);
    }

    #[test]
    fn tempo_gaps_and_varispeed_limits_fall_back_to_a_mixramp_fade() {
        let (a, b) = (track(128.0), track(118.0)); // 8.5 %
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings::default());
        assert_eq!(p.kind, TransitionKind::MixRampFade);
        assert!(p.reason.contains("tempo gap"), "{}", p.reason);
        check_skip(&p, 240_000);

        let b = track(125.0); // 2.4 %: fine for time-stretch, too much for varispeed
        let s = AutoMixSettings { keep_pitch: false, ..Default::default() };
        assert_eq!(plan(Some(&a), Some(&b), 240_000, 240_000, &s).kind, TransitionKind::MixRampFade);
        let b = track(126.0); // 1.6 %
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &s);
        assert_eq!(p.kind, TransitionKind::BeatMatched);
        assert!(!p.keep_pitch);

        let off = AutoMixSettings { beat_match: false, ..Default::default() };
        let p = plan(Some(&a), Some(&a), 240_000, 240_000, &off);
        assert_eq!(p.kind, TransitionKind::MixRampFade);
        assert!(p.reason.contains("beat matching off"));
    }

    #[test]
    fn a_band_that_drifts_is_matched_on_its_steady_ends() {
        // Played without a click: one grid over the whole song misses its last beats (no stability),
        // but its last and first forty seconds each hold a steady beat. That is a beat-matched mix.
        let a = TrackAnalysis { stability: 0.0, ..track(128.0) };
        let b = TrackAnalysis { stability: 0.0, ..track(126.0) };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings::default());
        assert_eq!(p.kind, TransitionKind::BeatMatched, "{}", p.reason);
        check_skip(&p, 240_000);
    }

    #[test]
    fn a_song_that_changes_tempo_is_mixed_at_the_tempo_it_ends_on() {
        // 100 BPM for most of the song, 128 at the end: what the next song has to meet is 128.
        let a = TrackAnalysis { bpm: 100.0, ..track(128.0) };
        let p = plan(Some(&a), Some(&track(128.0)), 240_000, 240_000, &AutoMixSettings::default());
        assert_eq!(p.kind, TransitionKind::BeatMatched, "{}", p.reason);
        assert_eq!(p.tempo_ratio, 1.0, "the ends already agree: {}", p.reason);
    }

    #[test]
    fn unreliable_grids_are_not_beat_matched() {
        let a = track(128.0);
        // Unreliable over the whole song and over its intro alike.
        for b in [
            TrackAnalysis { bpm_confidence: 0.2, intro_bpm_confidence: 0.2, ..track(128.0) },
            TrackAnalysis { stability: 0.3, intro_stability: 0.3, ..track(128.0) },
            TrackAnalysis { bpm: 0.0, intro_bpm: 0.0, ..track(128.0) },
        ] {
            let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings::default());
            assert_eq!(p.kind, TransitionKind::MixRampFade, "{b:?}");
            assert!(p.reason.contains("no reliable beat grid"));
        }
    }

    #[test]
    fn one_good_grid_aligns_the_fade_to_it() {
        // Outgoing grid known, incoming not: the exit starts on the outgoing track's downbeats.
        let b = TrackAnalysis { bpm_confidence: 0.2, intro_bpm_confidence: 0.2, ..track(128.0) };
        let p = plan(Some(&track(128.0)), Some(&b), 240_000, 240_000, &AutoMixSettings::default());
        assert_eq!(p.kind, TransitionKind::MixRampFade);
        assert!(p.reason.contains("downbeat-aligned") && p.reason.contains("bars out"), "{}", p.reason);
        check_skip(&p, 240_000);
        // 8 bars of 128 BPM from a downbeat of the outgoing track.
        let beat = 60_000.0 / 128.0;
        assert!((p.duration_ms as f64 - 8.0 * 4.0 * beat).abs() <= 1.0, "{}", p.duration_ms);
        let beats = (p.out_start_ms as f64 - 120.0) / beat;
        assert!((beats - beats.round()).abs() < 0.01 && (beats.round() as i64) % 4 == 0, "{beats}");

        // Incoming grid known, outgoing not: the entrance lands on the incoming track's downbeats.
        let a = TrackAnalysis { stability: 0.3, outro_stability: 0.3, ..track(128.0) };
        let p = plan(Some(&a), Some(&track(128.0)), 240_000, 240_000, &AutoMixSettings::default());
        assert!(p.reason.contains("bars in"), "{}", p.reason);
        check_skip(&p, 240_000);
        assert_eq!(p.in_start_ms, 120);
    }

    #[test]
    fn mixramp_overlaps_the_quiet_ends() {
        let s = AutoMixSettings { max_transition_s: 12.0, match_loudness: true, ..Default::default() };
        let a = TrackAnalysis { bpm: 0.0, outro_bpm: 0.0, silence_end_ms: 230_000, mixramp_end_ms: 226_000, lufs: -8.0, ..track(128.0) };
        let b = TrackAnalysis { bpm: 0.0, intro_bpm: 0.0, silence_start_ms: 1_000, mixramp_start_ms: 3_000, lufs: -14.0, ..track(128.0) };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &s);
        assert_eq!(p.kind, TransitionKind::MixRampFade);
        // 4 s of quiet tail plus 2 s of quiet head.
        assert_eq!((p.out_start_ms, p.in_start_ms, p.duration_ms), (224_000, 1_000, 6_000));
        assert_eq!(p.in_fade_end_ms, 2_000, "the incoming fade follows its own ramp");
        assert_eq!(p.filter_start_ms, 3_000, "same key: soft LPF in the second half");
        assert!((p.filter_to_hz - SWEEP_TO_HZ_SOFT).abs() < 1.0);
        assert_eq!(p.in_gain_db, 6.0, "incoming 6 dB quieter gets 6 dB");
        check_skip(&p, 240_000);

        // An abrupt end into a loud start: still a mix you can hear - the last five seconds shared, the
        // outgoing song fading under the incoming one, which is at full level almost at once. It used to
        // be a 0.3 s click guard, which sounds like no mix at all.
        let a = TrackAnalysis { bpm: 0.0, outro_bpm: 0.0, silence_end_ms: 240_000, mixramp_end_ms: 240_000, ..track(128.0) };
        let b = TrackAnalysis { bpm: 0.0, intro_bpm: 0.0, silence_start_ms: 0, mixramp_start_ms: 0, ..track(128.0) };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &s);
        assert_eq!((p.duration_ms, p.in_fade_end_ms, p.out_start_ms), (MIN_MIXRAMP_MS, MIN_FADE_MS, 240_000 - MIN_MIXRAMP_MS));

        // Only one side analysed: its points, and an ordinary fade length - no shorter than any MixRamp.
        let p = plan(Some(&a), None, 240_000, 240_000, &s);
        assert_eq!(p.kind, TransitionKind::MixRampFade);
        assert_eq!(p.duration_ms, MIN_MIXRAMP_MS);
        assert_eq!(p.in_start_ms, 0);
    }

    #[test]
    fn never_skips_more_than_15_seconds() {
        // A hidden track: 40 s of silence at the end, and a 25 s silent lead-in on the next track.
        let a = TrackAnalysis { silence_end_ms: 200_000, mixramp_end_ms: 198_000, ..track(128.0) };
        let b = TrackAnalysis { silence_start_ms: 25_000, mixramp_start_ms: 26_000, ..track(128.0) };
        for s in [AutoMixSettings::default(), AutoMixSettings { beat_match: false, ..Default::default() }] {
            let p = plan(Some(&a), Some(&b), 240_000, 240_000, &s);
            check_skip(&p, 240_000);
        }
    }

    #[test]
    fn clashing_keys_echo_out() {
        let a = track(128.0);
        let b = TrackAnalysis { key: camelot(6, false), ..track(128.0) };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings { max_transition_s: 40.0, ..Default::default() });
        assert_eq!(p.kind, TransitionKind::EchoOut, "{}", p.reason);
        assert!(p.reason.contains("keys far apart"), "{}", p.reason);
        assert_eq!(p.echo_delay_ms, (60_000.0_f64 / 128.0).round() as i64);
        assert_eq!((p.echo_feedback, p.echo_wet_db), (0.45, -7.0));
        assert_eq!(p.bass_swap_ms, -1, "no bass swap on an echo-out");
        assert!((p.tempo_ratio - 1.0).abs() < 1e-9);
        check_skip(&p, 240_000);
        // With the echo off a clashing pair gets a short beat-matched mix instead.
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings { max_transition_s: 40.0, echo_out: false, ..Default::default() });
        assert_eq!(p.kind, TransitionKind::BeatMatched);
        assert!((p.duration_ms as f64 - 8.0 * 4.0 * 60_000.0 / 128.0).abs() <= 1.0, "{}", p.reason);
        assert!(p.reason.contains("clash"));
    }

    #[test]
    fn overlapping_vocals_echo_out_and_gaps_shorten() {
        let sung = |t: TrackAnalysis| TrackAnalysis { outro_vocal: 0.7, intro_vocal: 0.7, ..t };
        let p = plan(Some(&sung(track(128.0))), Some(&sung(track(128.0))), 240_000, 240_000, &AutoMixSettings::default());
        assert_eq!(p.kind, TransitionKind::EchoOut, "{}", p.reason);
        assert!(p.reason.contains("vocals overlap"), "{}", p.reason);
        // Quiet outro into a far louder intro: still beat-matched, but short.
        let b = TrackAnalysis { lufs: -2.0, ..track(128.0) };
        let p = plan(Some(&track(128.0)), Some(&b), 240_000, 240_000, &AutoMixSettings { max_transition_s: 40.0, ..Default::default() });
        assert_eq!(p.kind, TransitionKind::BeatMatched);
        assert!((p.duration_ms as f64 - 8.0 * 4.0 * 60_000.0 / 128.0).abs() <= 1.0, "{}", p.reason);
        // Same for a timbre mismatch: a dark outro into a bright intro.
        let b = TrackAnalysis { intro_centroid: 5000.0, ..track(128.0) };
        let p = plan(Some(&track(128.0)), Some(&b), 240_000, 240_000, &AutoMixSettings { max_transition_s: 40.0, ..Default::default() });
        assert_eq!(p.kind, TransitionKind::BeatMatched);
        assert!((p.duration_ms as f64 - 8.0 * 4.0 * 60_000.0 / 128.0).abs() <= 1.0, "{}", p.reason);
    }

    #[test]
    fn a_stale_analysis_of_a_different_file_is_ignored() {
        let a = TrackAnalysis { duration_ms: 300_000, ..track(128.0) };
        let p = plan(Some(&a), None, 240_000, 240_000, &AutoMixSettings::default());
        assert_eq!(p.kind, TransitionKind::EqualPowerFade);
    }

    #[test]
    fn switches_turn_effects_off() {
        let s = AutoMixSettings { bass_swap: false, filter_effects: false, ..Default::default() };
        let p = plan(Some(&track(128.0)), Some(&track(128.0)), 240_000, 240_000, &s);
        assert_eq!(p.kind, TransitionKind::BeatMatched);
        assert_eq!((p.bass_swap_ms, p.filter_start_ms, p.tempo_ratio, p.tempo_ramp_ms), (-1, -1, 1.0, 0));
    }
}
