//! Beat This!, the optional neural beat tracker, as AutoMix uses it: each end of a song is read once, the constant
//! grid AutoMix stores is fitted to the model's beats and downbeats, and that grid replaces the classical one when it
//! is sure. The classical grid stays where the model is not sure, and each end records which of the two it holds, so
//! a later run knows what is left to do and a fresh classical measurement does not throw the model's work away
//! (`carry`).
//!
//! Everything here is plain Rust and always built; only running the model needs the `neural-beats` feature
//! (`neural`, and [`read`]). nori-engine's measurer is what runs it, on songs whole on the disk.

use crate::types::TrackAnalysis;

use super::plan::{MIN_BPM_CONFIDENCE, MIN_STABILITY};

/// Whether this build carries the model's runtime (the `neural-beats` feature).
pub const AVAILABLE: bool = cfg!(feature = "neural-beats");

/// The grid is the classical tracker's, and the model has not looked at this end yet.
pub const GRID_CLASSICAL: i32 = 0;
/// The model looked at this end and was not sure enough, so the classical grid stays.
pub const GRID_CHECKED: i32 = 1;
/// The grid is Beat This!'s.
pub const GRID_NEURAL: i32 = 2;

/// How much of each end the model reads: its training window, and the half minute a mix plays over.
pub const WINDOW_MS: i64 = 30_000;
/// Less music than this at an end is not worth a run (and cannot hold the 8 beats a grid needs at slow tempos).
const MIN_WINDOW_MS: i64 = 8_000;
/// How much of each end [`Ends`] keeps: a little more than the window, so the music's first and last half minute
/// is inside it even with some silence before or after.
pub const ENDS_MS: i64 = 35_000;

/// One end of a song, where a mix happens.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MixEnd {
    Intro,
    Outro,
}

/// One end's grid as the model found it, in track time.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct EndGrid {
    pub bpm: f64,
    pub offset_ms: f64,
    pub confidence: f32,
    pub stability: f32,
    pub downbeat_phase: i32,
    pub beats_per_bar: i32,
    /// A second bar start the model points at nearly as often (-1 when its bar is clear); see `settle_bar`.
    pub other_phase: i32,
    /// A beat in the middle of the window, ms: where it is compared with the classical grid.
    pub anchor_ms: f64,
}

/// Whether the planner would beat-match on `g`: the same gate it applies to every grid.
pub fn confident(g: &EndGrid) -> bool {
    g.bpm > 0.0 && g.bpm.is_finite() && g.confidence >= MIN_BPM_CONFIDENCE && g.stability >= MIN_STABILITY
}

/// The classical grid at `end` of `row`: (bpm, first beat ms, downbeat phase, beats in a bar).
fn classical(row: &TrackAnalysis, end: MixEnd) -> (f64, f64, i32, i32) {
    let own = match end {
        MixEnd::Intro => row.intro_beats_per_bar,
        MixEnd::Outro => row.outro_beats_per_bar,
    };
    let meter = if own > 0 { own } else if row.beats_per_bar == 3 { 3 } else { 4 };
    match end {
        MixEnd::Intro => (row.intro_bpm, row.intro_beat_offset_ms, row.intro_downbeat_phase, meter),
        MixEnd::Outro => (row.outro_bpm, row.outro_beat_offset_ms, row.outro_downbeat_phase, meter),
    }
}

/// A grid whose bar the model could not settle (`other_phase`: it marked every other beat, as it does where only
/// drums play) takes its bar start from the classical grid at the same end, when that one has the same tempo and
/// metre and starts its bars on one of the two candidates. Otherwise there is no telling, and `None` keeps the
/// classical grid: a bar started on the wrong beat is a worse mix than a plainer one.
pub fn settle_bar(row: &TrackAnalysis, end: MixEnd, g: EndGrid) -> Option<EndGrid> {
    if g.other_phase < 0 {
        return Some(g);
    }
    let (bpm, offset, phase, meter) = classical(row, end);
    if !(bpm > 0.0 && bpm.is_finite()) || (bpm / g.bpm - 1.0).abs() > 0.03 || meter != g.beats_per_bar {
        return None;
    }
    let m = meter as i64;
    // The classical downbeat nearest the middle of the window.
    let pc = 60_000.0 / bpm;
    let n = ((g.anchor_ms - offset) / pc).round() as i64;
    let n = n - (n - phase as i64).rem_euclid(m);
    let down = [n, n + m].map(|k| offset + k as f64 * pc).into_iter().min_by(|a, b| (a - g.anchor_ms).abs().total_cmp(&(b - g.anchor_ms).abs()))?;
    // Which of the model's two bar starts lands on it, within a quarter of a beat.
    let p = 60_000.0 / g.bpm;
    let lands = |ph: i32| {
        let k = ((down - g.offset_ms) / p).round() as i64;
        let k = k - (k - ph as i64).rem_euclid(m);
        [k, k + m].iter().any(|k| (g.offset_ms + *k as f64 * p - down).abs() < 0.25 * p)
    };
    match (lands(g.downbeat_phase), lands(g.other_phase)) {
        (true, false) => Some(EndGrid { other_phase: -1, ..g }),
        (false, true) => Some(EndGrid { downbeat_phase: g.other_phase, other_phase: -1, ..g }),
        _ => None,
    }
}

