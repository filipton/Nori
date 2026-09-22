//! Synthetic tracks with a known tempo, downbeat, key and structure, for tests: a click track with
//! chords, noise and an optional tempo drift, rendered as mono f32.

use std::f64::consts::PI;

/// A deterministic noise source (xorshift), so tests never flake.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

#[derive(Clone)]
pub struct Synth {
    pub rate: u32,
    pub bpm: f64,
    pub secs: f64,
    /// Time of beat 0, seconds.
    pub first_beat: f64,
    /// Which beat index (mod 4) carries the kick that marks the bar.
    pub downbeat: usize,
    /// Off-beat click at this fraction of the beat (0.5 straight, 0.67 swung); 0 for none.
    pub offbeat: f64,
    /// White noise level, linear.
    pub noise: f64,
    pub lead_silence: f64,
    pub tail_silence: f64,
    /// Chords (root pitch class, minor), one per bar, cycling; empty for no harmony.
    pub chords: Vec<(usize, bool)>,
    /// Bars at the start that only have the hats (no kick, no chords), and at the end.
    pub intro_bars: usize,
    pub outro_bars: usize,
    /// Tempo at the end, for drifting tracks (0 = steady).
    pub end_bpm: f64,
}

impl Synth {
    pub fn new(bpm: f64) -> Self {
        Synth {
            rate: 44100,
            bpm,
            secs: 60.0,
            first_beat: 0.25,
            downbeat: 0,
            offbeat: 0.0,
            noise: 0.0,
            lead_silence: 0.0,
            tail_silence: 0.0,
            chords: Vec::new(),
            intro_bars: 0,
            outro_bars: 0,
            end_bpm: 0.0,
        }
    }

    /// Beat times, seconds.
    pub fn beats(&self) -> Vec<f64> {
        let mut t = self.lead_silence + self.first_beat;
        let end = self.lead_silence + self.secs;
        let mut out = Vec::new();
        while t < end {
            out.push(t);
            let bpm = if self.end_bpm > 0.0 { self.bpm + (self.end_bpm - self.bpm) * (t - self.lead_silence) / self.secs } else { self.bpm };
            t += 60.0 / bpm;
        }
        out
    }

    pub fn render(&self) -> Vec<f32> {
        let rate = self.rate as f64;
        let total = ((self.lead_silence + self.secs + self.tail_silence) * rate) as usize;
        let music_end = ((self.lead_silence + self.secs) * rate) as usize;
        let mut x = vec![0f64; total];
        let beats = self.beats();
        let bars = beats.len() / 4;
        let add = |x: &mut Vec<f64>, at: f64, freq: f64, amp: f64, decay_s: f64, len_s: f64| {
            let s = (at * rate) as usize;
            let n = (len_s * rate) as usize;
            for i in 0..n {
                if s + i >= music_end {
                    break;
                }
                let t = i as f64 / rate;
                x[s + i] += amp * (-t / decay_s).exp() * (2.0 * PI * freq * t).sin();
            }
        };
        for (i, &t) in beats.iter().enumerate() {
            let bar = i / 4;
            let quiet = bar < self.intro_bars || bar + self.outro_bars >= bars;
            // Hat on every beat: a noisy high click.
            add(&mut x, t, 6000.0, 0.25, 0.01, 0.04);
            add(&mut x, t, 3100.0, 0.2, 0.012, 0.04);
            if !quiet && i % 4 == self.downbeat {
                add(&mut x, t, 55.0, 0.8, 0.12, 0.4);
            } else if !quiet {
                // A low tom under the chroma range, so the drums do not vote for a key.
                add(&mut x, t, 90.0, 0.25, 0.04, 0.12);
            }
            if self.offbeat > 0.0 {
                let period = beats.get(i + 1).map_or(60.0 / self.bpm, |n| n - t);
                add(&mut x, t + self.offbeat * period, 7000.0, 0.12, 0.008, 0.03);
            }
        }
        if !self.chords.is_empty() {
            // Chords change on the bar lines of the kick.
            let starts: Vec<f64> = beats.iter().enumerate().filter(|(i, _)| i % 4 == self.downbeat).map(|(_, t)| *t).collect();
            for (b, w) in starts.windows(2).enumerate() {
                let bar = b + usize::from(self.downbeat > 0);
                if bar < self.intro_bars || bar + self.outro_bars >= bars {
                    continue;
                }
                let (root, minor) = self.chords[b % self.chords.len()];
                let third = if minor { 3 } else { 4 };
                for (k, iv) in [0usize, third, 7].iter().enumerate() {
                    let midi = 48 + root + iv + if k == 0 { 0 } else { 12 };
                    let f = 440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0);
                    for h in 1..=3 {
                        add(&mut x, w[0], f * h as f64, 0.08 / h as f64, 2.0, w[1] - w[0]);
                    }
                }
            }
        }
        let mut rng = Rng(0x9E3779B97F4A7C15);
        let start = (self.lead_silence * rate) as usize;
        for v in &mut x[start..music_end] {
            *v += self.noise * rng.next();
        }
        x.iter().map(|v| v.clamp(-1.0, 1.0) as f32).collect()
    }
}

