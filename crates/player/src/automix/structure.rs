//! Bar- and phrase-level structure on top of the beat grid, and the key.
//! Metre: what repeats - 3/4 when the beats' rhythm and harmony come round every three clearly better than every
//! two or four, else 4/4. Downbeats: "downbeat voting" over the bar's phases, adding how hard the low band lands
//! (the kick) and chroma change (chords tend to change on the one). For a grid that only ever holds one metre and
//! one phase this is what an HMM over bar positions would decode, without the per-frame cost. Phrases: bar
//! boundaries at multiples of 8 bars from the first downbeat, kept when the energy jumps there. Key: the song's
//! pitch profile, its tuning taken out, against key profiles (Temperley's major, Krumhansl-Kessler's minor).

use super::analysis::Features;
use super::loudness;
use super::tempo::Tempo;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Downbeat {
    pub phase: i32,
    pub confidence: f32,
    /// 3 or 4; 0 when nothing was measured.
    pub beats_per_bar: i64,
}

/// A section boundary: the 4 bars after differ from the 4 before by this much (RMS over the bar features, each in
/// units of its spread across the song). Tuned on the synthetic songs of `eval.rs`, where it finds 26 of 32
/// intro and outro boundaries (a level-jump rule found 12): drums-only intros and outros score well above it, a
/// voice coming in only just.
const NOVELTY_MIN: f64 = 0.6;
const NOVELTY_MIN_BLOCKS: f64 = 0.8;
/// The smallest change in each bar feature worth calling a change - 3 dB of level, 4 dB of low end or tonal energy,
/// a third more or less onset strength (log), a quarter octave of brightness, 8 % of voice-band share. Half of it
/// floors each feature's spread, so a song that never changes is not measured against its own noise.
const SECTION_SCALE: [f64; 6] = [3.0, 4.0, 4.0, 0.3, 0.25, 0.08];
/// Units (bars, or blocks without a grid) compared either side of a boundary.
const SECTION_SPAN: usize = 4;
/// How far a boundary may thin the music and still end an intro (or fill it and still start an outro).
const FULLER_EPS: f64 = 0.25;
/// How much better the beats must repeat every three than every two or four to call the bar 3/4.
const TRIPLE_MARGIN: f64 = 0.3;

fn frame_of(f: &Features, t: f64) -> isize {
    ((t - f.t0) * f.fps).round() as isize
}

fn z(v: &mut [f64]) {
    let n = v.len().max(1) as f64;
    let mean = v.iter().sum::<f64>() / n;
    let sd = (v.iter().map(|x| (x - mean).powi(2)).sum::<f64>() / n).sqrt();
    v.iter_mut().for_each(|x| *x = if sd > 1e-12 { (*x - mean) / sd } else { 0.0 });
}

/// Mean chroma, unit length, of the chroma frames centred in `[a, b)`.
fn chroma_between(f: &Features, a: f64, b: f64) -> [f64; 12] {
    let mut c = [0f64; 12];
    if f.chroma_step <= 0.0 {
        return c;
    }
    let j0 = ((a - f.chroma_t0) / f.chroma_step).ceil().max(0.0) as usize;
    let j1 = (((b - f.chroma_t0) / f.chroma_step).ceil().max(0.0) as usize).min(f.chroma.len());
    for frame in f.chroma.get(j0..j1.max(j0)).unwrap_or(&[]) {
        for (a, v) in c.iter_mut().zip(frame) {
            *a += *v as f64;
        }
    }
    let norm = c.iter().map(|v| v * v).sum::<f64>().sqrt();
    if norm > 0.0 {
        c.iter_mut().for_each(|v| *v /= norm);
    }
    c
}

/// Rise of the linear low-band level over two frames, per frame: how hard something heavy (a kick, a bass note)
/// lands. The log-compressed low-band flux answers a different question - how much the band changed relative to
/// itself - and reads a snare's faint low leakage into a silent band as loudly as a kick into a sustained bass.
fn low_rise(f: &Features) -> Vec<f32> {
    let rms: Vec<f32> = f.low_power.iter().map(|p| p.max(0.0).sqrt()).collect();
    (0..rms.len()).map(|k| if k < 2 { 0.0 } else { (rms[k] - rms[k - 2]).max(0.0) }).collect()
}

