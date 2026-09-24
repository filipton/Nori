//! Audio offload: a song's packets handed as they are to an output that decodes them itself (a phone's
//! audio chip), instead of being decoded here. Only while nothing would change a sample
//! (`nori_player::policy::audio_policy`: no equalizer, speed, silence skipping or transitions, an output
//! the chip reaches) and the output says it takes the song's compression; the engine decides that, and
//! plays everything else on the CPU.
//!
//! What media3's sink does in offload, done the same way: songs of one format join without a gap on one
//! track, each told its encoder delay and padding before its first packet
//! ([`OffloadOutput::delay_padding`]) and the one before closed with [`OffloadOutput::end_of_stream`];
//! a song in another format waits for the track to play out and gets a track of its own; the volume is
//! the song's ReplayGain (and the controls' fades), never the samples. An Opus stream goes in Ogg pages,
//! as media3's `OggOpusAudioPacketizer` hands it to the chip.
//!
//! The track is asked to hold minutes of music ([`TRACK_US`]), and is topped up when it holds less than
//! half a minute ([`LOW_US`]), the next song written then too: the engine's thread sleeps minutes between
//! writes. It wakes only for a top-up, the ear reaching the next song (to say so, and to set its volume),
//! a fade's steps, the end of what was written, and the output tearing the track down, which the platform
//! says at once. The top-up's time is worked out from what was written and what the track has played, so
//! it does not rely on the platform calling back when it has room.

use std::collections::VecDeque;

use nori_player::pipeline::{Queue, Reading, Songs};
use nori_player::transitions::in_album_run;

pub use crate::demux::{Coded, CodedSong, Coding};
use crate::demux::Demuxed;
use crate::library::{Library, Sources};

/// Whether an output decodes a compression itself, and whether it joins songs without a gap.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Support {
    No,
    Plain,
    Gapless,
}

/// An output that decodes compressed songs itself: on Android, an AudioTrack opened for offload.
/// Called only from the engine's thread. The platform wakes that thread (it is the one that opened the
/// track) whenever the track wants more or was torn down.
pub trait OffloadOutput: Send {
    /// Whether the output decodes `coded` itself, where the music goes now.
    fn supports(&mut self, coded: Coded) -> Support;
    /// A track for `coded` holding about `bytes`, paused; replaces one that is open. The bytes it holds.
    fn open(&mut self, coded: Coded, bytes: usize) -> Result<usize, String>;
    /// Takes what it has room for of `data` without waiting: the bytes taken, or the error the track
    /// answered with. `data` stands for `frames` frames of music (what a simulated track plays).
    fn write(&mut self, data: &[u8], frames: u64) -> Result<usize, i32>;
    /// The encoder's delay and padding of the song whose packets come next.
    fn delay_padding(&mut self, delay: u32, padding: u32);
    /// The last packet written was the last of its song: the next one follows without a gap. False when
    /// the platform would not take it (Android's `setOffloadEndOfStream` throws unless the track plays).
    /// Android stops the track with it (`native_stop`, `PLAYSTATE_STOPPING`) until the platform has
    /// presented everything written, so a track told it is let go rather than flushed.
    fn end_of_stream(&mut self) -> bool;
    fn play(&mut self);
    fn pause(&mut self);
    /// Drops what was written and not played. Only paused.
    fn flush(&mut self);
    fn set_volume(&mut self, volume: f32);
    /// Frames of music presented since the track was opened or flushed; none when the platform could not
    /// be asked (a failed call is no reading, never nought). A platform may start counting again from
    /// nought at a song joined without a gap.
    fn head(&mut self) -> Option<u64>;
    /// Whether the track has played everything written up to the last end of stream.
    fn presented(&mut self) -> bool;
    /// The track was torn down since this was last asked (the output went where the chip cannot follow):
    /// it plays nothing more.
    fn torn_down(&mut self) -> bool;
    fn close(&mut self);
    /// What the platform said when it was last asked whether it decodes `coded`, in its own words (the
    /// call and its answer), for a report of why a song plays on the CPU.
    fn said(&mut self, _coded: Coded) -> Option<String> {
        None
    }
    /// Something the offload path did that a perf report should say (why a song ended, a play head
    /// that made no sense), in words.
    fn note(&mut self, _what: &str) {}
    /// How many times faster than the music's own pace the track may present it: 1 for a real device,
    /// whose play head cannot run ahead of the clock; a simulated one may run faster.
    fn pace(&self) -> f64 {
        1.0
    }
}

/// Why the CPU plays a song although the settings let the output decode songs itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OnCpu {
    /// It would not open as packets.
    Unread,
    /// Its compression is not one an output decodes itself here (FLAC, Vorbis, ALAC, HE-AAC, PCM).
    Compression(&'static str),
    /// The output does not decode it where the music goes now; what the platform said.
    Unsupported(Coded, Option<String>),
    /// It has an encoder delay or padding to cut, it joins a song of its album without a gap (the one
    /// before it or the one after it follows it on the album, as "keep albums gapless" has it), and the
    /// output joins songs of its compression only with a gap: offloaded, that join would not be gapless.
    NotGapless { coded: Coded, delay: u32, padding: u32, said: Option<String> },
    /// The offloaded track would not open.
    WouldNotOpen(Coded),
    /// The offloaded track was torn down, or refused a write.
    Failed,
    /// The platform's count of what the track played made no sense (it could not be read, or ran ahead of
    /// the clock), or it would not take a song's end of stream: in words.
    Head(String),
}

impl OnCpu {
    /// In words, for the perf report and the log.
    pub fn words(&self) -> String {
        let said = |s: &Option<String>| s.as_ref().map(|s| format!(" ({s})")).unwrap_or_default();
        match self {
            OnCpu::Unread => "the song would not open as packets".into(),
            OnCpu::Compression(c) => format!("{c} is not a compression the output decodes"),
            OnCpu::Unsupported(c, s) => format!("the output does not decode {} at {} Hz x{}{}", c.coding.name(), c.rate, c.channels, said(s)),
            OnCpu::NotGapless { coded, delay, padding, said: s } => format!(
                "{} with an encoder delay of {delay} and padding of {padding} joins a song of its album without a gap, which needs gapless offload, and the output does not do it{}",
                coded.coding.name(),
                said(s)
            ),
            OnCpu::WouldNotOpen(c) => format!("the offloaded track for {} would not open", c.coding.name()),
            OnCpu::Failed => "the offloaded track failed".into(),
            OnCpu::Head(why) => format!("the offloaded track could not be followed: {why}"),
        }
    }
}

