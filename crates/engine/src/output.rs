//! The sound card's side. A platform writes one small [`AudioOutput`]: open a device, and from the
//! device's own thread call [`Feed::pull`] for every buffer it wants. The feed reads a lock-free ring
//! that one engine thread fills in bursts; pulling never blocks, never allocates and never takes a
//! lock, so the device thread cannot be held up by anything the player does.
//!
//! The ring holds float samples at the device's rate and channel count. The engine's output below the
//! sound chain ([`RingTrack`]) converts the chain's audio (16-bit, or float for high quality output)
//! into it, resampling only when the device would not take the stream's own rate, and keeps the map
//! from ring frames back to song time that the playhead is read through.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::Thread;

pub use nori_player::outputs::OutputKind;

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

/// Where the music goes: the kind of device, and the name it gives itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub kind: OutputKind,
    pub name: String,
}

/// Called by an output whenever the device the music goes to changes (from any thread).
pub type DeviceWatch = Box<dyn Fn(Device) + Send + Sync>;

/// A sound card, or anything that takes the music the way one does (a file). Called only from the
/// engine's thread; the device's own thread only ever calls [`Feed::pull`].
pub trait AudioOutput: Send {
    /// From now on `changed` is told which device the music goes to: once when the output knows, and
    /// again whenever the system moves it (headphones plugged in, a Bluetooth device connected), so
    /// the core can give each device its own sound. An output that cannot tell says nothing.
    fn watch(&mut self, _changed: DeviceWatch) {}
    /// Picks the device's format, as close to `want` as it goes. Nothing plays yet.
    fn open(&mut self, want: OutputFormat) -> Result<OutputFormat, String>;
    /// From now on the device pulls every buffer it plays from `feed`; it starts paused.
    fn start(&mut self, feed: Feed) -> Result<(), String>;
    /// Stops pulling, and lets the device sleep.
    fn pause(&mut self);
    fn resume(&mut self);
    /// How long a sample pulled now takes to be heard, µs.
    fn latency_us(&self) -> u64;
    /// Whether the device plays float samples as they are ([`Feed::pull`]) rather than 16-bit ones
    /// ([`Feed::pull_i16`]). With high quality output on, songs are then decoded and carried to it in
    /// float; otherwise the chain runs in 16 bits, as Android's does without float output.
    fn takes_float(&mut self) -> bool {
        false
    }
    /// The music the ring held was dropped (a seek, a jump): what the device itself still holds of it is
    /// stale too. Called on the engine's thread after the ring let it go; an output with a buffer of its
    /// own worth hearing (seconds, not milliseconds) wakes its thread here, and [`Feed::flushed`] says
    /// where the new music starts.
    fn flush(&mut self) {}
    /// A volume fade from `from` (or wherever the volume is) to `target` over `ms`. An output whose device
    /// holds seconds of music runs it at the device (its own volume), where it is heard when asked for,
    /// and says true; otherwise the ring runs it on the samples it hands out.
    fn ramp(&mut self, _from: Option<f32>, _target: f32, _ms: i64) -> bool {
        false
    }
    /// Whether the device still holds music it took from the ring and has not played: with a buffer of
    /// seconds, the music is not over when the ring runs empty.
    fn holding(&self) -> bool {
        false
    }
    /// Whether the device takes the ring's music in bursts of seconds rather than a steady trickle. The
    /// ring then does not run down between its pulls, so the engine sleeps until the pull that crosses
    /// the low mark wakes it, with no timer guessing when that will be.
    fn bursts(&self) -> bool {
        false
    }
    /// The device stopped taking music and could not be opened again (a sound server that died): asked
    /// by the engine whenever it looks, and woken for with [`Feed::wake_engine`]. The engine then stops,
    /// says so, and lets the output go, so the next play opens a new one where the music was.
    fn failed(&mut self) -> Option<String> {
        None
    }
    /// Lets the device go.
    fn close(&mut self);
}

