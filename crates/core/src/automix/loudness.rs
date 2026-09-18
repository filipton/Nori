//! Level measurements at the native rate: BS.1770 K-weighted loudness (integrated, gated), plain RMS for silence
//! trimming, and the MixRamp points. Everything is accumulated in 100 ms blocks while the audio streams past, so
//! the whole track costs two biquads per sample and one float per block.

/// Block length for every level measurement.
pub const BLOCK_MS: i64 = 100;
/// Silence trim threshold, dBFS RMS.
pub const SILENCE_DB: f64 = -55.0;
/// MixRamp threshold relative to integrated loudness, dB.
pub const MIXRAMP_DB: f64 = -17.0;

#[derive(Clone, Copy, Default)]
struct Biquad {
    b: [f64; 3],
    a: [f64; 2],
    s: [f64; 2],
}

impl Biquad {
    #[inline]
    fn run(&mut self, x: f64) -> f64 {
        let y = self.b[0] * x + self.s[0];
        self.s[0] = self.b[1] * x - self.a[0] * y + self.s[1];
        self.s[1] = self.b[2] * x - self.a[1] * y;
        y
    }
}

/// The two BS.1770 K-weighting stages for any sample rate (the libebur128 derivation).
fn k_weighting(rate: f64) -> [Biquad; 2] {
    use std::f64::consts::PI;
    let (f0, g, q) = (1681.974450955533, 3.999843853973347, 0.7071752369554196);
    let k = (PI * f0 / rate).tan();
    let vh = 10f64.powf(g / 20.0);
    let vb = vh.powf(0.4996667741545416);
    let a0 = 1.0 + k / q + k * k;
    let shelf = Biquad {
        b: [(vh + vb * k / q + k * k) / a0, 2.0 * (k * k - vh) / a0, (vh - vb * k / q + k * k) / a0],
        a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0],
        s: [0.0; 2],
    };
    let (f0, q) = (38.13547087602444, 0.5003270373238773);
    let k = (PI * f0 / rate).tan();
    let a0 = 1.0 + k / q + k * k;
    let hp = Biquad { b: [1.0, -2.0, 1.0], a: [2.0 * (k * k - 1.0) / a0, (1.0 - k / q + k * k) / a0], s: [0.0; 2] };
    [shelf, hp]
}

/// Streaming 100 ms block meter.
pub struct Meter {
    k: [Biquad; 2],
    block: usize,
    n: usize,
    acc_k: f64,
    acc_raw: f64,
    /// Mean square per block, K-weighted.
    pub blocks_k: Vec<f32>,
    /// Mean square per block, unweighted.
    pub blocks_raw: Vec<f32>,
}

impl Meter {
    pub fn new(rate: f64, expected_blocks: usize) -> Self {
        Meter {
            k: k_weighting(rate),
            block: ((rate * BLOCK_MS as f64 / 1000.0).round() as usize).max(1),
            n: 0,
            acc_k: 0.0,
            acc_raw: 0.0,
            blocks_k: Vec::with_capacity(expected_blocks),
            blocks_raw: Vec::with_capacity(expected_blocks),
        }
    }

    #[inline]
    pub fn push(&mut self, x: f32) {
        let x = x as f64;
        let s = self.k[0].run(x);
        let y = self.k[1].run(s);
        self.acc_k += y * y;
        self.acc_raw += x * x;
        self.n += 1;
        if self.n == self.block {
            self.flush_block();
        }
    }

    fn flush_block(&mut self) {
        if self.n == 0 {
            return;
        }
        self.blocks_k.push((self.acc_k / self.n as f64) as f32);
        self.blocks_raw.push((self.acc_raw / self.n as f64) as f32);
        (self.acc_k, self.acc_raw, self.n) = (0.0, 0.0, 0);
    }

    /// Closes a partial last block when it is at least a quarter full; a shorter one would read as a false fade.
    pub fn finish(&mut self) {
        if self.n * 4 >= self.block {
            self.flush_block();
        }
        (self.acc_k, self.acc_raw, self.n) = (0.0, 0.0, 0);
    }
}

fn lufs_of(mean_square: f64) -> f64 {
    if mean_square <= 1e-12 {
        -120.0
    } else {
        -0.691 + 10.0 * mean_square.log10()
    }
}

pub fn db(mean_square: f64) -> f64 {
    10.0 * mean_square.max(1e-12).log10()
}

/// Momentary (400 ms) mean squares, one per 100 ms step: window `j` covers blocks `j..j+4`.
fn momentary(blocks_k: &[f32]) -> impl Iterator<Item = f64> + '_ {
    blocks_k.windows(4.min(blocks_k.len()).max(1)).map(|w| w.iter().map(|v| *v as f64).sum::<f64>() / w.len() as f64)
}

