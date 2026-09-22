//! Tempo and beats from the onset-strength curve: autocorrelation weighted by a log-normal prior around 120 BPM,
//! then Ellis (2007) dynamic-programming beat tracking, then a least-squares constant grid through the beats.
//! The grid fit is what gives the precise BPM (the autocorrelation lag is only good to about 2 %) and the
//! stability measure (residual jitter against the grid, relative to the beat).

const MIN_BPM: f64 = 40.0;
const MAX_BPM: f64 = 240.0;
const PRIOR_BPM: f64 = 120.0;
/// Width of the log-normal prior, octaves.
const PRIOR_OCTAVES: f64 = 1.0;
/// Autocorrelation taper, seconds: the same bias towards short lags that averaging 8 s local autocorrelations gives.
const AC_WINDOW_S: f64 = 8.0;
/// Ellis / librosa tightness: how hard the DP holds to the period.
const TIGHTNESS: f64 = 100.0;

#[derive(Debug, Clone, Default)]
pub struct Tempo {
    /// Final tempo, from the grid fit when the fit is good. 0 when nothing periodic was found.
    pub bpm: f64,
    /// The autocorrelation peak, before the grid fit.
    pub raw_bpm: f64,
    /// The stronger of the half / double alternatives to `raw_bpm`, and its score relative to the winner (0..1).
    pub alt_bpm: f64,
    pub alt_score: f64,
    pub confidence: f32,
    /// First grid beat, seconds, 0 <= offset < period.
    pub offset_s: f64,
    pub stability: f32,
    /// Tracked beat times, seconds.
    pub beats: Vec<f64>,
    /// Beat period of the grid, seconds.
    pub period_s: f64,
}

impl Tempo {
    /// Grid index of time `t` (fractional).
    pub fn grid_pos(&self, t: f64) -> f64 {
        (t - self.offset_s) / self.period_s
    }
}

/// Folds `bpm` by factors of two to the octave closest to `reference`.
pub fn fold(reference: f64, bpm: f64) -> f64 {
    if !(reference > 0.0 && bpm > 0.0) || !reference.is_finite() || !bpm.is_finite() {
        return bpm;
    }
    let k = (reference / bpm).log2().round();
    bpm * 2f64.powf(k)
}

/// Speed the incoming track must play at to lock to the outgoing one, after folding ×2/×½: `out / fold(out, in)`.
/// 1.0 when either tempo is unknown.
pub fn match_ratio(out_bpm: f64, in_bpm: f64) -> f64 {
    let f = fold(out_bpm, in_bpm);
    if out_bpm > 0.0 && f > 0.0 && out_bpm.is_finite() && f.is_finite() {
        out_bpm / f
    } else {
        1.0
    }
}

fn median(v: &mut [f64]) -> f64 {
    if v.is_empty() {
        return 0.0;
    }
    v.sort_unstable_by(|a, b| a.total_cmp(b));
    v[v.len() / 2]
}

/// Vertex offset of the parabola through three points, in (-0.5, 0.5).
fn parabolic(a: f64, b: f64, c: f64) -> f64 {
    let d = a - 2.0 * b + c;
    if d.abs() < 1e-12 {
        0.0
    } else {
        (0.5 * (a - c) / d).clamp(-0.5, 0.5)
    }
}

/// Moving average with a centred window of `w` frames, O(n).
fn moving_mean(x: &[f32], w: usize) -> Vec<f32> {
    let n = x.len();
    let h = w / 2;
    let mut prefix = Vec::with_capacity(n + 1);
    prefix.push(0f64);
    for v in x {
        prefix.push(prefix.last().unwrap() + *v as f64);
    }
    (0..n)
        .map(|i| {
            let (a, b) = (i.saturating_sub(h), (i + h + 1).min(n));
            ((prefix[b] - prefix[a]) / (b - a) as f64) as f32
        })
        .collect()
}

