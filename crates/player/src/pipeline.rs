//! The player around the transition engine, with the platform left out: a queue walked song by song,
//! one reading at a time, the engine fed in bursts, and below it an output shaped like media3's
//! AudioSink with nori's processors in it (equalizer, silence skipping, speed and pitch) over whatever
//! plays the samples. The simulated player (`sim`, a virtual AudioTrack on a virtual clock) and the
//! desktop player (`nori-engine`, a ring buffer a sound card pulls from) are both this, with their own
//! songs, their own device below the sink and their own clock.
//!
//! What a platform brings: [`Songs`] (a song id opened for reading, as decoded buffers), [`Track`]
//! (the device buffer the sink writes into and whose playhead it reads), [`App`] (the transition
//! planner and the log) and [`Queue`] (the playlist, wherever it is kept).

use crate::burst::{Burst, Fed, BUFFER_US};
use crate::dsp::{Band, Equalizer};
use crate::engine::{Downstream, Heard, Host, StreamFormat, TransitionEngine, POSITION_NOT_SET};
use crate::heard::{HeardTracker, Seen};
use crate::pcm::{Encoding, Format};
use crate::playlist::Playlist;
use crate::queue::{measure_ahead, ErrorRun, OnError, PlaybackError};
use crate::silence::SilenceSkipper;
use crate::sound::sound_on;
use crate::speed::{speed_active, SpeedPitch};
use crate::transitions::WindowSong;
use crate::transport::{rebuild, Chain, ChainAct, ChainChange, Rebuild};

/// media3 starts the renderer's timeline here, so timestamps are never small numbers.
pub const BASE_OFFSET_US: i64 = 1_000_000_000_000;
/// The player reads the next song only once the one it is reading is this close to its end.
pub const READ_AHEAD_US: i64 = 10_000_000;
/// The equalizer screen's output buffer.
pub const SHALLOW_US: i64 = 500_000;
/// media3 resyncs its clock when a buffer's timestamp is this far from where the count says it should be.
const PTS_TOLERANCE_US: i64 = 200_000;
/// The limiter as the settings run it.
const LIMITER_RELEASE_MS: f64 = 120.0;
const LIMITER_LOOKAHEAD_MS: f64 = 5.0;
/// How many buffers one turn offers at most before it lets the thread do something else.
const BUFFERS_PER_TURN: usize = 256;

/// The sound settings, as the settings store hands them to the chain.
#[derive(Debug, Clone, PartialEq)]
pub struct Sound {
    /// The equalizer's bands; empty is the equalizer off.
    pub bands: Vec<Band>,
    pub preamp_db: f64,
    pub crossfeed_db: f64,
    pub balance: f64,
    pub mono: bool,
    pub limiter: bool,
    pub threshold_db: f64,
}

impl Default for Sound {
    fn default() -> Self {
        Sound { bands: Vec::new(), preamp_db: 0.0, crossfeed_db: 0.0, balance: 0.0, mono: false, limiter: false, threshold_db: -1.0 }
    }
}

impl Sound {
    /// Whether anything here touches the samples: the equalizer processor then sits in the chain.
    pub fn on(&self) -> bool {
        sound_on(!self.bands.is_empty() || self.preamp_db != 0.0, self.crossfeed_db as f32, self.balance as f32, self.mono, self.limiter)
    }

    /// The chain set up the way `follow_chain` in the core sets it up.
    pub fn apply(&self, eq: &mut Equalizer) {
        eq.configure(&self.bands, self.preamp_db, self.crossfeed_db);
        let lookahead = if self.limiter { LIMITER_LOOKAHEAD_MS } else { 0.0 };
        eq.configure_output(self.balance, self.mono, self.threshold_db, LIMITER_RELEASE_MS, lookahead);
    }
}

/// What the sink writes into and reads its playhead from: an AudioTrack's buffer on a phone, a ring a
/// sound card pulls from on a desktop, a list of pieces on a virtual clock in the tests. Everything is
/// in the sink's format (16-bit interleaved at the stream's rate); the track converts if its device
/// wants something else.
pub trait Track {
    /// The sink's format from now on (a new stream shape, or the sink built again).
    fn open(&mut self, format: Format);
    /// Bytes handed over and not played yet, in the sink's format.
    fn queued_bytes(&self) -> usize;
    /// Takes `data` whole (the sink never offers more than there is room for), standing for `media`
    /// frames of the song: speed and silence skipping make the two differ.
    fn write(&mut self, data: &[u8], media: f64);
    /// Frames of the song played out since the last flush, as the ear has them.
    fn played_media(&mut self) -> f64;
    /// Nothing handed over is left to play.
    fn is_empty(&self) -> bool;
    /// Everything handed over is dropped, and the playhead starts again at nought.
    fn flush(&mut self);
    fn play(&mut self);
    fn pause(&mut self);
}

