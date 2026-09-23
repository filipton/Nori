//! Transitions between tracks inside one output stream: plain crossfades and AutoMix (beat-matched,
//! with a bass swap, a filter sweep on the way out and a tempo stretch on the way in). The planner
//! decides each transition; this decides where the audio goes. It sits between a decoder that hands
//! it one track's PCM after another and the real output below it ([`Downstream`]), which on Android
//! is media3's AudioSink and on a desktop whatever plays the samples.
//!
//! Until the outgoing track reaches the planned start, audio passes straight through. From there it
//! is held (at most the length of the transition; anything past that the plan chose to skip is
//! dropped). When the next track begins, its opening is mixed into what was held and the result goes
//! on. Nothing here runs between transitions except one position check per buffer.
//!
//! When the two sides disagree on rate or channels, the incoming side is converted to the outgoing one
//! and the mix runs at the outgoing rate. Beyond a transition the same rule holds for the whole queue:
//! the downstream format is latched on the first PCM stream and every later stream is converted to
//! it, so the output below is never rebuilt when the next song has another rate - rebuilding is a
//! stop and a start, about half a second of silence right where the new song plays alone. The latch
//! clears on reset and never engages for non-PCM streams or while the output is held bit-perfect,
//! where the native format must reach the wire untouched; see [`TransitionEngine::lock_rate`].
//!
//! It also hands every decoded buffer of a not-yet-analysed track to the streaming analyser, so the
//! tempo, beat grid and cue points come from audio the platform is decoding anyway.
//!
//! The decoder runs well ahead of what is heard, so the engine also keeps what the ear is at
//! ([`Heard`]): through a transition the player's own clock is ahead of the sound, and a seek bar or
//! a title must follow the sound.
//!
//! This is a port of the Android `TransitionSink`, function for function; the comments carry over
//! because every rule in here was learnt from a fault heard on a phone.

use std::collections::VecDeque;

use crate::automix::analysis::Analyzer;
use crate::automix::mixer::Mixer;
use crate::automix::resample::Resampler;
use crate::pcm::{mix_raw, ByteStretcher, Format};

/// µs; `i64::MIN` as media3 has it: no position yet.
pub const POSITION_NOT_SET: i64 = i64::MIN;
const TIME_UNSET: i64 = i64::MIN + 1;
/// How little sound may be left below before a held ending is let go rather than mixed.
const DRY_US: i64 = 1_500_000;
/// How young a hold is exempt from that: born with no runway (a seek just landed in the transition),
/// decode still has to sprint. Normal holds are born with runway to spare, so this changes nothing
/// for them; a truly starved one is let go when this expires.
const HOLD_GRACE_MS: i64 = 10_000;
/// How long a null plan is trusted before it is asked for again.
const NULL_PLAN_RETRY_MS: i64 = 2_000;

/// How to get out of one track into the next, as the planner hands it over.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub incoming_id: String,
    pub out_start_us: i64,
    pub duration_us: i64,
    pub in_skip_us: i64,
    /// `automix::mixer::params(plan)`.
    pub mixer: Vec<f32>,
    pub tempo_ratio: f32,
    pub keep_pitch: bool,
    pub ramp_us: i64,
    /// Capture this many µs of outgoing audio and wrap for `duration_us`; 0 = capture the full duration.
    pub out_loop_us: i64,
    /// After the skip, loop the first this many µs of the incoming track for the rest of the mix; 0 = off.
    pub in_loop_us: i64,
}

impl Plan {
    fn stretching(&self) -> bool {
        (self.tempo_ratio - 1.0).abs() > 1e-4
    }
}

/// A stream's format as the decoder announces it. `format` is `None` for audio that is not samples
/// (offload, passthrough): nothing can be mixed or converted then.
#[derive(Debug, Clone, PartialEq)]
pub struct StreamFormat {
    /// The song whose audio this is, when the platform knows.
    pub id: Option<String>,
    pub format: Option<Format>,
}

/// The real output below. Every call is made from the thread that calls the engine.
pub trait Downstream {
    /// A platform token for a format (on Android, the AudioSinkConfig), handed back to [`Downstream::configure`].
    type Config: Clone;
    /// Opens the output for `config`, whose samples are `format` (`None`: not samples, e.g. offload).
    fn configure(&mut self, config: &Self::Config, format: Option<Format>);
    /// Offers `data[from..]` at `pts_us`; returns whether all of it was taken and how many bytes were.
    /// The whole buffer and a read position, as a ByteBuffer has them: a platform whose output insists
    /// on being offered the same buffer again after taking part of it (media3 does) can recognise it.
    fn handle_buffer(&mut self, data: &[u8], from: usize, pts_us: i64) -> (bool, usize);
    fn handle_discontinuity(&mut self);
    /// µs, or [`POSITION_NOT_SET`].
    fn position_us(&mut self, source_ended: bool) -> i64;
}

/// What the engine asks of the platform while it works. Called on the engine's thread; must be quick.
pub trait Host {
    /// The transition out of `outgoing_id`, or `None` for gapless.
    fn plan_for(&mut self, outgoing_id: &str) -> Option<Plan>;
    /// Whether `song_id` still needs analysing: `Some` with its length in ms (0 when unknown), which
    /// sizes the analysis up front so it never grows while the song plays.
    fn wants_analysis(&mut self, song_id: &str) -> Option<u64>;
    /// The analyser heard all of `song_id` it was going to: `frames` at `rate`, `channels` wide.
    fn analysed(&mut self, song_id: &str, analyzer: Analyzer, channels: usize, frames: u64, rate: u32);
    /// The ear left the player, or caught up with it (see [`Heard`]).
    fn heard_changed(&mut self) {}
    fn log(&mut self, _message: &str) {}
    /// A monotonic clock, ms.
    fn now_ms(&self) -> i64;
}

/// What is heard while the player's own position runs ahead of the ear: the song whose ending is
/// held and the place in it, in the song's own time. The player is told the held ending has played
/// before it has (see [`TransitionEngine::position_us`]); a seek bar shows this instead.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Heard {
    /// `None` whenever the player's position is what is heard.
    pub id: Option<String>,
    pub us: i64,
    /// [`Host::now_ms`] when `us` was read: the sound moves on between readings.
    pub at_ms: i64,
    /// Where in the held song the ear leaves it for the mix, in the song's own time.
    pub until_us: i64,
    /// A mix is being heard right now.
    pub mixing: bool,
    /// The song a hold is mixing into, where the ear lands in it when the mix becomes audible (its
    /// planned skip, plus however late the hold began), and how fast it runs through the mix.
    pub next_id: Option<String>,
    pub next_from_us: i64,
    pub next_rate: f32,
    /// The song whose ending is being mixed out of, and where in it the mix becomes audible.
    pub from_id: Option<String>,
    pub audible_us: i64,
}

impl Heard {
    /// Becomes a copy of `o`, reusing the strings already held: no allocation unless an id grows.
    pub fn assign(&mut self, o: &Heard) {
        fn id(slot: &mut Option<String>, o: &Option<String>) {
            match (slot.as_mut(), o) {
                (Some(s), Some(v)) => s.clone_from(v),
                _ => slot.clone_from(o),
            }
        }
        id(&mut self.id, &o.id);
        id(&mut self.next_id, &o.next_id);
        id(&mut self.from_id, &o.from_id);
        (self.us, self.at_ms, self.until_us, self.mixing) = (o.us, o.at_ms, o.until_us, o.mixing);
        (self.next_from_us, self.next_rate, self.audible_us) = (o.next_from_us, o.next_rate, o.audible_us);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Pass,
    Hold,
    Mix,
}

/// A queued piece of output. `measure`: the first chunk of a mix, whose timestamp jump the output
/// below applies the moment it is offered; see [`TransitionEngine::drain`].
struct Chunk {
    data: Vec<u8>,
    pos: usize,
    pts_us: i64,
    resync: bool,
    measure: bool,
}

/// Staged decode-ahead formats: which song, its format, and the platform's token for it.
struct Staged<C> {
    id: Option<String>,
    format: Option<Format>,
    config: C,
}

pub struct TransitionEngine<C: Clone> {
    /// The downstream format; `None` until the first stream, or while it is not samples.
    out: Option<Format>,
    pending_config: Option<(Option<Format>, C)>,

    /// The song whose audio is arriving now, from the last configure.
    current_id: Option<String>,
    /// The song actually flowing now, from the last discontinuity - never from decode-ahead. A
    /// configure for the next track arrives while this one still plays, and planning the hold off
    /// that id silently skips the transition (a seek past the planned start does the same).
    playing_id: Option<String>,
    /// Flushed, and nothing has flowed since: the next configure is for the stream about to play.
    fresh: bool,
    /// The decoder's buffer went straight down and the output took only part of it. The platform offers
    /// the rest again, and it must go down the same way: an output like media3's insists on being given
    /// the same buffer until it has all of it, and anything else throws. So a plan that arrived in
    /// between waits for the next buffer (and begins that little bit late, which a late hold handles).
    input_owed: bool,
    offset_us: i64,

