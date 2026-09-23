//! The sound card's side. A platform writes one small [`AudioOutput`]: open a device, and from the
//! device's own thread call [`Feed::pull`] for every buffer it wants. The feed reads a lock-free ring
//! that one engine thread fills in bursts; pulling never blocks, never allocates and never takes a
//! lock, so the device thread cannot be held up by anything the player does.
//!
//! The ring holds float samples at the device's rate and channel count. The engine's output below the
//! sound chain ([`RingTrack`]) converts the chain's 16-bit audio into it, resampling only when the
//! device would not take the stream's own rate, and keeps the map from ring frames back to song time
//! that the playhead is read through.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::Thread;

use nori_player::automix::resample::Resampler;
use nori_player::burst::{BUFFER_US, LOW_US};
use nori_player::pcm::{Encoding, Format};
use nori_player::pipeline::Track;

/// What a device plays: float samples, interleaved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputFormat {
    pub rate: u32,
    pub channels: usize,
}

/// A sound card, or anything that takes the music the way one does (a file). Called only from the
/// engine's thread; the device's own thread only ever calls [`Feed::pull`].
pub trait AudioOutput: Send {
    /// Picks the device's format, as close to `want` as it goes. Nothing plays yet.
    fn open(&mut self, want: OutputFormat) -> Result<OutputFormat, String>;
    /// From now on the device pulls every buffer it plays from `feed`; it starts paused.
    fn start(&mut self, feed: Feed) -> Result<(), String>;
    /// Stops pulling, and lets the device sleep.
    fn pause(&mut self);
    fn resume(&mut self);
    /// How long a sample pulled now takes to be heard, µs.
    fn latency_us(&self) -> u64;
    /// Lets the device go.
    fn close(&mut self);
}

/// How far above empty the ring is when the engine is woken to fill it: a little under the burst's
/// low mark, so the burst's own count (which includes what the device holds) agrees it is time.
pub const WAKE_LOW_US: i64 = LOW_US - 250_000;
/// The ring's room beyond the sink's deep buffer: the resampler's rounding and a device's first pull.
const SLACK_US: i64 = 2_000_000;

/// Shared between the engine's thread (the only writer) and the device's (the only reader).
pub(crate) struct Ring {
    /// Float samples as their bits: plain loads and stores on every machine that matters, and no
    /// data race even when a flush lets the writer run over what the reader is still looking at.
    slots: Box<[AtomicU32]>,
    frames: u64,
    channels: usize,
    rate: u32,
    /// Frames written and read since the ring was made; they only grow.
    write: AtomicU64,
    read: AtomicU64,
    /// A flush: everything before this frame is dropped unplayed.
    discard: AtomicU64,
    /// The engine sleeps until the ring runs down to `low` frames: the reader wakes it then, once.
    waiting: AtomicBool,
    low: AtomicU64,
    engine: Thread,
    /// A volume fade the reader runs sample by sample: where from (NaN: wherever it is), where to,
    /// over how many frames, and a count that moves whenever a new one is asked for.
    gain_from: AtomicU32,
    gain_target: AtomicU32,
    ramp_frames: AtomicU32,
    ramp_gen: AtomicU32,
    /// The music is over: running dry now is the end, not an underrun.
    ended: AtomicBool,
    /// Pulls that found the ring short while music was due.
    underruns: AtomicU64,
}

impl Ring {
    fn new(format: OutputFormat, engine: Thread) -> Ring {
        let frames = (format.rate as i64 * (BUFFER_US + SLACK_US) / 1_000_000) as u64;
        let slots = (0..frames as usize * format.channels).map(|_| AtomicU32::new(0)).collect();
        Ring {
            slots,
            frames,
            channels: format.channels,
            rate: format.rate,
            write: AtomicU64::new(0),
            read: AtomicU64::new(0),
            discard: AtomicU64::new(0),
            waiting: AtomicBool::new(false),
            low: AtomicU64::new(0),
            engine,
            gain_from: AtomicU32::new(f32::NAN.to_bits()),
            gain_target: AtomicU32::new(1f32.to_bits()),
            ramp_frames: AtomicU32::new(0),
            ramp_gen: AtomicU32::new(0),
            ended: AtomicBool::new(false),
            underruns: AtomicU64::new(0),
        }
    }