/// media3's AudioSink with nori's processors in it, over a [`Track`]. Its clock is media3's: the
/// timestamp of the first buffer after a flush, moved by the difference whenever a buffer arrives more
/// than 200 ms from where the count of submitted frames says it should be (or after a discontinuity),
/// plus the song time of what the track has played. The transition engine relies on exactly that: a
/// mix is stamped in the next song's time, and the clock follows the moment it is offered.
///
/// Once its buffers have grown to the stream's buffer size, nothing here allocates.
pub struct Sink<T: Track> {
    pub format: Option<Format>,
    /// Every token the engine configured this output with, in order.
    pub configs: Vec<u32>,
    /// Times the output was opened for another format after the first.
    pub rebuilds: usize,
    /// How much audio the track may hold.
    pub capacity_us: i64,
    /// Whether the equalizer processor sits in the chain; set when the sink is built.
    pub dsp: bool,
    eq: Option<Equalizer>,
    sound: Sound,
    sound_dirty: bool,
    skip_silence: bool,
    silence: Option<SilenceSkipper>,
    speed_pitch: (f32, f32),
    speed: Option<SpeedPitch>,
    start_media_us: i64,
    needs_init: bool,
    needs_sync: bool,
    submitted_frames: u64,
    /// Buffers whose timestamp was more than 200 ms off: each is a stutter on a phone.
    pub timestamp_jumps: usize,
    /// A buffer taken only in part: the next offer must be the rest of it, or media3 throws.
    owed: Option<(usize, usize)>,
    /// Processed audio still to go into the track (from `pending_pos` on), the song time it stands
    /// for, and song time not yet attached to any output.
    pending: Vec<u8>,
    pending_pos: usize,
    pending_media: f64,
    carry: f64,
    samples_in: Vec<i16>,
    samples_out: Vec<i16>,
    stage: Vec<u8>,
    stage2: Vec<u8>,
    /// The source has ended: running dry now is the end of the music, not a gap.
    pub source_ended: bool,
    /// The largest reduction the limiter reported, dB.
    pub gain_reduction_db: f32,
    pub track: T,
}

impl<T: Track> Sink<T> {
    pub fn new(capacity_us: i64, dsp: bool, sound: Sound, track: T) -> Sink<T> {
        Sink {
            format: None,
            configs: Vec::new(),
            rebuilds: 0,
            capacity_us,
            dsp,
            eq: None,
            sound,
            sound_dirty: true,
            skip_silence: false,
            silence: None,
            speed_pitch: (1.0, 1.0),
            speed: None,
            start_media_us: 0,
            needs_init: true,
            needs_sync: false,
            submitted_frames: 0,
            timestamp_jumps: 0,
            owed: None,
            pending: Vec::new(),
            pending_pos: 0,
            pending_media: 0.0,
            carry: 0.0,
            samples_in: Vec::new(),
            samples_out: Vec::new(),
            stage: Vec::new(),
            stage2: Vec::new(),
            source_ended: false,
            gain_reduction_db: 0.0,
            track,
        }
    }

    /// The sink as a stop and a prepare make it again, over the same track: a new chain for the
    /// settings as they are now, everything in flight dropped, the stages' settings kept.
    pub fn rebuild(&mut self, capacity_us: i64, dsp: bool, sound: Sound) {
        self.format = None;
        self.configs.clear();
        self.rebuilds = 0;
        self.capacity_us = capacity_us;
        self.dsp = dsp;
        self.eq = None;
        self.sound = sound;
        self.sound_dirty = true;
        self.silence = None;
        self.speed = None;
        self.start_media_us = 0;
        self.needs_init = true;
        self.needs_sync = false;
        self.submitted_frames = 0;
        self.timestamp_jumps = 0;
        self.owed = None;
        self.pending.clear();
        self.pending_pos = 0;
        self.pending_media = 0.0;
        self.carry = 0.0;
        self.source_ended = false;
        self.gain_reduction_db = 0.0;
        self.track.flush();
    }

    pub fn queued_us(&self) -> i64 {
        self.format.map_or(0, |f| f.us(self.track.queued_bytes()))
    }

    /// The buffer size the track is opened with, bytes.
    pub fn buffer_bytes(&self) -> usize {
        self.format.map_or(0, |f| f.bytes(self.capacity_us))
    }

    fn build_processors(&mut self) {
        let Some(f) = self.format else { return };
        self.eq = self.dsp.then(|| Equalizer::new(f.rate, f.channels));
        self.sound_dirty = true;
        self.build_stages();
    }

    fn build_stages(&mut self) {
        let Some(f) = self.format else { return };
        self.silence = self.skip_silence.then(|| SilenceSkipper::new(f.rate, f.channels));
        self.speed = speed_active(self.speed_pitch.0, self.speed_pitch.1).then(|| {
            let mut s = SpeedPitch::new(f.rate, f.channels, Encoding::Pcm16);
            s.set(self.speed_pitch.0, self.speed_pitch.1);
            s.flush();
            s
        });
    }

    /// New sound settings: the equalizer picks them up on its next buffer, live.
    pub fn set_sound(&mut self, sound: Sound) {
        self.sound = sound;
        self.sound_dirty = true;
    }

    /// Speed and pitch, and silence skipping: what is inside the stages now plays out first, then
    /// they start again with the new settings, as media3 applies new parameters after draining.
    pub fn set_stages(&mut self, speed: f32, pitch: f32, skip_silence: bool) {
        self.drain_stages();
        self.speed_pitch = (speed, pitch);
        self.skip_silence = skip_silence;
        self.build_stages();
    }