pub fn estimate(onset: &[f32], fps: f64, t0: f64) -> Tempo {
    let n = onset.len();
    let tau_lo = ((fps * 60.0 / MAX_BPM).floor() as usize).max(1);
    let tau_hi = (fps * 60.0 / MIN_BPM).ceil() as usize;
    if n < tau_hi * 4 {
        return Tempo::default();
    }

    // Detrended, unit-variance envelope for the autocorrelation.
    let trend = moving_mean(onset, (fps * 1.0) as usize | 1);
    let mut env: Vec<f32> = onset.iter().zip(&trend).map(|(o, t)| o - t).collect();
    let var = env.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64;
    if var < 1e-12 {
        return Tempo::default();
    }
    let inv = (1.0 / var.sqrt()) as f32;
    env.iter_mut().for_each(|v| *v *= inv);

    let mut ac = vec![0f64; tau_hi + 3];
    for (tau, a) in ac.iter_mut().enumerate().take(tau_hi + 3).skip(tau_lo.saturating_sub(1)) {
        if tau >= n {
            break;
        }
        let s: f32 = env[..n - tau].iter().zip(&env[tau..]).map(|(x, y)| x * y).sum();
        *a = s as f64 / n as f64;
    }
    let ac0 = env.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64;
    let window = AC_WINDOW_S * fps;
    let weight = |tau: f64| {
        let bpm = 60.0 * fps / tau;
        let z = (bpm / PRIOR_BPM).log2() / PRIOR_OCTAVES;
        (1.0 - tau / window).max(0.0) * (-0.5 * z * z).exp()
    };
    let score: Vec<f64> = (0..ac.len()).map(|t| if t < tau_lo || t > tau_hi { 0.0 } else { ac[t].max(0.0) * weight(t as f64) }).collect();
    let best = (tau_lo..=tau_hi).max_by(|a, b| score[*a].total_cmp(&score[*b])).unwrap_or(tau_lo);
    if score[best] <= 0.0 {
        return Tempo::default();
    }
    let tau = best as f64 + parabolic(score[best - 1], score[best], score[best + 1]);
    let raw_bpm = 60.0 * fps / tau;
    let pulse = ac[best] / ac0.max(1e-12);

    // Octave alternatives, read off the same curve by linear interpolation.
    let at = |t: f64| -> f64 {
        if t < tau_lo as f64 || t > tau_hi as f64 {
            return 0.0;
        }
        let i = t.floor() as usize;
        let f = t - i as f64;
        let hi = (i + 1).min(ac.len() - 1);
        (ac[i] * (1.0 - f) + ac[hi] * f).max(0.0) * weight(t)
    };
    let (half, double) = (at(tau * 2.0), at(tau / 2.0));
    let (alt_bpm, alt_raw) = if half >= double { (raw_bpm / 2.0, half) } else { (raw_bpm * 2.0, double) };

    // Beat tracking on the plain (non-negative) envelope.
    let std = (onset.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / n as f64).sqrt().max(1e-9);
    let norm: Vec<f32> = onset.iter().map(|v| (*v as f64 / std) as f32).collect();
    let local = gaussian_smooth(&norm, tau / 32.0);
    let frames = track(&local, tau);
    let beats: Vec<f64> = frames
        .iter()
        .map(|&b| {
            let d = if b > 0 && b + 1 < n { parabolic(local[b - 1] as f64, local[b] as f64, local[b + 1] as f64) } else { 0.0 };
            t0 + (b as f64 + d) / fps
        })
        .collect();

    let mut t = Tempo { raw_bpm, alt_bpm, alt_score: alt_raw / score[best], period_s: 60.0 / raw_bpm, ..Default::default() };
    if beats.len() < 8 {
        t.beats = beats;
        t.bpm = raw_bpm;
        return t;
    }

    let (a, b, rms) = fit_grid(&beats, 60.0 / raw_bpm);
    // The fit is trusted when it lands within 4 % of the autocorrelation (it cannot jump an octave).
    let period = if (b / (60.0 / raw_bpm) - 1.0).abs() < 0.04 { b } else { 60.0 / raw_bpm };
    t.period_s = period;
    t.bpm = 60.0 / period;
    t.offset_s = a.rem_euclid(period);
    t.stability = stability(&beats, period, rms);

    // Confidence: how periodic the envelope is, and how much stronger it is on the beats than overall.
    let music: Vec<f32> = local.iter().copied().filter(|v| *v > 0.05).collect();
    let mean_all = if music.is_empty() { 0.0 } else { music.iter().map(|v| *v as f64).sum::<f64>() / music.len() as f64 };
    let mean_beats = frames.iter().map(|&f| local[f] as f64).sum::<f64>() / frames.len() as f64;
    let align = if mean_all > 0.0 { mean_beats / mean_all } else { 0.0 };
    let pulse_c = ((pulse - 0.05) / 0.25).clamp(0.0, 1.0);
    let align_c = ((align - 1.3) / 1.2).clamp(0.0, 1.0);
    t.confidence = (0.5 * (pulse_c + align_c)) as f32;
    t.beats = beats;
    t
}