    /// Where the reader is, a flush taken into account.
    fn read_at(&self) -> u64 {
        self.read.load(Ordering::Acquire).max(self.discard.load(Ordering::Acquire))
    }

    /// Frames written and not yet read.
    fn filled(&self) -> u64 {
        self.write.load(Ordering::Acquire).saturating_sub(self.read_at())
    }

    fn ramp(&self, from: Option<f32>, target: f32, ms: i64) {
        self.gain_from.store(from.unwrap_or(f32::NAN).to_bits(), Ordering::Relaxed);
        self.gain_target.store(target.to_bits(), Ordering::Relaxed);
        self.ramp_frames.store((ms.max(0) as u64 * self.rate as u64 / 1000) as u32, Ordering::Relaxed);
        self.ramp_gen.fetch_add(1, Ordering::Release);
    }
}

/// The device thread's end of the ring. Owned by the device's callback; see [`Feed::pull`].
pub struct Feed {
    ring: Arc<Ring>,
    gain: f32,
    target: f32,
    step: f32,
    gen: u32,
}

impl Feed {
    fn new(ring: Arc<Ring>) -> Feed {
        Feed { ring, gain: 1.0, target: 1.0, step: 0.0, gen: 0 }
    }

    pub fn format(&self) -> OutputFormat {
        OutputFormat { rate: self.ring.rate, channels: self.ring.channels }
    }

    /// Fills `out` (interleaved, the device's channels) with the music, silence past what there is.
    /// Returns the frames of music it held. Lock-free and allocation-free: made for the device's own
    /// real-time thread.
    pub fn pull(&mut self, out: &mut [f32]) -> usize {
        self.pull_as(out, |v| v)
    }

    /// [`Feed::pull`] into 16-bit samples, for a device that takes nothing else.
    pub fn pull_i16(&mut self, out: &mut [i16]) -> usize {
        self.pull_as(out, |v| (v * 32768.0).round().clamp(-32768.0, 32767.0) as i16)
    }

    fn pull_as<S: Copy + Default>(&mut self, out: &mut [S], conv: impl Fn(f32) -> S) -> usize {
        let r = &*self.ring;
        let ch = r.channels;
        let want = (out.len() / ch) as u64;
        let w = r.write.load(Ordering::Acquire);
        let at = r.read_at();
        let n = w.saturating_sub(at).min(want);
        let gen = r.ramp_gen.load(Ordering::Acquire);
        if gen != self.gen {
            self.gen = gen;
            let from = f32::from_bits(r.gain_from.load(Ordering::Relaxed));
            if !from.is_nan() {
                self.gain = from;
            }
            self.target = f32::from_bits(r.gain_target.load(Ordering::Relaxed));
            let frames = r.ramp_frames.load(Ordering::Relaxed);
            self.step = if frames == 0 { self.target - self.gain } else { (self.target - self.gain) / frames as f32 };
        }
        for i in 0..n {
            if self.gain != self.target {
                self.gain += self.step;
                if (self.step > 0.0 && self.gain > self.target) || (self.step < 0.0 && self.gain < self.target) || self.step == 0.0 {
                    self.gain = self.target;
                }
            }
            let slot = ((at + i) % r.frames) as usize * ch;
            let o = i as usize * ch;
            for c in 0..ch {
                out[o + c] = conv(f32::from_bits(r.slots[slot + c].load(Ordering::Relaxed)) * self.gain);
            }
        }
        out[n as usize * ch..].fill(S::default());
        r.read.store(at + n, Ordering::Release);
        if n < want && !r.ended.load(Ordering::Relaxed) && w > 0 {
            r.underruns.fetch_add(1, Ordering::Relaxed);
        }
        // Run down to the low mark: the engine's next burst is due. One wake per burst.
        if w - (at + n) <= r.low.load(Ordering::Relaxed) && r.waiting.load(Ordering::Relaxed) && r.waiting.swap(false, Ordering::AcqRel) {
            r.engine.unpark();
        }
        n as usize
    }