    /// The stages' held audio out into the track.
    fn drain_stages(&mut self) {
        let mut out = std::mem::take(&mut self.stage);
        out.clear();
        if let Some(s) = self.silence.as_mut() {
            s.end_of_stream(&mut out);
        }
        if let Some(sp) = self.speed.as_mut() {
            let mut sped = std::mem::take(&mut self.stage2);
            sped.clear();
            if !out.is_empty() {
                sp.process(&out, &mut sped);
            }
            sp.end_of_stream(&mut sped);
            std::mem::swap(&mut out, &mut sped);
            self.stage2 = sped;
        }
        if !out.is_empty() {
            self.push_pending(&out);
        }
        self.stage = out;
        self.write_pending();
    }

    fn pending_left(&self) -> bool {
        self.pending_pos < self.pending.len()
    }

    fn push_pending(&mut self, data: &[u8]) {
        let media = std::mem::take(&mut self.carry);
        if self.pending_left() {
            self.pending.drain(..self.pending_pos);
            self.pending_pos = 0;
            self.pending.extend_from_slice(data);
            self.pending_media += media;
        } else {
            self.pending.clear();
            self.pending_pos = 0;
            self.pending.extend_from_slice(data);
            self.pending_media = media;
        }
    }

    fn room_bytes(&self) -> usize {
        let Some(f) = self.format else { return 0 };
        f.bytes(self.capacity_us).saturating_sub(self.track.queued_bytes()) / f.frame_bytes() * f.frame_bytes()
    }

    /// Moves processed audio into the track as far as it has room; true when none is left over.
    fn write_pending(&mut self) -> bool {
        if !self.pending_left() {
            return true;
        }
        let room = self.room_bytes();
        let left = self.pending.len() - self.pending_pos;
        let n = room.min(left);
        if n > 0 {
            // The song time goes with the bytes in proportion, so what is left keeps its share.
            let media = self.pending_media * n as f64 / left as f64;
            self.pending_media -= media;
            self.track.write(&self.pending[self.pending_pos..self.pending_pos + n], media);
            self.pending_pos += n;
        }
        let done = !self.pending_left();
        if done {
            self.pending.clear();
            self.pending_pos = 0;
        }
        done
    }

    /// Runs `input` through the processors in media3's order (equalizer, silence skipping, speed) into
    /// the pending output. Every buffer here is kept between calls.
    fn process(&mut self, input: &[u8], frames: u64) {
        self.carry += frames as f64;
        let mut data = std::mem::take(&mut self.stage);
        data.clear();
        match self.eq.as_mut().filter(|e| !e.is_identity()) {
            Some(eq) => {
                self.samples_in.clear();
                self.samples_in.extend(input.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])));
                self.samples_out.resize(self.samples_in.len(), 0);
                eq.process_i16(&self.samples_in, &mut self.samples_out);
                self.gain_reduction_db = self.gain_reduction_db.max(eq.gain_reduction_db());
                data.extend(self.samples_out.iter().flat_map(|v| v.to_le_bytes()));
            }
            None => data.extend_from_slice(input),
        }
        if let Some(s) = self.silence.as_mut() {
            let mut next = std::mem::take(&mut self.stage2);
            next.clear();
            s.process(&data, &mut next);
            std::mem::swap(&mut data, &mut next);
            self.stage2 = next;
        }
        if let Some(sp) = self.speed.as_mut() {
            let mut next = std::mem::take(&mut self.stage2);
            next.clear();
            sp.process(&data, &mut next);
            std::mem::swap(&mut data, &mut next);
            self.stage2 = next;
        }
        if !data.is_empty() {
            self.push_pending(&data);
        }
        self.stage = data;
    }

    fn processing(&self) -> bool {
        self.eq.as_ref().is_some_and(|e| !e.is_identity()) || self.silence.is_some() || self.speed.is_some()
    }

    /// The equalizer set up for the latest settings, on the buffer after they changed.
    fn follow_sound(&mut self) {
        if self.sound_dirty {
            if let Some(eq) = self.eq.as_mut() {
                self.sound.apply(eq);
            }
            self.sound_dirty = false;
        }
    }

    pub fn play(&mut self) {
        self.track.play();
    }

    pub fn pause(&mut self) {
        self.track.pause();
    }

    /// Everything queued and processed is dropped, and the clock starts again at the next buffer.
    pub fn flush(&mut self) {
        self.track.flush();
        self.pending.clear();
        self.pending_pos = 0;
        self.pending_media = 0.0;
        self.carry = 0.0;
        self.owed = None;
        self.needs_init = true;
        self.needs_sync = false;
        self.submitted_frames = 0;
        self.source_ended = false;
        if let Some(eq) = self.eq.as_mut() {
            eq.reset();
        }
        if let Some(s) = self.silence.as_mut() {
            s.flush();
        }
        if let Some(s) = self.speed.as_mut() {
            s.flush();
        }
    }

    /// The end of the queue: what the chain still holds comes out, as media3 drains its processors.
    /// The limiter keeps its look-ahead (5 ms) and nothing more would push it out, so that much
    /// silence goes through it; the silence skipper and the speed stage give up what they hold.
    pub fn end_of_stream(&mut self) {
        let Some(f) = self.format else { return };
        let held = self.eq.as_ref().filter(|e| !e.is_identity()).map_or(0, Equalizer::delay_frames);
        if held > 0 {
            // Once per queue, so the silence is made here rather than kept.
            self.process(&vec![0u8; held * f.frame_bytes()], 0);
        }
        self.drain_stages();
    }

    /// Processed audio waiting for room goes into the track as far as it fits: once the source has
    /// ended nothing else offers it.
    pub fn write_out(&mut self) {
        self.write_pending();
    }

    /// Processed audio is waiting for room in the track.
    pub fn pending(&self) -> bool {
        self.pending_left()
    }

    /// Whether everything handed over has been played.
    pub fn drained(&self) -> bool {
        self.track.is_empty() && !self.pending_left()
    }
}

