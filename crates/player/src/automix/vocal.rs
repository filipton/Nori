//! Where a voice sings in a song: a compact vocal activity curve, measured in the same pass as the rest of the
//! analysis from the FFT it already takes, so that synced lyrics can be checked against the audio (nori-lyrics
//! sync.rs).
//!
//! **What it measures.** A singing voice is a pitched sound in the voice band (300 Hz to 3 kHz) that never holds
//! still: vibrato, glides and a new vowel on every syllable move its harmonics from frame to frame. Drums are in
//! the band too, but they are noise, not peaks; pads and held chords are peaks, but they sit still. So each frame
//! the band's clear spectral peaks are found (a bin above its neighbours and five times the bins three away: a
//! Hann window's main lobe over its skirts), and what is added up is how far the log level moved since the frame
//! before in the three bins around each peak. On the synthetic songs of `eval.rs` this "pitched movement" tells
//! sung from unsung frames with an area under the ROC curve of 0.986 (`vocal_curve_eval`), where the band's
//! share of the power (the `vocal` curve AutoMix's gates read) gets 0.71 and the band's plain spectral flux 0.85.
//!
//! **On real songs** (nori-lyrics sync_tune.rs: 59 songs of a real library, the lines of lyrics the services
//! agree on taken as where the voice is) it tells much less: an AUC of about 0.7 in pop, rock and rap, and none
//! at all in metal (about 0.5): distorted guitars are pitched and restless too, and a solo reads as singing. What
//! still works is timing: the lines' starts fall where the curve rises, so offsets are found (within 250 ms for
//! 95 % of timings shifted by hand) even where the level itself says little. The band and the peak test were
//! chosen there: 300 Hz to 3 kHz and five times (from 250 Hz to 4 kHz and ten times) put the offsets' 90th
//! percentile error from 136 to 38 ms and told good timings from another song's better (AUC 0.88 to 0.94 on how
//! much better they fit where they are than slid far off). A lower bar still (three times) left too few songs
//! with a curve that moves; subtracting the movement of the bins that are no peak's did worse everywhere.
//!
//! **Cost.** Per analysis frame (86 a second) the band's power (about 125 bins) from the spectrum the analysis
//! has already taken, the peak test, and two square roots and a logarithm per bin around a peak; nothing
//! allocated. 15 ms for a four-minute song on the host, a twelfth of the analysis front end (`vocal_cost`). What
//! is kept is one number per [`CURVE_EVERY`] frames (about 17 a second), stored as one byte each: about a
//! kilobyte a minute. (A byte of 16 levels, or 11 frames a second, measured worse on `sync_eval`.)
//!
//! Mono only: the analyser is fed the downmix, so the voice's usual place in the middle of the stereo image is
//! not used.

use rustfft::num_complex::Complex32;

/// The voice band, Hz.
pub const LO_HZ: f64 = 300.0;
pub const HI_HZ: f64 = 3000.0;
/// Log compression of the band's amplitudes, as the onset curves have it: linear below -60 dB.
const GAMMA: f32 = 1000.0;
/// A peak is this many times the power of the bins three away from it.
const PEAK_OVER: f32 = 5.0;
/// Analysis frames per curve frame: about 17 a second, 58 ms each.
pub const CURVE_EVERY: usize = 5;
/// The stored form's version: a curve in another is measured again.
pub const CURVE_VERSION: u8 = 2;
/// Bytes of the stored form before the levels: the version, the frame rate and the time of frame 0.
const HEADER: usize = 9;
/// A curve frame's movement `m` is stored as `ln(1 + m) * LEVEL_SCALE`, clamped to a byte.
const LEVEL_SCALE: f32 = 40.0;

/// The per-frame side of the curve, fed each analysis frame's spectrum.
pub struct Tracker {
    lo: usize,
    hi: usize,
    amp_norm: f32,
    /// The band's power, with three bins of margin each side for the peak test.
    pow: Vec<f32>,
    /// The band's power the frame before.
    prev: Vec<f32>,
    acc: f32,
    n: usize,
    curve: Vec<f32>,
}

