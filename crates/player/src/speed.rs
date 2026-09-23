//! Playback speed and pitch as a streaming processor: [`crate::sonic::Sonic`] (media3's algorithm,
//! ported exactly) plus the books media3's `SonicAudioProcessor` keeps, so a player can tell how much
//! of the song the output it has played stands for. At speed 1 and pitch 1 it is not in the path.

use crate::pcm::Encoding;
use crate::sonic::Sonic;

/// Below this much output the processed ratio is too noisy to trust; the nominal speed is used.
const MIN_BYTES_FOR_DURATION_SCALING: u64 = 1024;
const CLOSE_THRESHOLD: f32 = 0.0001;

/// Whether speed and pitch change the sound at all: at 1x and pitch 1 (within float noise) the stage
/// stays out of the chain, as media3's own does.
pub fn speed_active(speed: f32, pitch: f32) -> bool {
    (speed - 1.0).abs() >= CLOSE_THRESHOLD || (pitch - 1.0).abs() >= CLOSE_THRESHOLD
}

/// The media time `playout_us` of output stands for at the nominal `speed`, before any output has been
/// counted (or with no stage at all).
pub fn nominal_media_us(speed: f32, playout_us: i64) -> i64 {
    (speed as f64 * playout_us as f64) as i64
}

/// How long `media_us` of the song takes to play at the nominal `speed`.
pub fn nominal_playout_us(speed: f32, media_us: i64) -> i64 {
    (media_us as f64 / speed as f64) as i64
}

enum Engine {
    Short(Sonic<i16>),
    Float(Sonic<f32>),
}

pub struct SpeedPitch {
    rate: u32,
    ch: usize,
    enc: Encoding,
    speed: f32,
    pitch: f32,
    engine: Engine,
    input_bytes: u64,
    output_bytes: u64,
    staged_i16: Vec<i16>,
    staged_f32: Vec<f32>,
}

impl SpeedPitch {
    pub fn new(rate: u32, channels: usize, enc: Encoding) -> SpeedPitch {
        let ch = channels.clamp(1, 8);
        let mut p = SpeedPitch {
            rate,
            ch,
            enc,
            speed: 1.0,
            pitch: 1.0,
            engine: Engine::Short(Sonic::new(rate, ch, 1.0, 1.0, rate)),
            input_bytes: 0,
            output_bytes: 0,
            staged_i16: Vec::new(),
            staged_f32: Vec::new(),
        };
        p.flush();
        p
    }

    /// Takes effect at the next [`SpeedPitch::flush`], when a player applies new parameters.
    pub fn set(&mut self, speed: f32, pitch: f32) {
        self.speed = if speed > 0.0 && speed.is_finite() { speed } else { 1.0 };
        self.pitch = if pitch > 0.0 && pitch.is_finite() { pitch } else { 1.0 };
    }

    pub fn active(&self) -> bool {
        speed_active(self.speed, self.pitch)
    }

    /// A new stream (a seek, or new parameters): a fresh engine at the current settings.
    pub fn flush(&mut self) {
        self.engine = match self.enc {
            Encoding::Pcm16 => Engine::Short(Sonic::new(self.rate, self.ch, self.speed, self.pitch, self.rate)),
            Encoding::Float => Engine::Float(Sonic::new(self.rate, self.ch, self.speed, self.pitch, self.rate)),
        };
        self.input_bytes = 0;
        self.output_bytes = 0;
    }