/// Gaussian smoothing, sigma in frames (at least half a frame).
fn gaussian_smooth(x: &[f32], sigma: f64) -> Vec<f32> {
    let sigma = sigma.max(0.5);
    let h = (sigma * 3.0).ceil() as isize;
    let k: Vec<f32> = (-h..=h).map(|i| (-0.5 * (i as f64 / sigma).powi(2)).exp() as f32).collect();
    let n = x.len() as isize;
    (0..n)
        .map(|i| {
            let mut s = 0f32;
            for (j, w) in k.iter().enumerate() {
                let p = i + j as isize - h;
                if p >= 0 && p < n {
                    s += x[p as usize] * w;
                }
            }
            s
        })
        .collect()
}

/// Ellis 2007: every frame's best score is its own onset strength plus the best predecessor's score minus a penalty
/// for straying from the period, `TIGHTNESS * ln(gap / period)²`. Backtracking from the last strong frame gives the
/// beats. Leading and trailing beats weaker than half the beats' RMS are trimmed (silence, fade-outs).
fn track(local: &[f32], period: f64) -> Vec<usize> {
    let n = local.len();
    let lo = ((period / 2.0).round() as usize).max(1);
    let hi = ((period * 2.0).round() as usize).max(lo + 1);
    let txwt: Vec<f64> = (lo..=hi).map(|d| -TIGHTNESS * (d as f64 / period).ln().powi(2)).collect();
    let thresh = 0.01 * local.iter().fold(0f32, |m, v| m.max(*v)) as f64;
    let mut cum = vec![0f64; n];
    let mut back = vec![usize::MAX; n];
    let mut started = false;
    for i in 0..n {
        let mut best = f64::NEG_INFINITY;
        let mut arg = usize::MAX;
        if i >= lo {
            let top = i - lo;
            let bottom = i.saturating_sub(hi);
            for p in bottom..=top {
                let s = cum[p] + txwt[i - p - lo];
                if s > best {
                    (best, arg) = (s, p);
                }
            }
        }
        let own = local[i] as f64;
        if !started && own < thresh {
            cum[i] = own;
        } else {
            cum[i] = own + if arg == usize::MAX { 0.0 } else { best };
            back[i] = if started { arg } else { usize::MAX };
            started = true;
        }
    }
    // The last beat: the last local maximum of the cumulative score above half the median of all local maxima.
    let maxima: Vec<usize> = (1..n.saturating_sub(1)).filter(|&i| cum[i] > cum[i - 1] && cum[i] >= cum[i + 1]).collect();
    if maxima.is_empty() {
        return Vec::new();
    }
    let mut vals: Vec<f64> = maxima.iter().map(|&i| cum[i]).collect();
    let med = median(&mut vals);
    let Some(&last) = maxima.iter().rev().find(|&&i| cum[i] >= 0.5 * med) else { return Vec::new() };
    let mut beats = vec![last];
    let mut i = last;
    while back[i] != usize::MAX && back[i] < i {
        i = back[i];
        beats.push(i);
    }
    beats.reverse();

    let rms = (beats.iter().map(|&b| (local[b] as f64).powi(2)).sum::<f64>() / beats.len() as f64).sqrt();
    let strong = |b: &usize| local[*b] as f64 >= 0.5 * rms;
    let first = beats.iter().position(strong).unwrap_or(0);
    let end = beats.iter().rposition(strong).map_or(beats.len(), |p| p + 1);
    beats[first..end].to_vec()
}

/// Least-squares line `t = a + b k` through the beats, where `k` counts periods (a dropped or doubled beat moves
/// `k` by the right amount instead of bending the line). One pass of outlier rejection. Returns (a, b, residual
/// spread in seconds).
/// Per-beat timing spread (median, ms) that still scores zero; 14 ms or less passes the planner's 0.6.
const JITTER_ZERO_MS: f64 = 35.0;
/// Tempo change between a stretch's first and second half that scores zero; 1.2 % or less passes.
const DRIFT_ZERO_PCT: f64 = 3.0;