/// Puts what the model found at one end into `row`: its grid when it is confident and its bar is settled
/// (`GRID_NEURAL`), otherwise only the mark that it has looked (`GRID_CHECKED`), leaving the classical grid. Returns
/// whether the grid was adopted.
pub fn merge(row: &mut TrackAnalysis, end: MixEnd, grid: Option<EndGrid>) -> bool {
    let adopted = grid.filter(confident).and_then(|g| settle_bar(row, end, g));
    match (end, adopted) {
        (MixEnd::Intro, Some(g)) => {
            (row.intro_bpm, row.intro_bpm_confidence, row.intro_beat_offset_ms, row.intro_stability, row.intro_downbeat_phase) =
                (g.bpm, g.confidence, g.offset_ms, g.stability, g.downbeat_phase);
            row.intro_beats_per_bar = g.beats_per_bar;
            row.intro_grid_source = GRID_NEURAL;
        }
        (MixEnd::Outro, Some(g)) => {
            (row.outro_bpm, row.outro_bpm_confidence, row.outro_beat_offset_ms, row.outro_stability, row.outro_downbeat_phase) =
                (g.bpm, g.confidence, g.offset_ms, g.stability, g.downbeat_phase);
            row.outro_beats_per_bar = g.beats_per_bar;
            row.outro_grid_source = GRID_NEURAL;
        }
        (MixEnd::Intro, None) => row.intro_grid_source = row.intro_grid_source.max(GRID_CHECKED),
        (MixEnd::Outro, None) => row.outro_grid_source = row.outro_grid_source.max(GRID_CHECKED),
    }
    adopted.is_some()
}

/// A new classical measurement `fresh` of the file `old` describes keeps the model's work: its grids where it had
/// them, and the mark where it looked. Only for the same file (lengths within a second); another file under the
/// same id starts again.
pub fn carry(old: &TrackAnalysis, fresh: &mut TrackAnalysis) {
    if (old.duration_ms - fresh.duration_ms).abs() > 1000 {
        return;
    }
    if old.intro_grid_source == GRID_NEURAL {
        (fresh.intro_bpm, fresh.intro_bpm_confidence, fresh.intro_beat_offset_ms, fresh.intro_stability, fresh.intro_downbeat_phase) =
            (old.intro_bpm, old.intro_bpm_confidence, old.intro_beat_offset_ms, old.intro_stability, old.intro_downbeat_phase);
        fresh.intro_beats_per_bar = old.intro_beats_per_bar;
    }
    if old.outro_grid_source == GRID_NEURAL {
        (fresh.outro_bpm, fresh.outro_bpm_confidence, fresh.outro_beat_offset_ms, fresh.outro_stability, fresh.outro_downbeat_phase) =
            (old.outro_bpm, old.outro_bpm_confidence, old.outro_beat_offset_ms, old.outro_stability, old.outro_downbeat_phase);
        fresh.outro_beats_per_bar = old.outro_beats_per_bar;
    }
    fresh.intro_grid_source = fresh.intro_grid_source.max(old.intro_grid_source);
    fresh.outro_grid_source = fresh.outro_grid_source.max(old.outro_grid_source);
}

/// Whether the model has yet to look at `end` of `row`.
pub fn needs_end(row: &TrackAnalysis, end: MixEnd) -> bool {
    match end {
        MixEnd::Intro => row.intro_grid_source < GRID_CHECKED,
        MixEnd::Outro => row.outro_grid_source < GRID_CHECKED,
    }
}