    phase: Phase,
    plan: Option<Plan>,
    plan_for: Option<String>,
    replan_wanted: bool,
    last_null_at: i64,
    tail: Vec<u8>,
    /// The capacity of the hold for this transition, bytes.
    tail_limit: usize,
    tail_len: usize,
    tail_read: usize,
    /// The output timestamp holding began at: everything from here on is inside this engine, unheard.
    held_from_us: i64,
    held_at: i64,
    /// The song whose ending is held and its stream offset, so the ear's place is in the song's own time.
    held_id: Option<String>,
    held_offset_us: i64,
    /// How far into the planned transition the hold began: nought unless a seek landed inside it.
    late_us: i64,
    /// How much of the outgoing track has been swallowed into the hold, µs.
    held_us: i64,
    /// The last position given to the player, which may never go backwards.
    reported: i64,
    /// The mix is stamped in the incoming track's time, and the output below moves its clock forward by
    /// the difference the moment the first mixed chunk is offered - seconds before that chunk is heard,
    /// with the ending's last unheld stretch still playing out. Until the clock reaches `shift_until_us`
    /// (the first mixed sample), what is heard is the clock less this.
    shift_us: i64,
    shift_until_us: i64,
    /// The output timestamp the queued mix runs to, so a cut-short mix resumes the ending after it.
    mixed_end_us: i64,
    /// The output timestamp the mix is heard from; mixing is true between it and `mixed_end_us`.
    mix_from_us: i64,
    skip_left: usize,
    resync_next: bool,
    measure_next: bool,
    /// Mixed and stretched audio carries its own continuous clock; real timestamps resume after a resync.
    synthetic_pts_us: i64,
    /// Mix-time frame cursor when the outgoing hold is looped for longer than it was captured.
    mix_out_frame: usize,
    mix_out_frames: usize,
    out_loop_frames: usize,

    mixer: Option<Mixer>,
    mixer_format: u64,
    stretch: Option<ByteStretcher>,
    /// What the live stretcher works in (the incoming domain while converting).
    stretch_format: Option<Format>,
    /// Built with the plan, taken up when the next track begins; see `prepare`.
    pending_stretch: Option<(ByteStretcher, bool)>,
    loop_buf: Vec<u8>,

    analyzer: Option<(Analyzer, usize)>,
    analyzer_for: Option<String>,
    analyzer_rate: u32,
    analysis_tainted: bool,
    analysis_buf: Vec<f32>,

    /// The incoming stream's format while it is converted to the downstream one.
    conv_in: Option<Format>,
    /// Which stream id the converter was armed for: buffers flowing now are always in this format.
    conv_id: Option<String>,
    resampler: Option<Resampler>,
    /// Formats decoded ahead while a transition runs; armed once their buffers flow, never downstream.
    staged: VecDeque<Staged<C>>,
    /// The id whose buffers the mix is consuming, so a further decode-ahead configure never re-arms it.
    mix_source_id: Option<String>,
    /// The downstream format stays on whatever the first PCM stream brought. False only while the
    /// platform holds the output bit-perfect (or hi-res): then every stream passes through native
    /// and the output below follows it.
    pub lock_rate: bool,

    queue: VecDeque<Chunk>,
    pool: Vec<Vec<u8>>,

    heard: Heard,
}

impl<C: Clone> Default for TransitionEngine<C> {
    fn default() -> Self {
        Self::new()
    }
}

impl<C: Clone> TransitionEngine<C> {
    pub fn new() -> Self {
        TransitionEngine {
            out: None,
            pending_config: None,
            current_id: None,
            playing_id: None,
            fresh: false,
            input_owed: false,
            offset_us: 0,
            phase: Phase::Pass,
            plan: None,
            plan_for: None,
            replan_wanted: false,
            last_null_at: 0,
            tail: Vec::new(),
            tail_limit: 0,
            tail_len: 0,
            tail_read: 0,
            held_from_us: TIME_UNSET,
            held_at: 0,
            held_id: None,
            held_offset_us: 0,
            late_us: 0,
            held_us: 0,
            reported: i64::MIN,
            shift_us: 0,
            shift_until_us: TIME_UNSET,
            mixed_end_us: TIME_UNSET,
            mix_from_us: TIME_UNSET,
            skip_left: 0,
            resync_next: false,
            measure_next: false,
            synthetic_pts_us: TIME_UNSET,
            mix_out_frame: 0,
            mix_out_frames: 0,
            out_loop_frames: 0,
            mixer: None,
            mixer_format: 0,
            stretch: None,
            stretch_format: None,
            pending_stretch: None,
            loop_buf: Vec::new(),
            analyzer: None,
            analyzer_for: None,
            analyzer_rate: 0,
            analysis_tainted: false,
            analysis_buf: Vec::new(),
            conv_in: None,
            conv_id: None,
            resampler: None,
            staged: VecDeque::new(),
            mix_source_id: None,
            lock_rate: true,
            queue: VecDeque::new(),
            pool: Vec::new(),
            heard: Heard { until_us: i64::MAX, audible_us: i64::MAX, next_rate: 1.0, ..Default::default() },
        }
    }

    /// What the ear is at; see [`Heard`].
    pub fn heard(&self) -> &Heard {
        &self.heard
    }

    /// Something the plan was made without - an analysis measured since - has arrived, so the plan
    /// for the track playing is asked for again at the next buffer.
    pub fn replan(&mut self) {
        self.replan_wanted = true;
    }

    // ---- configuration ----

    /// The decoder announces a stream. `config` is handed to [`Downstream::configure`] if and when
    /// this format goes downstream.
    pub fn configure<D: Downstream<Config = C>, H: Host>(&mut self, down: &mut D, host: &mut H, stream: StreamFormat, config: C) {
        let id = stream.id.clone();
        let f = stream.format;
        match f {
            Some(f) => host.log(&format!("sink: {} {} Hz x{} enc={}", id.as_deref().unwrap_or("?"), f.rate, f.channels, f.encoding.media3())),
            None => host.log(&format!("sink: {} - not PCM, no transitions", id.as_deref().unwrap_or("?"))),
        }
        // Just after a flush (a skip, a jump in the queue, a seek) no buffer has flowed yet, and the
        // first format announced is the stream about to flow - never decode-ahead. Treated as ahead, it
        // was staged to be armed "when its buffers flow", but a flush had just emptied the stage and no
        // discontinuity was coming: a 48 kHz song skipped to under a 44.1 kHz latch then played
        // unconverted, 8.8 % slow and flat, with a timestamp resync - a stutter - every two seconds.
        let for_current = id.is_none() || id == self.current_id || self.fresh;
        if self.fresh {
            self.fresh = false;
            if id.is_some() {
                self.playing_id = id.clone();
            }
        }
        if let Some(new) = id.as_ref().filter(|i| Some(*i) != self.current_id.as_ref()) {
            let new = new.clone();
            self.on_new_stream(host, new);
        }
        let Some(f) = f else {
            // Not samples: there is nothing to convert, so the downstream format has to follow.
            // Anything held is played out as it is first.
            self.drop_converter();
            if self.queue.is_empty() && self.phase == Phase::Pass {
                self.apply(down, None, &config);
                return;
            }
            self.abandon_transition(host);
            self.pending_config = Some((None, config));
            return;
        };
        if !self.lock_rate {
            // Bit-perfect or hi-res: the native format reaches the wire; the output below follows.
            self.drop_converter();
            if self.out == Some(f) {
                return;
            }
            self.abandon_transition(host);
            self.pending_config = Some((Some(f), config));
            return;
        }
        let Some(out) = self.out else {
            // The first PCM stream sets the downstream format for the whole queue.
            self.apply(down, Some(f), &config);
            host.log(&format!("sink pins {} Hz x{} for the queue", f.rate, f.channels));
            return;
        };
        if f == out {
            // At the pinned format already. The output below was opened for exactly this and stays
            // open: forwarding the configure would rebuild it on every track change. Decode-ahead for
            // another stream only waits its turn.
            if self.phase == Phase::Pass && for_current {
                self.drop_converter();
                return;
            }
            self.staged.push_back(Staged { id, format: Some(f), config });
            return;
        }
        if self.phase == Phase::Pass && for_current {
            // The stream whose buffers are flowing changed format: convert it from here on.
            if !self.arm_conversion(host, id, f) {
                self.abandon_transition(host);
                self.pending_config = Some((Some(f), config));
            }
            return;
        }
        // Decode-ahead (or a same-format stranger): the mix is never killed for it. Armed once its
        // buffers flow - at the discontinuity, or when the mix is out.
        self.staged.push_back(Staged { id, format: Some(f), config });
    }

    /// Converts the stream `id` (native `src`) to the pinned format. The stretcher works in the
    /// incoming domain from here on (converted after), so one built in `prepare` is dropped.
    fn arm_conversion<H: Host>(&mut self, host: &mut H, id: Option<String>, src: Format) -> bool {
        let Some(out) = self.out else { return false };
        self.resampler = Resampler::new(src.rate as i32, src.channels as i32, out.rate as i32, out.channels as i32);
        if self.resampler.is_none() {
            self.conv_in = None;
            self.conv_id = None;
            return false;
        }
        self.conv_in = Some(src);
        self.conv_id = id;
        self.pending_stretch = None;
        host.log(&format!("converting {} Hz x{} -> {} Hz x{}", src.rate, src.channels, out.rate, out.channels));
        true
    }