    /// Interleaved bytes in (the configured encoding); whatever output is ready is appended to `out`.
    pub fn process(&mut self, input: &[u8], out: &mut Vec<u8>) {
        self.input_bytes += input.len() as u64;
        match &mut self.engine {
            Engine::Short(s) => {
                self.staged_i16.clear();
                self.staged_i16.extend(input.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])));
                s.queue_input(&self.staged_i16);
            }
            Engine::Float(s) => {
                self.staged_f32.clear();
                self.staged_f32.extend(input.chunks_exact(4).map(|c| f32::from_le_bytes([c[0], c[1], c[2], c[3]])));
                s.queue_input(&self.staged_f32);
            }
        }
        self.drain(out);
    }

    fn drain(&mut self, out: &mut Vec<u8>) {
        let before = out.len();
        match &mut self.engine {
            Engine::Short(s) => {
                let n = s.output_frames() * self.ch;
                self.staged_i16.resize(n, 0);
                let got = s.get_output(&mut self.staged_i16) * self.ch;
                for v in &self.staged_i16[..got] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
            Engine::Float(s) => {
                let n = s.output_frames() * self.ch;
                self.staged_f32.resize(n, 0.0);
                let got = s.get_output(&mut self.staged_f32) * self.ch;
                for v in &self.staged_f32[..got] {
                    out.extend_from_slice(&v.to_le_bytes());
                }
            }
        }
        self.output_bytes += (out.len() - before) as u64;
    }

    /// The input has ended: whatever is still inside comes out.
    pub fn end_of_stream(&mut self, out: &mut Vec<u8>) {
        match &mut self.engine {
            Engine::Short(s) => s.queue_end_of_stream(),
            Engine::Float(s) => s.queue_end_of_stream(),
        }
        self.drain(out);
    }

    fn processed_input_bytes(&self) -> u64 {
        let pending = match &self.engine {
            Engine::Short(s) => s.pending_input_frames() * self.ch * 2,
            Engine::Float(s) => s.pending_input_frames() * self.ch * 4,
        } as u64;
        self.input_bytes.saturating_sub(pending)
    }

    /// The media time `playout_us` of played output stands for, as media3 works it out.
    pub fn media_duration_us(&self, playout_us: i64) -> i64 {
        if self.output_bytes >= MIN_BYTES_FOR_DURATION_SCALING {
            (playout_us as i128 * self.processed_input_bytes() as i128 / self.output_bytes as i128) as i64
        } else {
            nominal_media_us(self.speed, playout_us)
        }
    }

    /// The other way round: how long `media_us` of the song takes to play.
    pub fn playout_duration_us(&self, media_us: i64) -> i64 {
        let processed = self.processed_input_bytes();
        if self.output_bytes >= MIN_BYTES_FOR_DURATION_SCALING && processed > 0 {
            (media_us as i128 * self.output_bytes as i128 / processed as i128) as i64
        } else {
            nominal_playout_us(self.speed, media_us)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_stage_is_in_the_chain_only_when_it_changes_the_sound() {
        assert!(!speed_active(1.0, 1.0));
        assert!(!speed_active(1.00005, 0.99995), "float noise");
        assert!(speed_active(1.25, 1.0));
        assert!(speed_active(1.0, 0.9));
        assert!(speed_active(1.0001, 1.0), "the threshold itself counts");
        assert_eq!(nominal_media_us(1.5, 1_000_000), 1_500_000);
        assert_eq!(nominal_playout_us(2.0, 1_000_000), 500_000);
        assert_eq!(nominal_playout_us(1.0, 7), 7);
    }

    fn sine(secs: f64, hz: f64) -> Vec<u8> {
        (0..(44100.0 * secs) as usize)
            .flat_map(|i| {
                let v = ((i as f64 / 44100.0 * hz * std::f64::consts::TAU).sin() * 12000.0) as i16;
                [v, v]
            })
            .flat_map(|v| v.to_le_bytes())
            .collect()
    }

    #[test]
    fn faster_is_shorter() {
        let mut p = SpeedPitch::new(44100, 2, Encoding::Pcm16);
        p.set(1.5, 1.0);
        p.flush();
        let mut out = Vec::new();
        for c in sine(3.0, 220.0).chunks(8192) {
            p.process(c, &mut out);
        }
        p.end_of_stream(&mut out);
        let secs = out.len() as f64 / 4.0 / 44100.0;
        assert!((secs - 2.0).abs() < 0.01, "{secs}");
    }

    #[test]
    fn media_time_runs_at_the_speed() {
        let mut p = SpeedPitch::new(44100, 2, Encoding::Pcm16);
        p.set(1.5, 1.0);
        p.flush();
        let mut out = Vec::new();
        p.process(&sine(3.0, 220.0), &mut out);
        let m = p.media_duration_us(1_000_000);
        assert!((m - 1_500_000).abs() < 20_000, "{m}");
    }

    #[test]
    fn at_one_it_is_inactive() {
        assert!(!SpeedPitch::new(48000, 2, Encoding::Float).active());
    }
}

#[cfg(test)]
mod cost {
    #[test]
    #[ignore]
    fn sonic_cost() {
        let x: Vec<u8> = (0..44100usize * 60 * 2).flat_map(|i| ((((i as f32) * 0.01).sin() * 9000.0) as i16).to_le_bytes()).collect();
        let mut p = super::SpeedPitch::new(44100, 2, crate::pcm::Encoding::Pcm16);
        p.set(1.25, 1.0);
        p.flush();
        let mut out = Vec::new();
        let t = std::time::Instant::now();
        for c in x.chunks(8192) { p.process(c, &mut out); out.clear(); }
        let el = t.elapsed().as_secs_f64();
        println!("sonic: 60 s at 1.25x in {:.0} ms = {:.3} % of one core", el * 1000.0, el / 60.0 * 100.0);
    }
}