/// Whether one grid can stand for these beats: the lower of how tightly they sit on it and how little
/// the tempo moves between the first half and the second.
///
/// It used to be the spread alone, against 5 % of a beat. Measured on real records that was wrong both
/// ways: a band played to within 6-14 ms scored nothing (and half of that at a doubled tempo, because
/// the allowance shrank with the beat), while what really spoils a beat-matched mix - the tempo moving
/// under it - was never looked at. Milliseconds are what the ear hears, so the spread is judged in them;
/// the drift is judged by fitting each half on its own.
fn stability(beats: &[f64], period: f64, rms: f64) -> f32 {
    let jitter = 1.0 - (rms / 1.4826 * 1000.0) / JITTER_ZERO_MS;
    let half = beats.len() / 2;
    let drift = if half >= 8 {
        let (_, p1, _) = fit_grid(&beats[..half], period);
        let (_, p2, _) = fit_grid(&beats[half..], period);
        if p1 > 0.0 && p2 > 0.0 { 1.0 - ((p2 / p1 - 1.0).abs() * 100.0) / DRIFT_ZERO_PCT } else { 0.0 }
    } else {
        1.0
    };
    jitter.min(drift).clamp(0.0, 1.0) as f32
}

pub fn fit_grid(beats: &[f64], period: f64) -> (f64, f64, f64) {
    let mut d: Vec<f64> = beats.windows(2).map(|w| w[1] - w[0]).collect();
    let p = {
        let m = median(&mut d);
        if m > 0.0 && (m / period - 1.0).abs() < 0.2 {
            m
        } else {
            period
        }
    };
    let mut k = Vec::with_capacity(beats.len());
    let mut idx = 0f64;
    k.push(0.0);
    for w in beats.windows(2) {
        idx += ((w[1] - w[0]) / p).round().max(1.0);
        k.push(idx);
    }
    let line = |use_: &dyn Fn(usize) -> bool| -> (f64, f64) {
        let (mut sn, mut sk, mut st, mut skk, mut skt) = (0.0, 0.0, 0.0, 0.0, 0.0);
        for (i, (&ki, &ti)) in k.iter().zip(beats).enumerate() {
            if use_(i) {
                sn += 1.0;
                sk += ki;
                st += ti;
                skk += ki * ki;
                skt += ki * ti;
            }
        }
        let den = sn * skk - sk * sk;
        if den.abs() < 1e-12 {
            return (beats[0], p);
        }
        let b = (sn * skt - sk * st) / den;
        ((st - b * sk) / sn, b)
    };
    let (a, b) = line(&|_| true);
    let res: Vec<f64> = k.iter().zip(beats).map(|(ki, ti)| ti - (a + b * ki)).collect();
    let mut abs: Vec<f64> = res.iter().map(|r| r.abs()).collect();
    let mad = median(&mut abs).max(0.002);
    let (a, b) = line(&|i| res[i].abs() <= 4.0 * mad);
    // Robust spread (1.4826 MAD ~ sigma for Gaussian jitter): a few misplaced beats do not make a steady track
    // look unsteady, a drifting tempo still does because most residuals grow.
    let mut abs: Vec<f64> = k.iter().zip(beats).map(|(ki, ti)| (ti - (a + b * ki)).abs()).collect();
    (a, b, 1.4826 * median(&mut abs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_compares_tempos_modulo_octaves() {
        assert!((fold(128.0, 64.5) - 129.0).abs() < 1e-9);
        assert!((fold(87.0, 174.0) - 87.0).abs() < 1e-9);
        assert!((fold(120.0, 123.0) - 123.0).abs() < 1e-9);
        assert_eq!(fold(0.0, 120.0), 120.0);
        assert!((match_ratio(128.0, 64.0) - 1.0).abs() < 1e-9);
        assert!((match_ratio(126.0, 120.0) - 1.05).abs() < 1e-9);
        assert!((match_ratio(87.0, 174.0) - 1.0).abs() < 1e-9);
        assert_eq!(match_ratio(0.0, 120.0), 1.0);
        assert_eq!(match_ratio(f64::NAN, 120.0), 1.0);
    }

    #[test]
    fn the_grid_fit_survives_a_dropped_beat() {
        let mut beats: Vec<f64> = (0..40).map(|i| 0.25 + i as f64 * 0.5).collect();
        beats.remove(17);
        let (a, b, rms) = fit_grid(&beats, 0.5);
        assert!((a - 0.25).abs() < 1e-9 && (b - 0.5).abs() < 1e-9 && rms < 1e-9, "{a} {b} {rms}");
    }

    #[test]
    fn a_flat_envelope_has_no_tempo() {
        let t = estimate(&vec![0.0; 5000], 86.0, 0.0);
        assert_eq!(t.bpm, 0.0);
        assert_eq!(t.confidence, 0.0);
        assert_eq!(estimate(&[1.0; 10], 86.0, 0.0).bpm, 0.0);
    }
}