    /// No conversion: buffers flow at the pinned format already. Keeps the latch.
    fn drop_converter(&mut self) {
        self.resampler = None;
        self.conv_in = None;
        self.conv_id = None;
    }

    /// A seek: the converter keeps its formats but its stream position starts over.
    fn reset_converter(&mut self) {
        if let (Some(src), Some(out)) = (self.conv_in, self.out) {
            self.resampler = Resampler::new(src.rate as i32, src.channels as i32, out.rate as i32, out.channels as i32);
            if self.resampler.is_none() {
                self.conv_in = None;
            }
        }
    }

    fn converting(&self) -> bool {
        self.resampler.is_some()
    }

    /// The staged format of `id` (its buffers flow now): arm it, or drop the converter when it is
    /// already at the pinned format. Anything staged for another id waits its turn.
    fn arm_staged_for<H: Host>(&mut self, host: &mut H, id: Option<String>) {
        let mut found: Option<Staged<C>> = None;
        let mut keep = VecDeque::new();
        while let Some(s) = self.staged.pop_front() {
            if s.id == id || (id.is_none() && found.is_none()) {
                found = Some(s);
            } else {
                keep.push_back(s);
            }
        }
        self.staged = keep;
        let Some(s) = found else { return };
        let Some(f) = s.format else {
            self.drop_converter();
            return;
        };
        if Some(f) == self.out {
            if self.conv_id.is_some() {
                self.drop_converter();
            }
            return;
        }
        // Already converting this stream: re-arming would drop the frames of history the
        // interpolator carries and click.
        if self.conv_id == id && self.conv_in == Some(f) {
            return;
        }
        if !self.arm_conversion(host, id, f) {
            self.abandon_transition(host);
            self.pending_config = Some((Some(f), s.config));
        }
    }

    fn apply<D: Downstream<Config = C>>(&mut self, down: &mut D, f: Option<Format>, config: &C) {
        self.out = f;
        down.configure(config, f);
    }

    pub fn set_output_stream_offset_us(&mut self, offset_us: i64) {
        self.offset_us = offset_us;
    }

    // ---- the audio path ----

    /// One decoded buffer at `pts_us`. Returns whether all of it was taken, and how many bytes were:
    /// the platform offers the rest again later.
    pub fn handle_buffer<D: Downstream<Config = C>, H: Host>(&mut self, down: &mut D, host: &mut H, buffer: &[u8], pts_us: i64) -> (bool, usize) {
        self.fresh = false;
        if let Some((f, config)) = self.pending_config.take() {
            if !self.drain(down) {
                self.pending_config = Some((f, config));
                return (false, 0);
            }
            self.apply(down, f, &config);
        }
        let Some(out) = self.out else { return down.handle_buffer(buffer, 0, pts_us) };
        if !self.drain(down) {
            return (false, 0);
        }
        let native = self.conv_in.unwrap_or(out);
        match self.phase {
            Phase::Pass => {
                // Converted audio lives in a buffer the next call reuses: anything kept is copied.
                // The stretcher works in the native domain, so it always sees the native buffer.
                if self.converting() || self.stretch.is_some() {
                    self.feed_analysis(host, buffer, native);
                }
                if self.stretch.is_some() {
                    self.stretch_out(host, buffer, pts_us);
                    self.drain(down);
                    return (true, buffer.len());
                }
                let converted = if self.converting() {
                    match self.converted(host, buffer) {
                        Some(b) => Some(b),
                        None => {
                            self.drain(down);
                            return (true, buffer.len());
                        }
                    }
                } else {
                    None
                };
                let result = self.pass_or_hold(down, host, converted.as_deref().unwrap_or(buffer), buffer.len(), pts_us, out);
                if let Some(b) = converted {
                    self.recycle(b);
                }
                if let Some(r) = result {
                    return r;
                }
            }
            Phase::Hold => {
                self.feed_analysis(host, buffer, native);
                if self.converting() {
                    if let Some(b) = self.converted(host, buffer) {
                        self.hold(&b, out);
                        self.recycle(b);
                    }
                } else {
                    self.hold(buffer, out);
                }
            }
            Phase::Mix => {
                self.feed_analysis(host, buffer, native);
                self.mix(host, buffer, pts_us, out);
            }
        }
        self.drain(down);
        (true, buffer.len())
    }

    /// The plain-sailing part of a buffer's journey, after any conversion: straight through, or - at the
    /// planned start of a transition - the head through and the rest into the hold. `Some` is the
    /// answer to give the caller now; `None` means the buffer was taken in whole.
    fn pass_or_hold<D: Downstream<Config = C>, H: Host>(&mut self, down: &mut D, host: &mut H, buf: &[u8], whole: usize, pts_us: i64, out: Format) -> Option<(bool, usize)> {
        if self.input_owed && !self.converting() {
            return Some(self.pass(down, host, buf, whole, pts_us, false));
        }
        if self.playing_id.is_none() {
            self.playing_id = self.current_id.clone();
        }
        let mut p = self.refresh_plan(host);
        if p.is_none() && self.playing_id != self.current_id {
            // The discontinuity trail went cold (rapid skips stranding a stale id): trust
            // decode again. Sticks, so this costs one replan.
            self.playing_id = self.current_id.clone();
            p = self.refresh_plan(host);
        }
        let track_pos = pts_us - self.offset_us;
        let fb = out.frame_bytes();
        let frames = (buf.len() / fb) as i64;
        let start_frame = p.map_or(i64::MAX, |(start_us, _)| (start_us - track_pos) * out.rate as i64 / 1_000_000);
        // Inside the transition but past its start (a seek, or decode already ahead when the plan
        // arrived): hold from here with what is left, so the mix still fires at the boundary.
        // Past the planned region there is nothing to hold any more.
        let skip_transition = p.is_some_and(|(_, duration_us)| start_frame < -duration_us * out.rate as i64 / 1_000_000);
        if start_frame >= frames || skip_transition {
            return Some(self.pass(down, host, buf, whole, pts_us, self.converting()));
        }
        // Cloned only here, once per transition: every other buffer reads the plan in place.
        let p = self.plan.clone().expect("a plan exists past this point");
        let late = start_frame < 0;
        let before = start_frame.max(0) as usize * fb;
        if !self.converting() {
            self.feed_analysis(host, buf, out);
        }
        if before > 0 {
            let head = self.copy_of(&buf[..before]);
            self.enqueue(head, pts_us);
        }
        // A seek landed inside the transition: the mix will run from this far in, as it would
        // have sounded had the song played on into it (see handle_discontinuity).
        self.late_us = if late { -start_frame * 1_000_000 / out.rate as i64 } else { 0 };
        self.begin_hold(&p, out);
        if late {
            host.log(&format!("transition: late hold, {} ms in", self.late_us / 1000));
        }
        self.held_from_us = pts_us + (before / fb) as i64 * 1_000_000 / out.rate as i64;
        self.heard.audible_us = self.held_from_us - self.held_offset_us;
        self.held_at = host.now_ms();
        let at = down.position_us(false);
        let runway = if at == POSITION_NOT_SET { i64::MAX } else { self.held_from_us - at };
        host.log(&if runway == i64::MAX {
            "holding the ending, no sound still in the sink".to_string()
        } else {
            format!("holding the ending, {} ms of sound still in the sink", runway / 1000)
        });
        if runway < DRY_US && !late {
            // Decode never pulled ahead - a seek just before the boundary, or the next track still
            // fetching. The dry guard would let go within milliseconds, so do not hold at all.
            host.log(&format!("transition: no runway ({} ms), letting the ending play", runway / 1000));
            self.abandon_transition(host);
            return Some(self.pass(down, host, &buf[before..], whole, pts_us, true));
        }
        self.hold(&buf[before..], out);
        None
    }

    /// Straight through. The output below refuses buffers on purpose (battery-friendly feeding) and the
    /// platform then offers the same audio again, so the analyser is only given what was taken.
    /// Converted audio (`copy`) always goes through the queue as a copy. `whole` is the length of the
    /// caller's buffer, reported as taken when this goes through the queue.
    fn pass<D: Downstream<Config = C>, H: Host>(&mut self, down: &mut D, host: &mut H, buffer: &[u8], whole: usize, pts_us: i64, copy: bool) -> (bool, usize) {
        let out = self.out.expect("pass runs on PCM");
        // A resync waiting (the track back on its own timestamps after a stretch) goes down with the
        // buffer it belongs to, which only the queue knows how to do.
        if !self.queue.is_empty() || copy || self.resync_next {
            if !copy {
                self.feed_analysis(host, buffer, out);
            }
            let c = self.copy_of(buffer);
            self.enqueue(c, pts_us);
            self.drain(down);
            return (true, whole);
        }
        let (taken, used) = down.handle_buffer(buffer, 0, pts_us);
        self.input_owed = !taken;
        self.feed_analysis(host, &buffer[..used], out);
        (taken, used)
    }