fn peak_near(v: &[f32], k: isize, reach: isize) -> f64 {
    let lo = (k - reach).max(0) as usize;
    let hi = ((k + reach + 1).max(0) as usize).min(v.len());
    v.get(lo..hi.max(lo)).unwrap_or(&[]).iter().fold(0f32, |m, x| m.max(*x)) as f64
}

/// Mean over `n` of the cosine between rows `n` and `n + lag` of `rows`, each row centred on the column means
/// first, so that what every beat shares does not count as repetition.
fn lag_similarity(rows: &[Vec<f64>], lag: usize) -> f64 {
    if rows.len() <= lag + 4 {
        return 0.0;
    }
    let dims = rows[0].len();
    let mut mean = vec![0f64; dims];
    for r in rows {
        for (m, v) in mean.iter_mut().zip(r) {
            *m += v / rows.len() as f64;
        }
    }
    let centred: Vec<Vec<f64>> = rows.iter().map(|r| r.iter().zip(&mean).map(|(v, m)| v - m).collect()).collect();
    let mut total = 0.0;
    for i in 0..centred.len() - lag {
        let (a, b) = (&centred[i], &centred[i + lag]);
        let dot: f64 = a.iter().zip(b).map(|(x, y)| x * y).sum();
        let na = a.iter().map(|x| x * x).sum::<f64>().sqrt();
        let nb = b.iter().map(|x| x * x).sum::<f64>().sqrt();
        if na > 1e-9 && nb > 1e-9 {
            total += dot / (na * nb);
        }
    }
    total / (centred.len() - lag) as f64
}

/// `music` is the audible span, seconds. `meter` is the bar length when it is already known (the whole song's,
/// for a window of it); otherwise it is measured here.
pub fn downbeat(t: &Tempo, f: &Features, music: (f64, f64), meter: Option<i64>) -> Downbeat {
    if t.period_s <= 0.0 || t.bpm <= 0.0 {
        return Downbeat::default();
    }
    let first = t.grid_pos(music.0).ceil() as i64;
    let last = t.grid_pos(music.1).floor() as i64 - 1;
    if last - first < 16 {
        return Downbeat::default();
    }
    let rise = low_rise(f);
    let reach = ((t.period_s / 8.0) * f.fps).round().max(1.0) as isize;
    let quarter = t.period_s / 4.0;
    let mut kick = Vec::with_capacity((last - first + 1) as usize);
    let mut change = Vec::with_capacity(kick.capacity());
    let mut rhythm: Vec<Vec<f64>> = Vec::with_capacity(kick.capacity());
    let mut prev = chroma_between(f, t.offset_s + (first - 1) as f64 * t.period_s, t.offset_s + first as f64 * t.period_s);
    for n in first..=last {
        let tn = t.offset_s + n as f64 * t.period_s;
        kick.push(peak_near(&rise, frame_of(f, tn), reach));
        let c = chroma_between(f, tn, tn + t.period_s);
        change.push(1.0 - c.iter().zip(&prev).map(|(a, b)| a * b).sum::<f64>());
        // What the beat sounds like: onsets on it and on its quarters, weight landing on it and half way.
        let mut r: Vec<f64> = (0..4).map(|q| peak_near(&f.onset, frame_of(f, tn + q as f64 * quarter), (reach / 2).max(1))).collect();
        r.push(peak_near(&rise, frame_of(f, tn), reach));
        r.push(peak_near(&rise, frame_of(f, tn + 2.0 * quarter), reach));
        rhythm.push(r);
        prev = c;
    }
    z(&mut kick);
    z(&mut change);
    // Each rhythm dimension on its own scale, so onsets and levels weigh alike.
    for d in 0..rhythm[0].len() {
        let mut col: Vec<f64> = rhythm.iter().map(|r| r[d]).collect();
        z(&mut col);
        for (r, v) in rhythm.iter_mut().zip(col) {
            r[d] = v;
        }
    }
    let evidence: Vec<f64> = kick.iter().zip(&change).map(|(k, c)| k + c).collect();
    let fold = |m: i64| -> Vec<f64> {
        let mut score = vec![0f64; m as usize];
        let mut count = vec![0usize; m as usize];
        for (i, n) in (first..=last).enumerate() {
            let p = n.rem_euclid(m) as usize;
            score[p] += evidence[i];
            count[p] += 1;
        }
        score.iter().zip(&count).map(|(s, c)| s / (*c).max(1) as f64).collect()
    };
    // Metre: a bar is what repeats. 4/4 unless the beats repeat every three clearly better than every two or four.
    let change_rows: Vec<Vec<f64>> = change.iter().map(|c| vec![*c]).collect();
    let sim = |lag: usize| lag_similarity(&rhythm, lag) + 0.5 * lag_similarity(&change_rows, lag);
    let (s2, s3, s4) = (sim(2), sim(3), sim(4));
    let beats_per_bar = match meter {
        Some(m @ (3 | 4)) => m,
        _ if s3 - s2.max(s4) > TRIPLE_MARGIN => 3,
        _ => 4,
    };
    let score = fold(beats_per_bar);
    let mut order: Vec<usize> = (0..score.len()).collect();
    order.sort_by(|a, b| score[*b].total_cmp(&score[*a]));
    let margin = score[order[0]] - score[order[1]];
    Downbeat { phase: order[0] as i32, confidence: ((margin - 0.15) / 0.6).clamp(0.0, 1.0) as f32, beats_per_bar }
}

