//! nori-engine's output on Android (`nori_engine::AudioOutput`): the engine's ring poured into an
//! AudioTrack in bursts, the way the ExoPlayer path's sink feeds its deep AudioTrack buffer.
//!
//! One thread of its own, which sleeps between bursts. The track holds one of the engine's bursts and a
//! little more ([`TRACK_US`]); the thread wakes when about a second of it is left ([`LOW_US`]) and moves
//! everything the ring has into it in one go, which runs the ring past its own low mark, so the engine
//! is woken in the same moment to decode the next burst. While music plays the two wake together about
//! every ten seconds, and nothing else of this output runs; the engine keeps no timer of its own for the
//! ring (`AudioOutput::bursts`), since between top-ups it does not run down.
//!
//! The thread wakes only for:
//! - the track running down to its low mark: one timed sleep, computed from the track's clock;
//! - a command from the engine (play, pause, a flush, a fade): the engine unparks it;
//! - filling the track after a start, a resume or a flush, every 20 ms until it is full (a fraction of a
//!   second while the engine decodes its first burst), backing off to a second while the engine has
//!   nothing yet (the network is slow);
//! - a fade, every 16 ms while it runs (`nori_player::transport::FADE_TICK_MS`);
//! - the device's clock settling after a start: twice in the first second, so the playhead is the
//!   device's own and not a guess;
//! - the end of the music: once, when the track has played its last frame.
//!
//! Paused, or at the end, it sleeps until the engine says something.
//!
//! With seconds of music inside the track, what the ring does to samples as they are pulled is heard
//! seconds later. So the fades run at the track's volume (the engine hands them over through
//! `AudioOutput::ramp`), and a flush empties the track as well as the ring (`AudioOutput::flush`, with
//! `Feed::flushed` saying which pull holds the new music). ReplayGain stays in the ring: it changes on
//! the first sample of a song, which is where the pull puts it.
//!
//! No JNI here: the AudioTrack is a [`Sink`], so the tests below run the whole thing on a simulated
//! track and a virtual clock. `player.rs` has the real one.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread::{JoinHandle, Thread};
use std::time::Duration;

use nori_engine::{AudioOutput, DeviceWatch, Feed, OutputFormat};
use nori_player::burst::BUFFER_US;
use nori_player::transport::{fade_step, FADE_TICK_MS};
use parking_lot::Mutex;

/// The track is topped up again when this much music is left in it.
pub(crate) const LOW_US: i64 = 1_000_000;
/// How much the track holds: one of the engine's bursts, the low mark, and half a second for what the
/// ear moves on while a burst is decoded. A top-up then takes all the ring has, so the ring always runs
/// down past its own low mark and wakes the engine at the same moment: the engine has no timer of its
/// own for it (`AudioOutput::bursts`).
pub(crate) const TRACK_US: i64 = BUFFER_US + LOW_US + 500_000;
/// The samples moved per write, in bytes.
pub(crate) const CHUNK_BYTES: usize = 128 * 1024;
/// How often the track is filled while it is being filled after a start or a flush.
const FILL_TICK_MS: u64 = 20;
/// The longest the filling waits between looks while the engine has nothing to give.
const STARVED_MAX_MS: u64 = 1_000;
/// After a start, when the device's clock is read again: its first readings come late.
const SETTLE_MS: [i64; 2] = [250, 1_000];

/// What the output needs of an AudioTrack.
pub(crate) trait Sink: Send {
    /// Where samples are put before they are written: the same memory for the sink's whole life,
    /// [`CHUNK_BYTES`] long.
    fn staging(&mut self) -> &mut [f32];
    /// Writes bytes `from..from + len` of the staging memory, as much as fits without waiting. The
    /// bytes taken.
    fn write(&mut self, from: usize, len: usize) -> usize;
    fn play(&mut self);
    fn pause(&mut self);
    /// Drops what was written and not yet played. Only while paused or stopped.
    fn flush(&mut self);
    /// Plays what was written to its end, then stops: for a track that would otherwise wait to be
    /// full before it starts, at the end of the music.
    fn stop(&mut self);
    fn set_volume(&mut self, volume: f32);
    /// Frames heard since the last flush, and the monotonic time (ns) that was true at; none while the
    /// device cannot say.
    fn heard(&mut self, playing: bool) -> Option<(u64, i64)>;
    fn release(&mut self);
}

/// A sink as it was opened: how many frames its buffer holds, and whether it only starts once that
/// buffer is full (Android before 12, which has no start threshold).
pub(crate) struct Opened {
    pub sink: Box<dyn Sink>,
    pub frames: u64,
    pub starts_full: bool,
}

/// Opens the device: `frames` is the buffer asked for.
pub(crate) trait Opener: Send {
    fn open(&mut self, format: OutputFormat, float: bool, frames: u64) -> Result<Opened, String>;
}

/// The engine's end of the ring as the writer uses it: `nori_engine::Feed`, or a simulated one.
pub(crate) trait Ring: Send {
    fn available(&self) -> usize;
    fn pull(&mut self, out: &mut [f32]) -> usize;
    fn pull_i16(&mut self, out: &mut [i16]) -> usize;
    fn flushed(&mut self) -> bool;
    fn ending(&self) -> bool;
}