    /// The plan for the song playing, asked for when it is not known yet; its start and length, µs.
    fn refresh_plan<H: Host>(&mut self, host: &mut H) -> Option<(i64, i64)> {
        let span = |plan: &Option<Plan>| plan.as_ref().map(|p| (p.out_start_us, p.duration_us));
        let id = self.playing_id.as_deref()?;
        // A null is momentary more often than it is an answer (queue surgery still in flight, analyses
        // landing), so it is retried on later buffers, throttled. A plan sticks until asked again or the
        // track changes.
        let now = host.now_ms();
        if self.plan_for.as_deref() == Some(id) && !self.replan_wanted && (self.plan.is_some() || now - self.last_null_at <= NULL_PLAN_RETRY_MS) {
            return span(&self.plan);
        }
        let id = id.to_string();
        self.replan_wanted = false;
        self.plan = host.plan_for(&id);
        self.plan_for = Some(id);
        if self.plan.is_none() {
            self.last_null_at = now;
        }
        if let Some(p) = self.plan.clone() {
            self.prepare(&p);
        }
        span(&self.plan)
    }

    /// Everything the transition needs, built when the plan is made rather than when it starts: the
    /// mix begins between two buffers, and memory and a stretcher's tables allocated right then are
    /// exactly the kind of work that leaves the output with nothing to write - a catch in the sound.
    fn prepare(&mut self, p: &Plan) {
        let Some(out) = self.out else { return };
        let bytes = out.bytes(p.duration_us);
        if bytes > 0 && self.tail.len() < bytes {
            self.tail.resize(bytes, 0);
        }
        let key = out.rate as u64 * 100 + out.channels as u64;
        if self.mixer.is_none() || self.mixer_format != key {
            self.mixer = Some(Mixer::new(out.rate, out.channels));
            self.mixer_format = key;
        }
        if p.stretching() {
            if self.pending_stretch.as_ref().is_some_and(|(_, k)| *k != p.keep_pitch) {
                self.pending_stretch = None;
            }
            if self.pending_stretch.is_none() {
                self.pending_stretch = Some((ByteStretcher::new(out.rate, out.channels, p.keep_pitch), p.keep_pitch));
            }
        }
        while self.pool.len() < 8 {
            self.pool.push(Vec::with_capacity(16384));
        }
    }

    fn begin_hold(&mut self, p: &Plan, out: Format) {
        self.held_id = self.playing_id.clone().or_else(|| self.current_id.clone());
        self.held_offset_us = self.offset_us;
        self.heard.next_rate = if p.stretching() { p.tempo_ratio } else { 1.0 };
        self.heard.next_from_us = p.in_skip_us + (self.late_us.clamp(0, p.duration_us) as f64 * self.heard.next_rate as f64) as i64;
        self.heard.next_id = Some(p.incoming_id.clone());
        self.heard.from_id = self.held_id.clone();
        self.heard.audible_us = i64::MAX;
        // Outro remix: only the loop slice is captured; the mix reads it with wrap for the full duration.
        let hold_us = if p.out_loop_us > 0 { p.out_loop_us } else { p.duration_us };
        let bytes = out.bytes(hold_us);
        if self.tail.len() < bytes {
            self.tail.resize(bytes, 0);
        }
        // Begun late, the hold is what is left of the overlap: past it the plan skips the ending.
        let late_hold = if p.out_loop_us > 0 { 0 } else { self.late_us.clamp(0, p.duration_us) };
        self.tail_limit = out.bytes((hold_us - late_hold).max(0));
        self.tail_len = 0;
        self.phase = Phase::Hold;
    }

    /// Outgoing audio from the start of the transition; whatever does not fit is audio the plan skips.
    fn hold(&mut self, buffer: &[u8], out: Format) {
        self.held_us += out.us(buffer.len());
        let n = buffer.len().min(self.tail_limit - self.tail_len);
        if n > 0 {
            self.tail[self.tail_len..self.tail_len + n].copy_from_slice(&buffer[..n]);
            self.tail_len += n;
        }
    }

    /// The next track begins (or a seek landed): its opening is mixed into what was held, or the
    /// held ending goes out unmixed.
    pub fn handle_discontinuity<D: Downstream<Config = C>, H: Host>(&mut self, down: &mut D, host: &mut H) {
        let p = self.plan.clone();
        match (self.phase, p, self.out) {
            (Phase::Hold, Some(p), Some(out)) if self.tail_len > 0 => {
                // Its format was staged while it decoded ahead; arm it now, so the stretcher and the
                // skip below measure the incoming track in its own domain and the mix runs at the pinned one.
                self.arm_staged_for(host, self.current_id.clone());
                self.mix_source_id = self.current_id.clone();
                self.playing_id = Some(p.incoming_id.clone());
                let key = out.rate as u64 * 100 + out.channels as u64;
                if self.mixer.is_none() || self.mixer_format != key {
                    self.mixer = Some(Mixer::new(out.rate, out.channels));
                    self.mixer_format = key;
                }
                let late = self.late_us.clamp(0, p.duration_us);
                if let Some(m) = self.mixer.as_mut() {
                    m.configure(&p.mixer);
                    // A hold that began inside the transition (a seek) runs the mix from that point: the
                    // curves as far along as they would be, the incoming track as far in as it would be.
                    if late > 0 {
                        m.seek((late * out.rate as i64 / 1_000_000) as u64);
                    }
                }
                let stretching = p.stretching();
                let in_late_us = if stretching { (late as f64 * p.tempo_ratio as f64) as i64 } else { late };
                // The stretcher works in the incoming domain when the sides disagree (converted after
                // stretching), else in the outgoing one; the skip is in the same domain.
                let s_fmt = self.conv_in.unwrap_or(out);
                if stretching {
                    let taken = if !self.converting() { self.pending_stretch.take().filter(|(_, k)| *k == p.keep_pitch).map(|(s, _)| s) } else { None };
                    self.pending_stretch = None;
                    let mut s = taken.unwrap_or_else(|| ByteStretcher::new(s_fmt.rate, s_fmt.channels, p.keep_pitch));
                    s.configure(
                        p.tempo_ratio as f64,
                        ((p.duration_us - late) * s_fmt.rate as i64 / 1_000_000).max(0) as u64,
                        (p.ramp_us * s_fmt.rate as i64 / 1_000_000).max(0) as u64,
                    );
                    self.stretch = Some(s);
                    self.stretch_format = Some(s_fmt);
                }
                let at = down.position_us(false);
                host.log(&format!(
                    "mixing: the next track arrived {} ms into the hold with {} of sound left",
                    host.now_ms() - self.held_at,
                    if at == POSITION_NOT_SET { "no".to_string() } else { format!("{} ms", (self.held_from_us - at) / 1000) }
                ));
                // The held audio is about to go out as the mix, so it stops counting as played-but-unheard.
                // What was already reported stands until the sound really catches up with it.
                self.held_us = 0;
                self.skip_left = s_fmt.bytes(p.in_skip_us + in_late_us);
                self.tail_read = 0;
                self.mix_out_frame = 0;
                self.mix_out_frames = ((p.duration_us - late) * out.rate as i64 / 1_000_000).max(0) as usize;
                self.out_loop_frames = if p.out_loop_us > 0 { ((p.out_loop_us * out.rate as i64 / 1_000_000) as usize).max(1) } else { 0 };
                self.mixed_end_us = TIME_UNSET;
                self.mix_from_us = TIME_UNSET;
                self.resync_next = true;
                self.measure_next = true;
                self.phase = Phase::Mix;
            }
            _ => {
                self.abandon_transition(host);
                // No mix: the new track's buffers flow from here, so its staged format arms now.
                self.playing_id = self.current_id.clone();
                self.arm_staged_for(host, self.current_id.clone());
                self.mix_source_id = None;
                down.handle_discontinuity();
            }
        }
        self.plan = None;
        self.plan_for = None;
    }