/// How much music a track is asked to hold.
pub const TRACK_US: i64 = 240_000_000;
/// The track is topped up, and the next song written, when it holds less than this.
pub const LOW_US: i64 = 30_000_000;
/// The least and the most bytes a track is asked for.
const MIN_BYTES: usize = 512 * 1024;
const MAX_BYTES: usize = 8 * 1024 * 1024;
/// A song's bitrate when its bytes do not say: the most a song usually has, so the track is not too small.
const GUESS_BPS: u32 = 320_000;
/// Bytes put together for one write.
const STAGE_BYTES: usize = 256 * 1024;
/// The last of what was written is taken as played this close to its end: a chip's count of what it
/// presented can stop a decoder's delay short of what was written.
const END_SLACK_US: i64 = 100_000;
/// How long to look again while the end of the music is due and the track has not said it got there.
const END_LOOK_MS: i64 = 100;
/// The track saying it played everything is taken as the end only this close to it by its own count.
const PRESENTED_NEAR_US: i64 = 3_000_000;
/// How far the play head may be ahead of the clock since the track began playing (a start's latency, the
/// thread's own lateness), ms.
const CLOCK_SLACK_MS: i64 = 500;
/// Readings of the play head in a row that made no sense (none, or ahead of the clock), or ends of
/// stream refused while playing, before the CPU takes over.
const STRIKES: u32 = 3;
/// How soon to look again at a play head that made no sense, or an end of stream still to say, ms.
const LOOK_AGAIN_MS: i64 = 300;

/// One song handed to the track: where it starts in the track's frames, and how long it is once all of it
/// is written.
#[derive(Debug, Clone)]
struct Placed {
    index: usize,
    id: String,
    start: u64,
    frames: Option<u64>,
    /// Where in the song its first frame is (a seek lands in a packet).
    from_ms: i64,
    level: f32,
    /// Counts songs placed, so a song placed again (repeat one) is another.
    seq: u64,
}

/// What comes after the last song written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tail {
    /// The end of the queue, or of the song the sleep timer stops at.
    End,
    /// Queue index `i` from its start, which this track cannot play: another format (another track), or a
    /// song for the CPU.
    Then(usize),
}

/// What a turn found that the engine has to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Step {
    Fine,
    /// Queue index `i` from `ms` is for the CPU: its compression is not offloaded, or the track failed
    /// (`refused`: torn down, or a write refused with an error).
    ToPcm { index: usize, ms: i64, refused: bool },
}

/// The track's count of frames presented, made one that only grows: a platform that starts counting
/// again at a gapless join counts on from the song that begins there.
#[derive(Debug, Clone, Copy, Default)]
struct Head {
    base: u64,
    last: u64,
    /// A reading below the last one, not yet taken for a count started again (the next reading says).
    lower: Option<u64>,
}

/// What a reading of the play head was.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Seen {
    Fine,
    /// Lower than the last, where the ear may have reached the next song: the count started again there.
    Joined,
    /// Lower than the last, and not at a join: kept aside until the next reading says what it was.
    Dip,
    /// Lower than the last twice, the second no lower than the first: the platform counts again from
    /// nought (after a standby, say), and the count goes on from where the ear was.
    Restarted,
}

impl Head {
    /// `raw` read now; `join`, where the song after the one the count is in starts, only when the clock
    /// says the ear may have got there; `most`, the furthest the ear can be by the clock. A reading that
    /// would put the ear past `most` changes nothing, and is Err with where it would have put it.
    fn read(&mut self, raw: u64, join: Option<u64>, most: u64) -> Result<(u64, Seen), u64> {
        let (base, last, seen) = if raw >= self.last {
            (self.base, raw, Seen::Fine)
        } else if let Some(start) = join {
            (start, raw, Seen::Joined)
        } else if self.lower.is_some_and(|l| raw >= l) {
            (self.base + self.last, raw, Seen::Restarted)
        } else {
            self.lower = Some(raw);
            return Ok((self.base + self.last, Seen::Dip));
        };
        if base + last > most {
            return Err(base + last);
        }
        (self.base, self.last, self.lower) = (base, last, None);
        Ok((base + last, seen))
    }
}

/// A volume fade at the track: from, to, when it began (ms), how long.
#[derive(Debug, Clone, Copy)]
struct Fade {
    from: f32,
    to: f32,
    start_ms: i64,
    ms: i32,
}

/// The song being written.
struct Writing {
    r: Demuxed,
    /// Frames of music written of it.
    frames: u64,
    ogg: Option<Ogg>,
}

/// The engine's offload path: the track, the songs handed to it, and the song being written.
pub(crate) struct Offload {
    out: Box<dyn OffloadOutput>,
    /// The format the track is open for, whether it joins songs without a gap, and the bytes it holds.
    open: Option<(Coded, bool, usize)>,
    supported: Vec<(Coded, Support)>,
    placed: VecDeque<Placed>,
    writing: Option<Writing>,
    /// The song to start with: opened, waiting to be ready, then placed or handed to the CPU.
    starting: Option<(usize, i64, Result<Demuxed, String>)>,
    /// The song after the one written last, opened once that one was read to its end.
    next: Option<(usize, Result<Demuxed, String>)>,
    tail: Option<Tail>,
    stage: Vec<u8>,
    staged: usize,
    stage_frames: u64,
    written_bytes: u64,
    written_frames: u64,
    head: Head,
    /// The frames presented as last read.
    heard_at: u64,
    playing: bool,
    /// The song the music stops at the end of (the sleep timer's "end of this song").
    pub stop_after: Option<usize>,
    gain: f32,
    level: f32,
    fade: Option<Fade>,
    seq: u64,
    /// The last turn found the next packet's bytes still on their way.
    waiting: bool,
    /// A write since the last end of stream, so the next song's join has one to close.
    pending_eos: bool,
    /// The last write was refused in part: the track is full.
    full: bool,
    /// Why the last song this path gave up went to the CPU.
    pub(crate) on_cpu: Option<OnCpu>,
    /// The song placed last has an encoder delay or padding the output cannot cut (it does not do
    /// gapless offload), so it is heard with a few milliseconds of near silence at its ends: in words,
    /// for the report.
    pub(crate) gapped: Option<String>,
    /// The time of the turn under way, ms.
    now_ms: i64,
    /// When the play head was last read and made sense (or the track began playing), and the frames heard
    /// then: the head cannot be further on than the clock has run since. None before the track plays.
    clock: Option<(i64, u64)>,
    /// The play head's raw reading, as last read.
    raw: Option<u64>,
    /// Readings of the play head in a row that made no sense, or ends of stream refused, and the last
    /// one's words.
    strikes: u32,
    strike_why: String,
    /// An end of stream is due and was not said: the track was not playing, or refused it.
    eos_due: bool,
    /// Ends of stream the platform refused in a row while its track played.
    eos_refusals: u32,
    /// The track was told to play since it was opened or last paused: Android takes an end of stream
    /// only then.
    started: bool,
    /// The frames written when the platform last took an end of stream on this track: its word that it
    /// presented everything is about that one only while nothing was written since.
    eos_at: Option<u64>,
    /// How what was written ended is noted, once.
    end_noted: bool,
}