/// Whether the model still has to look at an end of `row`.
pub fn needs_model(row: &TrackAnalysis) -> bool {
    needs_end(row, MixEnd::Intro) || needs_end(row, MixEnd::Outro)
}

/// The stretch of the file the model reads at `end`, ms: the first or last `WINDOW_MS` of the music (silence
/// trimmed), as far as `have` (the audio at hand) reaches. `None` when too little of it is there.
pub fn window(row: &TrackAnalysis, end: MixEnd, have: (i64, i64)) -> Option<(i64, i64)> {
    let (start, stop) = (row.silence_start_ms.max(0), row.silence_end_ms);
    if stop <= start {
        return None;
    }
    let (from, to) = match end {
        MixEnd::Intro => (start, (start + WINDOW_MS).min(stop)),
        MixEnd::Outro => ((stop - WINDOW_MS).max(start), stop),
    };
    let (from, to) = (from.max(have.0), to.min(have.1));
    (to - from >= MIN_WINDOW_MS).then_some((from, to))
}

/// The rate the model reads at, near which [`Ends`] keeps its samples.
const MODEL_RATE: f64 = 22_050.0;

/// The first and last [`ENDS_MS`] of a song as it is decoded, mono and brought to about 22 kHz by averaging whole
/// groups of samples (44.1 kHz to 22.05, 48 to 24), the way the model's front end would: the head copied until it is
/// full, the tail through a ring that always holds the latest. Sized once for the rate, so feeding never allocates;
/// 3.4 MB at the most, for as long as one song is measured.
pub struct Ends {
    /// Samples averaged into one.
    k: usize,
    rate: u32,
    head: Vec<f32>,
    ring: Vec<f32>,
    at: usize,
    /// Samples kept so far, head and ring alike (the ring's count is `total` too).
    total: u64,
    sum: f32,
    summed: usize,
}

impl Ends {
    pub fn new(rate: u32) -> Ends {
        let k = ((rate as f64 / MODEL_RATE).round() as usize).max(1);
        let rate = (rate as usize / k) as u32;
        let len = (ENDS_MS as u64 * rate as u64 / 1000) as usize;
        Ends { k, rate, head: Vec::with_capacity(len), ring: vec![0.0; len], at: 0, total: 0, sum: 0.0, summed: 0 }
    }

    /// The rate the samples are kept at.
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Interleaved samples of `channels` channels.
    pub fn feed(&mut self, x: &[f32], channels: usize) {
        let ch = channels.max(1);
        let scale = 1.0 / (ch * self.k) as f32;
        for frame in x.chunks_exact(ch) {
            self.sum += frame.iter().sum::<f32>();
            self.summed += 1;
            if self.summed < self.k {
                continue;
            }
            let v = self.sum * scale;
            (self.sum, self.summed) = (0.0, 0);
            if self.head.len() < self.head.capacity() {
                self.head.push(v);
            }
            if !self.ring.is_empty() {
                self.ring[self.at] = v;
                self.at = (self.at + 1) % self.ring.len();
            }
            self.total += 1;
        }
    }

    /// The song's first samples; they start at 0 ms.
    pub fn head(&self) -> &[f32] {
        &self.head
    }

    /// The song's last samples in order, oldest first, and where they start in the song, ms. Puts the ring in order
    /// where it is, so it is only for when the song is done.
    pub fn tail(&mut self) -> (&[f32], i64) {
        let kept = (self.total as usize).min(self.ring.len());
        if self.total as usize >= self.ring.len() {
            self.ring.rotate_left(self.at);
            self.at = 0;
        }
        let start_ms = ((self.total - kept as u64) * 1000 / self.rate.max(1) as u64) as i64;
        (&self.ring[..kept], start_ms)
    }
}