    /// The incoming track, mixed into the held ending of the outgoing one.
    fn mix<H: Host>(&mut self, host: &mut H, buffer: &[u8], pts_us: i64, out: Format) {
        let mut buffer = buffer;
        if self.skip_left > 0 {
            let n = self.skip_left.min(buffer.len());
            buffer = &buffer[n..];
            self.skip_left -= n;
            if buffer.is_empty() {
                return;
            }
        }
        // Through the stretcher in the incoming domain, then converted to the outgoing one the held
        // tail is in.
        let stretched = if self.stretch.is_some() {
            match self.stretched(host, buffer) {
                Some(b) => Some(b),
                None => return,
            }
        } else {
            None
        };
        let buffer: &[u8] = stretched.as_deref().unwrap_or(buffer);
        let converted = if self.converting() {
            match self.converted(host, buffer) {
                Some(b) => Some(b),
                None => {
                    if let Some(b) = stretched {
                        self.recycle(b);
                    }
                    return;
                }
            }
        } else {
            None
        };
        let src: &[u8] = converted.as_deref().unwrap_or(buffer);
        let fb = out.frame_bytes();
        let remaining = if self.out_loop_frames > 0 { self.mix_out_frames.saturating_sub(self.mix_out_frame) } else { usize::MAX };
        let tail_frames = if self.out_loop_frames > 0 { remaining } else { (self.tail_len - self.tail_read) / fb };
        let frames = (src.len() / fb).min(remaining).min(tail_frames);
        let mut used = 0;
        if frames > 0 {
            let bytes = frames * fb;
            if self.out_loop_frames > 0 {
                let hold_frames = (self.tail_len / fb).max(1);
                let mut chunk = std::mem::take(&mut self.loop_buf);
                chunk.resize(bytes, 0);
                self.wrap_out(hold_frames, self.out_loop_frames, self.mix_out_frame, &mut chunk, frames, fb);
                if let Some(m) = self.mixer.as_mut() {
                    unsafe { mix_raw(m, chunk.as_ptr(), src.as_ptr(), chunk.as_mut_ptr(), frames, out.encoding) };
                }
                let at = self.stamp(pts_us, frames, out);
                let c = self.copy_of(&chunk);
                self.loop_buf = chunk;
                self.enqueue(c, at);
                self.mixed_end_us = at + frames as i64 * 1_000_000 / out.rate as i64;
                self.mix_out_frame += frames;
            } else {
                let r = self.tail_read;
                if let Some(m) = self.mixer.as_mut() {
                    let t = self.tail[r..r + bytes].as_mut_ptr();
                    unsafe { mix_raw(m, t, src.as_ptr(), t, frames, out.encoding) };
                }
                let at = self.stamp(pts_us, frames, out);
                let mut c = self.take_pooled(bytes);
                c.extend_from_slice(&self.tail[r..r + bytes]);
                self.enqueue(c, at);
                self.mixed_end_us = at + frames as i64 * 1_000_000 / out.rate as i64;
                self.tail_read += bytes;
            }
            used = bytes;
        }
        if used < src.len() && self.out_loop_frames == 0 {
            let rest = &src[used..];
            let at = self.stamp(pts_us, rest.len() / fb, out);
            let c = self.copy_of(rest);
            self.enqueue(c, at);
            self.mixed_end_us = at + (rest.len() / fb) as i64 * 1_000_000 / out.rate as i64;
        }
        if let Some(b) = converted {
            self.recycle(b);
        }
        if let Some(b) = stretched {
            self.recycle(b);
        }
        if (self.out_loop_frames > 0 && self.mix_out_frame >= self.mix_out_frames) || (self.out_loop_frames == 0 && self.tail_read >= self.tail_len) {
            self.phase = Phase::Pass;
            self.finish_conversion(host);
        }
    }

    /// Outgoing wrap: when the hold is exactly the loop slice every frame wraps; otherwise frames before
    /// the loop region play once and the last `loop_frames` repeat (outro remix).
    fn wrap_out(&self, hold_frames: usize, loop_frames: usize, from_frame: usize, dst: &mut [u8], frames: usize, fb: usize) {
        let lp = loop_frames.clamp(1, hold_frames);
        let prefix = hold_frames.saturating_sub(lp);
        let mut i = 0;
        while i < frames {
            let f = from_frame + i;
            let src_frame = if f < prefix { f } else { prefix + (f - prefix) % lp };
            let run = if f < prefix { (frames - i).min(prefix - f) } else { (frames - i).min(lp - (f - prefix) % lp) };
            let (s, n) = (src_frame * fb, run * fb);
            dst[i * fb..i * fb + n].copy_from_slice(&self.tail[s..s + n]);
            i += run;
        }
    }

    /// Incoming-domain audio into the pinned format the mix runs at. `None` when nothing comes out yet
    /// (the converter holds lookahead). A conversion that cannot run drops the converter and lets the
    /// ending play unmixed rather than wedging on a buffer that will never convert.
    fn converted<H: Host>(&mut self, host: &mut H, input: &[u8]) -> Option<Vec<u8>> {
        let (Some(src), Some(out)) = (self.conv_in, self.out) else { return None };
        let in_frames = input.len() / src.frame_bytes().max(1);
        let need = (((in_frames as u64 * out.rate as u64 / src.rate.max(1) as u64) as usize + 4) * out.frame_bytes()).max(16384);
        let mut b = self.take_pooled(need);
        b.resize(need, 0);
        let mut r = self.resampler.as_mut().and_then(|rs| rs.process(input, src.encoding.media3(), &mut b, out.encoding.media3()));
        if r.is_none() && self.resampler.is_some() {
            b.resize(b.len() * 2 + 16384, 0);
            r = self.resampler.as_mut().and_then(|rs| rs.process(input, src.encoding.media3(), &mut b, out.encoding.media3()));
        }
        let Some((_, made)) = r else {
            host.log(&format!("conversion {} Hz x{} -> {} Hz x{} failed, letting the ending play", src.rate, src.channels, out.rate, out.channels));
            self.recycle(b);
            self.drop_converter();
            self.abandon_transition(host);
            return None;
        };
        b.truncate(made);
        if made == 0 {
            self.recycle(b);
            None
        } else {
            Some(b)
        }
    }

    /// The mix is out: the mixed-in track's own format stays armed, anything staged further ahead
    /// waits, and the mix stops counting as a mix.
    fn finish_conversion<H: Host>(&mut self, host: &mut H) {
        self.mix_source_id = None;
        self.arm_staged_for(host, self.current_id.clone());
    }

    /// Timestamps while mixing and stretching are the engine's own running clock, so a stretch never
    /// reads as a jump.
    fn stamp(&mut self, pts_us: i64, frames: usize, out: Format) -> i64 {
        if self.stretch.is_none() && self.synthetic_pts_us == TIME_UNSET {
            return pts_us;
        }
        let at = if self.synthetic_pts_us == TIME_UNSET { pts_us } else { self.synthetic_pts_us };
        self.synthetic_pts_us = at + frames as i64 * 1_000_000 / out.rate as i64;
        at
    }

    fn stretched<H: Host>(&mut self, _host: &mut H, input: &[u8]) -> Option<Vec<u8>> {
        let fmt = self.stretch_format.or(self.out)?;
        let need = input.len() * 3 + self.stretch.as_ref()?.latency_frames() * fmt.frame_bytes() + 8192;
        let mut buf = self.take_pooled(need);
        buf.resize(need, 0);
        let s = self.stretch.as_mut()?;
        let mut produced = 0;
        let mut at = 0;
        while at < input.len() {
            let (used, made) = s.process(&input[at..], &mut buf[produced..], fmt.encoding);
            at += used;
            produced += made;
            if used == 0 && made == 0 {
                break;
            }
        }
        if s.bypassed() {
            produced = self.finish_stretch(&mut buf, produced, fmt);
        }
        buf.truncate(produced);
        if produced == 0 {
            self.recycle(buf);
            None
        } else {
            Some(buf)
        }
    }

    /// After a mix the incoming track keeps going through the stretcher until its tempo is back to normal.
    fn stretch_out<H: Host>(&mut self, host: &mut H, buffer: &[u8], pts_us: i64) {
        let Some(out) = self.out else { return };
        // Everything the stretcher hands out continues the running clock, its last audio too, which comes
        // out as it finishes and hands the track back to its own timestamps. Stamped after that it went
        // down at 0: the output's clock fell back to the start of the queue, the next ending was held
        // against a clock that could never reach it, and it was never let go - silence to the end.
        let at = if self.synthetic_pts_us == TIME_UNSET { pts_us } else { self.synthetic_pts_us };
        let Some(s) = self.stretched(host, buffer) else { return };
        let o = if self.converting() {
            let c = self.converted(host, &s);
            self.recycle(s);
            match c {
                Some(o) => o,
                None => return,
            }
        } else {
            s
        };
        let frames = o.len() / out.frame_bytes();
        // Finished: the track's own timestamps begin with the buffer after this audio, and that is where
        // the output takes its new reference.
        let resync = self.stretch.is_none() && std::mem::take(&mut self.resync_next);
        self.enqueue(o, at);
        self.resync_next |= resync;
        // Only a running clock moves on: the stretcher may have just finished and handed the track
        // back to its own timestamps. (The Kotlin sink added to the unset marker here, leaving a garbage
        // clock for the next mix to stamp its audio with.)
        if self.synthetic_pts_us != TIME_UNSET {
            self.synthetic_pts_us += frames as i64 * 1_000_000 / out.rate as i64;
        }
    }

    fn finish_stretch(&mut self, buf: &mut Vec<u8>, produced: usize, fmt: Format) -> usize {
        let more = match self.stretch.as_mut() {
            Some(s) => {
                if buf.len() < produced + 65536 {
                    buf.resize(produced + 65536, 0);
                }
                s.drain(&mut buf[produced..], fmt.encoding)
            }
            None => 0,
        };
        self.stretch = None;
        self.stretch_format = None;
        // Back on the track's own timestamps: tell the real output to take the next one as a new reference.
        self.resync_next = true;
        self.synthetic_pts_us = TIME_UNSET;
        produced + more
    }