impl Offload {
    pub(crate) fn new(out: Box<dyn OffloadOutput>) -> Offload {
        Offload {
            out,
            open: None,
            supported: Vec::new(),
            placed: VecDeque::new(),
            writing: None,
            starting: None,
            next: None,
            tail: None,
            stage: Vec::with_capacity(STAGE_BYTES + 64 * 1024),
            staged: 0,
            stage_frames: 0,
            written_bytes: 0,
            written_frames: 0,
            head: Head::default(),
            heard_at: 0,
            playing: false,
            stop_after: None,
            gain: 1.0,
            level: 1.0,
            fade: None,
            seq: 0,
            waiting: false,
            pending_eos: false,
            full: false,
            on_cpu: None,
            gapped: None,
            now_ms: 0,
            clock: None,
            raw: None,
            strikes: 0,
            strike_why: String::new(),
            eos_due: false,
            eos_refusals: 0,
            started: false,
            eos_at: None,
            end_noted: false,
        }
    }

    /// The offload path holds a song: it is the one playing (or paused).
    pub(crate) fn active(&self) -> bool {
        self.starting.is_some() || !self.placed.is_empty()
    }

    pub(crate) fn playing(&self) -> bool {
        self.playing && self.active()
    }

    /// Whether the track is open, and for what: for the perf report and the screen.
    pub(crate) fn track(&self) -> Option<Coded> {
        self.open.map(|o| o.0)
    }

    /// Whether the output decodes `coded`, asked once per format until the output moves.
    fn support(&mut self, coded: Coded) -> Support {
        if let Some((_, s)) = self.supported.iter().find(|(c, _)| *c == coded) {
            return *s;
        }
        let s = self.out.supports(coded);
        self.supported.push((coded, s));
        s
    }

    /// The music goes to another device: what the output decodes there is asked again.
    pub(crate) fn output_moved(&mut self) {
        self.supported.clear();
    }

    /// Why this output would not play song `r` (none: it would): it would not open, its compression is
    /// not one an output decodes, the output does not decode it, or it has a gap to cut at a join that
    /// must be gapless (`album`: [`Offload::in_album`]) and the output joins songs only with one. media3
    /// requires gapless support for every song with a delay or padding; here only an album's songs in
    /// order need it, and between unrelated songs the encoder's few milliseconds (576 samples of delay
    /// and about a thousand of padding in an MP3) are near silence where a gap is expected anyway.
    pub(crate) fn refuses(&mut self, r: &Demuxed, album: bool) -> Option<OnCpu> {
        if r.error().is_some() {
            return Some(OnCpu::Unread);
        }
        let Some(s) = r.coded() else { return Some(OnCpu::Compression(r.compression().unwrap_or("an unknown compression"))) };
        let (coded, delay, padding) = (s.coded, s.delay, s.padding);
        match self.support(coded) {
            Support::No => Some(OnCpu::Unsupported(coded, self.out.said(coded))),
            Support::Plain if (delay != 0 || padding != 0) && album => Some(OnCpu::NotGapless { coded, delay, padding, said: self.out.said(coded) }),
            Support::Plain | Support::Gapless => None,
        }
    }

    /// Whether song `i` joins a song of its album without a gap in the order they play: the song before it
    /// or the one after it follows it on the same album, the rule that keeps albums gapless
    /// (`nori_player::transitions::in_album_run`; never while shuffling).
    pub(crate) fn in_album<L: Library, Q: Queue>(&self, i: usize, tracks: &Sources<L>, queue: &Q) -> bool {
        let (ids, before, after, shuffling) = queue.read(|q| {
            let repeat = q.repeat();
            (q.ids().to_vec(), q.previous_of(i, repeat), q.next_of(i, repeat), q.shuffling())
        });
        let after = after.filter(|_| self.stop_after != Some(i));
        let about = |k: usize| ids.get(k).map(|id| tracks.about(id));
        let Some(song) = about(i) else { return false };
        in_album_run(before.and_then(about).as_ref(), &song, after.and_then(about).as_ref(), shuffling)
    }

    /// Queue index `i` from `from_ms`: the track is emptied, and the song opened as packets. Whether it
    /// plays here is known once it is open ([`Offload::turn`] says so).
    pub(crate) fn start<L: Library, Q: Queue>(&mut self, i: usize, from_ms: i64, tracks: &mut Sources<L>, queue: &Q) -> usize {
        self.empty();
        self.stop_after = None;
        // A song that arriving on skips (an explicit one) gives way to the first after it that does not.
        let i = if queue.skips(i) { self.next_of(i, queue).unwrap_or(i) } else { i };
        let id = queue.read(|q| q.ids()[i].clone());
        let opened = tracks.open_packets(&id, from_ms, false);
        self.starting = Some((i, from_ms, opened));
        i
    }

    /// Everything handed to the track goes, and what was being written; the track stays open.
    fn empty(&mut self) {
        if self.eos_at.is_some() && !self.out.presented() && self.open.take().is_some() {
            // Told an end of stream, Android's track was stopped until it presents it (then it starts
            // itself again): paused and flushed before that, it starts again as stopping, refuses every
            // write that does not wait (`blockUntilOffloadDrain`), and may say later that it presented
            // the flushed song's end. media3 never flushes an offloaded track; a new one is opened for
            // what comes.
            self.out.close();
            self.started = false;
        } else if self.open.is_some() {
            self.out.pause();
            self.started = false;
            self.out.flush();
        }
        self.placed.clear();
        self.writing = None;
        self.starting = None;
        self.next = None;
        self.tail = None;
        self.stage.clear();
        self.staged = 0;
        self.stage_frames = 0;
        self.written_bytes = 0;
        self.written_frames = 0;
        self.head = Head::default();
        self.heard_at = 0;
        self.waiting = false;
        self.pending_eos = false;
        self.full = false;
        self.clock = None;
        self.raw = None;
        self.strikes = 0;
        self.eos_due = false;
        self.eos_refusals = 0;
        self.eos_at = None;
        self.end_noted = false;
    }

