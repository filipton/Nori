//! Transition planning: a pure function from what is known about two tracks to a `TransitionPlan`.
//! The fallback ladder from the research, most to least informed:
//!
//! 1. both grids confident and stable, tempos within reach: beat-matched, bar-aligned, with bass swap;
//! 2. something analysed, no usable grid: overlap from the MixRamp points and trimmed silence, with a filter sweep;
//! 3. nothing known: a fixed equal-power crossfade;
//! 4. same album in order: gapless, no mixing.
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
const BASS_CUT_HZ: f32 = 180.0;
const SWEEP_FROM_HZ: f32 = 18_000.0;
/// Where the outgoing low-pass ends: beat-matched (lows already swapped out) and plain fades.
const SWEEP_TO_HZ_MATCHED: f32 = 400.0;
const SWEEP_TO_HZ_FADE: f32 = 500.0;

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
    let (a, b) = (usable(out, out_dur), usable(inc, in_dur));
    let mut why_not = String::new();
    if let (Some(a), Some(b)) = (a, b) {
        if !s.beat_match {
            why_not = "beat matching off".into();
        } else if !grid_ok(a) || !grid_ok(b) {
            why_not = format!(
                "no reliable beat grid (out {:.0} BPM conf {:.2} stab {:.2}, in {:.0} BPM conf {:.2} stab {:.2}; needs conf ≥ {:.1}, stab ≥ {:.1})",
                a.bpm, a.bpm_confidence, a.stability, b.bpm, b.bpm_confidence, b.stability,
                MIN_BPM_CONFIDENCE, MIN_STABILITY
            );
        } else {
            match beat_matched(a, b, out_dur, in_dur, max_len, s) {
                Ok(p) => return p,
                Err(e) => why_not = e,
            }
        }
    }
    // One grid survived (or the lock failed on two good ones): align to what exists rather than
    // fading blind. See one_grid.
    if s.beat_match && (a.is_some_and(grid_ok) || b.is_some_and(grid_ok)) {
        if let Some(p) = one_grid(a, b, out_dur, in_dur, max_len, s, &why_not) {
            return p;
        }
    }
    if a.is_some() || b.is_some() {
        return mixramp(a, b, out_dur, in_dur, max_len, s, &why_not);
    }
    let dur = max_len.min(MAX_BLIND_FADE_MS);
    blank(TransitionKind::EqualPowerFade, out_dur - dur, 0, dur, "not analysed: equal-power fade".into())
}