impl<T: Track> Downstream for Sink<T> {
    type Config = u32;

    fn configure(&mut self, config: &u32, format: Option<Format>) {
        self.configs.push(*config);
        let f = format.expect("the player plays PCM");
        if self.format != Some(f) {
            if self.format.is_some() {
                self.rebuilds += 1;
            }
            self.format = Some(f);
            self.track.open(f);
            self.build_processors();
        }
    }

    fn handle_buffer(&mut self, data: &[u8], from: usize, pts_us: i64) -> (bool, usize) {
        let f = self.format.expect("configured before the first buffer");
        let fb = f.frame_bytes();
        let key = (data.as_ptr() as usize + from, data.len() - from);
        let continuing = self.owed.take();
        if let Some(owed) = continuing {
            assert_eq!(owed, key, "offered another buffer while one was only partly taken (media3 throws here)");
        }
        if continuing.is_none() {
            if self.needs_init {
                self.start_media_us = pts_us.max(0);
                self.needs_init = false;
                self.needs_sync = false;
            } else {
                let expected = self.start_media_us + (self.submitted_frames as i128 * 1_000_000 / f.rate as i128) as i64;
                if !self.needs_sync && (expected - pts_us).abs() > PTS_TOLERANCE_US {
                    self.timestamp_jumps += 1;
                    self.needs_sync = true;
                }
                if self.needs_sync {
                    self.start_media_us += pts_us - expected;
                    self.needs_sync = false;
                }
            }
        }
        if !self.write_pending() {
            self.owed = Some(key);
            return (false, 0);
        }
        let input = &data[from..];
        self.follow_sound();
        if self.processing() {
            let frames = (input.len() / fb) as u64;
            self.submitted_frames += frames;
            self.process(input, frames);
            self.write_pending();
            return (true, input.len());
        }
        let n = self.room_bytes().min(input.len()) / fb * fb;
        if n > 0 {
            self.track.write(&input[..n], (n / fb) as f64);
            self.submitted_frames += (n / fb) as u64;
        }
        if n < input.len() {
            self.owed = Some((key.0 + n, key.1 - n));
            return (false, n);
        }
        (true, n)
    }

    fn handle_discontinuity(&mut self) {
        self.needs_sync = true;
    }

    fn position_us(&mut self, _source_ended: bool) -> i64 {
        match self.format {
            Some(f) if !self.needs_init => self.start_media_us + (self.track.played_media() * 1_000_000.0 / f.rate as f64) as i64,
            _ => POSITION_NOT_SET,
        }
    }
}

/// One song opened for reading: decoded buffers of 16-bit interleaved samples, one at a time.
pub trait Reading {
    fn format(&self) -> Format;
    /// The song's length, µs, as far as it is known (exactly, once it has been read to its end).
    fn duration_us(&self) -> i64;
    /// Whether the next buffer can be read without waiting (its bytes have arrived). A reading that
    /// is not ready is asked again on the next turn.
    fn ready(&self) -> bool {
        true
    }
    /// The next buffer; false at the end of the song.
    fn fill(&mut self) -> bool;
    /// The buffer [`Reading::fill`] made.
    fn buffer(&self) -> &[u8];
    /// Where in the song the buffer begins, µs.
    fn at_us(&self) -> i64;
}

/// The songs a queue names, as a platform opens them.
pub trait Songs {
    type Reading: Reading;
    /// Song `id` read from `from_ms` on (a seek lands where the format lets it, and what comes before
    /// the place asked for is decoded and dropped). An error is a song that will not play.
    fn open(&mut self, id: &str, from_ms: i64) -> Result<Self::Reading, String>;
    /// What the planner and the seek bar know of `id`: its length as tagged, its album and number.
    fn about(&self, id: &str) -> WindowSong;
    /// `id` plays next: a platform may start fetching it now, in the same burst as the song before.
    fn upcoming(&mut self, _id: &str) {}
}

/// The app around the player: the transition planner, the analysis store and the log.
pub trait App: Host {
    /// The player's clock as the next calls into the engine are made: [`Host::now_ms`] answers it.
    fn clock(&mut self, now_ms: i64);
    /// Whether AutoMix is on: the songs coming up are measured then.
    fn auto_mix(&self) -> bool;
    /// The planner's window: the song before the current one, then it and those after it in play
    /// order, repeat included.
    fn window(&mut self, window: Vec<WindowSong>, shuffling: bool);
    /// Measures what it has not measured of `ids`, the songs coming up.
    fn measure_ahead<S: Songs>(&mut self, _songs: &mut S, _ids: &[String]) {}
}