    /// Lets the track go (a long pause, or the CPU takes over): where the ear was, as (queue index, ms).
    pub(crate) fn release(&mut self) -> Option<(usize, i64)> {
        let at = self.heard().map(|(i, ms, _)| (i, ms)).or(self.starting.as_ref().map(|s| (s.0, s.1)));
        self.empty();
        if self.open.take().is_some() {
            self.out.close();
            self.started = false;
        }
        self.playing = false;
        self.stop_after = None;
        self.fade = None;
        at
    }

    pub(crate) fn play(&mut self) {
        self.playing = true;
        if self.open.is_some() && !self.placed.is_empty() {
            self.out.play();
            self.started = true;
            if self.eos_due {
                self.end_stream();
            }
        }
    }

    pub(crate) fn pause(&mut self) {
        self.playing = false;
        if let Some(f) = self.fade.take() {
            // The fade the pause waited for ends at its target, not a step short of it.
            self.gain = f.to;
            self.volume();
        }
        if self.open.is_some() {
            self.out.pause();
            self.started = false;
        }
    }

    /// A fade of the track's volume from `from` (or where it is) to `to` over `ms`, from `now_ms`.
    pub(crate) fn ramp(&mut self, from: Option<f32>, to: f32, ms: i64, now_ms: i64) {
        self.fade = Some(Fade { from: from.unwrap_or(self.gain), to, start_ms: now_ms, ms: ms.clamp(0, i32::MAX as i64) as i32 });
        self.follow_fade(now_ms);
    }

    /// The ReplayGain volume of the song heard, set now (the settings changed).
    pub(crate) fn set_level(&mut self, level: f32) {
        self.level = level;
        if let Some(p) = self.placed.front_mut() {
            p.level = level;
        }
        self.volume();
    }

    fn volume(&mut self) {
        if self.open.is_some() {
            self.out.set_volume(self.gain * self.level);
        }
    }

    fn follow_fade(&mut self, now_ms: i64) {
        let Some(f) = self.fade else { return };
        let (v, done) = nori_player::transport::fade_step(f.from, f.to, f.start_ms, now_ms, f.ms);
        self.gain = v;
        if done {
            self.fade = None;
        }
        self.volume();
    }

    fn rate(&self) -> u32 {
        self.open.map_or(48_000, |o| o.0.rate).max(1)
    }

    /// The furthest the ear can be by the clock: where it was at the last reading that made sense (or
    /// when the track began playing), moved on by the time since at the output's pace, and a little.
    fn most(&self) -> u64 {
        let rate = self.rate() as f64 * self.out.pace();
        let run = |ms: i64| (ms.max(0) as f64 * rate / 1000.0) as u64;
        match self.clock {
            Some((t, at)) => at + run(self.now_ms - t + CLOCK_SLACK_MS),
            None => self.heard_at + run(CLOCK_SLACK_MS),
        }
    }

    /// Frames presented now, as a count that only grows and never runs ahead of the clock. A reading
    /// that makes no sense leaves the ear where it was, and is a strike.
    fn read_head(&mut self) -> u64 {
        if self.open.is_none() {
            return self.heard_at;
        }
        let Some(raw) = self.out.head() else {
            self.strike("the platform's play head could not be read".into());
            return self.heard_at;
        };
        self.raw = Some(raw);
        let most = self.most();
        let counted = self.head.base + self.head.last;
        // A count started again at a join, only where the clock says the ear may be by now.
        let join = self.placed.iter().map(|p| p.start).find(|&s| s > counted).filter(|&s| s <= most);
        match self.head.read(raw, join, most) {
            Ok((at, seen)) => {
                match seen {
                    Seen::Dip => self.note(format!("the play head read {raw} after {}, not at a join: looked at again", self.head.last)),
                    Seen::Restarted => self.note(format!("the play head counts again from nought ({raw}): the count goes on from {counted} frames")),
                    Seen::Fine | Seen::Joined => {}
                }
                self.heard_at = at.min(self.written_frames).max(self.heard_at);
                if seen != Seen::Dip {
                    self.strikes = 0;
                    self.clock = Some((self.now_ms, self.heard_at));
                }
            }
            Err(at) => {
                let ms = |f: u64| f as i64 * 1000 / self.rate() as i64;
                self.strike(format!("the play head read {raw} frames, {} ms in, ahead of the clock's {} ms", ms(at), ms(most)));
            }
        }
        self.heard_at
    }

    /// A reading that made no sense, or an end of stream refused.
    fn strike(&mut self, why: String) {
        self.strikes += 1;
        self.note(format!("{why} ({} of {STRIKES})", self.strikes));
        self.strike_why = why;
    }

    fn note(&mut self, what: String) {
        self.out.note(&what);
    }

    /// The song the ear is on, where in it (ms), and which placing of it: the songs before it are gone.
    pub(crate) fn heard(&mut self) -> Option<(usize, i64, u64)> {
        let at = self.heard_at;
        while self.placed.len() > 1 && self.placed[1].start <= at {
            self.placed.pop_front();
            let level = self.placed[0].level;
            self.level = level;
            self.volume();
        }
        let p = self.placed.front()?;
        let rate = self.rate() as i64;
        Some((p.index, p.from_ms + (at.saturating_sub(p.start) as i64) * 1000 / rate, p.seq))
    }

    /// The song the ear is on, without reading anything.
    pub(crate) fn current(&self) -> Option<usize> {
        self.placed.front().map(|p| p.index).or(self.starting.as_ref().map(|s| s.0))
    }

    /// Music of what was written still to be heard, µs.
    fn in_track_us(&self) -> i64 {
        (self.written_frames.saturating_sub(self.heard_at) as i128 * 1_000_000 / self.rate() as i128) as i64
    }