    /// Frames of music waiting in the ring.
    pub fn available(&self) -> usize {
        self.ring.filled() as usize
    }

    /// The music is over: what is left in the ring is the last of it.
    pub fn ending(&self) -> bool {
        self.ring.ended.load(Ordering::Acquire)
    }

    /// The music is over and everything was pulled: a file stops writing here.
    pub fn finished(&self) -> bool {
        self.ring.ended.load(Ordering::Acquire) && self.ring.filled() == 0
    }
}

/// The [`Track`] under the engine's sink: the ring, the device, and the way back from ring frames
/// to song time.
pub(crate) struct RingTrack {
    output: Box<dyn AudioOutput>,
    ring: Option<Arc<Ring>>,
    device: Option<OutputFormat>,
    format: Option<Format>,
    resampler: Option<Resampler>,
    converted: Vec<u8>,
    engine: Thread,
    /// The ring's write position at the last flush: frames from here on are this timeline's.
    base: u64,
    /// Where each stretch of written frames ends (frames since the flush) and the song time it takes
    /// the playhead to, and the start of the first stretch still ahead of the playhead.
    marks: VecDeque<(u64, f64)>,
    from: (u64, f64),
    written: (u64, f64),
    playing: bool,
    /// Why the device would not open, until the engine has said so.
    pub failed: Option<String>,
}

impl RingTrack {
    pub(crate) fn new(output: Box<dyn AudioOutput>) -> RingTrack {
        RingTrack {
            output,
            ring: None,
            device: None,
            format: None,
            resampler: None,
            converted: Vec::new(),
            engine: std::thread::current(),
            base: 0,
            marks: VecDeque::with_capacity(64),
            from: (0, 0.0),
            written: (0, 0.0),
            playing: false,
            failed: None,
        }
    }

    /// Music in the ring, µs of the device's time.
    pub(crate) fn filled_us(&self) -> i64 {
        match (&self.ring, self.device) {
            (Some(r), Some(d)) => (r.filled() as i128 * 1_000_000 / d.rate as i128) as i64,
            _ => 0,
        }
    }

    pub(crate) fn latency_us(&self) -> i64 {
        self.output.latency_us() as i64
    }

    /// The engine goes to sleep until the ring holds no more than `us` of music.
    pub(crate) fn wake_at(&self, us: i64) {
        if let Some(r) = &self.ring {
            r.low.store((us.max(0) as u64 * r.rate as u64 / 1_000_000) as u64, Ordering::Relaxed);
            r.waiting.store(true, Ordering::Release);
        }
    }

    /// A volume fade from `from` (or wherever the volume is) to `target` over `ms`, run by the device
    /// thread from its next pull.
    pub(crate) fn ramp(&self, from: Option<f32>, target: f32, ms: i64) {
        if let Some(r) = &self.ring {
            r.ramp(from, target, ms);
        }
    }

    /// Whether the music is over (no gaps are counted past it).
    pub(crate) fn set_ended(&self, ended: bool) {
        if let Some(r) = &self.ring {
            r.ended.store(ended, Ordering::Release);
        }
    }

    pub(crate) fn underruns(&self) -> u64 {
        self.ring.as_ref().map_or(0, |r| r.underruns.load(Ordering::Relaxed))
    }