impl Ring for Feed {
    fn available(&self) -> usize {
        Feed::available(self)
    }
    fn pull(&mut self, out: &mut [f32]) -> usize {
        Feed::pull(self, out)
    }
    fn pull_i16(&mut self, out: &mut [i16]) -> usize {
        Feed::pull_i16(self, out)
    }
    fn flushed(&mut self) -> bool {
        Feed::flushed(self)
    }
    fn ending(&self) -> bool {
        Feed::ending(self)
    }
}

/// CLOCK_MONOTONIC in ns: the clock the device's timestamps are on (Java's `System.nanoTime`).
// Both fields are 32 bits on a 32-bit ABI.
#[allow(clippy::unnecessary_cast)]
pub(crate) fn mono_ns() -> i64 {
    let mut t = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a plain system call writing into the struct handed to it.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut t) };
    t.tv_sec as i64 * 1_000_000_000 + t.tv_nsec as i64
}

/// The track's clock: moved by the writer, read by the engine (through `latency_us`) and by the
/// screen, from any thread.
#[derive(Default)]
pub(crate) struct Clock(Mutex<Counts>);

#[derive(Default, Clone, Copy)]
struct Counts {
    rate: u32,
    /// Frames pulled for the track since the last flush, written or about to be.
    ahead: u64,
    /// Frames the track took since the last flush.
    given: u64,
    /// Frames heard at `at_ns`, and whether the count moves on from there.
    heard: u64,
    at_ns: i64,
    running: bool,
}

impl Counts {
    fn heard_at(&self, now_ns: i64) -> u64 {
        let moved = if self.running && self.rate > 0 { ((now_ns - self.at_ns).max(0) as i128 * self.rate as i128 / 1_000_000_000) as u64 } else { 0 };
        (self.heard + moved).min(self.given)
    }
}

impl Clock {
    /// Frames between what was pulled and what has been heard: how far behind the ring the ear is.
    pub(crate) fn latency_frames(&self, now_ns: i64) -> u64 {
        let c = *self.0.lock();
        c.ahead.saturating_sub(c.heard_at(now_ns))
    }

    pub(crate) fn latency_us(&self, now_ns: i64) -> u64 {
        let rate = self.0.lock().rate.max(1) as u64;
        self.latency_frames(now_ns) * 1_000_000 / rate
    }

    /// Frames waiting in the track (or pulled for it) and not yet heard.
    fn fill(&self, now_ns: i64) -> u64 {
        self.latency_frames(now_ns)
    }

    fn update(&self, f: impl FnOnce(&mut Counts)) {
        f(&mut self.0.lock());
    }

    /// The count stops where it is now (a pause asked for).
    fn freeze(&self, now_ns: i64) {
        self.update(|c| {
            c.heard = c.heard_at(now_ns);
            c.at_ns = now_ns;
            c.running = false;
        });
    }

    /// The count moves on from here (the track was started).
    fn run(&self, now_ns: i64) {
        self.update(|c| {
            c.heard = c.heard_at(now_ns);
            c.at_ns = now_ns;
            c.running = true;
        });
    }

    /// What the device says it has played, taken over when it makes sense.
    fn anchor(&self, frames: u64, at_ns: i64, running: bool) {
        self.update(|c| {
            if frames <= c.given {
                c.heard = frames;
                c.at_ns = at_ns;
                c.running = running;
            }
        });
    }
}

/// What the engine asked of the writer since it last looked.
#[derive(Default)]
pub(crate) struct Control {
    pub playing: bool,
    pub ramp: Option<(Option<f32>, f32, i64)>,
    pub stop: bool,
}

#[derive(Clone, Copy)]
struct Fade {
    from: f32,
    to: f32,
    start_ms: i64,
    ms: i32,
}

/// The writer thread's state: what moves music from the ring into the track, one step per wake.
pub(crate) struct Writer<R: Ring> {
    ring: R,
    sink: Box<dyn Sink>,
    clock: Arc<Clock>,
    bytes: Arc<AtomicU64>,
    channels: usize,
    rate: u32,
    float: bool,
    capacity: u64,
    low: u64,
    starts_full: bool,
    /// Bytes at the start of the staging memory the track did not take yet.
    staged: (usize, usize),
    playing: bool,
    /// Filling the track to full: after a start, a resume or a flush.
    filling: bool,
    /// Stopped at the end of the music, the track playing out what it has.
    drained: bool,
    volume: f32,
    fade: Option<Fade>,
    /// When the track was last started, and how many of the settling readings were taken since.
    started_ns: i64,
    settled: usize,
    /// How long the next look waits while the ring has nothing to give.
    starved_ms: u64,
}

impl<R: Ring> Writer<R> {
    pub(crate) fn new(ring: R, opened: Opened, format: OutputFormat, float: bool, clock: Arc<Clock>, bytes: Arc<AtomicU64>) -> Writer<R> {
        let rate = format.rate;
        clock.update(|c| *c = Counts { rate, ..Counts::default() });
        let low = (rate as i64 * LOW_US / 1_000_000) as u64;
        Writer {
            ring,
            sink: opened.sink,
            clock,
            bytes,
            channels: format.channels,
            rate,
            float,
            capacity: opened.frames.max(low * 2),
            low,
            starts_full: opened.starts_full,
            staged: (0, 0),
            playing: false,
            filling: true,
            drained: false,
            volume: 1.0,
            fade: None,
            started_ns: 0,
            settled: SETTLE_MS.len(),
            starved_ms: FILL_TICK_MS,
        }
    }