    /// Everything written has been heard and nothing more comes: what follows, if anything was set.
    pub(crate) fn done(&mut self) -> Option<Tail> {
        let tail = self.tail?;
        if self.writing.is_some() || self.staged < self.stage.len() {
            return None;
        }
        let end = self.written_frames;
        let us = |us: i64| (us * self.rate() as i64 / 1_000_000) as u64;
        let by_head = self.heard_at + us(END_SLACK_US) >= end;
        // The platform's word that it played to the end of stream counts only near the end (it says so
        // at every join as well, for the song before it), and only for the end of stream said last with
        // nothing written since.
        let by_word = !by_head && self.eos_at == Some(end) && self.heard_at + us(PRESENTED_NEAR_US) >= end && self.out.presented();
        if !by_head && !by_word {
            return None;
        }
        if std::mem::replace(&mut self.end_noted, true) {
            return Some(tail);
        }
        let ms = |f: u64| f as i64 * 1000 / self.rate() as i64;
        let (id, start) = self.placed.front().map_or((String::new(), 0), |p| (p.id.clone(), p.start));
        let raw = self.raw.map_or("nothing".into(), |r| r.to_string());
        let what = format!(
            "{id} ended {}: the play head read {raw}, {} ms heard of {} ms written for it, its end of stream {}",
            if by_head { "by the play head" } else { "by the platform's word that it presented everything" },
            ms(self.heard_at.saturating_sub(start)),
            ms(end.saturating_sub(start)),
            if self.eos_at == Some(end) { "said" } else { "not said" },
        );
        self.note(what);
        Some(tail)
    }

    /// The next song in play order after `i`, the one arriving on would not skip; none after the song the
    /// music stops at.
    fn next_of<Q: Queue>(&self, i: usize, queue: &Q) -> Option<usize> {
        if self.stop_after == Some(i) {
            return None;
        }
        let first = queue.read(|q| q.next_of(i, q.repeat()))?;
        let mut at = first;
        for _ in 0..queue.read(|q| q.len()) {
            if !queue.skips(at) {
                return Some(at);
            }
            match queue.read(|q| q.next_of(at, q.repeat())) {
                Some(n) if n != first => at = n,
                _ => return Some(at),
            }
        }
        Some(at)
    }

    /// One turn: the song starting is placed once it is open, the track topped up when it is due, the
    /// fade moved on. `gain` says each song's ReplayGain volume.
    pub(crate) fn turn<L: Library, Q: Queue>(&mut self, now_ms: i64, tracks: &mut Sources<L>, queue: &Q, gain: &mut dyn FnMut(usize, &str) -> f32) -> Step {
        self.now_ms = now_ms;
        self.follow_fade(now_ms);
        if self.open.is_some() && self.out.torn_down() {
            return self.fallback(true);
        }
        if let Some(step) = self.begin(tracks, queue, gain) {
            return step;
        }
        self.read_head();
        if self.eos_due && self.playing {
            self.end_stream();
        }
        if self.strikes >= STRIKES || self.eos_refusals >= STRIKES {
            let why = std::mem::take(&mut self.strike_why);
            self.note(format!("offload given up, the CPU plays on: {why}"));
            let step = self.fallback(true);
            self.on_cpu = Some(OnCpu::Head(why));
            return step;
        }
        if self.placed.is_empty() || self.in_track_us() >= self.low_us() {
            return Step::Fine;
        }
        match self.fill(tracks, queue, gain) {
            Ok(()) => Step::Fine,
            Err(_) => self.fallback(true),
        }
    }

    /// The track is topped up when it holds less than this, µs: [`LOW_US`], or half of a track too small
    /// for that (as its bytes and the songs' bitrate make it), so it is never looked at for nothing.
    fn low_us(&self) -> i64 {
        let Some((_, _, bytes)) = self.open else { return LOW_US };
        if self.written_bytes == 0 || self.written_frames == 0 {
            return LOW_US;
        }
        let holds = bytes as i128 * self.written_frames as i128 / self.written_bytes as i128 * 1_000_000 / self.rate() as i128;
        LOW_US.min((holds / 2) as i64)
    }

    /// The CPU takes over where the ear is.
    fn fallback(&mut self, refused: bool) -> Step {
        if refused {
            self.on_cpu = Some(OnCpu::Failed);
        }
        let at = self.heard().map(|(i, ms, _)| (i, ms)).or(self.starting.as_ref().map(|s| (s.0, s.1)));
        self.release();
        match at {
            Some((index, ms)) => Step::ToPcm { index, ms, refused },
            None => Step::Fine,
        }
    }

    /// The song starting, once it is open: placed on a track for its format, or handed to the CPU.
    fn begin<L: Library, Q: Queue>(&mut self, tracks: &mut Sources<L>, queue: &Q, gain: &mut dyn FnMut(usize, &str) -> f32) -> Option<Step> {
        let (i, ms) = self.starting.as_ref().map(|s| (s.0, s.1))?;
        let ready = match &mut self.starting.as_mut().expect("checked").2 {
            Ok(r) => r.ready(),
            Err(_) => true,
        };
        if !ready {
            self.waiting = true;
            return Some(Step::Fine);
        }
        self.waiting = false;
        let (_, _, opened) = self.starting.take().expect("checked");
        let to_pcm = Some(Step::ToPcm { index: i, ms, refused: false });
        let Ok(r) = opened else {
            self.on_cpu = Some(OnCpu::Unread);
            return to_pcm;
        };
        let album = self.in_album(i, tracks, queue);
        if let Some(why) = self.refuses(&r, album) {
            self.on_cpu = Some(why);
            return to_pcm;
        }
        let Some(song) = r.coded().cloned() else { return to_pcm };
        let coded = song.coded;
        if self.open.is_none_or(|o| o.0 != coded) {
            if self.open.take().is_some() {
                self.out.close();
                self.started = false;
            }
            let bps = if song.bitrate > 0 { song.bitrate } else { GUESS_BPS };
            let bytes = ((bps as i128 * TRACK_US as i128 / 8_000_000) as usize).clamp(MIN_BYTES, MAX_BYTES);
            match self.out.open(coded, bytes) {
                Ok(held) => {
                    let gapless = self.support(coded) == Support::Gapless;
                    self.open = Some((coded, gapless, held.max(1)));
                }
                Err(_) => {
                    self.on_cpu = Some(OnCpu::WouldNotOpen(coded));
                    return Some(Step::ToPcm { index: i, ms, refused: true });
                }
            }
        }
        let gapless = self.open.is_some_and(|o| o.1);
        self.gapped = (!gapless && (song.delay != 0 || song.padding != 0)).then(|| {
            let said = self.out.said(coded).map(|s| format!(" ({s})")).unwrap_or_default();
            format!("its encoder delay of {} and padding of {} heard as a moment of near silence at its ends: no song of its album joins it, and the output does not do gapless offload{said}", song.delay, song.padding)
        });
        let id = queue.read(|q| q.ids()[i].clone());
        // A song started part way in has no delay left to cut.
        let delay = if song.from_frame > 0 { 0 } else { song.delay };
        self.out.delay_padding(delay, song.padding);
        let from_ms = song.from_frame * 1000 / coded.rate.max(1) as i64;
        let level = gain(i, &id);
        self.level = level;
        self.seq += 1;
        self.placed.push_back(Placed { index: i, id, start: 0, frames: None, from_ms, level, seq: self.seq });
        let ogg = (coded.coding == Coding::Opus).then(|| Ogg::new(song.setup.as_deref()));
        self.writing = Some(Writing { r, frames: 0, ogg });
        self.volume();
        if self.fill(tracks, queue, gain).is_err() {
            return Some(self.fallback(true));
        }
        if self.playing {
            self.out.play();
            self.started = true;
            self.clock = Some((self.now_ms, self.heard_at));
            if self.eos_due {
                self.end_stream();
            }
        }
        Some(Step::Fine)
    }