/// How far above empty the ring is when the engine is woken to fill it: a little under the burst's
/// low mark, so the burst's own count (which includes what the device holds) agrees it is time.
pub const WAKE_LOW_US: i64 = LOW_US - 250_000;
/// The ring's room beyond the sink's deep buffer: the resampler's rounding and a device's first pull.
const SLACK_US: i64 = 2_000_000;
/// Volume changes waiting in the ring at once: one per song start, and twelve seconds of ring hold
/// more than this many songs only when they are a second or two long.
const SWITCHES: usize = 8;

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
    /// The ReplayGain volume, under the fade, and a count that moves when it is set outright.
    level: AtomicU32,
    level_gen: AtomicU32,
    /// Volume changes at ring frames, each song's from the frame it starts at: a small queue the
    /// writer fills before it publishes those frames, so the reader can never play one at the wrong
    /// level. Entries `switch_from..switch_tail` are live (a flush moves `switch_from` on).
    switch_at: [AtomicU64; SWITCHES],
    switch_level: [AtomicU32; SWITCHES],
    switch_tail: AtomicU64,
    switch_from: AtomicU64,
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
            level: AtomicU32::new(1f32.to_bits()),
            level_gen: AtomicU32::new(0),
            switch_at: std::array::from_fn(|_| AtomicU64::new(0)),
            switch_level: std::array::from_fn(|_| AtomicU32::new(0)),
            switch_tail: AtomicU64::new(0),
            switch_from: AtomicU64::new(0),
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
    /// The ReplayGain volume as the reader has it, the setting it came from, and the next volume
    /// change still to reach.
    level: f32,
    level_gen: u32,
    switch_head: u64,
    /// The flush the last pull saw, and whether one happened since [`Feed::flushed`] was last asked.
    seen: u64,
    flushed: bool,
}

impl Feed {
    fn new(ring: Arc<Ring>) -> Feed {
        let level = f32::from_bits(ring.level.load(Ordering::Acquire));
        Feed { ring, gain: 1.0, target: 1.0, step: 0.0, gen: 0, level, level_gen: 0, switch_head: 0, seen: 0, flushed: false }
    }