/// Where the playlist is kept: the player's own, or the core's.
pub trait Queue {
    fn read<R>(&self, f: impl FnOnce(&Playlist) -> R) -> R;
    /// The player moved to `index` by itself (a song ended, a jump).
    fn moved_to(&mut self, index: usize);
    fn set_repeat(&mut self, mode: u8);
}

impl Queue for Playlist {
    fn read<R>(&self, f: impl FnOnce(&Playlist) -> R) -> R {
        f(self)
    }

    fn moved_to(&mut self, index: usize) {
        Playlist::moved_to(self, index);
    }

    fn set_repeat(&mut self, mode: u8) {
        Playlist::set_repeat(self, mode);
    }
}

/// A stream handed to the output: which song, where it starts in the renderer's time, and how long it is.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Period {
    index: usize,
    offset_us: i64,
    duration_us: i64,
}

/// The song being read, and where it starts in the renderer's timeline.
struct Reader<R> {
    index: usize,
    offset_us: i64,
    r: R,
    pos: usize,
    ended: bool,
}

impl<R: Reading> Reader<R> {
    fn new(index: usize, offset_us: i64, r: R) -> Reader<R> {
        Reader { index, offset_us, r, pos: 0, ended: false }
    }

    fn fill(&mut self) -> bool {
        self.pos = 0;
        let got = self.r.fill();
        self.ended = !got;
        got
    }

    fn left(&self) -> bool {
        self.pos < self.r.buffer().len()
    }
}

/// ExoPlayer, as far as the audio cares: a queue, one renderer reading one song at a time, and the
/// transition engine in front of the output. Every call into the engine goes through [`Fed`] with the
/// player's clock, as the Android glue makes it.
pub struct Player<S: Songs, T: Track, A: App, Q: Queue> {
    pub now_ms: i64,
    pub engine: TransitionEngine<u32>,
    pub burst: Burst,
    pub sink: Sink<T>,
    pub app: A,
    pub queue: Q,
    pub tracks: S,
    /// Transport decisions for the sound chain: deferred rebuilds, the equalizer screen's buffer.
    pub chain: Chain,
    pub sound: Sound,
    pub tracker: HeardTracker,
    /// How close to the end of the song being read the player must be before it reads the next.
    pub read_ahead_us: i64,
    /// Whether the songs coming up are measured whenever the queue moves (with AutoMix on).
    pub measure_on_move: bool,
    reading: Option<Reader<S::Reading>>,
    /// The song after the one being read, opened as soon as that one is read to its end: which queue
    /// index, and what opening it gave.
    next: Option<(usize, Result<S::Reading, String>)>,
    periods: Vec<Period>,
    playing: bool,
    /// The renderer's position: where a seek or a start put it, then what the engine reports once
    /// the output has a clock.
    position_us: i64,
    current: Option<usize>,
    source_ended: bool,
    token: u32,
    speed: (f32, f32),
    skip_silence: bool,
    /// Every song change the player made, with the time it happened. A platform takes them as it
    /// reports them.
    pub changes: Vec<(i64, usize)>,
    /// Songs that would not play and why, as the player met them; a platform takes them as it reports them.
    pub failures: Vec<(String, String)>,
    /// The run of songs that would not play, and whether the user lets the player skip them.
    pub errors: ErrorRun,
    pub skip_on_error: bool,
    /// A song that would not play, found while reading ahead; its error is raised when playback gets there.
    failed: Option<(usize, String)>,
    /// The last turn stopped at its budget of buffers with the output still taking them.
    hungry: bool,
}

impl<S: Songs, T: Track, A: App, Q: Queue> Player<S, T, A, Q> {
    /// A player over `queue`, nothing playing yet, writing into `track` through a deep buffer.
    pub fn build(tracks: S, queue: Q, app: A, track: T) -> Self {
        let mut p = Player {
            now_ms: 1_000,
            engine: TransitionEngine::new(),
            burst: Burst::default(),
            sink: Sink::new(BUFFER_US, false, Sound::default(), track),
            app,
            queue,
            tracks,
            chain: Chain::default(),
            sound: Sound::default(),
            tracker: HeardTracker::new(),
            read_ahead_us: READ_AHEAD_US,
            measure_on_move: true,
            reading: None,
            next: None,
            periods: Vec::new(),
            playing: false,
            position_us: POSITION_NOT_SET,
            current: None,
            source_ended: false,
            token: 0,
            speed: (1.0, 1.0),
            skip_silence: false,
            changes: Vec::new(),
            failures: Vec::new(),
            errors: ErrorRun::new(),
            skip_on_error: true,
            failed: None,
            hungry: false,
        };
        p.sync_queue();
        p
    }

    /// The id at queue (list) index `i`.
    pub fn id_at(&self, i: usize) -> String {
        self.queue.read(|q| q.ids()[i].clone())
    }

    fn next_of(&self, i: usize) -> Option<usize> {
        self.queue.read(|q| q.next_of(i, q.repeat()))
    }