fn mean_db(v: &[f32]) -> f64 {
    if v.is_empty() {
        return -120.0;
    }
    loudness::db(v.iter().map(|x| *x as f64).sum::<f64>() / v.len() as f64)
}

/// Intro end and outro start, seconds. With a usable grid they sit on bar lines a multiple of 4 bars from the
/// first downbeat; without one, on 2 s blocks. Either way they are where the music's sound changes most
/// (`section_changes`); a song that never changes falls back to its steady ending (the last 8-bar line at least
/// 16 bars before the end) or, without a grid, to the 0.5 s energy envelope.
pub fn cues(t: &Tempo, db: &Downbeat, f: &Features, music: (f64, f64), grid_ok: bool) -> (f64, f64) {
    let len = music.1 - music.0;
    if len <= 0.0 {
        return (music.0, music.1);
    }
    if grid_ok && t.period_s > 0.0 {
        if let Some(c) = phrase_cues(t, db, f, music) {
            return c;
        }
    }
    // 0.5 s blocks, dB; "loud" is within 3 dB of the median of the audible part.
    let bl: Vec<f64> = f.blocks_raw.chunks(5).map(|c| mean_db(c)).collect();
    let (a, b) = ((music.0 / 0.5) as usize, ((music.1 / 0.5).ceil() as usize).min(bl.len()));
    if b <= a {
        return (music.0, music.1);
    }
    let mut sorted = bl[a..b].to_vec();
    sorted.sort_unstable_by(|x, y| x.total_cmp(y));
    let loud = sorted[sorted.len() / 2] - 3.0;
    let intro = bl[a..b].iter().position(|v| *v >= loud).map_or(music.0, |i| (a + i) as f64 * 0.5).max(music.0);
    let outro = bl[a..b].iter().rposition(|v| *v >= loud).map_or(music.1, |i| (a + i + 1) as f64 * 0.5).min(music.1);
    // Blocks instead of bars: the same section changes, only not on a bar line.
    let units: Vec<f64> = (0..).map(|k| music.0 + BLOCK_S * k as f64).take_while(|u| *u <= music.1).collect();
    let (i, o) = section_changes(f, &units, 1, &t.beats, music);
    let intro = i.unwrap_or(intro);
    let outro = o.unwrap_or(outro);
    (intro, outro.max(intro))
}

/// What one bar sounds like, for telling sections apart: level, low end, tonal (chord) energy, onset density,
/// brightness and the voice-band share, each a mean over the bar.
fn frames_between<'a>(f: &Features, v: &'a [f32], a: f64, b: f64) -> &'a [f32] {
    let i = frame_of(f, a).max(0) as usize;
    let j = (frame_of(f, b).max(0) as usize).min(v.len());
    v.get(i..j.max(i)).unwrap_or(&[])
}