    /// Writes into the track what it has room for: the rest of the song being written, and the songs
    /// after it that join it without a gap. Err when the track refused a write with an error.
    fn fill<L: Library, Q: Queue>(&mut self, tracks: &mut Sources<L>, queue: &Q, gain: &mut dyn FnMut(usize, &str) -> f32) -> Result<(), i32> {
        loop {
            // The track holds minutes at most, whatever the platform would take: the rest waits for a
            // top-up.
            if self.in_track_us() >= TRACK_US {
                return Ok(());
            }
            if self.staged < self.stage.len() {
                if !self.write_staged()? {
                    return Ok(());
                }
                continue;
            }
            if let Some(w) = self.writing.as_mut() {
                if !w.r.ready() {
                    self.waiting = true;
                    return Ok(());
                }
                self.waiting = false;
                self.stage.clear();
                self.staged = 0;
                self.stage_frames = 0;
                while self.stage.len() < STAGE_BYTES && w.r.packet() {
                    let frames = w.r.packet_frames();
                    match w.ogg.as_mut() {
                        Some(ogg) => ogg.page(w.r.buffer(), &mut self.stage),
                        None => self.stage.extend_from_slice(w.r.buffer()),
                    }
                    self.stage_frames += frames;
                    w.frames += frames;
                    if !w.r.ready() {
                        break;
                    }
                }
                if !self.stage.is_empty() {
                    continue;
                }
                if !w.r.ready() {
                    continue;
                }
                // Read to its end: its length is known, and what follows is looked at.
                let frames = w.frames;
                if let Some((_, why)) = w.r.error() {
                    let id = self.placed.back().map(|p| p.id.clone()).unwrap_or_default();
                    let ms = frames as i64 * 1000 / self.rate() as i64;
                    self.note(format!("{id} was read to an early end at {ms} ms: {why}"));
                }
                self.writing = None;
                if let Some(p) = self.placed.back_mut() {
                    p.frames = Some(frames);
                }
                let last = self.placed.back().map(|p| p.index).expect("a song is placed");
                match self.next_of(last, queue) {
                    None => self.close(Tail::End),
                    Some(n) => {
                        let id = queue.read(|q| q.ids()[n].clone());
                        self.next = Some((n, tracks.open_packets(&id, 0, false)));
                    }
                }
                continue;
            }
            // The next song is written once the track runs low, so an edit of the queue before then
            // costs nothing; the whole of it is here by then (fetched as the song before began).
            if self.next.is_none() || self.in_track_us() >= self.low_us() {
                return Ok(());
            }
            let (n, opened) = self.next.as_mut().expect("checked");
            let n = *n;
            let song = match opened {
                Ok(r) => {
                    if !r.ready() {
                        self.waiting = true;
                        return Ok(());
                    }
                    r.error().is_none().then(|| r.coded().cloned()).flatten()
                }
                Err(_) => None,
            };
            let joins = self.open.is_some_and(|(c, gapless, _)| gapless && song.as_ref().is_some_and(|s| s.coded == c));
            let Some(song) = song.filter(|_| joins) else {
                // Another format, or not for this output: the track plays out, and then it is decided.
                self.next = None;
                self.close(Tail::Then(n));
                return Ok(());
            };
            if self.pending_eos {
                // The song before is closed first: the platform takes that only while the track plays.
                self.end_stream();
                if self.pending_eos {
                    return Ok(());
                }
            }
            let (_, opened) = self.next.take().expect("checked");
            let r = opened.expect("checked");
            self.out.delay_padding(song.delay, song.padding);
            let id = queue.read(|q| q.ids()[n].clone());
            let level = gain(n, &id);
            let start = self.written_frames;
            self.seq += 1;
            let from_ms = song.from_frame * 1000 / song.coded.rate.max(1) as i64;
            self.placed.push_back(Placed { index: n, id, start, frames: None, from_ms, level, seq: self.seq });
            let ogg = (song.coded.coding == Coding::Opus).then(|| Ogg::new(song.setup.as_deref()));
            self.writing = Some(Writing { r, frames: 0, ogg });
        }
    }

    /// Nothing more is written after what is: `tail` follows once it has played.
    fn close(&mut self, tail: Tail) {
        self.end_stream();
        self.tail = Some(tail);
    }

    /// The last packet written closes its song: said to the platform while the track plays, or once it
    /// does (Android refuses it otherwise). A refusal while playing is a strike.
    fn end_stream(&mut self) {
        if !self.pending_eos {
            self.eos_due = false;
            return;
        }
        if !self.playing || !self.started || self.open.is_none() {
            self.eos_due = true;
            return;
        }
        if self.out.end_of_stream() {
            self.pending_eos = false;
            self.eos_due = false;
            self.eos_refusals = 0;
            self.eos_at = Some(self.written_frames);
        } else {
            self.eos_due = true;
            self.eos_refusals += 1;
            let why = "the platform would not take the end of stream while its track played";
            self.note(format!("{why} ({} of {STRIKES})", self.eos_refusals));
            self.strike_why = why.into();
        }
    }