    /// Song time up to ring frame `p` (since the flush), from the stretches written.
    fn media_at(&mut self, p: u64) -> f64 {
        while let Some(&m) = self.marks.front() {
            if m.0 > p {
                break;
            }
            self.from = m;
            self.marks.pop_front();
        }
        match self.marks.front() {
            Some(&(end, media)) if end > self.from.0 => self.from.1 + (p - self.from.0) as f64 / (end - self.from.0) as f64 * (media - self.from.1),
            _ => self.from.1,
        }
    }
}

impl Track for RingTrack {
    fn open(&mut self, format: Format) {
        self.format = Some(format);
        if self.device.is_none() {
            // The first stream picks the device's format for as long as it stays open: later streams
            // are converted to it, as the transition engine converts them to the first one, so the
            // device is never opened again between songs.
            let want = OutputFormat { rate: format.rate, channels: format.channels };
            let opened = self.output.open(want).and_then(|d| {
                let ring = Arc::new(Ring::new(d, self.engine.clone()));
                self.output.start(Feed::new(ring.clone()))?;
                Ok((d, ring))
            });
            match opened {
                Ok((d, ring)) => {
                    self.device = Some(d);
                    self.ring = Some(ring);
                    self.base = 0;
                    if self.playing {
                        self.output.resume();
                    }
                }
                Err(e) => self.failed = Some(e),
            }
        }
        self.resampler = self.device.filter(|d| d.rate != format.rate || d.channels != format.channels).and_then(|d| {
            Resampler::new(format.rate as i32, format.channels as i32, d.rate as i32, d.channels as i32)
        });
    }

    fn queued_bytes(&self) -> usize {
        let (Some(r), Some(d), Some(f)) = (&self.ring, self.device, self.format) else { return 0 };
        (r.filled() as u128 * f.rate as u128 / d.rate as u128) as usize * f.frame_bytes()
    }

    fn write(&mut self, data: &[u8], media: f64) {
        let (Some(r), Some(d), Some(f)) = (self.ring.clone(), self.device, self.format) else { return };
        let w = r.write.load(Ordering::Relaxed);
        let ch = d.channels;
        let put = |at: u64, samples: &mut dyn Iterator<Item = f32>| {
            let mut n = 0u64;
            let mut c = 0;
            let mut slot = ((at % r.frames) as usize) * ch;
            for v in samples {
                r.slots[slot + c].store(v.to_bits(), Ordering::Relaxed);
                c += 1;
                if c == ch {
                    c = 0;
                    n += 1;
                    slot = (((at + n) % r.frames) as usize) * ch;
                }
            }
            n
        };
        let frames = match self.resampler.as_mut() {
            None => put(w, &mut data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)),
            Some(rs) => {
                let in_frames = data.len() / f.frame_bytes();
                let need = ((in_frames as u64 * d.rate as u64 / f.rate as u64) as usize + 4) * ch * 4;
                if self.converted.len() < need {
                    self.converted.resize(need, 0);
                }
                let Some((_, made)) = rs.process(data, Encoding::PCM_16, &mut self.converted, Encoding::FLOAT) else { return };
                put(w, &mut self.converted[..made].chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])))
            }
        };
        r.write.store(w + frames, Ordering::Release);
        self.written.0 += frames;
        self.written.1 += media;
        // Stretches at the same pace are one: at one times speed, with no resampling, the whole song
        // is a single mark.
        let pace = |from: (u64, f64), to: (u64, f64)| (to.1 - from.1) / (to.0 - from.0).max(1) as f64;
        let before = self.marks.len().checked_sub(2).map_or(self.from, |i| self.marks[i]);
        match self.marks.back_mut() {
            Some(last) if frames > 0 && (pace(before, *last) - media / frames as f64).abs() < 1e-3 => *last = self.written,
            _ => self.marks.push_back(self.written),
        }
    }

    fn played_media(&mut self) -> f64 {
        let (Some(r), Some(d)) = (self.ring.clone(), self.device) else { return 0.0 };
        let latency = self.output.latency_us() * d.rate as u64 / 1_000_000;
        let p = r.read_at().saturating_sub(latency).saturating_sub(self.base);
        self.media_at(p)
    }

    fn is_empty(&self) -> bool {
        self.ring.as_ref().is_none_or(|r| r.filled() == 0)
    }

    fn flush(&mut self) {
        if let Some(r) = &self.ring {
            let w = r.write.load(Ordering::Relaxed);
            r.discard.store(w, Ordering::Release);
            r.ended.store(false, Ordering::Release);
            self.base = w;
        }
        self.marks.clear();
        self.from = (0, 0.0);
        self.written = (0, 0.0);
        if let (Some(f), Some(d)) = (self.format, self.device) {
            if self.resampler.is_some() {
                self.resampler = Resampler::new(f.rate as i32, f.channels as i32, d.rate as i32, d.channels as i32);
            }
        }
    }

    fn play(&mut self) {
        self.playing = true;
        if self.device.is_some() {
            self.output.resume();
        }
    }

    fn pause(&mut self) {
        self.playing = false;
        if self.device.is_some() {
            self.output.pause();
        }
    }
}