fn bar_vector(f: &Features, a: f64, b: f64) -> [f64; 6] {
    let mean = |v: &[f32]| if v.is_empty() { 0.0 } else { v.iter().map(|x| *x as f64).sum::<f64>() / v.len() as f64 };
    let tonal = {
        let j0 = ((a - f.chroma_t0) / f.chroma_step).ceil().max(0.0) as usize;
        let j1 = (((b - f.chroma_t0) / f.chroma_step).ceil().max(0.0) as usize).min(f.chroma.len());
        let frames = f.chroma.get(j0..j1.max(j0)).unwrap_or(&[]);
        let sum: f64 = frames.iter().map(|c| c.iter().map(|v| *v as f64).sum::<f64>()).sum();
        loudness::db(sum * sum / frames.len().max(1) as f64 + 1e-12)
    };
    [
        mean_db(frames_between(f, &f.power, a, b)),
        mean_db(frames_between(f, &f.low_power, a, b)),
        tonal,
        (mean(frames_between(f, &f.onset, a, b)) + 1e-3).ln(),
        (mean(frames_between(f, &f.centroid, a, b)) + 1.0).log2(),
        mean(frames_between(f, &f.vocal, a, b)),
    ]
}

/// Unit length of the section search without a beat grid, seconds.
const BLOCK_S: f64 = 2.0;

/// The intro's end and the outro's start among `units` (bar or block start times, the last one the end of the
/// last unit): boundaries every `step` units where what the music sounds like over the 4 units after differs
/// from the 4 before - Foote's novelty, on bars (fewer at the very end, so a short outro still counts). An intro
/// ends at the first clear change early on that does not thin the music out; an outro starts at the last one
/// late on that does not fill it up. A drums-only intro under a house track changes the level little - the kick
/// carries most of it - but chords, bass and tonal energy all arrive at once, which a level jump alone never saw.
/// Without a grid the units are blocks, and a boundary found is moved to the nearest tracked beat in `snap`.
fn section_changes(f: &Features, units: &[f64], step: usize, snap: &[f64], music: (f64, f64)) -> (Option<f64>, Option<f64>) {
    let n = units.len().saturating_sub(1);
    if n < 12 {
        return (None, None);
    }
    // Each feature on its own spread across the song, floored at half a noticeable change: a song with sections
    // is judged against its own contrasts, one that never changes stays quiet.
    let mut v: Vec<[f64; 6]> = units.windows(2).map(|w| bar_vector(f, w[0], w[1])).collect();
    for d in 0..6 {
        let mean = v.iter().map(|r| r[d]).sum::<f64>() / n as f64;
        let sd = (v.iter().map(|r| (r[d] - mean).powi(2)).sum::<f64>() / n as f64).sqrt().max(0.5 * SECTION_SCALE[d]);
        v.iter_mut().for_each(|r| r[d] = (r[d] - mean) / sd);
    }
    let span = |m0: usize, m1: usize| -> [f64; 6] {
        let mut s = [0f64; 6];
        for r in &v[m0..m1] {
            for (a, x) in s.iter_mut().zip(r) {
                *a += x / (m1 - m0) as f64;
            }
        }
        s
    };
    // (novelty, how much fuller it gets) at unit m: 4 units either side, down to 2 at the end of the song.
    let change = |m: usize| -> (f64, f64) {
        let w = SECTION_SPAN.min(n - m);
        let (before, after) = (span(m - SECTION_SPAN, m), span(m, m + w));
        let novelty = (before.iter().zip(&after).map(|(x, y)| (y - x).powi(2)).sum::<f64>() / 6.0).sqrt();
        let fuller = (after[0] + after[1] + after[2] + after[3] - before[0] - before[1] - before[2] - before[3]) / 4.0;
        (novelty, fuller)
    };
    // Blocks cut across bars and phrases, so they differ from each other more at random: a higher bar.
    let novelty_min = if snap.is_empty() { NOVELTY_MIN } else { NOVELTY_MIN_BLOCKS };
    let to_beat = |t: f64| -> f64 {
        snap.iter().copied().filter(|b| (b - t).abs() <= BLOCK_S).min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs())).unwrap_or(t)
    };
    let len = music.1 - music.0;
    let candidates: Vec<usize> = (SECTION_SPAN..=n - 2).filter(|m| m % step == 0).collect();
    let intro = candidates
        .iter()
        .take_while(|&&m| units[m] <= music.0 + (0.4 * len).min(90.0) && m + SECTION_SPAN <= n)
        .find(|&&m| {
            let (nov, fuller) = change(m);
            nov >= novelty_min && fuller > -FULLER_EPS
        })
        .map(|&m| to_beat(units[m]));
    let outro_from = (music.0 + 0.5 * len).max(music.1 - 90.0);
    let outro = candidates
        .iter()
        .rev()
        .take_while(|&&m| units[m] >= outro_from)
        .find(|&&m| {
            let (nov, fuller) = change(m);
            nov >= novelty_min && fuller < FULLER_EPS
        })
        .map(|&m| to_beat(units[m]));
    (intro, outro)
}

