//! Bar- and phrase-level structure on top of the beat grid, and the key.
//! Downbeats: "downbeat voting" over the 4 possible bar phases, adding kick-band onset strength and chroma change
//! (chords tend to change on the one). Phrases: bar boundaries at multiples of 8 bars from the first downbeat,
//! kept when the energy jumps there. Key: the track's chroma against Temperley's key profiles.

use super::analysis::Features;
use super::loudness;
use super::tempo::Tempo;

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Downbeat {
    pub phase: i32,
    pub confidence: f32,
}

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

/// `music` is the audible span, seconds.
pub fn downbeat(t: &Tempo, f: &Features, music: (f64, f64)) -> Downbeat {
    if t.period_s <= 0.0 || t.bpm <= 0.0 {
        return Downbeat::default();
    }
    let first = t.grid_pos(music.0).ceil() as i64;
    let last = t.grid_pos(music.1).floor() as i64 - 1;
    if last - first < 16 {
        return Downbeat::default();
    }
    let reach = ((t.period_s / 8.0) * f.fps).round().max(1.0) as isize;
    let mut bass = Vec::with_capacity((last - first + 1) as usize);
    let mut change = Vec::with_capacity(bass.capacity());
    let mut prev = chroma_between(f, t.offset_s + (first - 1) as f64 * t.period_s, t.offset_s + first as f64 * t.period_s);
    for n in first..=last {
        let tn = t.offset_s + n as f64 * t.period_s;
        let k = frame_of(f, tn);
        let lo = (k - reach).max(0) as usize;
        let hi = ((k + reach + 1).max(0) as usize).min(f.low_onset.len());
        bass.push(f.low_onset.get(lo..hi.max(lo)).unwrap_or(&[]).iter().fold(0f32, |m, v| m.max(*v)) as f64);
        let c = chroma_between(f, tn, tn + t.period_s);
        change.push(1.0 - c.iter().zip(&prev).map(|(a, b)| a * b).sum::<f64>());
        prev = c;
    }
    z(&mut bass);
    z(&mut change);
    let mut score = [0f64; 4];
    let mut count = [0usize; 4];
    for (i, n) in (first..=last).enumerate() {
        let p = n.rem_euclid(4) as usize;
        score[p] += bass[i] + change[i];
        count[p] += 1;
    }
    for p in 0..4 {
        score[p] /= count[p].max(1) as f64;
    }
    let mut order = [0usize, 1, 2, 3];
    order.sort_by(|a, b| score[*b].total_cmp(&score[*a]));
    let margin = score[order[0]] - score[order[1]];
    Downbeat { phase: order[0] as i32, confidence: ((margin - 0.15) / 0.6).clamp(0.0, 1.0) as f32 }
}

fn mean_db(v: &[f32]) -> f64 {
    if v.is_empty() {
        return -120.0;
    }
    loudness::db(v.iter().map(|x| *x as f64).sum::<f64>() / v.len() as f64)
}

/// Intro end and outro start, seconds. With a usable grid they sit on bar boundaries a multiple of 8 bars from the
/// first downbeat; otherwise they come from the 0.5 s energy envelope.
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
    (intro, outro.max(intro))
}

fn phrase_cues(t: &Tempo, db: &Downbeat, f: &Features, music: (f64, f64)) -> Option<(f64, f64)> {
    let bar = 4.0 * t.period_s;
    // First downbeat at or just before the music starts.
    let mut n0 = (t.grid_pos(music.0) - 0.5).ceil() as i64;
    while n0.rem_euclid(4) != db.phase as i64 {
        n0 += 1;
    }
    let start = t.offset_s + n0 as f64 * t.period_s;
    let bars = ((music.1 - start) / bar).floor() as i64;
    if bars < 16 {
        return None;
    }
    let at = |m: i64| start + m as f64 * bar;
    let slice = |v: &[f32], m0: i64, m1: i64| -> f64 {
        let a = frame_of(f, at(m0.max(0))).max(0) as usize;
        let b = (frame_of(f, at(m1.min(bars))).max(0) as usize).min(v.len());
        mean_db(v.get(a..b.max(a)).unwrap_or(&[]))
    };
    // Energy after minus energy before a boundary, 4 bars each side.
    let jump = |v: &[f32], m: i64| slice(v, m, m + 4) - slice(v, m - 4, m);
    let len = music.1 - music.0;

    let intro_limit = music.0 + (0.4 * len).min(90.0);
    let intro = (1..)
        .map(|k| 8 * k)
        .take_while(|&m| at(m) <= intro_limit && m + 4 <= bars)
        .find(|&m| jump(&f.power, m) >= 3.0 || jump(&f.low_power, m) >= 6.0)
        .map_or(music.0, at);

    let outro_from = (music.0 + 0.5 * len).max(music.1 - 90.0);
    let candidates: Vec<i64> = (1..).map(|k| 8 * k).take_while(|&m| m + 4 <= bars).filter(|&m| at(m) >= outro_from).collect();
    let outro = candidates
        .iter()
        .rev()
        .find(|&&m| -jump(&f.power, m) >= 3.0 || -jump(&f.low_power, m) >= 6.0)
        .copied()
        // A steady ending: the last phrase boundary at least 16 bars before the end.
        .or_else(|| (1..).map(|k| 8 * k).take_while(|&m| m + 16 <= bars).last())
        .map_or(music.0, at);
    Some((intro.max(music.0), outro.clamp(intro.max(music.0), music.1)))
}

/// Temperley's (Kostka-Payne) profiles. Against Krumhansl-Kessler they weigh the leading tone up and the fifth
/// down, which is what keeps a chroma full of third partials (every note also sounds its fifth) from reading as
/// the dominant key.
const MAJOR: [f64; 12] = [5.0, 2.0, 3.5, 2.0, 4.5, 4.0, 2.0, 4.5, 2.0, 3.5, 1.5, 4.0];
const MINOR: [f64; 12] = [5.0, 2.0, 3.5, 4.5, 2.0, 4.0, 2.0, 4.5, 3.5, 2.0, 1.5, 4.0];

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

/// `(camelot code, confidence)`; `(0, 0)` without tonal content.
pub fn key(chroma: &[[f32; 12]]) -> (i32, f32) {
    let mut c = [0f64; 12];
    for frame in chroma {
        for (a, v) in c.iter_mut().zip(frame) {
            *a += *v as f64;
        }
    }
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