    fn frame_bytes(&self) -> usize {
        self.channels * if self.float { 4 } else { 2 }
    }

    /// One wake: does what the engine asked and what the track needs, and says how long to sleep
    /// (ms; `None` until the engine says something).
    pub(crate) fn step(&mut self, now_ns: i64, c: &mut Control) -> Option<u64> {
        let now_ms = now_ns / 1_000_000;
        let mut wake: Option<u64> = None;
        let mut at = |ms: u64| wake = Some(wake.map_or(ms, |w| w.min(ms)));
        if let Some((from, to, ms)) = c.ramp.take() {
            self.fade = Some(Fade { from: from.unwrap_or(self.volume), to, start_ms: now_ms, ms: ms.clamp(0, i32::MAX as i64) as i32 });
        }
        if let Some(f) = self.fade {
            let (v, done) = fade_step(f.from, f.to, f.start_ms, now_ms, f.ms);
            self.sink.set_volume(v);
            self.volume = v;
            if done {
                self.fade = None;
            } else {
                at(FADE_TICK_MS as u64);
            }
        }
        if c.playing != self.playing {
            self.playing = c.playing;
            log(if self.playing { "plays" } else { "pauses" });
            if self.playing {
                self.start(now_ns);
            } else {
                self.sink.pause();
                self.clock.freeze(now_ns);
                if let Some((frames, ns)) = self.sink.heard(false) {
                    self.clock.anchor(frames, ns, false);
                }
            }
        }
        // A flush shows in the next pull, even one that takes nothing: the track follows it at once.
        self.ring.pull(&mut []);
        if self.ring.flushed() {
            self.restart(now_ns);
        }
        if !self.playing {
            return wake;
        }
        if self.settled < SETTLE_MS.len() {
            let due = self.started_ns + SETTLE_MS[self.settled] * 1_000_000;
            if now_ns >= due {
                self.settled += 1;
                self.read_clock();
            }
            if let Some(ms) = SETTLE_MS.get(self.settled) {
                at(((self.started_ns + ms * 1_000_000 - now_ns).max(0) / 1_000_000) as u64 + 1);
            }
        }
        if let Some(ms) = self.fill(now_ns) {
            at(ms);
        }
        wake
    }

    /// The track starts (or starts again): filled to full first, the clock from the device's word.
    fn start(&mut self, now_ns: i64) {
        self.sink.play();
        self.clock.run(now_ns);
        self.filling = true;
        self.started_ns = now_ns;
        self.settled = 0;
        self.starved_ms = FILL_TICK_MS;
    }

    /// The ring was flushed: so is the track, and it fills again from the new music.
    fn restart(&mut self, now_ns: i64) {
        log("emptied for the music that follows");
        self.sink.pause();
        self.sink.flush();
        self.staged = (0, 0);
        self.drained = false;
        self.clock.update(|c| {
            c.ahead = 0;
            c.given = 0;
            c.heard = 0;
            c.at_ns = now_ns;
            c.running = false;
        });
        if self.playing {
            self.start(now_ns);
        } else {
            self.filling = true;
        }
    }

    fn read_clock(&mut self) {
        if let Some((frames, ns)) = self.sink.heard(self.playing) {
            self.clock.anchor(frames, ns, self.playing);
        }
    }

    /// Tops the track up when it is due, and says when to look again.
    fn fill(&mut self, now_ns: i64) -> Option<u64> {
        let ms = |frames: u64, rate: u32| frames * 1000 / rate.max(1) as u64;
        let fill = self.clock.fill(now_ns);
        if self.drained {
            if self.ring.available() == 0 {
                return None;
            }
            // More music after the end was drained (the queue grew in its last seconds): the stopped
            // track is started again from nothing, all of the old music having been heard.
            self.restart(now_ns);
        } else if !self.filling && fill > self.low {
            return Some(ms(fill - self.low, self.rate) + 1);
        }
        self.read_clock();
        let full = self.top_up(now_ns);
        let fill = self.clock.fill(now_ns);
        if full {
            self.filling = false;
            self.starved_ms = FILL_TICK_MS;
            return Some(ms(fill.saturating_sub(self.low), self.rate) + 1);
        }
        if self.ring.ending() && self.ring.available() == 0 {
            // The last of the music is in the track. One that starts only when full is told to play
            // what it has; either way the next look is when it has played out.
            self.filling = false;
            if self.starts_full && !self.drained {
                self.sink.stop();
                self.drained = true;
            }
            return Some(ms(fill, self.rate) + 1);
        }
        // The ring had less than the track has room for. Between bursts that is only the engine decoding
        // the next one, and the track has plenty; while filling, or low, the engine is still decoding its
        // first burst or waiting for the network: look again soon, less often the longer it takes.
        if !self.filling && fill > self.low {
            return Some(ms(fill - self.low, self.rate) + 1);
        }
        let wait = self.starved_ms;
        self.starved_ms = (self.starved_ms * 2).min(STARVED_MAX_MS);
        Some(wait)
    }