impl Tracker {
    /// For an FFT of `n` points at `sr` Hz whose |X| times `amp_norm` is a sine's amplitude; `frames` analysis
    /// frames expected (sizes the curve, so nothing grows while it plays).
    pub fn new(n: usize, sr: f64, amp_norm: f32, frames: usize) -> Self {
        let bin_hz = sr / n as f64;
        let lo = ((LO_HZ / bin_hz).ceil() as usize).max(4);
        let hi = ((HI_HZ.min(sr / 2.0 * 0.9) / bin_hz) as usize).min(n / 2 - 4).max(lo + 8);
        let w = hi - lo + 1;
        Tracker {
            lo,
            hi,
            amp_norm,
            pow: vec![0.0; w + 6],
            prev: vec![0.0; w],
            acc: 0.0,
            n: 0,
            curve: Vec::with_capacity(frames / CURVE_EVERY + 2),
        }
    }

    /// One analysis frame's spectrum (at least `n / 2` bins).
    pub fn frame(&mut self, spec: &[Complex32]) {
        let (lo, hi) = (self.lo, self.hi);
        for (p, c) in self.pow.iter_mut().zip(&spec[lo - 3..=hi + 3]) {
            *p = c.norm_sqr();
        }
        // Amplitudes only where they are read: the bins around a peak (and every bin for the next frame to
        // compare with, which is the last frame's `pow`, kept as it was).
        let w = self.prev.len();
        let g = GAMMA * self.amp_norm;
        let mut moved = 0f32;
        let mut done = 0usize;
        for j in 0..w {
            let k = j + 3;
            let p = self.pow[k];
            if p >= self.pow[k - 1] && p >= self.pow[k + 1] && p > PEAK_OVER * 0.5 * (self.pow[k - 3] + self.pow[k + 3]) {
                for i in j.saturating_sub(1).max(done)..(j + 2).min(w) {
                    let now = 1.0 + g * self.pow[i + 3].sqrt();
                    let before = 1.0 + g * self.prev[i].sqrt();
                    moved += (now / before).ln().abs();
                }
                done = (j + 2).min(w);
            }
        }
        self.prev.copy_from_slice(&self.pow[3..3 + w]);
        self.acc += moved;
        self.n += 1;
        if self.n == CURVE_EVERY {
            self.curve.push(self.acc / CURVE_EVERY as f32);
            (self.acc, self.n) = (0.0, 0);
        }
    }

    /// The curve so far, one value per [`CURVE_EVERY`] analysis frames; the tracker starts again.
    pub fn take(&mut self) -> Vec<f32> {
        let out = std::mem::take(&mut self.curve);
        self.reset();
        out
    }

    pub fn reset(&mut self) {
        self.prev.fill(0.0);
        self.curve.clear();
        (self.acc, self.n) = (0.0, 0);
    }
}

/// A song's vocal activity: one byte per frame, higher where a voice (or something as restless and pitched)
/// sounds. Frame `k` is at `t0 + k / fps` seconds.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct VocalCurve {
    pub fps: f32,
    pub t0: f32,
    pub level: Vec<u8>,
}

impl VocalCurve {
    /// From the raw curve of an analysis at `fps` analysis frames a second whose frame 0 is at `t0`.
    pub fn from_raw(raw: &[f32], fps: f64, t0: f64) -> Self {
        let e = CURVE_EVERY as f64;
        VocalCurve {
            fps: (fps / e) as f32,
            // A curve frame is the mean of its analysis frames: it sits at their middle.
            t0: (t0 + (e - 1.0) / 2.0 / fps) as f32,
            level: raw.iter().map(|m| ((1.0 + m.max(0.0)).ln() * LEVEL_SCALE).round().clamp(0.0, 255.0) as u8).collect(),
        }
    }