/// The song's bar lines: from the first downbeat of the beat (or of the music, when the beat starts less than two
/// bars in) to the end of the music, and whether the opening before them is beatless. None for fewer than 16 bars.
struct Bars {
    start: f64,
    bar: f64,
    beatless: bool,
    /// Bar starts; the last one is the end of the last whole bar.
    units: Vec<f64>,
}

fn bar_lines(t: &Tempo, db: &Downbeat, music: (f64, f64)) -> Option<Bars> {
    // No beat, no bars (a bar of no length would make endless ones).
    if !(t.period_s > 0.0) {
        return None;
    }
    let bpb = if db.beats_per_bar == 3 { 3 } else { 4 };
    let bar = bpb as f64 * t.period_s;
    // The first downbeat at or just before the beat starts: the tracked beats leave out a beatless opening.
    let beat_from = t.beats.first().copied().unwrap_or(music.0).max(music.0);
    let mut n0 = (t.grid_pos(beat_from) - 0.5).ceil() as i64;
    while n0.rem_euclid(bpb) != db.phase as i64 {
        n0 += 1;
    }
    let mut start = t.offset_s + n0 as f64 * t.period_s;
    // Less than two bars before it is a pickup, not an intro: count from the music's own first downbeat.
    let beatless = start - music.0 >= 2.0 * bar;
    if !beatless {
        while start - bar >= music.0 - 0.5 * t.period_s {
            start -= bar;
        }
    }
    let bars = ((music.1 - start) / bar).floor() as i64;
    if bars < 16 {
        return None;
    }
    Some(Bars { start, bar, beatless, units: (0..=bars).map(|m| start + m as f64 * bar).collect() })
}

/// How far into the song a drop may be and still be entered on: past this the intro would be skipped by more than
/// any mix can cover, or the "drop" is a chorus.
const DROP_REACH_S: f64 = 75.0;
/// A drop reaches the body of the song - within this much of its median bar in level, low end and tonal energy -
/// from bars that lacked at least this much of one of them.
const DROP_BODY_DB: [f64; 3] = [2.0, 3.0, 3.0];
const DROP_LACK_DB: [f64; 3] = [3.0, 3.0, 3.0];