    /// Moves what the ring has into the track, as much as it has room for. True when the track is
    /// full.
    fn top_up(&mut self, now_ns: i64) -> bool {
        let fb = self.frame_bytes();
        let chunk_frames = CHUNK_BYTES / fb;
        loop {
            if self.staged.1 > 0 && !self.write_staged() {
                return true;
            }
            let room = self.capacity.saturating_sub(self.clock.fill(now_ns));
            if room == 0 {
                return true;
            }
            let n = (room as usize).min(self.ring.available()).min(chunk_frames);
            if n == 0 {
                return false;
            }
            // Counted before the pull: the ring's read position moves in it, and the engine must never
            // see the ear ahead of where it is.
            self.clock.update(|c| c.ahead += n as u64);
            let samples = n * self.channels;
            let got = if self.float {
                self.ring.pull(&mut self.sink.staging()[..samples])
            } else {
                let staging = self.sink.staging();
                // SAFETY: the staging memory is f32s, so it is aligned for i16 and twice as many of them
                // fit; the slice lives no longer than the borrow of the staging it was made from.
                let halves = unsafe { std::slice::from_raw_parts_mut(staging.as_mut_ptr() as *mut i16, staging.len() * 2) };
                self.ring.pull_i16(&mut halves[..samples])
            };
            if self.ring.flushed() {
                // The pull began the new music: what the track held of the old goes.
                self.restart(now_ns);
                self.clock.update(|c| c.ahead = got as u64);
            } else if got < n {
                self.clock.update(|c| c.ahead -= (n - got) as u64);
            }
            if got == 0 {
                return false;
            }
            self.staged = (0, got * fb);
            if !self.write_staged() {
                return true;
            }
        }
    }

    /// Writes what is staged. False when the track would not take all of it (it is full).
    fn write_staged(&mut self) -> bool {
        let (from, len) = self.staged;
        let taken = self.sink.write(from, len).min(len);
        let fb = self.frame_bytes();
        self.clock.update(|c| c.given += (taken / fb) as u64);
        self.bytes.fetch_add(taken as u64, Ordering::Relaxed);
        self.staged = if taken < len { (from + taken, len - taken) } else { (0, 0) };
        taken == len
    }
}

/// Shared by the output (on the engine's thread), its writer thread and the app's doors.
#[derive(Default)]
pub(crate) struct Shared {
    control: Mutex<Control>,
    writer: Mutex<Option<Thread>>,
    pub clock: Arc<Clock>,
    /// Bytes handed to the track since the output was made, for the test bridge.
    pub bytes: Arc<AtomicU64>,
    /// Told whenever the track's route changes.
    pub watch: Mutex<Option<DeviceWatch>>,
}

impl Shared {
    fn tell(&self, f: impl FnOnce(&mut Control)) {
        f(&mut self.control.lock());
        if let Some(t) = &*self.writer.lock() {
            t.unpark();
        }
    }
}

/// The engine's output over an AudioTrack.
pub(crate) struct TrackOutput {
    opener: Box<dyn Opener>,
    float: bool,
    shared: Arc<Shared>,
    format: Option<OutputFormat>,
    thread: Option<JoinHandle<()>>,
}

impl TrackOutput {
    /// `float` is the high quality output setting: the track takes float samples, else 16-bit ones.
    pub(crate) fn new(opener: Box<dyn Opener>, float: bool, shared: Arc<Shared>) -> TrackOutput {
        TrackOutput { opener, float, shared, format: None, thread: None }
    }
}

impl AudioOutput for TrackOutput {
    fn watch(&mut self, changed: DeviceWatch) {
        *self.shared.watch.lock() = Some(changed);
    }

    /// The song's own rate as it is (a DAC asked for bit-perfect gets exactly that), in mono or stereo.
    fn open(&mut self, want: OutputFormat) -> Result<OutputFormat, String> {
        let f = OutputFormat { rate: want.rate.clamp(8_000, 192_000), channels: want.channels.clamp(1, 2) };
        self.format = Some(f);
        Ok(f)
    }

    fn start(&mut self, feed: Feed) -> Result<(), String> {
        let format = self.format.ok_or("the output was not opened")?;
        self.close();
        let frames = (format.rate as i64 * TRACK_US / 1_000_000) as u64;
        let opened = self.opener.open(format, self.float, frames).inspect_err(|e| log(&format!("the AudioTrack would not open: {e}")))?;
        *self.shared.control.lock() = Control::default();
        let writer = Writer::new(feed, opened, format, self.float, self.shared.clock.clone(), self.shared.bytes.clone());
        let shared = self.shared.clone();
        let t = std::thread::Builder::new().name("nori-track".into()).spawn(move || run(writer, shared)).map_err(|e| e.to_string())?;
        *self.shared.writer.lock() = Some(t.thread().clone());
        self.thread = Some(t);
        Ok(())
    }

    fn pause(&mut self) {
        self.shared.clock.freeze(mono_ns());
        self.shared.tell(|c| c.playing = false);
    }

    fn resume(&mut self) {
        self.shared.tell(|c| c.playing = true);
    }

    fn latency_us(&self) -> u64 {
        self.shared.clock.latency_us(mono_ns())
    }

    /// Android's AudioTrack takes float at any rate; whether it gets it is the setting's.
    fn takes_float(&mut self) -> bool {
        true
    }

    fn flush(&mut self) {
        self.shared.tell(|_| {});
    }

    fn ramp(&mut self, from: Option<f32>, target: f32, ms: i64) -> bool {
        self.shared.tell(|c| c.ramp = Some((from, target, ms)));
        true
    }