fn beat_matched(a: &TrackAnalysis, b: &TrackAnalysis, out_dur: i64, in_dur: i64, max_len: i64, s: &AutoMixSettings) -> Result<TransitionPlan, String> {
    let mut ratio = match_ratio(a.bpm, b.bpm);
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
    let beat_a = 60_000.0 / a.bpm;
    let bar = 4.0 * beat_a;
    let b_bpm = fold(a.bpm, b.bpm);
    let beat_b = 60_000.0 / b_bpm;
    let keys_known = a.key_confidence >= 0.4 && b.key_confidence >= 0.4;
    let clash = keys_known && key_distance(a.key, b.key) > 2;
    let max_bars = if clash { 8 } else { 16 };

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
    // Latest downbeat of A at or before `t`.
    let downbeat_before = |t: f64| -> f64 {
        let mut n = ((t - a.beat_offset_ms) / beat_a).floor() as i64;
        while n.rem_euclid(4) != phase_a {
            n -= 1;
        }
        a.beat_offset_ms + n as f64 * beat_a
    };
    for bars in [16i64, 8, 4].into_iter().filter(|b| *b <= max_bars) {
        let dur = bars as f64 * bar;
        if dur > max_len as f64 || dur * ratio > (in_dur - in_start) as f64 / 2.0 {
            continue;
        }
        // Phrase-aligned when the outro cue allows it (a multiple of 8 bars after it), else the last downbeat that fits.
        let latest = downbeat_before(end_a as f64 - dur);
        let phrase = if a.outro_start_ms > 0 && (a.outro_start_ms as f64) <= latest {
            let k = ((latest - a.outro_start_ms as f64) / (8.0 * bar) + 1e-6).floor();
            Some(a.outro_start_ms as f64 + k * 8.0 * bar)
        } else {
            None
        };
        let start = phrase.filter(|p| out_dur as f64 - (p + dur) <= MAX_SKIP_MS as f64).unwrap_or(latest);
        if start < a.silence_start_ms as f64 || out_dur as f64 - (start + dur) > MAX_SKIP_MS as f64 {
            continue;
        }
        let dur_ms = dur.round() as i64;
        // Bass swap: where the incoming intro ends if that falls inside the window, else half way; on a bar line.
        let intro_wall = (b.intro_end_ms - in_start) as f64 / ratio;
        let swap_bars = if b.intro_end_ms > in_start && intro_wall >= bar && intro_wall <= dur - bar {
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
        if s.filter_effects {
            (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) = (swap, dur_ms, SWEEP_FROM_HZ, SWEEP_TO_HZ_MATCHED);
        }
        p.in_gain_db = loudness_trim(Some(a), Some(b), s);
        p.reason = format!(
            "beat-matched {bars} bars, {:.1} -> {:.1} BPM ({:+.1} %){}{}",
            b.bpm,
            a.bpm,
            (ratio - 1.0) * 100.0,
            if phrase.is_some() && start == phrase.unwrap_or(-1.0) { ", on the outro phrase" } else { "" },
            if clash { ", keys clash: short" } else { "" }
        );
        return Ok(p);
    }
    Err("no bar-aligned window fits".into())
}

/// One side has a usable grid and the two could not be locked (the other grid is unreliable, the
/// tempos are too far apart, or no shared window fits): align to the grid that exists. An exit that
/// starts on the outgoing track's downbeats, or an entrance that lands on the incoming track's,
/// still sounds intentional where a blind fade sounds accidental. No tempo change: with only one
/// tempo known there is nothing to match to, so this is a well-placed fade, not a mix.
fn one_grid(a: Option<&TrackAnalysis>, b: Option<&TrackAnalysis>, out_dur: i64, in_dur: i64, max_len: i64, s: &AutoMixSettings, why_not: &str) -> Option<TransitionPlan> {
    let tail = if why_not.is_empty() { String::new() } else { format!(" ({why_not})") };
    let end_a = a.map_or(out_dur, |x| music_end(x, out_dur)).max(out_dur - MAX_SKIP_MS);
    // The outgoing grid carries the exit: 8 bars of it, then 4, starting on its downbeats.
    if let Some(g) = a.filter(|x| grid_ok(x)) {
        let beat = 60_000.0 / g.bpm;
        let phase = g.downbeat_phase.rem_euclid(4) as i64;
        let in_start = b.map_or(0, |x| x.silence_start_ms.clamp(0, MAX_SKIP_MS.min(in_dur / 3)));
        for bars in [8i64, 4] {
            let dur = (bars as f64 * 4.0 * beat).round() as i64;
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
            if s.filter_effects && dur >= 2000 {
                (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) = (0, dur, SWEEP_FROM_HZ, SWEEP_TO_HZ_FADE);
            }
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
        for bars in [8i64, 4] {
            let dur = (bars as f64 * 4.0 * beat).round() as i64;
            if dur > max_len || dur < MIN_FADE_MS || dur > in_dur - in_start {
                continue;
            }
            let start = end_a - dur;
            if start < 0 || out_dur - (start + dur) > MAX_SKIP_MS {
                continue;
            }
            let mut p = blank(TransitionKind::MixRampFade, start, in_start, dur, String::new());
            p.fade_curve = FadeCurve::SineSquared;
            if s.filter_effects && dur >= 2000 {
                (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) = (0, dur, SWEEP_FROM_HZ, SWEEP_TO_HZ_FADE);
            }
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
    let dur = dur.clamp(MIN_FADE_MS.min(max_len), max_len);
    let mut p = blank(TransitionKind::MixRampFade, end_a - dur, in_start, dur, String::new());
    // A loud start comes in at once rather than being faded up; a quiet one rides its own ramp.
    p.in_fade_end_ms = head.map_or(dur, |h| h.clamp(MIN_FADE_MS.min(dur), dur));
    if s.filter_effects && dur >= 2000 {
        (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) = (0, dur, SWEEP_FROM_HZ, SWEEP_TO_HZ_FADE);
    }
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
            analysed_ms: 0,
        }
    }

    fn check_skip(p: &TransitionPlan, out_dur: i64) {
        assert!(p.in_start_ms <= MAX_SKIP_MS, "{p:?}");
        assert!(out_dur - (p.out_start_ms + p.duration_ms) <= MAX_SKIP_MS, "{p:?}");
        assert!(p.out_start_ms >= 0 && p.duration_ms >= 0 && p.out_start_ms + p.duration_ms <= out_dur, "{p:?}");
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
        assert!(p.filter_start_ms >= 0 && p.filter_to_hz < 1000.0);
        assert_eq!(p.in_gain_db, 0.0, "loudness matching is off by default");

        let long = AutoMixSettings { max_transition_s: 40.0, ..Default::default() };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &long);
        assert!((p.duration_ms as f64 - 16.0 * bar).abs() <= 1.0, "room for 16 bars: {}", p.reason);
        check_skip(&p, 240_000);
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
    fn unreliable_grids_are_not_beat_matched() {
        let a = track(128.0);
        for b in [
            TrackAnalysis { bpm_confidence: 0.2, ..track(128.0) },
            TrackAnalysis { stability: 0.3, ..track(128.0) },
            TrackAnalysis { bpm: 0.0, ..track(128.0) },
        ] {
            let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings::default());
            assert_eq!(p.kind, TransitionKind::MixRampFade, "{b:?}");
            assert!(p.reason.contains("no reliable beat grid"));
        }
    }

    #[test]
    fn one_good_grid_aligns_the_fade_to_it() {
        // Outgoing grid known, incoming not: the exit starts on the outgoing track's downbeats.
        let b = TrackAnalysis { bpm_confidence: 0.2, ..track(128.0) };
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
        let a = TrackAnalysis { stability: 0.3, ..track(128.0) };
        let p = plan(Some(&a), Some(&track(128.0)), 240_000, 240_000, &AutoMixSettings::default());
        assert!(p.reason.contains("bars in"), "{}", p.reason);
        check_skip(&p, 240_000);
        assert_eq!(p.in_start_ms, 120);
    }

    #[test]
    fn mixramp_overlaps_the_quiet_ends() {
        let s = AutoMixSettings { max_transition_s: 12.0, match_loudness: true, ..Default::default() };
        let a = TrackAnalysis { bpm: 0.0, silence_end_ms: 230_000, mixramp_end_ms: 226_000, lufs: -8.0, ..track(128.0) };
        let b = TrackAnalysis { bpm: 0.0, silence_start_ms: 1_000, mixramp_start_ms: 3_000, lufs: -14.0, ..track(128.0) };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &s);
        assert_eq!(p.kind, TransitionKind::MixRampFade);
        // 4 s of quiet tail plus 2 s of quiet head.
        assert_eq!((p.out_start_ms, p.in_start_ms, p.duration_ms), (224_000, 1_000, 6_000));
        assert_eq!(p.in_fade_end_ms, 2_000, "the incoming fade follows its own ramp");
        assert_eq!(p.filter_start_ms, 0);
        assert_eq!(p.in_gain_db, 6.0, "incoming 6 dB quieter gets 6 dB");
        check_skip(&p, 240_000);

        // An abrupt end into a loud start: a short click guard, the incoming track at full level almost at once.
        let a = TrackAnalysis { bpm: 0.0, silence_end_ms: 240_000, mixramp_end_ms: 240_000, ..track(128.0) };
        let b = TrackAnalysis { bpm: 0.0, silence_start_ms: 0, mixramp_start_ms: 0, ..track(128.0) };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &s);
        assert_eq!((p.duration_ms, p.in_fade_end_ms, p.out_start_ms), (MIN_FADE_MS, MIN_FADE_MS, 240_000 - MIN_FADE_MS));
        assert_eq!(p.filter_start_ms, -1, "too short for a sweep");

        // Only one side analysed: its points, and an ordinary fade length.
        let p = plan(Some(&a), None, 240_000, 240_000, &s);
        assert_eq!(p.kind, TransitionKind::MixRampFade);
        assert_eq!(p.duration_ms, 4_000);
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
    fn clashing_keys_get_a_short_mix() {
        let a = track(128.0);
        let b = TrackAnalysis { key: camelot(6, false), ..track(128.0) };
        let p = plan(Some(&a), Some(&b), 240_000, 240_000, &AutoMixSettings { max_transition_s: 40.0, ..Default::default() });
        assert_eq!(p.kind, TransitionKind::BeatMatched);
        assert!((p.duration_ms as f64 - 8.0 * 4.0 * 60_000.0 / 128.0).abs() <= 1.0, "{}", p.reason);
        assert!(p.reason.contains("clash"));
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