    /// The held audio goes out unmixed: the next track never came, or the mix itself was cut short.
    /// A cut-short mix plays only what has not gone out yet, so the ending is heard to its end instead
    /// of stopping where the mix did. The converter is untouched.
    fn abandon_transition<H: Host>(&mut self, host: &mut H) {
        self.measure_next = false;
        if self.tail_len > 0 && matches!(self.phase, Phase::Hold | Phase::Mix) {
            let from = if self.phase == Phase::Mix { self.tail_read.min(self.tail_len) } else { 0 };
            if from < self.tail_len {
                // At the timestamp it was held at, not at nought: this audio is the ending of the track,
                // in its own timeline, and the output below reads these to keep the clock. Past the start
                // of a mix the queue already holds the mix, so the rest follows it.
                let at = if self.phase == Phase::Mix && self.mixed_end_us != TIME_UNSET {
                    self.mixed_end_us
                } else if self.held_from_us != TIME_UNSET {
                    self.held_from_us
                } else {
                    0
                };
                let rest = self.tail[from..self.tail_len].to_vec();
                let c = self.copy_of(&rest);
                self.enqueue(c, at);
            }
        }
        if self.phase != Phase::Pass {
            host.log(&format!("transition abandoned in {:?}", self.phase));
        }
        // The ending plays out on its own; the next song starts from its beginning, in the player's word.
        self.heard.next_id = None;
        self.heard.from_id = None;
        self.phase = Phase::Pass;
        self.tail_len = 0;
        self.held_from_us = TIME_UNSET;
        self.held_us = 0;
        self.mix_source_id = None;
        // Given up on, not forgotten: the planned point is behind us now, and without this the next
        // buffer of the same track would start the hold over.
        self.plan = None;
        self.plan_for = self.current_id.clone();
    }

    // ---- analysis tap ----

    fn on_new_stream<H: Host>(&mut self, host: &mut H, id: String) {
        self.finish_analysis(host);
        self.current_id = Some(id);
        self.analysis_tainted = false;
        self.analyzer_for = None;
    }

    fn feed_analysis<H: Host>(&mut self, host: &mut H, bytes: &[u8], f: Format) {
        if self.current_id.is_none() || self.analysis_tainted || bytes.is_empty() {
            return;
        }
        if self.analyzer_for != self.current_id {
            let id = self.current_id.clone().expect("checked above");
            self.analyzer_for = Some(id.clone());
            self.analyzer = None;
            if let Some(expected_ms) = host.wants_analysis(&id) {
                self.analyzer = Some((Analyzer::new(f.rate, expected_ms), f.channels));
                self.analyzer_rate = f.rate;
            }
        }
        if let Some((a, ch)) = self.analyzer.as_mut() {
            // Decoded into one reused buffer: the analysis tap allocates nothing once it is running.
            self.analysis_buf.clear();
            crate::pcm::to_f32(bytes, f.encoding, &mut self.analysis_buf);
            a.feed_interleaved(&self.analysis_buf, *ch, |v| v);
        }
    }

    fn finish_analysis<H: Host>(&mut self, host: &mut H) {
        let taken = self.analyzer.take();
        let id = self.analyzer_for.clone();
        let (Some((a, ch)), Some(id)) = (taken, id) else { return };
        if self.analysis_tainted {
            return;
        }
        let frames = a.samples();
        host.analysed(&id, a, ch, frames, self.analyzer_rate);
    }

    // ---- output queue ----

    fn enqueue(&mut self, data: Vec<u8>, pts_us: i64) {
        if data.is_empty() {
            self.recycle(data);
            return;
        }
        self.queue.push_back(Chunk { data, pos: 0, pts_us, resync: self.resync_next, measure: self.measure_next });
        self.resync_next = false;
        self.measure_next = false;
    }

    /// An empty buffer of at least `n` bytes' capacity, from the pool when one fits: once playing, the
    /// audio path allocates nothing.
    fn take_pooled(&mut self, n: usize) -> Vec<u8> {
        let mut b = match self.pool.iter().position(|b| b.capacity() >= n) {
            Some(i) => self.pool.swap_remove(i),
            None => Vec::with_capacity(n.max(16384)),
        };
        b.clear();
        b
    }

    fn copy_of(&mut self, src: &[u8]) -> Vec<u8> {
        let mut b = self.take_pooled(src.len());
        b.extend_from_slice(src);
        b
    }

    fn recycle(&mut self, b: Vec<u8>) {
        if self.pool.len() < 32 {
            self.pool.push(b);
        }
    }

    /// Sends what is queued. False when the output below would not take it all yet (it asks again).
    fn drain<D: Downstream<Config = C>>(&mut self, down: &mut D) -> bool {
        loop {
            let Some(c) = self.queue.front_mut() else { return true };
            if c.resync && c.pos == 0 {
                down.handle_discontinuity();
            }
            let before = if c.measure { down.position_us(false) } else { 0 };
            let (taken, used) = down.handle_buffer(&c.data, c.pos, c.pts_us);
            c.pos += used;
            if c.measure {
                // The output below moved its clock to this chunk's time as it took it - or refused it
                // before looking (still draining), in which case the next offer is measured again.
                let after = down.position_us(false);
                let jumped = before != POSITION_NOT_SET && after != POSITION_NOT_SET && (after - before).abs() > 50_000;
                if jumped {
                    self.shift_us = after - before;
                    self.shift_until_us = c.pts_us;
                }
                if jumped || taken {
                    self.mix_from_us = c.pts_us;
                    c.measure = false;
                }
            }
            if !taken {
                return false;
            }
            let c = self.queue.pop_front().expect("front exists");
            self.recycle(c.data);
        }
    }

    /// The player asks for the playhead every few milliseconds whether or not it has audio to give,
    /// which makes this the one place that can notice the sound running out. While an ending is held
    /// nothing goes downstream, and it is only released when the next track's first buffer arrives. If
    /// that buffer is late, what is already below plays out and the listener gets a hole where the end
    /// of the song should be. So the ending is let go unmixed before that can happen.
    pub fn position_us<D: Downstream<Config = C>, H: Host>(&mut self, down: &mut D, host: &mut H, source_ended: bool) -> i64 {
        let at = down.position_us(source_ended);
        if at == POSITION_NOT_SET {
            return at;
        }
        if self.phase == Phase::Hold && self.held_from_us != TIME_UNSET && self.held_from_us - at < DRY_US && host.now_ms() - self.held_at > HOLD_GRACE_MS {
            host.log(&format!("transition: nothing to mix in yet with {} ms of sound left, letting the ending play", (self.held_from_us - at) / 1000));
            self.abandon_transition(host);
            self.drain(down);
        }
        // Held audio has left the output but has not been heard, and this is the only thing the player
        // asks about how far the track has got - so it is counted as played. The player reads the next
        // track only once this one is within ten seconds of its end, and the samples to mix into what is
        // held are the next track's: without this they arrived after the output had run dry. It is worth
        // exactly the audio in hand, and it is given back: once the mix begins, what is reported stands
        // still until what is really being heard has caught up with it.
        self.reported = self.reported.max(at + self.held_us);
        // The first mixed sample is heard: from here the clock below is the new song's own time.
        if self.shift_us != 0 && at >= self.shift_until_us {
            self.shift_us = 0;
        }
        let ear = at - self.shift_us;
        let was_heard = self.heard.id.is_some();
        match &self.held_id {
            Some(id) if self.reported > ear + 20_000 => {
                self.heard.us = ear - self.held_offset_us;
                self.heard.until_us = if self.held_from_us != TIME_UNSET { self.held_from_us - self.held_offset_us } else { i64::MAX };
                self.heard.at_ms = host.now_ms();
                // Asked on every position query: the id is copied only when it names another song.
                if self.heard.id.as_ref() != Some(id) {
                    self.heard.id = Some(id.clone());
                }
            }
            _ => self.heard.id = None,
        }
        if self.heard.id.is_some() != was_heard {
            host.heard_changed();
        }
        // Caught up after the hold: the held song is over with, and a later wobble of the clock below
        // must not be read as its ending still playing.
        if self.heard.id.is_none() && self.phase == Phase::Pass && self.shift_us == 0 {
            self.held_id = None;
        }
        if self.mix_from_us != TIME_UNSET && self.mixed_end_us != TIME_UNSET && at >= self.mixed_end_us {
            self.mix_from_us = TIME_UNSET;
        }
        self.heard.mixing = self.mix_from_us != TIME_UNSET && at >= self.mix_from_us;
        self.reported
    }

    /// The source ended: whatever is held goes out, and the analysis of the last track is finished.
    /// Returns whether everything queued went down, so the platform may tell the output to play to its end.
    pub fn play_to_end_of_stream<D: Downstream<Config = C>, H: Host>(&mut self, down: &mut D, host: &mut H) -> bool {
        self.abandon_transition(host);
        self.finish_analysis(host);
        self.drain(down)
    }