    /// Seconds covered.
    pub fn seconds(&self) -> f64 {
        if self.fps > 0.0 {
            self.level.len() as f64 / self.fps as f64
        } else {
            0.0
        }
    }

    /// The stored form: [`CURVE_VERSION`], the frame rate and frame 0's time (f32, little-endian), the levels.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(HEADER + self.level.len());
        out.push(CURVE_VERSION);
        out.extend_from_slice(&self.fps.to_le_bytes());
        out.extend_from_slice(&self.t0.to_le_bytes());
        out.extend_from_slice(&self.level);
        out
    }

    /// None for another version or a form that does not read.
    pub fn decode(b: &[u8]) -> Option<Self> {
        if b.len() < HEADER || b[0] != CURVE_VERSION {
            return None;
        }
        let fps = f32::from_le_bytes(b[1..5].try_into().ok()?);
        let t0 = f32::from_le_bytes(b[5..9].try_into().ok()?);
        (fps.is_finite() && fps > 0.0 && t0.is_finite()).then(|| VocalCurve { fps, t0, level: b[HEADER..].to_vec() })
    }
}

#[cfg(test)]
mod tests {
    use super::super::analysis::Analyzer;
    use super::super::eval::{corpus_all, Song, Style, FULL, SUNG};
    use super::*;

    fn curve_of(song: &Song) -> (VocalCurve, super::super::eval::Truth, f64) {
        let (x, truth) = song.render();
        let mut a = Analyzer::new(song.rate, 0);
        a.feed(&x);
        let secs = x.len() as f64 / song.rate as f64;
        (a.take_features().voice_curve(), truth, secs)
    }

    fn auc(pos: &[f32], neg: &[f32]) -> f64 {
        let mut all: Vec<(f32, bool)> = pos.iter().map(|v| (*v, true)).chain(neg.iter().map(|v| (*v, false))).collect();
        all.sort_by(|a, b| a.0.total_cmp(&b.0));
        let rank_sum: f64 = all.iter().enumerate().filter(|(_, (_, p))| *p).map(|(r, _)| (r + 1) as f64).sum();
        let (np, nn) = (pos.len() as f64, neg.len() as f64);
        (rank_sum - np * (np + 1.0) / 2.0) / (np * nn)
    }

    /// Sung frames against unsung ones, per sung song of the corpus, the curve smoothed over half a second as
    /// the sync check reads it.
    fn separation(song: &Song) -> f64 {
        let (c, truth, _) = curve_of(song);
        let r = (0.25 * c.fps) as usize;
        let (mut pos, mut neg) = (Vec::new(), Vec::new());
        for k in 0..c.level.len() {
            let t = c.t0 as f64 + k as f64 / c.fps as f64;
            if t < truth.music.0 + 1.0 || t > truth.music.1 - 1.0 {
                continue;
            }
            let (a, b) = (k.saturating_sub(r), (k + r + 1).min(c.level.len()));
            let v = c.level[a..b].iter().map(|v| *v as f32).sum::<f32>() / (b - a) as f32;
            if truth.sung_at(t) {
                pos.push(v)
            } else {
                neg.push(v)
            }
        }
        auc(&pos, &neg)
    }

    #[test]
    fn the_curve_rises_where_the_voice_sings() {
        let song = Song { sections: vec![(4, FULL), (8, SUNG), (4, FULL), (8, SUNG)], ..Song::new("sung", Style::Backbeat, 110.0, 2, false) };
        let a = separation(&song);
        assert!(a > 0.95, "sung told from unsung frames with an AUC of {a:.3}");
    }