    /// Calls into the engine as the JNI glue does: the output below fed in bursts, on this clock.
    fn call<R>(&mut self, f: impl FnOnce(&mut TransitionEngine<u32>, &mut Fed<'_, Sink<T>>, &mut A) -> R) -> R {
        self.app.clock(self.now_ms);
        let mut fed = Fed::new(&mut self.sink, &mut self.burst, self.now_ms);
        f(&mut self.engine, &mut fed, &mut self.app)
    }

    fn configure(&mut self, i: usize, format: Format) {
        self.token += 1;
        let (s, t) = (StreamFormat { id: Some(self.id_at(i)), format: Some(format) }, self.token);
        self.call(|e, d, a| e.configure(d, a, s, t));
    }

    /// Starts reading song `i` (opened as `r`, from `from_ms`) on a fresh timeline, after a flush.
    fn start_reading(&mut self, i: usize, from_ms: i64, offset_us: i64, r: S::Reading) {
        let (format, duration_us) = (r.format(), r.duration_us());
        self.reading = Some(Reader::new(i, offset_us, r));
        self.next = None;
        self.periods = vec![Period { index: i, offset_us, duration_us }];
        self.position_us = offset_us + from_ms * 1000;
        self.source_ended = false;
        self.configure(i, format);
        self.engine.set_output_stream_offset_us(offset_us);
    }

    /// A timeline for a new start, past everything played so far.
    fn fresh_offset(&self) -> i64 {
        self.periods.iter().map(|p| p.offset_us + p.duration_us).max().unwrap_or(BASE_OFFSET_US - 1_000_000) + 1_000_000
    }

    /// Plays queue index `i` from its start.
    pub fn play_from(&mut self, i: usize) {
        self.jump(i, 0);
        self.resume();
    }

    /// Queue index `i` from `from_ms`, without touching whether it plays.
    pub fn jump(&mut self, i: usize, from_ms: i64) {
        let id = self.id_at(i);
        let r = match self.tracks.open(&id, from_ms) {
            Ok(r) => r,
            Err(why) => return self.fail(i, why),
        };
        let offset = self.fresh_offset();
        self.call(|e, _, a| e.flush(a));
        self.burst.restart();
        self.sink.flush();
        self.queue.moved_to(i);
        self.start_reading(i, from_ms, offset, r);
        self.set_current(i);
    }

    pub fn resume(&mut self) {
        self.burst.restart();
        self.sink.play();
        self.playing = true;
    }

    pub fn pause(&mut self) {
        self.burst.restart();
        self.sink.pause();
        self.playing = false;
        if self.chain.paused() == ChainAct::Rebuild {
            self.rebuild_sink();
        }
    }

    pub fn playing(&self) -> bool {
        self.playing
    }

    /// Next, as the button does it.
    pub fn next(&mut self) -> bool {
        match self.queue.read(Playlist::next) {
            Some(n) => {
                self.jump(n, 0);
                true
            }
            None => false,
        }
    }

    /// Previous, as the button does it.
    pub fn previous(&mut self) -> bool {
        match self.queue.read(Playlist::previous) {
            Some(n) => {
                self.jump(n, 0);
                true
            }
            None => false,
        }
    }

    /// A seek in the song the player is on. The engine and the output are flushed, and the renderer
    /// starts reading there on the same timeline - announcing the song again, as media3 does when the
    /// stream it reads changes.
    pub fn seek(&mut self, ms: i64) {
        let Some(i) = self.current else { return };
        let offset = self.periods.iter().find(|p| p.index == i).map_or_else(|| self.fresh_offset(), |p| p.offset_us);
        let r = match self.tracks.open(&self.id_at(i), ms) {
            Ok(r) => r,
            Err(why) => return self.fail(i, why),
        };
        self.call(|e, _, a| e.flush(a));
        self.burst.restart();
        self.sink.flush();
        self.start_reading(i, ms, offset, r);
    }

    /// New sound settings. The equalizer follows them live; a processor joining or leaving the chain
    /// needs a rebuild, which waits for the next song while music plays.
    pub fn set_sound(&mut self, sound: Sound) {
        let was = self.sound.on();
        self.sound = sound.clone();
        self.sink.set_sound(sound);
        let change = ChainChange { processor_changed: was != self.sound.on(), ..Default::default() };
        match rebuild(change) {
            Rebuild::None => {}
            Rebuild::Now => self.rebuild_sink(),
            Rebuild::AtBoundary => {
                if !self.playing {
                    self.rebuild_sink();
                } else if self.chain.defer() {
                    self.app.log("chain swap deferred to the next track");
                }
            }
        }
    }

    /// The equalizer screen opened or closed: its shallow buffer comes and goes at the next boundary.
    pub fn set_tuning(&mut self, on: bool) {
        let act = self.chain.tuning(on, self.sound.on(), self.current.is_none(), self.playing);
        self.burst.enabled = self.chain.bursting(false);
        if act == ChainAct::Rebuild {
            self.rebuild_sink();
        }
    }

    /// Speed and pitch, which the sink's stage hears live.
    pub fn set_speed(&mut self, speed: f32, pitch: f32) {
        self.speed = (speed, pitch);
        self.sink.set_stages(speed, pitch, self.skip_silence);
        self.app.log(&format!("speed in chain: x{speed} pitch x{pitch}"));
    }

    pub fn set_skip_silence(&mut self, on: bool) {
        self.skip_silence = on;
        self.sink.set_stages(self.speed.0, self.speed.1, on);
    }

    /// Speed and pitch as set.
    pub fn speed(&self) -> (f32, f32) {
        self.speed
    }

    /// The output as a stop and a prepare make it again: the engine reset, a new chain for the
    /// settings as they are now, and the song read again from where it is.
    fn rebuild_sink(&mut self) {
        let at_ms = self.position_ms();
        self.call(|e, _, a| e.reset(a));
        self.burst.restart();
        let capacity = if self.chain.tuning { SHALLOW_US } else { BUFFER_US };
        self.sink.rebuild(capacity, self.sound.on(), self.sound.clone());
        self.sink.set_stages(self.speed.0, self.speed.1, self.skip_silence);
        if let Some(i) = self.current {
            let offset = self.fresh_offset();
            let r = match self.tracks.open(&self.id_at(i), at_ms.max(0)) {
                Ok(r) => r,
                Err(why) => return self.fail(i, why),
            };
            let f = r.format();
            self.start_reading(i, at_ms.max(0), offset, r);
            self.app.log(&format!("AudioTrack {} Hz buffer={}", f.rate, f.bytes(capacity)));
        }
    }

    /// Has the app measure the songs coming up that it has not measured, and asks the engine for its
    /// plan again.
    pub fn measure_ahead(&mut self) {
        let n = measure_ahead(self.app.auto_mix());
        let ids: Vec<String> = self.queue.read(|q| q.upcoming().take(n).map(|i| q.ids()[i].clone()).collect());
        if ids.is_empty() {
            return;
        }
        self.app.measure_ahead(&mut self.tracks, &ids);
        self.engine.replan();
    }

    /// Song `i` would not play (`why`): skipped as the platform's error handler does
    /// (`queue::ErrorRun`), or playback stops there.
    fn fail(&mut self, i: usize, why: String) {
        let id = self.id_at(i);
        let next = self.next_of(i);
        self.failures.push((id.clone(), why));
        match self.errors.failed(PlaybackError::Other, false, false, self.skip_on_error, next.is_some()) {
            OnError::Skip => {
                self.app.log(&format!("{id} will not play: skipped"));
                self.jump(next.expect("a skip has somewhere to go"), 0);
            }
            _ => {
                self.app.log(&format!("{id} will not play: stopped"));
                self.call(|e, _, a| e.reset(a));
                self.sink.flush();
                self.sink.pause();
                self.playing = false;
                self.reading = None;
                self.next = None;
            }
        }
    }

    /// The queue changed here (songs added, shuffle): the planner's window and the seek bar follow.
    pub fn queue_changed(&mut self) {
        self.sync_queue();
    }

    /// Repeat off, one or all (`playlist::REPEAT_*`): the player walks the queue that way from now on,
    /// and the plan out of the song playing is asked for again.
    pub fn set_repeat(&mut self, mode: u8) {
        self.queue.set_repeat(mode);
        self.sync_queue();
        self.engine.replan();
    }

    /// The queue as the planner and the seek bar see it: the window (the song before the current one,
    /// then it and those after it, in play order) and every song's length.
    fn sync_queue(&mut self) {
        // As the core hands it over (`playlist_window`): the song before, then eight as the player walks
        // them, repeat included.
        let current = self.current;
        let (window, shuffling, all) = self.queue.read(|q| {
            let repeat = q.repeat();
            let mut window = Vec::new();
            if let Some(c) = current.or(q.current()) {
                window.extend(q.previous_of(c, repeat));
                let mut at = Some(c);
                for _ in 0..8 {
                    let Some(i) = at else { break };
                    window.push(i);
                    at = q.next_of(i, repeat);
                }
            }
            let ids: Vec<String> = window.into_iter().map(|i| q.ids()[i].clone()).collect();
            (ids, q.shuffling(), q.ids().to_vec())
        });
        let window = window.iter().map(|id| self.tracks.about(id)).collect();
        self.app.window(window, shuffling);
        let songs: Vec<(String, i64)> = all.into_iter().map(|id| (id.clone(), self.tracks.about(&id).duration_ms)).collect();
        self.tracker.set_queue(songs);
    }

    fn set_current(&mut self, i: usize) {
        if self.current == Some(i) {
            return;
        }
        let first = self.current.is_none();
        self.current = Some(i);
        // A song that starts breaks a run of songs that would not.
        self.errors.played();
        self.queue.moved_to(i);
        self.changes.push((self.now_ms, i));
        self.sync_queue();
        if let Some(n) = self.next_of(i) {
            let id = self.id_at(n);
            self.tracks.upcoming(&id);
        }
        if !first && self.chain.boundary(false) == ChainAct::Rebuild {
            self.app.log("chain swap at the boundary");
            self.rebuild_sink();
        }
        if self.app.auto_mix() && self.measure_on_move {
            self.measure_ahead();
        }
    }

    /// The song the player is on: the stream the output's clock has reached.
    pub fn current(&self) -> Option<usize> {
        self.current
    }

    pub fn current_id(&self) -> Option<String> {
        self.current.map(|i| self.id_at(i))
    }

    /// The player's position in the song it is on, ms.
    pub fn position_ms(&self) -> i64 {
        if self.position_us == POSITION_NOT_SET {
            return 0;
        }
        let offset = self.current.and_then(|i| self.periods.iter().rev().find(|p| p.index == i)).map_or(0, |p| p.offset_us);
        (self.position_us - offset) / 1000
    }

    /// How long until the output's clock reaches the next song already handed to it, in song time;
    /// `None` when none is.
    pub fn until_next_song_us(&self) -> Option<i64> {
        if self.position_us == POSITION_NOT_SET {
            return None;
        }
        self.periods.iter().map(|p| p.offset_us).filter(|&o| o > self.position_us).min().map(|o| o - self.position_us)
    }

    /// What the seek bar shows: the song heard and the place in it.
    pub fn bar(&mut self) -> Seen {
        let (on, next, pos) = (self.current, self.queue.read(Playlist::next), self.position_ms());
        let heard = self.engine.heard().clone();
        self.tracker.at_index(&heard, self.now_ms, self.playing, on, next, pos)
    }

    /// What the engine says the ear has now.
    pub fn heard(&self) -> &Heard {
        self.engine.heard()
    }

    /// A mix is being heard right now.
    pub fn mixing(&self) -> bool {
        self.engine.heard().mixing
    }

    /// The last song has been read to its end and handed over.
    pub fn source_ended(&self) -> bool {
        self.source_ended
    }

    /// Whether everything has been played to the end.
    pub fn ended(&self) -> bool {
        self.source_ended && self.sink.drained()
    }

    /// The last turn ran out of its budget with the output still taking audio: another turn is due at
    /// once rather than when the output runs low.
    pub fn hungry(&self) -> bool {
        self.hungry
    }

    /// The song being read is waiting for its bytes.
    pub fn starved(&self) -> bool {
        self.reading.as_ref().is_some_and(|r| !r.left() && !r.ended && !r.r.ready())
    }

    /// One turn of the renderer at `now_ms`: the position is read, and the output is offered audio
    /// until it refuses.
    pub fn turn(&mut self, now_ms: i64) {
        self.now_ms = now_ms;
        if !self.playing {
            return;
        }
        let ended = self.source_ended;
        let at = self.call(|e, d, a| e.position_us(d, a, ended));
        if at != POSITION_NOT_SET {
            self.position_us = at;
            if let Some(p) = self.periods.iter().rev().find(|p| self.position_us >= p.offset_us).copied() {
                self.set_current(p.index);
            }
        }
        self.render();
        // The error of a song that would not play surfaces once the song before it has played out, as
        // media3 raises it when playback reaches it.
        if self.failed.is_some() && self.ended() {
            let (n, why) = self.failed.take().expect("checked");
            self.fail(n, why);
        }
    }

    fn render(&mut self) {
        self.hungry = false;
        for k in 0..BUFFERS_PER_TURN {
            if !self.ensure_buffer() {
                break;
            }
            self.hungry = k + 1 == BUFFERS_PER_TURN;
            let Player { engine, sink, burst, app, reading, now_ms, .. } = self;
            let r = reading.as_mut().expect("a buffer is ready");
            app.clock(*now_ms);
            let mut fed = Fed::new(sink, burst, *now_ms);
            let pts = r.offset_us + r.r.at_us();
            let (taken, used) = engine.handle_buffer(&mut fed, app, &r.r.buffer()[r.pos..], pts);
            r.pos += used;
            if !taken {
                self.hungry = false;
                return;
            }
        }
        if self.reading.as_ref().is_some_and(|r| r.ended) && (self.at_queue_end() || self.failed.is_some()) && !self.source_ended {
            if self.call(|e, d, a| e.play_to_end_of_stream(d, a)) {
                self.source_ended = true;
                self.sink.end_of_stream();
                self.sink.source_ended = true;
            }
        } else if self.source_ended {
            self.call(|e, d, _| e.queue_empty(d));
            self.sink.write_out();
        }
    }

    fn at_queue_end(&self) -> bool {
        self.reading.as_ref().is_some_and(|r| self.next_of(r.index).is_none())
    }

    /// A buffer to offer, reading on into the next song when this one is read to its end and the player
    /// is close enough to that end.
    fn ensure_buffer(&mut self) -> bool {
        loop {
            let Some(r) = self.reading.as_mut() else { return false };
            if r.left() {
                return true;
            }
            if !r.ended {
                if !r.r.ready() {
                    return false;
                }
                if r.fill() {
                    continue;
                }
            }
            let (i, end) = (r.index, r.offset_us + r.r.duration_us());
            if self.failed.is_some() {
                return false;
            }
            let Some(n) = self.next_of(i) else { return false };
            // The next song is opened as soon as this one is read to its end: a song that will not play
            // is known then, and a platform's fetch has the most time.
            if self.next.as_ref().is_none_or(|(at, _)| *at != n) {
                let opened = self.tracks.open(&self.id_at(n), 0);
                self.next = Some((n, opened));
            }
            if let Some((_, Err(why))) = &self.next {
                self.failed = Some((n, why.clone()));
                self.next = None;
                return false;
            }
            if self.position_us == POSITION_NOT_SET || self.position_us < end - self.read_ahead_us {
                return false;
            }
            let Some((_, Ok(next))) = self.next.take() else { unreachable!("an opened song is waiting") };
            // The next song: its format announced, then its first buffer is a new stream.
            let (format, duration_us) = (next.format(), next.duration_us());
            self.configure(n, format);
            self.call(|e, d, a| e.handle_discontinuity(d, a));
            self.engine.set_output_stream_offset_us(end);
            self.reading = Some(Reader::new(n, end, next));
            self.periods.push(Period { index: n, offset_us: end, duration_us });
        }
    }
}