    /// Seconds of music sit in the track after the ring has run empty: the end is when it has played them.
    fn holding(&self) -> bool {
        self.shared.clock.latency_frames(mono_ns()) > 0
    }

    /// The ring is emptied into the track every ten seconds or so, and stands still in between.
    fn bursts(&self) -> bool {
        true
    }

    fn close(&mut self) {
        if let Some(t) = self.thread.take() {
            self.shared.tell(|c| c.stop = true);
            let _ = t.join();
            *self.shared.writer.lock() = None;
        }
    }
}

impl Drop for TrackOutput {
    fn drop(&mut self) {
        self.close();
    }
}

/// One line in the app's log.
fn log(message: &str) {
    nori_core::alog::info(&format!("rust track: {message}"));
}

fn run<R: Ring>(mut w: Writer<R>, shared: Arc<Shared>) {
    loop {
        let mut c = {
            let mut g = shared.control.lock();
            Control { playing: g.playing, ramp: g.ramp.take(), stop: g.stop }
        };
        if c.stop {
            log("released");
            w.sink.release();
            return;
        }
        match w.step(mono_ns(), &mut c) {
            Some(ms) => std::thread::park_timeout(Duration::from_millis(ms)),
            None => std::thread::park(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nori_player::burst::LOW_US as RING_LOW_US;

    const RATE: u32 = 48_000;
    const MS: i64 = 1_000_000;

    /// An AudioTrack played on a virtual clock: what was written plays at the rate while it is started,
    /// once it is full when it starts only full.
    #[derive(Default)]
    struct Track {
        capacity: u64,
        starts_full: bool,
        buffered: u64,
        played: u64,
        started: bool,
        ready: bool,
        stopping: bool,
        volume: f32,
        flushes: u32,
        stops: u32,
        underruns: u32,
        written_bytes: u64,
        /// The value of the last sample written, to tell the music before a flush from the music after.
        last: f32,
    }

    struct FakeSink {
        track: Arc<Mutex<Track>>,
        staging: Vec<f32>,
        float: bool,
        now: Arc<AtomicU64>,
    }

    impl Sink for FakeSink {
        fn staging(&mut self) -> &mut [f32] {
            &mut self.staging
        }
        fn write(&mut self, from: usize, len: usize) -> usize {
            let mut t = self.track.lock();
            let fb = if self.float { 8 } else { 4 };
            let frames = ((t.capacity - t.buffered) as usize).min(len / fb);
            if frames > 0 {
                // SAFETY: the staging memory is f32s, aligned for i16, and `from` lies inside it.
                let sample: i16 = unsafe { *(self.staging.as_ptr() as *const i16).add(from / 2) };
                t.last = if self.float { self.staging[from / 4] } else { f32::from(sample) / 32767.0 };
            }
            t.buffered += frames as u64;
            t.written_bytes += (frames * fb) as u64;
            frames * fb
        }
        fn play(&mut self) {
            let mut t = self.track.lock();
            t.started = true;
            t.stopping = false;
            t.ready = !t.starts_full || t.buffered >= t.capacity;
        }
        fn pause(&mut self) {
            let mut t = self.track.lock();
            t.started = false;
            t.ready = false;
        }
        fn flush(&mut self) {
            let mut t = self.track.lock();
            assert!(!t.started, "a track is only flushed paused");
            t.buffered = 0;
            t.played = 0;
            t.flushes += 1;
        }
        fn stop(&mut self) {
            let mut t = self.track.lock();
            t.stopping = true;
            t.stops += 1;
        }
        fn set_volume(&mut self, volume: f32) {
            self.track.lock().volume = volume;
        }
        fn heard(&mut self, _playing: bool) -> Option<(u64, i64)> {
            Some((self.track.lock().played, self.now.load(Ordering::Relaxed) as i64))
        }
        fn release(&mut self) {}
    }

    impl Track {
        fn advance(&mut self, frames: u64) {
            if !self.started {
                return;
            }
            if !self.ready && (self.buffered >= self.capacity || self.stopping) {
                self.ready = true;
            }
            if !self.ready {
                return;
            }
            let n = frames.min(self.buffered);
            if n < frames && !self.stopping {
                self.underruns += 1;
            }
            self.buffered -= n;
            self.played += n;
        }
    }

    /// The engine's ring, simulated: the engine decodes a whole burst into it the moment a pull runs it
    /// down to its low mark, as `nori_engine`'s does, until `left` frames of music have been decoded.
    struct FakeRing {
        available: usize,
        left: u64,
        low: usize,
        burst: usize,
        refills: u32,
        flushed: bool,
        value: f32,
    }

    impl FakeRing {
        fn new(music_s: u64) -> FakeRing {
            let low = (RATE as i64 * (RING_LOW_US - 250_000) / 1_000_000) as usize;
            // A burst is ten seconds counted from the ear, which moves on while it is decoded: a little more.
            let burst = (RATE as i64 * (BUFFER_US + 200_000) / 1_000_000) as usize;
            let mut r = FakeRing { available: 0, left: music_s * RATE as u64, low, burst, refills: 0, flushed: false, value: 0.5 };
            r.refill();
            r
        }

        fn refill(&mut self) {
            let n = (self.burst as u64).min(self.left) as usize;
            self.left -= n as u64;
            self.available += n;
            self.refills += 1;
        }

        fn take(&mut self, frames: usize) -> usize {
            let n = frames.min(self.available);
            self.available -= n;
            if self.available <= self.low && self.left > 0 {
                self.refill();
            }
            n
        }

        /// A seek: what the ring held goes, and a burst of other music comes.
        fn flush(&mut self, value: f32) {
            self.available = 0;
            self.flushed = true;
            self.value = value;
            self.refill();
        }
    }

    impl Ring for Arc<Mutex<FakeRing>> {
        fn available(&self) -> usize {
            self.lock().available
        }
        fn pull(&mut self, out: &mut [f32]) -> usize {
            let mut r = self.lock();
            let n = r.take(out.len() / 2);
            out[..n * 2].fill(r.value);
            n
        }
        fn pull_i16(&mut self, out: &mut [i16]) -> usize {
            let mut r = self.lock();
            let n = r.take(out.len() / 2);
            out[..n * 2].fill((r.value * 32767.0) as i16);
            n
        }
        fn flushed(&mut self) -> bool {
            std::mem::take(&mut self.lock().flushed)
        }
        fn ending(&self) -> bool {
            let r = self.lock();
            r.left == 0
        }
    }

    struct Sim {
        writer: Writer<Arc<Mutex<FakeRing>>>,
        ring: Arc<Mutex<FakeRing>>,
        track: Arc<Mutex<Track>>,
        clock: Arc<Clock>,
        now: Arc<AtomicU64>,
        control: Control,
        wakes: u32,
        next: Option<i64>,
    }

    impl Sim {
        fn new(music_s: u64, float: bool, starts_full: bool) -> Sim {
            let ring = Arc::new(Mutex::new(FakeRing::new(music_s)));
            let capacity = (RATE as i64 * TRACK_US / 1_000_000) as u64;
            let track = Arc::new(Mutex::new(Track { capacity, starts_full, volume: 1.0, ..Track::default() }));
            let now = Arc::new(AtomicU64::new(1_000 * MS as u64));
            let sink = FakeSink { track: track.clone(), staging: vec![0.0; CHUNK_BYTES / 4], float, now: now.clone() };
            let clock = Arc::new(Clock::default());
            let format = OutputFormat { rate: RATE, channels: 2 };
            let opened = Opened { sink: Box::new(sink), frames: capacity, starts_full };
            let writer = Writer::new(ring.clone(), opened, format, float, clock.clone(), Arc::new(AtomicU64::new(0)));
            Sim { writer, ring, track, clock, now, control: Control::default(), wakes: 0, next: Some(0) }
        }

        fn now(&self) -> i64 {
            self.now.load(Ordering::Relaxed) as i64
        }

        /// The writer wakes now.
        fn wake(&mut self) {
            self.wakes += 1;
            let now = self.now();
            self.next = self.writer.step(now, &mut self.control).map(|ms| now + ms as i64 * MS);
        }

        /// Runs the virtual clock for `ms`, the writer waking whenever it asked to.
        fn run(&mut self, ms: i64) {
            let end = self.now() + ms * MS;
            loop {
                let to = self.next.map_or(end, |n| n.min(end));
                let step = (to - self.now()).max(0);
                self.track.lock().advance((step as u128 * RATE as u128 / 1_000_000_000) as u64);
                self.now.store(to as u64, Ordering::Relaxed);
                if self.next.is_some_and(|n| n <= to) {
                    self.wake();
                } else {
                    break;
                }
            }
        }

        fn play(&mut self) {
            self.control.playing = true;
            self.wake();
        }
    }

    #[test]
    fn while_music_plays_the_track_is_topped_up_about_every_ten_seconds_with_the_engine_woken_at_once() {
        let mut s = Sim::new(600, false, false);
        s.play();
        s.run(5_000);
        let (wakes, refills) = (s.wakes, s.ring.lock().refills);
        s.run(120_000);
        let t = s.track.lock();
        assert_eq!(t.underruns, 0, "never runs dry");
        let wakes = s.wakes - wakes;
        let refills = s.ring.lock().refills - refills;
        assert!((11..=14).contains(&wakes), "the writer woke {wakes} times in two minutes");
        assert!((11..=14).contains(&refills), "the engine decoded {refills} bursts in two minutes");
        // Every top-up took the whole ring past its low mark: each woke the engine for its next burst, so it
        // needs no timer of its own.
        assert_eq!(refills, wakes, "one burst decoded for every top-up");
    }

    #[test]
    fn a_start_fills_the_track_before_anything_else_and_then_sleeps() {
        let mut s = Sim::new(600, false, true);
        s.play();
        s.run(1_500);
        let t = s.track.lock();
        assert_eq!(t.buffered + t.played, t.capacity, "filled to full, so a track that waits for that starts");
        assert!(t.ready && t.played > 0, "and it plays");
        drop(t);
        // One wake to fill it, two to read the device's clock as it settles.
        assert!(s.wakes <= 4, "the first second and a half took {} wakes", s.wakes);
        let next = s.next.unwrap() - s.now();
        assert!(next > 8_000 * MS, "then it sleeps until the track is low: {} ms", next / MS);
    }

    #[test]
    fn the_clock_says_how_far_behind_the_ring_the_ear_is() {
        let mut s = Sim::new(600, true, false);
        s.play();
        s.run(3_333);
        let t = s.track.lock();
        let latency = s.clock.latency_frames(s.now());
        assert!(latency.abs_diff(t.buffered) <= RATE as u64 / 1000, "latency {latency} frames, {} in the track", t.buffered);
        assert_eq!(t.written_bytes, (t.buffered + t.played) * 8, "float stereo: eight bytes a frame");
    }

    #[test]
    fn a_flush_empties_the_track_and_it_fills_with_the_new_music() {
        let mut s = Sim::new(600, false, false);
        s.play();
        s.run(4_000);
        s.ring.lock().flush(-0.25);
        // The engine's flush wakes the writer.
        s.wake();
        let t = s.track.lock();
        assert_eq!(t.flushes, 1);
        assert!(t.last < 0.0, "what the track holds now is the new music");
        assert_eq!(t.played, 0, "counted again from the flush");
        assert_eq!(s.clock.latency_frames(s.now()), t.buffered, "and the clock with it");
    }

    #[test]
    fn paused_it_writes_nothing_and_sleeps_until_told() {
        let mut s = Sim::new(600, false, false);
        s.play();
        s.run(2_000);
        s.control.playing = false;
        s.wake();
        assert_eq!(s.next, None, "paused, nothing is due");
        let (written, latency) = (s.track.lock().written_bytes, s.clock.latency_frames(s.now()));
        s.now.fetch_add(60_000 * MS as u64, Ordering::Relaxed);
        assert_eq!(s.clock.latency_frames(s.now()), latency, "the clock stands still");
        s.wake();
        assert_eq!(s.track.lock().written_bytes, written);
        assert_eq!(s.next, None);
    }

    #[test]
    fn a_fade_runs_at_the_track_s_volume_and_ticks_only_while_it_lasts() {
        let mut s = Sim::new(600, false, false);
        s.play();
        s.run(1_000);
        let wakes = s.wakes;
        s.control.ramp = Some((None, 0.0, 160));
        s.wake();
        s.run(400);
        assert_eq!(s.track.lock().volume, 0.0);
        let ticks = s.wakes - wakes;
        assert!((10..=13).contains(&ticks), "{ticks} wakes for a 160 ms fade");
        let next = s.next.unwrap() - s.now();
        assert!(next > 5_000 * MS, "and none after it");
    }

    /// An AudioTrack playing on the real clock, keeping every sample written since its last flush.
    #[derive(Default)]
    struct Live {
        written: Vec<i16>,
        played_before: u64,
        since: Option<std::time::Instant>,
        flushes: u32,
        volumes: Vec<f32>,
    }

    impl Live {
        fn played(&self) -> u64 {
            let running = self.since.map_or(0, |t| (t.elapsed().as_secs_f64() * RATE as f64) as u64);
            (self.played_before + running).min(self.written.len() as u64 / 2)
        }
    }

    struct LiveSink(Arc<Mutex<Live>>, Vec<f32>);

    impl Sink for LiveSink {
        fn staging(&mut self) -> &mut [f32] {
            &mut self.1
        }
        fn write(&mut self, from: usize, len: usize) -> usize {
            // SAFETY: the staging memory is f32s, aligned for i16, and the writer keeps the range inside it.
            let samples = unsafe { std::slice::from_raw_parts((self.1.as_ptr() as *const u8).add(from) as *const i16, len / 2) };
            self.0.lock().written.extend_from_slice(samples);
            len
        }
        fn play(&mut self) {
            let mut l = self.0.lock();
            l.since.get_or_insert_with(std::time::Instant::now);
        }
        fn pause(&mut self) {
            let mut l = self.0.lock();
            l.played_before = l.played();
            l.since = None;
        }
        fn flush(&mut self) {
            let mut l = self.0.lock();
            l.written.clear();
            l.played_before = 0;
            l.flushes += 1;
        }
        fn stop(&mut self) {}
        fn set_volume(&mut self, volume: f32) {
            self.0.lock().volumes.push(volume);
        }
        fn heard(&mut self, _playing: bool) -> Option<(u64, i64)> {
            Some((self.0.lock().played(), mono_ns()))
        }
        fn release(&mut self) {}
    }

    struct LiveOpener(Arc<Mutex<Live>>);

    impl Opener for LiveOpener {
        fn open(&mut self, format: OutputFormat, float: bool, frames: u64) -> Result<Opened, String> {
            assert_eq!((format.rate, format.channels, float), (RATE, 2, false));
            Ok(Opened { sink: Box::new(LiveSink(self.0.clone(), vec![0.0; CHUNK_BYTES / 4])), frames, starts_full: false })
        }
    }

    /// Songs as WAV files in memory, streamed through a byte source.
    struct Wavs(Vec<(String, Arc<Vec<u8>>)>);

    impl nori_engine::ByteSource for Wavs {
        fn open(&self, url: &str, from: u64) -> Result<nori_engine::Body, String> {
            let f = self.0.iter().find(|(id, _)| id == url).ok_or("no such song")?.1.clone();
            let len = f.len() as u64;
            Ok(nori_engine::Body { start: from, len: Some(len), reader: Box::new(std::io::Cursor::new(f[from as usize..].to_vec())) })
        }
    }

    struct Songs(Arc<Wavs>);

    impl nori_engine::Library for Songs {
        fn locate(&mut self, id: &str) -> Result<nori_engine::Located, String> {
            Ok(nori_engine::Located { source: nori_engine::Source::Url { url: id.to_string(), bytes: self.0.clone() }, hint: Some("wav".into()), duration_ms: Some(3_000) })
        }
        fn about(&self, id: &str) -> nori_player::transitions::WindowSong {
            nori_player::transitions::WindowSong { id: id.to_string(), title: id.to_string(), duration_ms: 3_000, ..Default::default() }
        }
    }

    fn tone(secs: u32, hz: f64) -> Vec<i16> {
        (0..secs * RATE).flat_map(|i| {
            let v = ((std::f64::consts::TAU * hz * i as f64 / RATE as f64).sin() * 12_000.0) as i16;
            [v, v / 2]
        }).collect()
    }

    fn wav(samples: &[i16]) -> Vec<u8> {
        let data = samples.len() as u32 * 2;
        let mut w = Vec::new();
        for part in [&b"RIFF"[..], &(36 + data).to_le_bytes(), b"WAVEfmt ", &16u32.to_le_bytes(), &1u16.to_le_bytes(), &2u16.to_le_bytes()] {
            w.extend_from_slice(part);
        }
        for part in [&RATE.to_le_bytes()[..], &(RATE * 4).to_le_bytes(), &4u16.to_le_bytes(), &16u16.to_le_bytes(), b"data", &data.to_le_bytes()] {
            w.extend_from_slice(part);
        }
        w.extend(samples.iter().flat_map(|v| v.to_le_bytes()));
        w
    }

    fn wait(secs: u64, mut done: impl FnMut() -> bool) -> bool {
        let until = std::time::Instant::now() + Duration::from_secs(secs);
        while std::time::Instant::now() < until {
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    #[test]
    fn the_engine_plays_through_it_sample_for_sample_and_its_fades_and_jumps_reach_the_track() {
        let songs = [tone(3, 440.0), tone(3, 660.0)];
        let wavs = Arc::new(Wavs(vec![("a".into(), Arc::new(wav(&songs[0]))), ("b".into(), Arc::new(wav(&songs[1])))]));
        let live = Arc::new(Mutex::new(Live::default()));
        let shared = Arc::new(Shared::default());
        let output = TrackOutput::new(Box::new(LiveOpener(live.clone())), false, shared.clone());
        let queue = nori_engine::SharedQueue::default();
        queue.0.lock().set(vec!["a".into(), "b".into()], Some(0), false, 0);
        let mut app = nori_player::sim::App::new();
        app.prefs = nori_player::sim::prefs_off();
        let settings = nori_engine::Settings { fade_ms: 200, ..Default::default() };
        let config = nori_engine::Config { settings, ..Default::default() };
        let engine = nori_engine::Engine::start(Songs(wavs), app, queue, Box::new(output), config, |_| {});
        engine.queue_changed();
        engine.play_at(0, 0);

        assert!(wait(5, || live.lock().played() > RATE as u64 / 2), "it plays");
        {
            let l = live.lock();
            assert!(l.written.len() >= songs[0].len(), "the whole of a short song went into the track at once");
            assert!(l.written[..songs[0].len()] == songs[0][..], "sample for sample, the song's own");
        }
        assert_eq!(engine.status().state, nori_engine::State::Playing, "the ring ran empty, the track did not: still playing");

        // A jump to the second song a second in: the track is emptied, and holds the new music only.
        engine.play_at(1, 1_000);
        assert!(
            wait(5, || {
                let l = live.lock();
                l.flushes == 1 && l.written.len() > RATE as usize
            }),
            "the jump empties the track"
        );
        {
            let l = live.lock();
            let from = RATE as usize * 2;
            assert!(l.written[..RATE as usize] == songs[1][from..from + RATE as usize], "the new music from where it was asked");
        }
        assert!(wait(5, || engine.status().position_ms >= 1_200), "and the playhead follows the track's clock");
        let at = engine.status().position_ms;
        assert!(at < 2_500, "a second and a bit into the song, not further: {at}");

        // A pause fades at the track's volume, then the track stops.
        live.lock().volumes.clear();
        engine.pause();
        assert!(wait(5, || live.lock().since.is_none()), "paused");
        let l = live.lock();
        assert!(l.volumes.len() >= 5, "the fade out ran in steps: {:?}", l.volumes);
        assert_eq!(l.volumes.last(), Some(&0.0));
        drop(l);

        // Played on to the end of the queue: over only once the track has played its last frame.
        engine.play();
        assert!(wait(6, || engine.status().state == nori_engine::State::Ended), "the queue ends");
        let l = live.lock();
        assert!(l.played() + RATE as u64 / 10 >= l.written.len() as u64 / 2, "{} of {} frames heard at the end", l.played(), l.written.len() / 2);
        drop(l);
        engine.stop();
    }

    #[test]
    fn at_the_end_a_track_that_starts_only_full_is_told_to_play_what_it_has() {
        let mut s = Sim::new(4, false, true);
        s.play();
        s.run(10_000);
        let t = s.track.lock();
        assert_eq!(t.stops, 1, "stopped once, to play the last of it out");
        assert_eq!(t.played, 4 * RATE as u64, "every frame heard");
        drop(t);
        assert_eq!(s.next, None, "and then nothing more to do");
    }
}