    /// Writes what is staged. False when the track would not take all of it (it is full).
    fn write_staged(&mut self) -> Result<bool, i32> {
        let left = self.stage.len() - self.staged;
        let frames = self.stage_frames;
        let taken = self.out.write(&self.stage[self.staged..], frames)?.min(left);
        self.full = taken < left;
        // The frames go with the bytes in proportion; they are exact once the whole stage is taken.
        let part = if taken == left { frames } else { (frames as u128 * taken as u128 / left as u128) as u64 };
        self.stage_frames -= part;
        self.written_frames += part;
        self.written_bytes += taken as u64;
        self.staged += taken;
        if taken > 0 {
            self.pending_eos = true;
        }
        Ok(taken == left)
    }

    /// How long the thread may sleep before this path needs it, ms; none when nothing is due.
    pub(crate) fn wake_in(&self) -> Option<i64> {
        let mut d: Option<i64> = None;
        let mut at = |ms: i64| d = Some(d.map_or(ms, |x| x.min(ms)));
        if self.fade.is_some() {
            at(nori_player::transport::FADE_TICK_MS);
        }
        if self.starting.is_some() || self.waiting {
            // The loader wakes the thread when the bytes are there; this is in case it never does.
            at(1_000);
        }
        if !self.playing || self.placed.is_empty() {
            return d;
        }
        if self.strikes > 0 || self.head.lower.is_some() || self.eos_due {
            at(LOOK_AGAIN_MS);
        }
        let rate = self.rate() as i64;
        let ms = |frames: u64| frames as i64 * 1000 / rate;
        // The ear reaching the next song.
        if let Some(p) = self.placed.get(1) {
            at(ms(p.start.saturating_sub(self.heard_at)) + 5);
        }
        let more = self.writing.is_some() || self.next.is_some() || self.staged < self.stage.len();
        if more {
            // A track that refused a write while it seemed low holds more than its bytes say: a second.
            let floor = if self.full { 1_000 } else { 1 };
            at(((self.in_track_us() - self.low_us()) / 1000).max(0) + floor);
        } else if self.tail.is_some() {
            let end = ms(self.written_frames.saturating_sub(self.heard_at));
            at(if end > 0 { end + 5 } else { END_LOOK_MS });
        }
        d
    }

    /// The music stops at the end of the song heard (the sleep timer's "end of this song"), or goes on
    /// again (`false`). True when a song after it is written already, which cannot be taken back: the
    /// track then starts again where the ear is, and the stop is set again on it.
    pub(crate) fn pause_at_end<L: Library, Q: Queue>(&mut self, on: bool, tracks: &mut Sources<L>, queue: &Q) -> bool {
        let Some(c) = self.current() else { return false };
        self.stop_after = on.then_some(c);
        if on && self.placed.len() > 1 {
            return true;
        }
        if self.writing.is_some() {
            return false;
        }
        if on {
            self.next = None;
            self.close(Tail::End);
        } else if self.tail == Some(Tail::End) {
            // Taken back before the end: what follows is written after all.
            self.tail = None;
            match self.next_of(c, queue) {
                Some(n) => {
                    let id = queue.read(|q| q.ids()[n].clone());
                    self.next = Some((n, tracks.open_packets(&id, 0, false)));
                }
                None => self.close(Tail::End),
            }
        }
        false
    }

    /// The queue changed (an edit, shuffle, repeat): what the track holds is found again by id, and the
    /// song after the last one written is looked at again. A song already written after the one heard
    /// that no longer follows it cannot be taken back: the track starts again where the ear is (true).
    pub(crate) fn queue_changed<L: Library, Q: Queue>(&mut self, old: &[String], tracks: &mut Sources<L>, queue: &Q) -> bool {
        let new: Vec<String> = queue.read(|q| q.ids().to_vec());
        for p in self.placed.iter_mut() {
            match moved(old, &new, p.index, &p.id) {
                Some(k) => p.index = k,
                None => return true,
            }
        }
        if let Some(s) = self.starting.as_mut() {
            let id = old.get(s.0).cloned().unwrap_or_default();
            match moved(old, &new, s.0, &id) {
                Some(k) => s.0 = k,
                None => return true,
            }
        }
        for k in 1..self.placed.len() {
            if self.next_of(self.placed[k - 1].index, queue) != Some(self.placed[k].index) {
                return true;
            }
        }
        if self.writing.is_some() {
            return false;
        }
        let Some(last) = self.placed.back().map(|p| p.index) else { return false };
        let after = self.next_of(last, queue);
        let was = self.next.as_ref().map(|(n, _)| *n).or(match self.tail {
            Some(Tail::Then(n)) => Some(n),
            _ => None,
        });
        if was == after {
            return false;
        }
        // Another song follows now (or none): it is opened, and written at the join as any other.
        self.next = None;
        self.tail = None;
        match after {
            Some(n) => {
                let id = new[n].clone();
                self.next = Some((n, tracks.open_packets(&id, 0, false)));
            }
            None => self.close(Tail::End),
        }
        false
    }
}

/// Where the song at index `i` of `old` (`id`) is in `new`: the same id, nearest to where it was.
fn moved(old: &[String], new: &[String], i: usize, id: &str) -> Option<usize> {
    let id = old.get(i).map_or(id, String::as_str);
    new.iter().enumerate().filter(|(_, n)| *n == id).map(|(j, _)| j).min_by_key(|&j| j.abs_diff(i))
}

impl Drop for Offload {
    fn drop(&mut self) {
        if self.open.take().is_some() {
            self.out.close();
        }
    }
}

/// Opus packets put in Ogg pages for the chip, as media3's `OggOpusAudioPacketizer` does: the stream's
/// header (`OpusHead`) and an empty comment header first, then each packet in a page of its own, stamped
/// with the samples decoded up to its end.
pub(crate) struct Ogg {
    head: Option<Vec<u8>>,
    sequence: u32,
    granule: u64,
}

/// The stream every page belongs to.
const OGG_SERIAL: u32 = 0;

impl Ogg {
    pub(crate) fn new(opus_head: Option<&[u8]>) -> Ogg {
        let head = opus_head.filter(|h| h.starts_with(b"OpusHead")).map(<[u8]>::to_vec).unwrap_or_else(|| {
            // A plain stereo header: what media3 writes when the stream brought none.
            let mut h = b"OpusHead".to_vec();
            h.extend_from_slice(&[1, 2, 0x38, 0x01, 0x80, 0xbb, 0, 0, 0, 0, 0]);
            h
        });
        Ogg { head: Some(head), sequence: 0, granule: 0 }
    }