/// Where the arrangement arrives: the first four-bar line in the opening part of the song where the four bars
/// after it sound like the body of the song - level, low end and chords all there - and the bars before did not
/// (a drum intro has the level but not the chords, a pad has the chords but not the low end). A beatless opening
/// that gives way to the full band is a drop at the first downbeat. None for a song that starts full, or without
/// a usable grid.
pub fn drop_point(t: &Tempo, db: &Downbeat, f: &Features, music: (f64, f64)) -> Option<Drop> {
    let b = bar_lines(t, db, music)?;
    let n = b.units.len() - 1;
    let v: Vec<[f64; 6]> = b.units.windows(2).map(|w| bar_vector(f, w[0], w[1])).collect();
    let median = |d: usize| {
        let mut x: Vec<f64> = v.iter().map(|r| r[d]).collect();
        x.sort_by(|a, b| a.total_cmp(b));
        x[x.len() / 2]
    };
    let body = [median(0), median(1), median(2)];
    let mean = |rows: &[[f64; 6]]| -> [f64; 3] {
        let k = rows.len().max(1) as f64;
        [0, 1, 2].map(|d| rows.iter().map(|r| r[d]).sum::<f64>() / k)
    };
    let arrives = |after: [f64; 3]| (0..3).all(|d| after[d] >= body[d] - DROP_BODY_DB[d]);
    let lacked = |before: [f64; 3]| (0..3).any(|d| before[d] < body[d] - DROP_LACK_DB[d]);
    let reach = music.0 + (0.4 * (music.1 - music.0)).min(DROP_REACH_S);
    if b.beatless && arrives(mean(&v[..SECTION_SPAN.min(n)])) {
        let opening = bar_vector(f, music.0, b.start);
        if lacked([opening[0], opening[1], opening[2]]) {
            return Some(Drop { at: b.start, runup_tonal_db: (opening[2] - body[2]) as f32 });
        }
    }
    (SECTION_SPAN..=n.saturating_sub(SECTION_SPAN))
        .step_by(SECTION_SPAN)
        .take_while(|&m| b.units[m] <= reach)
        .find(|&m| arrives(mean(&v[m..m + SECTION_SPAN])) && lacked(mean(&v[m - SECTION_SPAN..m])))
        .map(|m| {
            // The most chordal four bars of the run-up, a beatless opening before the bars included: one pad
            // phrase is enough for a key to clash.
            let opening = if b.beatless { bar_vector(f, music.0, b.start)[2] } else { f64::MIN };
            let most = (0..m).step_by(SECTION_SPAN).map(|i| mean(&v[i..(i + SECTION_SPAN).min(m)])[2]).fold(opening, f64::max);
            Drop { at: b.units[m], runup_tonal_db: (most - body[2]) as f32 }
        })
}

/// A drop, and how much less chord (tonal) energy than the body of the song the most chordal four bars of the run-up
/// to it carry, dB: a drum intro reads far below, a pad or a sung intro near it.
#[derive(Clone, Copy, Debug)]
pub struct Drop {
    pub at: f64,
    pub runup_tonal_db: f32,
}

/// A closing breakdown: the level falls this far from the four bars before...
const BREAK_DB: f64 = 6.0;
/// ...no bar after it comes back within this much...
const BREAK_STAY_DB: f64 = 3.0;
/// ...and the beat goes with it: the low end falls this far, or the onsets thin by this much (natural log).
const BREAK_LOW_DB: f64 = 6.0;
const BREAK_ONSET: f64 = 0.5;
/// Only a short ending counts; a longer quiet stretch is a part of the song, not an ending to leave.
pub const BREAK_MAX_S: f64 = 24.0;

/// Where a closing breakdown begins, before `end` (the end of the music, or of the music before a hidden track's
/// gap): the earliest bar line in the last `BREAK_MAX_S` where the level falls by `BREAK_DB` from the four bars
/// before, the beat thins with it (low end or onsets), and no bar after comes back. A pad or piano coda after the
/// last chorus, a breakdown the song never returns from, a fade-out. Without a usable grid, 2 s blocks and the
/// nearest tracked beat. None when the song keeps its energy to the end.
pub fn breakdown(t: &Tempo, db: &Downbeat, f: &Features, music: (f64, f64), end: f64, grid_ok: bool) -> Option<f64> {
    let (units, snap): (Vec<f64>, &[f64]) = match bar_lines(t, db, (music.0, end)).filter(|_| grid_ok && t.period_s > 0.0) {
        Some(b) => (b.units, &[]),
        None => ((0..).map(|k| music.0 + BLOCK_S * k as f64).take_while(|u| *u <= end).collect(), &t.beats),
    };
    let n = units.len().checked_sub(1)?;
    if n < SECTION_SPAN + 1 {
        return None;
    }
    let v: Vec<[f64; 6]> = units.windows(2).map(|w| bar_vector(f, w[0], w[1])).collect();
    let mean = |rows: &[[f64; 6]]| -> [f64; 6] {
        let k = rows.len().max(1) as f64;
        [0, 1, 2, 3, 4, 5].map(|d| rows.iter().map(|r| r[d]).sum::<f64>() / k)
    };
    let m = (SECTION_SPAN..n).filter(|&m| end - units[m] <= BREAK_MAX_S).find(|&m| {
        let (before, after) = (mean(&v[m - SECTION_SPAN..m]), mean(&v[m..n]));
        after[0] <= before[0] - BREAK_DB
            && (after[1] <= before[1] - BREAK_LOW_DB || after[3] <= before[3] - BREAK_ONSET)
            && v[m..n].iter().all(|r| r[0] <= before[0] - BREAK_STAY_DB)
    })?;
    let at = units[m];
    Some(snap.iter().copied().filter(|b| (b - at).abs() <= BLOCK_S).min_by(|a, b| (a - at).abs().total_cmp(&(b - at).abs())).unwrap_or(at))
}