/// Beat This! over one end of a song: `x` is the stretch of it that starts `from_ms` into the file, mono at `rate`
/// (cut by [`window`]). The model's beats and downbeats, in the file's time, and the grid through them.
#[cfg(feature = "neural-beats")]
pub fn read(model: &super::neural::BeatThis, x: &[f32], rate: u32, end: MixEnd, from_ms: i64) -> Result<Option<EndGrid>, String> {
    let mut t = model.track_window(x, rate, end == MixEnd::Intro).map_err(|e| format!("running the beat model: {e}"))?;
    // In track time before the grid is fitted, so its first beat and downbeat count from the file's start.
    let from_s = from_ms as f64 / 1000.0;
    t.beats.iter_mut().chain(t.downbeats.iter_mut()).for_each(|v| *v += from_s);
    Ok(super::neural::grid(&t).map(|g| EndGrid {
        bpm: g.bpm,
        offset_ms: g.offset_s * 1000.0,
        confidence: g.confidence,
        stability: g.stability,
        downbeat_phase: g.downbeat_phase,
        beats_per_bar: g.beats_per_bar,
        other_phase: g.other_phase,
        anchor_ms: g.anchor_s * 1000.0,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row() -> TrackAnalysis {
        TrackAnalysis {
            song_id: "s".into(),
            analysis_version: super::super::ANALYSIS_VERSION,
            duration_ms: 200_000,
            silence_start_ms: 1_500,
            silence_end_ms: 198_000,
            intro_bpm: 100.0,
            intro_bpm_confidence: 0.3,
            outro_bpm: 101.0,
            outro_bpm_confidence: 0.9,
            outro_stability: 0.9,
            beats_per_bar: 4,
            ..Default::default()
        }
    }

    #[test]
    fn a_confident_grid_replaces_the_classical_one_and_an_unsure_one_only_marks_the_end() {
        let mut r = row();
        let sure = EndGrid { bpm: 128.0, offset_ms: 120.0, confidence: 0.9, stability: 0.8, downbeat_phase: 2, beats_per_bar: 3, other_phase: -1, anchor_ms: 0.0 };
        assert!(merge(&mut r, MixEnd::Intro, Some(sure)));
        assert_eq!((r.intro_bpm, r.intro_beat_offset_ms, r.intro_downbeat_phase, r.intro_beats_per_bar), (128.0, 120.0, 2, 3));
        assert_eq!(r.intro_grid_source, GRID_NEURAL);
        let unsure = EndGrid { stability: 0.2, ..sure };
        assert!(!merge(&mut r, MixEnd::Outro, Some(unsure)));
        assert_eq!((r.outro_bpm, r.outro_grid_source, r.outro_beats_per_bar), (101.0, GRID_CHECKED, 0));
        assert!(!needs_model(&r));
        // Looking again without an answer never demotes a grid the model already gave.
        assert!(!merge(&mut r, MixEnd::Intro, None));
        assert_eq!(r.intro_grid_source, GRID_NEURAL);
    }

    /// Two candidate bar starts: the classical grid's bar picks one when it has the same tempo; when it does not,
    /// or starts its bars on neither, the classical grid stays.
    #[test]
    fn an_unsettled_bar_is_settled_by_the_classical_grid_or_not_used() {
        // Classical: 120 BPM from 250 ms, bars starting on beat 1 (250 + 500 = 750 ms, then every 2 s).
        let r = TrackAnalysis { intro_bpm: 120.0, intro_beat_offset_ms: 250.0, intro_downbeat_phase: 1, intro_bpm_confidence: 0.2, ..row() };
        // The model: the same beats counted from 250 ms, bar on beat 3 or beat 1.
        let g = EndGrid { bpm: 120.2, offset_ms: 250.0, confidence: 1.0, stability: 1.0, downbeat_phase: 3, beats_per_bar: 4, other_phase: 1, anchor_ms: 15_250.0 };
        assert_eq!(settle_bar(&r, MixEnd::Intro, g).map(|g| (g.downbeat_phase, g.other_phase)), Some((1, -1)));
        let mut merged = r.clone();
        assert!(merge(&mut merged, MixEnd::Intro, Some(g)));
        assert_eq!((merged.intro_bpm, merged.intro_downbeat_phase, merged.intro_grid_source), (120.2, 1, GRID_NEURAL));
        // A classical grid at another tempo cannot tell, nor one whose bars start between the two.
        assert!(settle_bar(&TrackAnalysis { intro_bpm: 90.0, ..r.clone() }, MixEnd::Intro, g).is_none());
        assert!(settle_bar(&TrackAnalysis { intro_downbeat_phase: 2, ..r.clone() }, MixEnd::Intro, g).is_none());
        let mut kept = r.clone();
        assert!(!merge(&mut kept, MixEnd::Intro, Some(EndGrid { other_phase: 2, downbeat_phase: 0, ..g })));
        assert_eq!((kept.intro_bpm, kept.intro_grid_source), (120.0, GRID_CHECKED));
        // A settled bar needs nothing from the classical grid.
        assert_eq!(settle_bar(&TrackAnalysis { intro_bpm: 0.0, ..r }, MixEnd::Intro, EndGrid { other_phase: -1, ..g }).map(|g| g.downbeat_phase), Some(3));
    }

    #[test]
    fn a_fresh_measurement_of_the_same_file_keeps_the_models_work() {
        let mut old = row();
        merge(&mut old, MixEnd::Intro, Some(EndGrid { bpm: 128.0, offset_ms: 50.0, confidence: 1.0, stability: 1.0, downbeat_phase: 1, beats_per_bar: 4, other_phase: -1, anchor_ms: 0.0 }));
        merge(&mut old, MixEnd::Outro, None);
        let mut fresh = row();
        carry(&old, &mut fresh);
        assert_eq!((fresh.intro_bpm, fresh.intro_beat_offset_ms, fresh.intro_grid_source), (128.0, 50.0, GRID_NEURAL));
        assert_eq!((fresh.outro_bpm, fresh.outro_grid_source), (101.0, GRID_CHECKED));
        // Another file under the same id (a different length) starts again.
        let mut other = TrackAnalysis { duration_ms: 150_000, ..row() };
        carry(&old, &mut other);
        assert_eq!((other.intro_bpm, other.intro_grid_source, other.outro_grid_source), (100.0, GRID_CLASSICAL, GRID_CLASSICAL));
    }

    #[test]
    fn the_windows_are_the_first_and_last_half_minute_of_the_music() {
        let r = row();
        assert_eq!(window(&r, MixEnd::Intro, (0, 40_000)), Some((1_500, 31_500)));
        assert_eq!(window(&r, MixEnd::Outro, (160_000, 200_000)), Some((168_000, 198_000)));
        // Only as far as the audio at hand reaches; too little of it is no window at all.
        assert_eq!(window(&r, MixEnd::Intro, (0, 20_000)), Some((1_500, 20_000)));
        assert_eq!(window(&r, MixEnd::Outro, (0, 30_000)), None);
        assert_eq!(window(&TrackAnalysis { silence_end_ms: 0, ..r }, MixEnd::Intro, (0, 40_000)), None);
    }

    /// A song's ends kept as it is decoded: mono, at half of 44.1 kHz, the head from its first sample and the tail
    /// in order with where it starts.
    #[test]
    fn the_ends_keep_the_first_and_last_seconds_in_mono_at_half_the_rate() {
        let rate = 44_100u32;
        let mut e = Ends::new(rate);
        assert_eq!(e.rate(), 22_050);
        // 50 s of stereo whose left channel counts samples and whose right is its negative plus 2: the mono of a
        // frame is 1, and of a pair of frames too.
        let frames = 50 * rate as usize;
        let x: Vec<f32> = (0..frames).flat_map(|i| [i as f32, 2.0 - i as f32]).collect();
        // Fed in uneven buffers, with a buffer that splits a pair of frames.
        for piece in x.chunks(2 * 1001) {
            e.feed(piece, 2);
        }
        let kept = (ENDS_MS as usize) * 22_050 / 1000;
        assert_eq!(e.head().len(), kept);
        assert!(e.head().iter().all(|v| (*v - 1.0).abs() < 1e-6));
        let (tail, start) = e.tail();
        assert_eq!(tail.len(), kept);
        assert_eq!(start, ((frames / 2 - kept) * 1000 / 22_050) as i64);
        assert!(tail.iter().all(|v| (*v - 1.0).abs() < 1e-6));

        // A ramp on one channel: the tail is in order, oldest first.
        let mut e = Ends::new(rate);
        let ramp: Vec<f32> = (0..frames).map(|i| i as f32).collect();
        e.feed(&ramp, 1);
        let (tail, _) = e.tail();
        assert!(tail.windows(2).all(|w| w[1] > w[0]));
        assert_eq!(*tail.last().unwrap(), (frames - 2) as f32 + 0.5);

        // A song shorter than the ends: all of it, from the start.
        let mut e = Ends::new(48_000);
        e.feed(&vec![0.5; 48_000 * 10], 1);
        let (tail, start) = e.tail();
        assert_eq!((tail.len(), start, e.head().len()), (240_000, 0, 240_000));
    }
}