    /// Whether the ring was flushed since this was last asked, as the pulls found it: the frames the
    /// last pull returned (if any) are then the new music's, and what the device held from before it
    /// should go. A pull never mixes the two.
    pub fn flushed(&mut self) -> bool {
        std::mem::take(&mut self.flushed)
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
        // Read after the write position: a pull that sees music written after a flush sees the flush.
        let discard = r.discard.load(Ordering::Acquire);
        if discard != self.seen {
            self.seen = discard;
            self.flushed = true;
        }
        let at = r.read.load(Ordering::Acquire).max(discard);
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
        let level_gen = r.level_gen.load(Ordering::Acquire);
        if level_gen != self.level_gen {
            self.level_gen = level_gen;
            self.level = f32::from_bits(r.level.load(Ordering::Relaxed));
        }
        // The volume changes queued for frames in the ring; the write position was read first, so
        // every change for a frame up to it is visible too.
        let tail = r.switch_tail.load(Ordering::Acquire);
        self.switch_head = self.switch_head.max(r.switch_from.load(Ordering::Acquire));
        let next = |head: u64| if head < tail { r.switch_at[head as usize % SWITCHES].load(Ordering::Relaxed) } else { u64::MAX };
        let mut switch = next(self.switch_head);
        for i in 0..n {
            while at + i >= switch {
                self.level = f32::from_bits(r.switch_level[self.switch_head as usize % SWITCHES].load(Ordering::Relaxed));
                self.switch_head += 1;
                switch = next(self.switch_head);
            }
            let level = self.level;
            if self.gain != self.target {
                self.gain += self.step;
                if (self.step > 0.0 && self.gain > self.target) || (self.step < 0.0 && self.gain < self.target) || self.step == 0.0 {
                    self.gain = self.target;
                }
            }
            let slot = ((at + i) % r.frames) as usize * ch;
            let o = i as usize * ch;
            for c in 0..ch {
                out[o + c] = conv(f32::from_bits(r.slots[slot + c].load(Ordering::Relaxed)) * self.gain * level);
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

    /// Wakes the engine now, for something the device must tell it at once ([`AudioOutput::failed`]).
    pub fn wake_engine(&self) {
        self.ring.engine.unpark();
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
    /// The ReplayGain volume, kept for a device opened later, and the songs' volumes still to be put
    /// at the frames they start at, by where they start in song time since the flush.
    level: f32,
    starts: VecDeque<(f64, f32)>,
    /// Whether the device plays float, once asked.
    float: Option<bool>,
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
            level: 1.0,
            starts: VecDeque::with_capacity(SWITCHES),
            float: None,
            failed: None,
        }
    }

    /// The device is let go (a long pause); the next stream opens it again.
    pub(crate) fn release(&mut self) {
        if self.device.take().is_some() {
            self.output.close();
        }
        self.ring = None;
        self.format = None;
        self.resampler = None;
        self.marks.clear();
        self.from = (0, 0.0);
        self.written = (0, 0.0);
        self.starts.clear();
    }

    /// Whether the device plays float samples as they are; asked of it once.
    pub(crate) fn takes_float(&mut self) -> bool {
        *self.float.get_or_insert_with(|| self.output.takes_float())
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

    /// The device pulls in bursts ([`AudioOutput::bursts`]).
    pub(crate) fn bursts(&self) -> bool {
        self.output.bursts()
    }

    /// The engine goes to sleep until the ring holds no more than `us` of music.
    pub(crate) fn wake_at(&self, us: i64) {
        if let Some(r) = &self.ring {
            r.low.store((us.max(0) as u64 * r.rate as u64 / 1_000_000) as u64, Ordering::Relaxed);
            r.waiting.store(true, Ordering::Release);
        }
    }

    /// A volume fade from `from` (or wherever the volume is) to `target` over `ms`: run by the device
    /// when it says it does fades itself, otherwise by the device thread from its next pull.
    pub(crate) fn ramp(&mut self, from: Option<f32>, target: f32, ms: i64) {
        if let Some(r) = &self.ring {
            if !self.output.ramp(from, target, ms) {
                r.ramp(from, target, ms);
            }
        }
    }

    /// The ReplayGain volume from the next pull on, under whatever fade is running (the settings
    /// changed); the volumes set for songs still to start stay.
    pub(crate) fn set_level(&mut self, level: f32) {
        self.level = level;
        if let Some(r) = &self.ring {
            r.level.store(level.to_bits(), Ordering::Relaxed);
            r.level_gen.fetch_add(1, Ordering::Release);
        }
    }

    /// Puts the volume changes of songs starting within what is about to be written (`media` more
    /// song frames over `frames` ring frames) into the ring's queue, ahead of the frames themselves.
    fn queue_starts(&mut self, r: &Ring, frames: u64, media: f64) {
        let (at, from) = self.written;
        while let Some(&(start, level)) = self.starts.front() {
            if start > from + media {
                break;
            }
            self.starts.pop_front();
            let k = if media > 0.0 { ((start - from) / media).clamp(0.0, 1.0) } else { 0.0 };
            let frame = self.base + at + (frames as f64 * k).round() as u64;
            let tail = r.switch_tail.load(Ordering::Relaxed);
            let slot = tail as usize % SWITCHES;
            // A slot the reader has not passed yet is never written over.
            if tail >= SWITCHES as u64 && r.switch_at[slot].load(Ordering::Relaxed) >= r.read_at() {
                continue;
            }
            r.switch_at[slot].store(frame, Ordering::Relaxed);
            r.switch_level[slot].store(level.to_bits(), Ordering::Relaxed);
            r.switch_tail.store(tail + 1, Ordering::Release);
        }
    }

    /// Whether the music is over (no gaps are counted past it).
    pub(crate) fn set_ended(&self, ended: bool) {
        if let Some(r) = &self.ring {
            r.ended.store(ended, Ordering::Release);
        }
    }

    /// Why the device failed, once: it would not open, or it stopped taking music and would not open again.
    pub(crate) fn take_failure(&mut self) -> Option<String> {
        self.failed.take().map(|e| format!("the output would not open: {e}")).or_else(|| self.output.failed().map(|e| format!("the output stopped: {e}")))
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
                    ring.level.store(self.level.to_bits(), Ordering::Relaxed);
                    ring.level_gen.fetch_add(1, Ordering::Release);
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
        let floats = |b: &[u8]| f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        let frames = match (self.resampler.as_mut(), f.encoding) {
            (None, Encoding::Pcm16) => put(w, &mut data.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0)),
            (None, Encoding::Float) => put(w, &mut data.chunks_exact(4).map(floats)),
            (Some(rs), _) => {
                let in_frames = data.len() / f.frame_bytes();
                let need = ((in_frames as u64 * d.rate as u64 / f.rate as u64) as usize + 4) * ch * 4;
                if self.converted.len() < need {
                    self.converted.resize(need, 0);
                }
                let Some((_, made)) = rs.process(data, f.encoding.media3(), &mut self.converted, Encoding::FLOAT) else { return };
                put(w, &mut self.converted[..made].chunks_exact(4).map(floats))
            }
        };
        // The volume of a song starting in these frames is queued before the frames are published.
        if !self.starts.is_empty() {
            self.queue_starts(&r, frames, media);
        }
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
        self.ring.as_ref().is_none_or(|r| r.filled() == 0) && !self.output.holding()
    }

    fn flush(&mut self) {
        if let Some(r) = &self.ring {
            let w = r.write.load(Ordering::Relaxed);
            r.discard.store(w, Ordering::Release);
            r.ended.store(false, Ordering::Release);
            r.switch_from.store(r.switch_tail.load(Ordering::Relaxed), Ordering::Release);
            self.base = w;
            self.output.flush();
        }
        self.marks.clear();
        self.from = (0, 0.0);
        self.written = (0, 0.0);
        self.starts.clear();
        if let (Some(f), Some(d)) = (self.format, self.device) {
            if self.resampler.is_some() {
                self.resampler = Resampler::new(f.rate as i32, f.channels as i32, d.rate as i32, d.channels as i32);
            }
        }
    }

    fn song_starts(&mut self, media: f64, level: f32) {
        // More songs waiting to start than the queue holds happens only with songs of a second or two.
        if self.starts.len() < SWITCHES {
            self.starts.push_back((media, level));
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
    fn a_pull_says_when_the_music_before_it_was_flushed() {
        let feed = Arc::new(parking_lot::Mutex::new(None));
        let mut t = RingTrack::new(Box::new(Hand(feed.clone())));
        t.open(F);
        t.write(&pcm(&[1000; 100]), 100.0);
        let mut f = feed.lock().take().unwrap();
        let mut out = vec![0f32; 10];
        f.pull(&mut out);
        assert!(!f.flushed(), "nothing was dropped yet");
        t.flush();
        t.write(&pcm(&[-1000; 100]), 100.0);
        assert_eq!(f.pull(&mut out), 10);
        assert!(f.flushed(), "the pull after a flush says so");
        assert!(out.iter().all(|&v| v < 0.0), "and holds only the new music");
        f.pull(&mut out);
        assert!(!f.flushed(), "once");
    }

    #[test]
    fn a_songs_volume_is_queued_before_its_frames_are_there_to_be_pulled() {
        let feed = Arc::new(parking_lot::Mutex::new(None));
        let mut t = RingTrack::new(Box::new(Hand(feed.clone())));
        t.open(F);
        t.song_starts(0.0, 1.0);
        t.write(&pcm(&[16384; 100]), 100.0);
        // The next song starts 50 frames into the next write, and is known before it is written.
        t.song_starts(150.0, 0.5);
        t.write(&pcm(&[16384; 100]), 100.0);
        t.song_starts(250.0, 0.25);
        let mut out = vec![0f32; 300];
        let mut f = feed.lock();
        let f = f.as_mut().unwrap();
        // Pulled in pieces that straddle the change, as a device's periods do.
        assert_eq!(f.pull(&mut out[..120]), 120);
        assert_eq!(f.pull(&mut out[120..]), 80);
        assert!(out[..150].iter().all(|&v| v == 0.5), "the first song at full volume");
        assert!(out[150..200].iter().all(|&v| v == 0.25), "the next at its own, from its very first frame");
        t.write(&pcm(&[16384; 100]), 100.0);
        assert_eq!(f.pull(&mut out[..100]), 100);
        assert!(out[..50].iter().all(|&v| v == 0.25) && out[50..100].iter().all(|&v| v == 0.125), "and the one after");
        // A flush drops what was queued for frames that will never play.
        t.song_starts(400.0, 0.1);
        t.flush();
        t.write(&pcm(&[16384; 100]), 100.0);
        assert_eq!(f.pull(&mut out[..100]), 100);
        assert!(out[..100].iter().all(|&v| v == 0.125));
        t.set_level(1.0);
        t.write(&pcm(&[16384; 10]), 10.0);
        f.pull(&mut out[..10]);
        assert_eq!(out[0], 0.5, "the settings changed: at once");
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