/// Integrated loudness, BS.1770-4 gating (absolute -70 LUFS, relative -10 LU) over 400 ms windows with 75 % overlap.
pub fn integrated(blocks_k: &[f32]) -> f64 {
    let gated: Vec<f64> = momentary(blocks_k).filter(|ms| lufs_of(*ms) > -70.0).collect();
    if gated.is_empty() {
        return -70.0;
    }
    let rel = lufs_of(gated.iter().sum::<f64>() / gated.len() as f64) - 10.0;
    let (sum, n) = gated.iter().filter(|ms| lufs_of(**ms) > rel).fold((0.0, 0usize), |(s, n), v| (s + v, n + 1));
    if n == 0 {
        -70.0
    } else {
        lufs_of(sum / n as f64).max(-70.0)
    }
}

/// First and last audible moment, ms: the start of the first block above `SILENCE_DB` and the end of the last one.
/// `(0, 0)` when the whole track is silent.
pub fn silence_trim(blocks_raw: &[f32]) -> (i64, i64) {
    let loud = |v: &f32| db(*v as f64) > SILENCE_DB;
    match (blocks_raw.iter().position(loud), blocks_raw.iter().rposition(loud)) {
        (Some(a), Some(b)) => (a as i64 * BLOCK_MS, (b as i64 + 1) * BLOCK_MS),
        _ => (0, 0),
    }
}

/// MixRamp points, ms: the centre of the first 400 ms window at or above `lufs + MIXRAMP_DB`, and the centre of the
/// last one. `None` for silence.
pub fn mixramp(blocks_k: &[f32], lufs: f64) -> Option<(i64, i64)> {
    let thresh = lufs + MIXRAMP_DB;
    let (mut first, mut last) = (None, None);
    for (j, ms) in momentary(blocks_k).enumerate() {
        if lufs_of(ms) >= thresh {
            first.get_or_insert(j);
            last = Some(j);
        }
    }
    let centre = |j: usize| j as i64 * BLOCK_MS + 2 * BLOCK_MS;
    Some((centre(first?), centre(last?)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meter(x: &[f32], rate: f64) -> Meter {
        let mut m = Meter::new(rate, 0);
        x.iter().for_each(|v| m.push(*v));
        m.finish();
        m
    }

    fn sine(freq: f64, amp: f64, secs: f64, rate: f64) -> Vec<f32> {
        (0..(secs * rate) as usize).map(|i| (amp * (2.0 * std::f64::consts::PI * freq * i as f64 / rate).sin()) as f32).collect()
    }

    #[test]
    fn a_full_scale_1k_sine_reads_about_minus_3_lufs() {
        // BS.1770: a 0 dBFS 997 Hz sine in one channel reads -3.01 LUFS.
        for rate in [44100.0, 48000.0] {
            let m = meter(&sine(997.0, 1.0, 5.0, rate), rate);
            let l = integrated(&m.blocks_k);
            assert!((l + 3.01).abs() < 0.1, "{rate}: {l}");
        }
        let m = meter(&sine(997.0, 0.1, 5.0, 44100.0), 44100.0);
        assert!((integrated(&m.blocks_k) + 23.01).abs() < 0.1);
    }

    #[test]
    fn silence_is_minus_70_and_has_no_ramp() {
        let m = meter(&vec![0.0; 44100 * 3], 44100.0);
        assert_eq!(integrated(&m.blocks_k), -70.0);
        assert_eq!(silence_trim(&m.blocks_raw), (0, 0));
        assert_eq!(mixramp(&m.blocks_k, -70.0), None);
    }

    #[test]
    fn trims_and_ramps_find_the_edges() {
        let rate = 44100.0;
        let mut x = vec![0f32; (rate * 2.0) as usize]; // 2 s of silence
        let fade_in: Vec<f32> = sine(440.0, 0.5, 4.0, rate).iter().enumerate().map(|(i, v)| v * (i as f32 / (rate as f32 * 4.0))).collect();
        x.extend(fade_in); // a 4 s linear fade-in
        x.extend(sine(440.0, 0.5, 20.0, rate));
        x.extend(vec![0f32; (rate * 3.0) as usize]);
        let m = meter(&x, rate);
        let (start, end) = silence_trim(&m.blocks_raw);
        // A linear ramp of a -9 dBFS RMS tone crosses -55 dB after 4 s * 10^(-46/20) = 20 ms.
        assert!((2000..=2100).contains(&start), "start {start}");
        assert_eq!(end, 26000);
        let lufs = integrated(&m.blocks_k);
        let (r_in, r_out) = mixramp(&m.blocks_k, lufs).unwrap();
        // -17 dB is 0.14 in amplitude: 0.56 s into the ramp, the window centre lands a little after.
        assert!((2400..=3000).contains(&r_in), "ramp in {r_in}");
        assert!((25700..=26100).contains(&r_out), "ramp out {r_out}");
    }
}