    pub fn has_pending_data(&self) -> bool {
        !self.queue.is_empty()
    }

    /// Whether everything queued has gone down (drains first if not).
    pub fn queue_empty<D: Downstream<Config = C>>(&mut self, down: &mut D) -> bool {
        if !self.queue.is_empty() {
            self.drain(down);
        }
        self.queue.is_empty()
    }

    fn clear<H: Host>(&mut self, host: &mut H) {
        while let Some(c) = self.queue.pop_front() {
            self.recycle(c.data);
        }
        self.phase = Phase::Pass;
        self.tail_len = 0;
        self.tail_read = 0;
        self.skip_left = 0;
        self.mixed_end_us = TIME_UNSET;
        self.mix_from_us = TIME_UNSET;
        self.heard.mixing = false;
        self.held_from_us = TIME_UNSET;
        self.held_us = 0;
        self.reported = i64::MIN;
        self.held_id = None;
        self.late_us = 0;
        self.heard.next_id = None;
        self.heard.from_id = None;
        self.shift_us = 0;
        self.shift_until_us = TIME_UNSET;
        if self.heard.id.take().is_some() {
            host.heard_changed();
        }
        self.plan = None;
        self.plan_for = None;
        self.mix_source_id = None;
        self.resync_next = false;
        self.measure_next = false;
        self.synthetic_pts_us = TIME_UNSET;
        self.stretch = None;
        self.stretch_format = None;
        self.pending_stretch = None;
        // A seek: the latch and the formats stand (the output below is untouched), but the converter's
        // stream position starts over and anything staged is for another timeline.
        self.reset_converter();
        self.staged.clear();
        self.pending_config = None;
        // A seek: the analyser has not heard this track continuously any more.
        self.analyzer = None;
        self.analysis_tainted = true;
    }

    /// A seek, or a jump in the queue: everything in flight is for another timeline.
    pub fn flush<H: Host>(&mut self, host: &mut H) {
        self.clear(host);
        self.fresh = true;
        self.input_owed = false;
    }