    /// `packet` as a page (after the two header pages, before the first one) onto `out`.
    pub(crate) fn page(&mut self, packet: &[u8], out: &mut Vec<u8>) {
        if let Some(head) = self.head.take() {
            self.write(&head, 0x02, 0, out);
            let mut tags = b"OpusTags".to_vec();
            tags.extend_from_slice(&[0; 8]);
            self.write(&tags, 0, 0, out);
        }
        self.granule += opus_samples(packet);
        let granule = self.granule;
        self.write(packet, 0, granule, out);
    }

    fn write(&mut self, packet: &[u8], flags: u8, granule: u64, out: &mut Vec<u8>) {
        let from = out.len();
        let lacing = packet.len() / 255 + 1;
        out.extend_from_slice(b"OggS");
        out.push(0);
        out.push(flags);
        out.extend_from_slice(&granule.to_le_bytes());
        out.extend_from_slice(&OGG_SERIAL.to_le_bytes());
        out.extend_from_slice(&self.sequence.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.push(lacing as u8);
        for _ in 0..lacing - 1 {
            out.push(255);
        }
        out.push((packet.len() % 255) as u8);
        out.extend_from_slice(packet);
        let crc = ogg_crc(&out[from..]);
        out[from + 22..from + 26].copy_from_slice(&crc.to_le_bytes());
        self.sequence += 1;
    }
}

/// Samples (at 48 kHz) an Opus packet decodes to, from its table of contents (RFC 6716, 3.1).
pub(crate) fn opus_samples(packet: &[u8]) -> u64 {
    let Some(&toc) = packet.first() else { return 0 };
    let config = toc >> 3;
    let frame: u64 = match config {
        0..=11 => [480, 960, 1920, 2880][(config % 4) as usize],
        12..=15 => [480, 960][(config % 2) as usize],
        _ => [120, 240, 480, 960][(config % 4) as usize],
    };
    let frames = match toc & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => packet.get(1).map_or(0, |b| (b & 0x3f) as u64),
    };
    frame * frames
}

/// Ogg's page checksum: CRC-32 with the polynomial 0x04c11db7, unreflected, from nought.
fn ogg_crc(page: &[u8]) -> u32 {
    let mut crc = 0u32;
    for &b in page {
        crc ^= (b as u32) << 24;
        for _ in 0..8 {
            crc = if crc & 0x8000_0000 != 0 { (crc << 1) ^ 0x04c1_1db7 } else { crc << 1 };
        }
    }
    crc
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opus_packets_go_in_ogg_pages_with_their_samples_counted() {
        let mut ogg = Ogg::new(Some(b"OpusHead\x01\x02\x38\x01\x80\xbb\0\0\0\0\0"));
        let mut out = Vec::new();
        // A 20 ms packet (config 1: SILK 20 ms), one frame.
        ogg.page(&[0x08, 1, 2, 3], &mut out);
        // Three pages: the header, the comment header, the packet.
        let pages: Vec<usize> = out.windows(4).enumerate().filter(|(_, w)| *w == b"OggS").map(|(i, _)| i).collect();
        assert_eq!(pages.len(), 3);
        assert_eq!(out[pages[0] + 5], 0x02, "the first page begins the stream");
        let granule = u64::from_le_bytes(out[pages[2] + 6..pages[2] + 14].try_into().unwrap());
        assert_eq!(granule, 960, "20 ms at 48 kHz");
        // Each page's checksum is Ogg's own: computed with its checksum field zeroed.
        for (k, &p) in pages.iter().enumerate() {
            let end = pages.get(k + 1).copied().unwrap_or(out.len());
            let mut page = out[p..end].to_vec();
            let said = u32::from_le_bytes(page[22..26].try_into().unwrap());
            page[22..26].fill(0);
            assert_eq!(ogg_crc(&page), said);
        }
        // A packet of 255 bytes or more takes more than one lacing value.
        let mut out = Vec::new();
        ogg.page(&vec![0x08; 300], &mut out);
        assert_eq!(out[26], 2, "two lacing values");
        assert_eq!((out[27], out[28]), (255, 45));
    }

    #[test]
    fn the_samples_of_an_opus_packet_come_from_its_table_of_contents() {
        assert_eq!(opus_samples(&[0x08]), 960, "SILK 20 ms");
        assert_eq!(opus_samples(&[0xfc]), 960, "CELT 20 ms");
        assert_eq!(opus_samples(&[0xf9]), 1920, "two CELT 20 ms frames");
        assert_eq!(opus_samples(&[0xfb, 0x03]), 2880, "three frames, counted in the second byte");
    }

    #[test]
    fn the_head_counts_on_through_a_join_that_starts_it_again() {
        let mut h = Head::default();
        let far = u64::MAX;
        assert_eq!(h.read(1_000, None, far), Ok((1_000, Seen::Fine)));
        assert_eq!(h.read(4_990, None, far), Ok((4_990, Seen::Fine)));
        // The platform started counting again at the join: the next song began at 5 000.
        assert_eq!(h.read(20, Some(5_000), far), Ok((5_020, Seen::Joined)));
        assert_eq!(h.read(500, None, far), Ok((5_500, Seen::Fine)));
    }

    #[test]
    fn a_lower_reading_away_from_a_join_does_not_move_the_ear() {
        let mut h = Head::default();
        let far = u64::MAX;
        assert_eq!(h.read(44_100, None, far), Ok((44_100, Seen::Fine)));
        // Nought once (a failed call read as nought, a moment's reset): the ear stays, and the count goes
        // on as it was when the next reading is back.
        assert_eq!(h.read(0, None, far), Ok((44_100, Seen::Dip)));
        assert_eq!(h.read(46_000, None, far), Ok((46_000, Seen::Fine)));
        // Counting again from nought for good (a standby): taken at the second reading, from where the
        // ear was.
        assert_eq!(h.read(10, None, far), Ok((46_000, Seen::Dip)));
        assert_eq!(h.read(900, None, far), Ok((46_900, Seen::Restarted)));
        assert_eq!(h.read(1_900, None, far), Ok((47_900, Seen::Fine)));
    }

    #[test]
    fn a_reading_ahead_of_the_clock_changes_nothing() {
        let mut h = Head::default();
        assert_eq!(h.read(1_000, None, 50_000), Ok((1_000, Seen::Fine)));
        assert_eq!(h.read(4_000_000, None, 50_000), Err(4_000_000));
        assert_eq!(h.read(2_000, None, 50_000), Ok((2_000, Seen::Fine)), "the count as it was");
        // A join the clock says the ear cannot have reached is not one.
        assert_eq!(h.read(10, None, 50_000), Ok((2_000, Seen::Dip)));
    }
}