    #[test]
    fn it_is_about_a_kilobyte_a_minute_and_reads_back() {
        let song = Song { sections: vec![(24, SUNG)], ..Song::new("sung", Style::House, 120.0, 2, false) };
        let (c, _, secs) = curve_of(&song);
        let per_min = c.level.len() as f64 / secs * 60.0;
        assert!((1000.0..1100.0).contains(&per_min), "{per_min} bytes a minute");
        assert!((c.fps - 17.2).abs() < 0.1, "{}", c.fps);
        assert_eq!(VocalCurve::decode(&c.encode()), Some(c.clone()));
        assert_eq!(VocalCurve::decode(&[2, 0, 0]), None);
        let mut other = c.encode();
        other[0] = CURVE_VERSION + 1;
        assert_eq!(VocalCurve::decode(&other), None, "another version is measured again");
    }

    /// `cargo test --release -p nori-player vocal_cost -- --ignored --nocapture`: what the curve adds to the
    /// analysis of a four-minute song, the best of five runs of the analyser's own FFT loop with and without it
    /// (a loaded machine only ever adds time).
    #[test]
    #[ignore]
    fn vocal_cost() {
        use rustfft::FftPlanner;
        let song = Song { sections: vec![(8, FULL), (48, SUNG), (32, FULL), (32, SUNG)], ..Song::new("cost", Style::Backbeat, 120.0, 2, false) };
        let (x, _) = song.render();
        let secs = x.len() as f64 / song.rate as f64;
        let mono: Vec<f32> = x.as_chunks::<2>().0.iter().map(|p| (p[0] + p[1]) / 2.0).collect();
        let (n, hop, sr) = (1024usize, 256usize, song.rate as f64 / 2.0);
        let fft = FftPlanner::<f32>::new().plan_fft_forward(n);
        let win: Vec<f32> = (0..n).map(|i| (0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / n as f64).cos()) as f32).collect();
        let mut buf = vec![Complex32::default(); n];
        let mut scratch = vec![Complex32::default(); fft.get_inplace_scratch_len()];
        let mut run = |with: bool| {
            let mut t = Tracker::new(n, sr, 2.0 / win.iter().sum::<f32>(), mono.len() / hop);
            let start = std::time::Instant::now();
            let mut k = n;
            while k <= mono.len() {
                for (o, (s, w)) in buf.iter_mut().zip(mono[k - n..k].iter().zip(&win)) {
                    *o = Complex32::new(s * w, 0.0);
                }
                fft.process_with_scratch(&mut buf, &mut scratch);
                if with {
                    t.frame(&buf);
                }
                k += hop;
            }
            std::hint::black_box(t.take());
            start.elapsed().as_secs_f64() * 1000.0
        };
        let best = |r: &mut dyn FnMut() -> f64| (0..5).map(|_| r()).fold(f64::MAX, f64::min);
        let without = best(&mut || run(false));
        let with = best(&mut || run(true));
        let mut whole = || {
            let t = std::time::Instant::now();
            let mut a = Analyzer::new(song.rate, 0);
            a.feed(&x);
            std::hint::black_box(a.take_features());
            t.elapsed().as_secs_f64() * 1000.0
        };
        let analysis = best(&mut whole);
        println!(
            "{secs:.0} s song: FFT loop {without:.1} ms, with the curve {with:.1} ms: the curve costs {:.1} ms ({:.2} ms a minute), the whole front end {analysis:.1} ms",
            with - without,
            (with - without) / secs * 60.0
        );
    }

    /// `cargo test --release -p nori-player vocal_curve_eval -- --ignored --nocapture`: the table the module's
    /// comment quotes.
    #[test]
    #[ignore]
    fn vocal_curve_eval() {
        let songs: Vec<Song> = corpus_all().into_iter().filter(|s| s.sections.iter().any(|x| x.1.voice)).collect();
        let got: Vec<f64> = std::thread::scope(|sc| songs.iter().map(|s| sc.spawn(move || separation(s))).collect::<Vec<_>>().into_iter().map(|h| h.join().unwrap()).collect());
        for (s, a) in songs.iter().zip(&got) {
            println!("{:<20} AUC {a:.3}", s.name);
        }
        println!("mean AUC {:.3}", got.iter().sum::<f64>() / got.len() as f64);
    }
}