impl Drop for RingTrack {
    fn drop(&mut self) {
        self.output.close();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An output that only remembers the feed, for pulling by hand.
    struct Hand(Arc<parking_lot::Mutex<Option<Feed>>>);

    impl AudioOutput for Hand {
        fn open(&mut self, want: OutputFormat) -> Result<OutputFormat, String> {
            Ok(want)
        }
        fn start(&mut self, feed: Feed) -> Result<(), String> {
            *self.0.lock() = Some(feed);
            Ok(())
        }
        fn pause(&mut self) {}
        fn resume(&mut self) {}
        fn latency_us(&self) -> u64 {
            0
        }
        fn close(&mut self) {}
    }

    const F: Format = Format { rate: 1000, channels: 1, encoding: Encoding::Pcm16 };

    fn pcm(v: &[i16]) -> Vec<u8> {
        v.iter().flat_map(|s| s.to_le_bytes()).collect()
    }

    #[test]
    fn the_playhead_follows_song_time_through_a_change_of_pace() {
        let feed = Arc::new(parking_lot::Mutex::new(None));
        let mut t = RingTrack::new(Box::new(Hand(feed.clone())));
        t.open(F);
        // 100 frames standing for 100 of the song, then 100 standing for 200 (twice the speed).
        t.write(&pcm(&[1000; 100]), 100.0);
        t.write(&pcm(&[1000; 100]), 200.0);
        let mut out = vec![0f32; 150];
        assert_eq!(feed.lock().as_mut().unwrap().pull(&mut out), 150);
        assert!((t.played_media() - 200.0).abs() < 1e-9, "{}", t.played_media());
        assert!((out[0] - 1000.0 / 32768.0).abs() < 1e-6);
        t.flush();
        assert_eq!(t.played_media(), 0.0);
        assert!(t.is_empty(), "a flush drops what was unplayed");
        let mut out = vec![1f32; 10];
        assert_eq!(feed.lock().as_mut().unwrap().pull(&mut out), 0);
        assert!(out.iter().all(|&v| v == 0.0), "silence when there is nothing");
    }

    #[test]
    fn a_fade_is_run_by_the_puller() {
        let feed = Arc::new(parking_lot::Mutex::new(None));
        let mut t = RingTrack::new(Box::new(Hand(feed.clone())));
        t.open(F);
        t.write(&pcm(&[16384; 200]), 200.0);
        t.ramp(None, 0.0, 100);
        let mut out = vec![0f32; 200];
        feed.lock().as_mut().unwrap().pull(&mut out);
        assert!((out[0] - 0.5 * 0.99).abs() < 1e-3, "{}", out[0]);
        assert!((out[49] - 0.25).abs() < 1e-2, "half way down: {}", out[49]);
        assert!(out[100..].iter().all(|&v| v == 0.0), "down and staying down");
    }
}