/// On the bar grid: bars counted from the first downbeat of the beat (a beatless opening is the intro, ending
/// where the beat starts), section changes on bar lines a multiple of 4 bars on.
fn phrase_cues(t: &Tempo, db: &Downbeat, f: &Features, music: (f64, f64)) -> Option<(f64, f64)> {
    let Bars { start, bar, beatless, units } = bar_lines(t, db, music)?;
    let bars = units.len() as i64 - 1;
    let at = |m: i64| start + m as f64 * bar;
    let (intro, outro) = section_changes(f, &units, 4, &[], music);
    let intro = if beatless { start } else { intro.unwrap_or(music.0) };
    // A steady ending: the last phrase boundary at least 16 bars before the end.
    let outro = outro.unwrap_or_else(|| (1..).map(|k| 8 * k).take_while(|&m| m + 16 <= bars).last().map_or(music.0, at));
    Some((intro.max(music.0), outro.clamp(intro.max(music.0), music.1)))
}

/// Temperley's (Kostka-Payne) major profile and Krumhansl-Kessler's minor one. Temperley's major weighs the leading
/// tone up and the fifth down, which keeps a chroma full of third partials (every note also sounds its fifth) from
/// reading as the dominant key. His minor is the harmonic minor, though - leading tone 4, flat seventh 1.5 - and
/// pop and dance music in a minor key is mostly Aeolian, the flat seventh and rarely the leading tone: on the
/// synthetic songs of `eval.rs` it read three of five natural-minor songs as their relative major. Krumhansl and
/// Kessler's minor puts the flat seventh above the leading tone. Together: 13 of 16 keys there (11 before), every
/// miss a Camelot neighbour, and all four bare-triad progressions of `keys_of_simple_progressions`, where
/// Krumhansl-Kessler's major on its own hears the dominant.
const MAJOR: [f64; 12] = [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0];
const MINOR: [f64; 12] = [6.33, 2.68, 3.52, 5.38, 2.60, 3.53, 2.54, 4.75, 3.98, 2.69, 3.34, 3.17];

fn pearson(a: &[f64; 12], b: &[f64; 12], shift: usize) -> f64 {
    let ma = a.iter().sum::<f64>() / 12.0;
    let mb = b.iter().sum::<f64>() / 12.0;
    let (mut num, mut da, mut dbb) = (0.0, 0.0, 0.0);
    for i in 0..12 {
        let x = a[(i + shift) % 12] - ma;
        let y = b[i] - mb;
        num += x * y;
        da += x * x;
        dbb += y * y;
    }
    if da <= 0.0 || dbb <= 0.0 {
        0.0
    } else {
        num / (da * dbb).sqrt()
    }
}

/// Camelot code of a key: 1..12 minor (A), 13..24 major (B).
pub fn camelot(tonic: usize, minor: bool) -> i32 {
    let major_pc = if minor { (tonic + 3) % 12 } else { tonic % 12 };
    let num = (7 * major_pc + 7) % 12 + 1;
    num as i32 + if minor { 0 } else { 12 }
}

/// The tuning, semitones from A = 440 Hz (-0.5..0.5), from the peaks' circular mean.
pub fn tuning(f: &Features) -> f64 {
    let (c, s) = f.tuning_cs;
    if c.hypot(s) <= 1e-9 {
        0.0
    } else {
        s.atan2(c) / (2.0 * std::f64::consts::PI)
    }
}

/// The song's pitch-class profile with its own tuning taken out: every 10-cent slot goes to the pitch class whose
/// tuned centre it is nearest, weighted down towards the half-semitone between two. A band 40 cents sharp
/// otherwise reads a semitone high, or smeared over two keys.
pub fn tuned_profile(f: &Features) -> [f64; 12] {
    let tune = tuning(f);
    let slots = f.pitch.len().max(1) as f64;
    let mut c = [0f64; 12];
    for (s, v) in f.pitch.iter().enumerate() {
        let x = s as f64 * 12.0 / slots - tune;
        let d = x - x.round();
        let w = (1.0 - 2.0 * d.abs()).max(0.0);
        c[(x.round() as i64).rem_euclid(12) as usize] += w * v;
    }
    c
}