    /// Stopped: the latch goes with the output below, and the next playback pins again.
    pub fn reset<H: Host>(&mut self, host: &mut H) {
        self.clear(host);
        self.input_owed = false;
        self.drop_converter();
        self.staged.clear();
        self.mix_source_id = None;
        self.conv_id = None;
        self.out = None;
        self.mixer = None;
        self.pool.clear();
        self.tail = Vec::new();
        self.loop_buf = Vec::new();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::automix::{mixer, plan};
    use crate::pcm::Encoding;
    use crate::types::AutoMixSettings;

    const RATE: u32 = 44_100;
    const FMT: Format = Format { rate: RATE, channels: 2, encoding: Encoding::Pcm16 };

    /// The output below: takes everything, remembers what it took, and reports a playhead the test sets.
    #[derive(Default)]
    struct Down {
        taken: Vec<(Vec<u8>, i64)>,
        configured: Vec<u32>,
        discontinuities: usize,
        position: i64,
        /// Take at most this many bytes of the next offer (then everything again), as a full track does.
        take_only: Option<usize>,
        /// The buffer a partial take left pending: like media3, the next offer must be that same buffer.
        owed: Option<(usize, usize)>,
    }

    impl Downstream for Down {
        type Config = u32;
        fn configure(&mut self, config: &u32, _: Option<Format>) {
            self.configured.push(*config);
        }
        fn handle_buffer(&mut self, data: &[u8], from: usize, pts_us: i64) -> (bool, usize) {
            // Where the unread bytes start, and how many: what media3 sees as "the same buffer, moved on".
            let key = (data.as_ptr() as usize + from, data.len() - from);
            if let Some(owed) = self.owed {
                assert_eq!(owed, key, "offered another buffer while one was only partly taken (media3 throws here)");
            }
            let n = self.take_only.take().unwrap_or(usize::MAX).min(data.len() - from);
            self.taken.push((data[from..from + n].to_vec(), pts_us));
            let all = from + n == data.len();
            self.owed = if all { None } else { Some((key.0 + n, key.1 - n)) };
            (all, n)
        }
        fn handle_discontinuity(&mut self) {
            self.discontinuities += 1;
        }
        fn position_us(&mut self, _: bool) -> i64 {
            self.position
        }
    }

    impl Down {
        fn samples(&self) -> Vec<i16> {
            self.taken.iter().flat_map(|(d, _)| d.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]]))).collect()
        }
    }

    #[derive(Default)]
    struct Host_ {
        plans: std::collections::HashMap<String, Plan>,
        log: Vec<String>,
        now: i64,
        analysed: Vec<String>,
    }

    impl Host for Host_ {
        fn plan_for(&mut self, id: &str) -> Option<Plan> {
            self.plans.get(id).cloned()
        }
        fn wants_analysis(&mut self, _: &str) -> Option<u64> {
            Some(0)
        }
        fn analysed(&mut self, id: &str, _: Analyzer, _: usize, _: u64, _: u32) {
            self.analysed.push(id.to_string());
        }
        fn log(&mut self, m: &str) {
            self.log.push(m.to_string());
        }
        fn now_ms(&self) -> i64 {
            self.now
        }
    }

    fn stream(id: &str, f: Format) -> StreamFormat {
        StreamFormat { id: Some(id.into()), format: Some(f) }
    }

    /// `secs` of a constant stereo 16-bit value.
    fn tone(v: i16, secs: f64) -> Vec<u8> {
        let frames = (RATE as f64 * secs) as usize;
        (0..frames * 2).flat_map(|_| v.to_le_bytes()).collect()
    }

    /// Feeds `data` in decoder-sized buffers from `from_us`, track-relative.
    fn feed(e: &mut TransitionEngine<u32>, d: &mut Down, h: &mut Host_, data: &[u8], from_us: i64, offset_us: i64) {
        let chunk = 4096;
        let mut at = 0;
        while at < data.len() {
            let n = chunk.min(data.len() - at);
            let pts = offset_us + from_us + FMT.us(at);
            let (all, used) = e.handle_buffer(d, h, &data[at..at + n], pts);
            assert!(all && used == n, "the fake output takes everything");
            at += n;
        }
    }

    /// A plain 2 s equal-power fade out of `from` into `to`, starting `start_us` into `from`.
    fn fade(to: &str, start_us: i64) -> Plan {
        let s = AutoMixSettings { max_transition_s: 2.0, ..Default::default() };
        let t = plan::plan(None, None, 60_000, 60_000, &s);
        assert_eq!(t.duration_ms, 2000);
        Plan {
            incoming_id: to.into(),
            out_start_us: start_us,
            duration_us: 2_000_000,
            in_skip_us: 0,
            mixer: mixer::params(&t),
            tempo_ratio: 1.0,
            keep_pitch: true,
            ramp_us: 0,
            out_loop_us: 0,
            in_loop_us: 0,
        }
    }

    #[test]
    fn audio_passes_through_untouched_without_a_plan() {
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        e.configure(&mut d, &mut h, stream("a", FMT), 7);
        let a = tone(1000, 1.0);
        feed(&mut e, &mut d, &mut h, &a, 0, 0);
        assert_eq!(d.configured, vec![7], "the first PCM stream pins the output");
        assert_eq!(d.taken.iter().map(|(b, _)| b.len()).sum::<usize>(), a.len());
        assert!(d.samples().iter().all(|&v| v == 1000));
        assert_eq!(d.taken[1].1, FMT.us(4096), "timestamps are the decoder's");
    }

    #[test]
    fn a_track_at_another_rate_is_converted_not_passed_as_is() {
        // Pinned at 44.1 kHz by the first song; the second is 48 kHz. The output below must never be
        // rebuilt, and must never receive 48 kHz audio as if it were 44.1 (8.8 % slow and flat).
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        feed(&mut e, &mut d, &mut h, &tone(1000, 0.5), 0, 0);
        // As a decoder does it: the next stream is announced while this one still plays, then the boundary.
        let f48 = Format { rate: 48_000, ..FMT };
        e.configure(&mut d, &mut h, stream("b", f48), 2);
        e.handle_discontinuity(&mut d, &mut h);
        let b: Vec<u8> = (0..48_000 * 2).flat_map(|_| 500i16.to_le_bytes()).collect();
        let before = d.taken.iter().map(|(b, _)| b.len()).sum::<usize>();
        let mut at = 0;
        while at < b.len() {
            let n = 4096.min(b.len() - at);
            e.handle_buffer(&mut d, &mut h, &b[at..at + n], 1_000_000 + at as i64);
            at += n;
        }
        let got = d.taken.iter().map(|(b, _)| b.len()).sum::<usize>() - before;
        assert_eq!(d.configured, vec![1], "the output stays as it was opened");
        let secs = got as f64 / FMT.frame_bytes() as f64 / RATE as f64;
        assert!((secs - 1.0).abs() < 0.01, "one second of 48 kHz comes out as one second at 44.1: {secs}");
    }

    #[test]
    fn the_first_format_after_a_flush_is_the_one_about_to_play() {
        // A skip onto a 48 kHz song: after a flush no buffer has flowed yet, so its format is not
        // decode-ahead and must be converted from its first buffer.
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        feed(&mut e, &mut d, &mut h, &tone(1000, 0.3), 0, 0);
        e.flush(&mut h);
        e.configure(&mut d, &mut h, stream("b", Format { rate: 48_000, ..FMT }), 2);
        assert!(h.log.iter().any(|l| l.starts_with("converting 48000 Hz")), "{:?}", h.log);
    }

    #[test]
    fn a_planned_fade_mixes_the_next_track_into_the_held_ending() {
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        h.plans.insert("a".into(), fade("b", 1_000_000));
        d.position = POSITION_NOT_SET;
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        feed(&mut e, &mut d, &mut h, &tone(8000, 3.0), 0, 0);
        assert!(h.log.iter().any(|l| l.starts_with("holding the ending")), "{:?}", h.log);
        let passed = d.samples().len();
        assert_eq!(passed, FMT.bytes(1_000_000) / 2, "only the part before the transition went out");
        // The next track: announced ahead, then its audio after the boundary.
        e.configure(&mut d, &mut h, stream("b", FMT), 2);
        e.handle_discontinuity(&mut d, &mut h);
        feed(&mut e, &mut d, &mut h, &tone(-8000, 3.0), 0, 3_000_000);
        let s = d.samples();
        let frames = s.len() / 2;
        // 1 s alone, 2 s mixed, then the rest of b: the held second of a past the fade is skipped.
        assert_eq!(frames, (RATE as f64 * (1.0 + 3.0)) as usize);
        let mid = (RATE as usize * 2) * 2;
        assert!(s[mid].abs() < 8000, "the middle of the fade is a mix, not either track: {}", s[mid]);
        assert_eq!(s[..passed].iter().copied().collect::<std::collections::HashSet<_>>().len(), 1);
        assert!(s[s.len() - 10..].iter().all(|&v| v == -8000), "b plays alone after the fade");
        assert_eq!(d.discontinuities, 1, "one resync, for the mix's timestamps");
        assert_eq!(e.heard().next_id.as_deref(), Some("b"));
    }

    #[test]
    fn a_buffer_the_output_took_only_part_of_goes_back_to_it_whole() {
        // A buffer went straight through and the output, filling up, took only part of it. Then the plan
        // arrived (a replan after a seek or a queue change), with its start inside the rest of that
        // buffer. The rest must still go down as that same buffer - media3 throws otherwise, the player
        // resets and the held ending is lost - and the transition starts from the next buffer instead.
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        d.position = POSITION_NOT_SET;
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        let a = tone(1000, 3.0);
        let owed_at = 4096 * 10;
        let mut at = 0;
        while at < a.len() {
            let n = 4096.min(a.len() - at);
            if at == owed_at {
                d.take_only = Some(1024);
            }
            let slice = &a[at..at + n];
            let mut from = 0;
            loop {
                let (all, used) = e.handle_buffer(&mut d, &mut h, &slice[from..], FMT.us(at));
                from += used;
                if all {
                    break;
                }
                // Between the two offers the plan appears, starting inside what is left of this buffer.
                h.plans.insert("a".into(), fade("b", FMT.us(owed_at + 2048)));
                e.replan();
            }
            at += n;
        }
        assert!(h.log.iter().any(|l| l.starts_with("transition: late hold")), "the transition still happens, just late: {:?}", h.log);
    }

    #[test]
    fn no_plan_is_gapless() {
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        feed(&mut e, &mut d, &mut h, &tone(100, 1.0), 0, 0);
        e.configure(&mut d, &mut h, stream("b", FMT), 2);
        e.handle_discontinuity(&mut d, &mut h);
        feed(&mut e, &mut d, &mut h, &tone(200, 1.0), 0, 1_000_000);
        let s = d.samples();
        assert_eq!(s.len(), (RATE as usize * 2) * 2);
        assert_eq!(d.discontinuities, 1);
        assert_eq!(d.configured, vec![1], "same format: the output stays open");
    }

    #[test]
    fn an_ending_nobody_mixes_into_is_let_go_before_the_sound_runs_out() {
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        h.plans.insert("a".into(), fade("b", 1_000_000));
        // Decode is well ahead of the playhead when the hold begins (no reading yet: all runway).
        d.position = POSITION_NOT_SET;
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        feed(&mut e, &mut d, &mut h, &tone(1000, 3.0), 0, 0);
        let before = d.samples().len();
        // The playhead reaches the held point; the grace period is over; nothing came.
        h.now += HOLD_GRACE_MS + 1;
        d.position = 900_000;
        e.position_us(&mut d, &mut h, false);
        assert!(h.log.iter().any(|l| l.contains("letting the ending play")), "{:?}", h.log);
        assert_eq!(d.samples().len() - before, FMT.bytes(2_000_000) / 2, "the held ending went out unmixed");
    }

    #[test]
    fn no_runway_means_no_hold() {
        // Decode never pulled ahead of the playhead: holding would starve the output at once.
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        h.plans.insert("a".into(), fade("b", 1_000_000));
        d.position = 999_000;
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        feed(&mut e, &mut d, &mut h, &tone(1000, 3.0), 0, 0);
        assert!(h.log.iter().any(|l| l.contains("no runway")), "{:?}", h.log);
        assert_eq!(d.samples().len(), (RATE as usize * 3) * 2, "everything played straight through");
    }

    #[test]
    fn a_seek_into_the_transition_still_mixes_from_there() {
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        h.plans.insert("a".into(), fade("b", 1_000_000));
        d.position = POSITION_NOT_SET;
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        // Audio arrives from 1.5 s in: half a second into the planned fade.
        feed(&mut e, &mut d, &mut h, &tone(1000, 1.5), 1_500_000, 0);
        assert!(h.log.iter().any(|l| l.contains("late hold, 500 ms in")), "{:?}", h.log);
        e.configure(&mut d, &mut h, stream("b", FMT), 2);
        e.handle_discontinuity(&mut d, &mut h);
        assert_eq!(e.heard().next_from_us, 500_000, "the next song is entered where the fade would have reached it");
    }

    #[test]
    fn what_is_heard_follows_the_ear_not_the_player() {
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        h.plans.insert("a".into(), fade("b", 1_000_000));
        d.position = POSITION_NOT_SET;
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        feed(&mut e, &mut d, &mut h, &tone(1000, 3.0), 0, 0);
        // The ending is held and counted as played: the player is ahead of the ear.
        d.position = 500_000;
        let reported = e.position_us(&mut d, &mut h, false);
        assert!(reported > 500_000, "the held ending counts as played so the next track is read in time");
        let heard = e.heard();
        assert_eq!(heard.id.as_deref(), Some("a"));
        assert_eq!(heard.us, 500_000);
        assert!((heard.until_us - 1_000_000).abs() <= 23, "the ear leaves a where the mix begins, to the frame: {}", heard.until_us);
    }

    #[test]
    fn a_stretched_mix_hands_back_to_the_tracks_own_clock() {
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        let mut p = fade("b", 1_000_000);
        p.tempo_ratio = 1.03;
        p.ramp_us = 500_000;
        h.plans.insert("a".into(), p);
        d.position = POSITION_NOT_SET;
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        feed(&mut e, &mut d, &mut h, &tone(4000, 3.0), 0, 0);
        e.configure(&mut d, &mut h, stream("b", FMT), 2);
        e.handle_discontinuity(&mut d, &mut h);
        feed(&mut e, &mut d, &mut h, &tone(-4000, 6.0), 0, 3_000_000);
        assert!(d.discontinuities >= 2, "a resync into the mix and one back onto real timestamps: {}", d.discontinuities);
        let s = d.samples();
        assert!(s[s.len() - 10..].iter().all(|&v| v == -4000), "after the stretch the track plays as it is");
        // Nothing runs on a garbage clock afterwards: timestamps stay non-negative and ordered from the resync on.
        let last = d.taken.iter().rev().take(20).map(|(_, p)| *p).collect::<Vec<_>>();
        assert!(last.windows(2).all(|w| w[0] >= w[1]), "{last:?}");
        // From the mix on the clock only moves forward. The stretcher's last audio was once stamped 0,
        // pulling the output's clock back to the start of the queue: the next ending was then held against
        // a clock that could never reach it, and never let go.
        let from_mix: Vec<i64> = d.taken.iter().map(|(_, p)| *p).skip_while(|&p| p < 1_000_000).collect();
        assert!(from_mix.windows(2).all(|w| w[1] >= w[0]), "{from_mix:?}");
    }

    #[test]
    fn the_analysis_of_a_track_is_handed_over_when_the_next_one_starts() {
        let (mut e, mut d, mut h) = (TransitionEngine::<u32>::new(), Down::default(), Host_::default());
        e.configure(&mut d, &mut h, stream("a", FMT), 1);
        feed(&mut e, &mut d, &mut h, &tone(1000, 1.0), 0, 0);
        e.configure(&mut d, &mut h, stream("b", FMT), 2);
        assert_eq!(h.analysed, vec!["a".to_string()]);
    }
}