/// `(camelot code, confidence)` of a sequence of chroma frames; `(0, 0)` without tonal content.
pub fn key(chroma: &[[f32; 12]]) -> (i32, f32) {
    let mut c = [0f64; 12];
    for frame in chroma {
        for (a, v) in c.iter_mut().zip(frame) {
            *a += *v as f64;
        }
    }
    key_of(c)
}

/// `(camelot code, confidence)` of a pitch-class profile; `(0, 0)` without tonal content.
pub fn key_of(c: [f64; 12]) -> (i32, f32) {
    let mean = c.iter().sum::<f64>() / 12.0;
    if mean <= 1e-9 {
        return (0, 0.0);
    }
    let contrast = (c.iter().map(|v| (v - mean).powi(2)).sum::<f64>() / 12.0).sqrt() / mean;
    let mut scores: Vec<(f64, i32)> = Vec::with_capacity(24);
    for tonic in 0..12 {
        scores.push((pearson(&c, &MAJOR, tonic), camelot(tonic, false)));
        scores.push((pearson(&c, &MINOR, tonic), camelot(tonic, true)));
    }
    scores.sort_by(|a, b| b.0.total_cmp(&a.0));
    let (best, code) = scores[0];
    let conf = ((best - 0.5) / 0.35).clamp(0.0, 1.0) * ((contrast - 0.1) / 0.3).clamp(0.0, 1.0);
    (code, conf as f32)
}

/// "8B", "11A"; empty for 0.
pub fn camelot_name(code: i32) -> String {
    match code {
        1..=12 => format!("{code}A"),
        13..=24 => format!("{}B", code - 12),
        _ => String::new(),
    }
}

/// Steps on the Camelot wheel: 0 same key, 1 neighbour or relative major/minor. -1 when either key is unknown.
pub fn key_distance(a: i32, b: i32) -> i32 {
    if !(1..=24).contains(&a) || !(1..=24).contains(&b) {
        return -1;
    }
    let (na, nb) = ((a - 1) % 12, (b - 1) % 12);
    let d = (na - nb).rem_euclid(12);
    d.min(12 - d) + ((a > 12) != (b > 12)) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn camelot_codes_match_the_wheel() {
        assert_eq!(camelot_name(camelot(0, false)), "8B"); // C major
        assert_eq!(camelot_name(camelot(9, true)), "8A"); // A minor
        assert_eq!(camelot_name(camelot(7, false)), "9B"); // G major
        assert_eq!(camelot_name(camelot(11, false)), "1B"); // B major
        assert_eq!(camelot_name(camelot(0, true)), "5A"); // C minor
        assert_eq!(camelot_name(camelot(1, true)), "12A"); // C# minor
        assert_eq!(camelot_name(0), "");
        assert_eq!(key_distance(camelot(0, false), camelot(9, true)), 1, "relative minor");
        assert_eq!(key_distance(camelot(0, false), camelot(7, false)), 1, "a fifth up");
        assert_eq!(key_distance(camelot(0, false), camelot(6, false)), 6, "the tritone is the far side");
        assert_eq!(key_distance(camelot(0, false), camelot(0, false)), 0);
        assert_eq!(key_distance(0, 5), -1);
    }

    #[test]
    fn key_profiles_recognise_their_own_shape() {
        let rot = |p: &[f64; 12], t: usize| -> [f32; 12] {
            let mut c = [0f32; 12];
            for i in 0..12 {
                c[(i + t) % 12] = p[i] as f32;
            }
            c
        };
        assert_eq!(key(&[rot(&MAJOR, 2)]).0, camelot(2, false));
        assert_eq!(key(&[rot(&MINOR, 4)]).0, camelot(4, true));
        // A flat chroma correlates with nothing: whichever key sorts first, it must not be trusted.
        assert_eq!(key(&[[1.0; 12]]).1, 0.0);
        assert_eq!(key(&[]), (0, 0.0));
    }
}
