//! The perf build's invariant watchdogs: things that must always hold while the app plays, checked as
//! the app's own events and wakes go by, and said loudly on the perf timeline ("invariant" events, which
//! the report lists first) and in the log when one does not.
//!
//! Nothing here ticks. Each check is made when something already happened: the engine's thread woke
//! (nori-engine's watch hook), the AudioTrack's writer woke to top the track up, the player service said
//! a song or a skip, the settings changed, the screen was told a song. With the watch off (every build
//! but the perf one) each of those costs one atomic read. What holds:
//!
//! - an output that holds music plays it: while playing, the frames it presented move on within
//!   [`STILL_MS`] (an offloaded track that does not is starved: the S22's silent offload);
//! - one press of a skip moves one song;
//! - the song on the screen is the one heard, give or take [`DIFFER_MS`];
//! - the lyrics shown are the song heard's;
//! - with AutoMix on, every song in the queue has a length (the planner has nothing to plan from without);
//! - a setting changed is in the engine a second later, judged only while it plays through an open output
//!   and [`SETTLE_MS`] after the player service started or ended (an engine switch restarts it).
//!
//! [`Watch`] is the bookkeeping, plain and testable; the functions below keep one for the process and are
//! what the engine's hook, the track and the platform call.

use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

use std::sync::{Mutex, MutexGuard};

/// Playing, an output that presents nothing new for longer than this has stopped.
pub const STILL_MS: i64 = 2_000;
/// The screen may trail the ear by this much at a song change.
pub const DIFFER_MS: i64 = 1_000;
/// Presses closer together than this are one run of skips.
pub const RUN_MS: i64 = 3_000;
/// After the player service starts or ends (an engine switch does both) its settings are not judged for
/// this long: the service is still building its player, and a batch of settings arrived with it.
pub const SETTLE_MS: i64 = 3_000;
/// The most breaks kept for the self test to read back.
pub const MOST_BREAKS: usize = 50;

/// One invariant that did not hold: which, and what was seen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Break {
    pub kind: &'static str,
    pub detail: String,
}

impl Break {
    fn new(kind: &'static str, detail: String) -> Break {
        Break { kind, detail }
    }

    /// "offload-starved: ..." as the timeline and the log say it.
    pub fn line(&self) -> String {
        format!("{}: {}", self.kind, self.detail)
    }
}

/// How far an output has come, as one reading: `presented` and `written` in units of which there are
/// `rate` a second (frames, or ms). A new `song` (or a count that went back: a flush) starts afresh.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Moving {
    pub now_ms: i64,
    pub playing: bool,
    pub offloaded: bool,
    pub song: Option<usize>,
    pub written: u64,
    pub presented: u64,
    pub rate: u32,
}

#[derive(Debug, Clone, Copy)]
struct Progress {
    song: Option<usize>,
    presented: u64,
    since_ms: i64,
    said: bool,
}

/// A run of skips: the song it started from, the presses since and when the last was.
#[derive(Debug, Clone, Copy)]
struct Skips {
    from: i64,
    presses: u32,
    last_ms: i64,
    said: bool,
}

/// The watch's bookkeeping. Every method takes the time it is told at (wall clock, ms, or the output's
/// own clock for [`Watch::output`]) and answers the invariant that did not hold, if one did not.
#[derive(Default)]
pub struct Watch {
    outputs: Vec<(String, Progress)>,
    heard: Option<(String, i64)>,
    shown: Option<String>,
    differ_since: Option<i64>,
    differ_said: bool,
    /// Nobody can see the screen (the app is in the background, or the screen is off): what it shows is
    /// not compared, since nothing on it is drawn or brought up to date until it is seen again.
    hidden: bool,
    skips: Option<Skips>,
    queue_said: Vec<String>,
}

impl Watch {
    /// An output's reading, under `key` ("engine", "track"): playing, its count of what it
    /// presented must move while it holds music written and not presented.
    pub fn output(&mut self, key: &str, m: &Moving) -> Option<Break> {
        let i = match self.outputs.iter().position(|(k, _)| k == key) {
            Some(i) => i,
            None => {
                self.outputs.push((key.to_string(), Progress { song: m.song, presented: m.presented, since_ms: m.now_ms, said: false }));
                return None;
            }
        };
        let p = &mut self.outputs[i].1;
        if !m.playing || m.song != p.song || m.presented < p.presented {
            *p = Progress { song: m.song, presented: m.presented, since_ms: m.now_ms, said: false };
            return None;
        }
        if m.presented > p.presented {
            *p = Progress { song: m.song, presented: m.presented, since_ms: m.now_ms, said: false };
            return None;
        }
        let still = m.now_ms - p.since_ms;
        // Nothing written ahead is an output with nothing to play (a song's bytes awaited): not this one's.
        if still <= STILL_MS || p.said || m.written <= m.presented {
            return None;
        }
        p.said = true;
        let ms = |n: u64| n as i128 * 1000 / m.rate.max(1) as i128;
        let held = ms(m.written - m.presented);
        let (kind, what) = if m.offloaded { ("offload-starved", "the offloaded track") } else { ("stalled", "the output") };
        Some(Break::new(
            kind,
            format!("{key}: playing, but {what} presented nothing new for {still} ms, with {held} ms written and not presented (at {} ms of {} ms written)", ms(m.presented), ms(m.written)),
        ))
    }

    /// The song heard changed to `id` (the player service's word).
    pub fn heard(&mut self, now: i64, id: &str) -> Option<Break> {
        if self.heard.as_ref().is_none_or(|(h, _)| h != id) {
            self.heard = Some((id.to_string(), now));
        }
        self.compare(now)
    }

    /// The screen shows `id` now (none: nothing).
    pub fn shown(&mut self, now: i64, id: Option<&str>) -> Option<Break> {
        self.shown = id.map(str::to_string);
        self.compare(now)
    }

    /// Whether the screen can be seen: the app in the foreground with the screen on. A difference is only
    /// counted while it can, and from the moment it came back, so the time it spent off is not the
    /// screen's lateness, and it has the same [`DIFFER_MS`] to catch up as after any change of song.
    pub fn visible(&mut self, now: i64, on: bool) -> Option<Break> {
        if !on {
            self.hidden = true;
            self.differ_since = None;
            return None;
        }
        if self.hidden {
            self.hidden = false;
            self.differ_since = None;
        }
        self.compare(now)
    }

    /// Whether the screen and the ear agree, looked at now: a difference said once, when it has lasted
    /// longer than [`DIFFER_MS`]. The player says itself which song is heard, a mix's too, so the screen
    /// follows the ear through one with no grace of its own.
    pub fn compare(&mut self, now: i64) -> Option<Break> {
        let (Some((heard, _)), Some(shown)) = (&self.heard, &self.shown) else {
            self.differ_since = None;
            return None;
        };
        if self.hidden {
            return None;
        }
        if heard == shown {
            self.differ_since = None;
            self.differ_said = false;
            return None;
        }
        let since = *self.differ_since.get_or_insert(now);
        if self.differ_said || now - since <= DIFFER_MS {
            return None;
        }
        self.differ_said = true;
        Some(Break::new("shown-heard", format!("the screen showed {shown} while {heard} was heard, for {} ms", now - since)))
    }

    /// The user pressed next or previous on the song at `index` (its place in the queue).
    pub fn skip(&mut self, now: i64, index: i64) {
        match &mut self.skips {
            Some(s) if now - s.last_ms <= RUN_MS => {
                s.presses += 1;
                s.last_ms = now;
            }
            _ => self.skips = Some(Skips { from: index, presses: 1, last_ms: now, said: false }),
        }
    }

    /// The player arrived on the song at `index`: by itself (`auto`, a song that ended), or by a jump.
    /// A run of skips that moved further than it was pressed is a break. Under shuffle the places in the
    /// queue say nothing of how far the order moved, and nothing is judged.
    pub fn arrived(&mut self, now: i64, index: i64, auto: bool, shuffled: bool) -> Option<Break> {
        if auto || shuffled {
            self.skips = None;
            return None;
        }
        let s = self.skips.as_mut()?;
        if now - s.last_ms > RUN_MS {
            self.skips = None;
            return None;
        }
        let moved = (index - s.from).unsigned_abs();
        if moved <= s.presses as u64 || s.said {
            return None;
        }
        s.said = true;
        let presses = s.presses;
        Some(Break::new("skip", format!("{presses} skip press{} moved {moved} songs, from queue place {} to {index}", if presses == 1 { "" } else { "es" }, s.from)))
    }

    /// Lyrics of the song `id` went up on the screen.
    pub fn lyrics(&mut self, now: i64, id: &str) -> Option<Break> {
        let (heard, since) = self.heard.as_ref()?;
        // Shown just as the song changed: the screen follows a moment later, and takes them down.
        if heard == id || now - since <= DIFFER_MS {
            return None;
        }
        Some(Break::new("lyrics", format!("lyrics of {id} shown while {heard} is heard (since {} ms)", now - since)))
    }

    /// The queue as it is now: with AutoMix on, the songs in it without a length (`missing`, ids) of
    /// `total`. Said once for each set of songs.
    pub fn queue(&mut self, auto_mix: bool, missing: &[String], total: u32) -> Option<Break> {
        if !auto_mix || missing.is_empty() {
            self.queue_said.clear();
            return None;
        }
        if self.queue_said == missing {
            return None;
        }
        self.queue_said = missing.to_vec();
        let named: Vec<&str> = missing.iter().take(5).map(String::as_str).collect();
        let more = if missing.len() > 5 { format!(" and {} more", missing.len() - 5) } else { String::new() };
        Some(Break::new("queue-duration", format!("AutoMix is on and {} of {total} songs in the queue have no length: {}{more}", missing.len(), named.join(", "))))
    }
}

/// Whether the engine can be held to the settings now: only while it plays through an open output (paused,
/// or with the output let go, the sound chain is not in any path) and not within [`SETTLE_MS`] of the
/// player service starting or ending (`engine_since`).
pub fn settings_judged(now: i64, playing: bool, output_open: bool, engine_since: Option<i64>) -> bool {
    playing && output_open && engine_since.is_none_or(|t| now - t > SETTLE_MS)
}

/// A setting a second after it changed, against what the engine shows: each pair that disagrees.
pub fn settings_held(expected: &[(&str, bool, bool)]) -> Option<Break> {
    let off: Vec<String> = expected.iter().filter(|(_, want, got)| want != got).map(|(what, want, got)| format!("{what}: {got}, expected {want}")).collect();
    if off.is_empty() {
        return None;
    }
    Some(Break::new("setting", format!("a second after the settings changed the engine still shows {}", off.join("; "))))
}

// ---- the process's watch ----

static ON: AtomicBool = AtomicBool::new(false);
static WATCH: Mutex<Option<Watch>> = Mutex::new(None);
static BREAKS: Mutex<Vec<String>> = Mutex::new(Vec::new());
static ENGINE: Mutex<Option<PerfEngineSeen>> = Mutex::new(None);
/// The self test's volume on every output, as f32 bits: 1 unless it is running quietly.
/// When the player service last started or ended, wall ms; `i64::MIN` for never.
static ENGINE_SINCE: AtomicI64 = AtomicI64::new(i64::MIN);
static QUIET: AtomicU32 = AtomicU32::new(0x3F80_0000);

/// Whether the watch is on: one atomic read, for every caller to ask first.
#[inline]
pub fn on() -> bool {
    ON.load(Ordering::Relaxed)
}

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// A lock that a panic while it was held does not poison for good: the watch must never stop the app.
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

fn with<R>(f: impl FnOnce(&mut Watch) -> R) -> R {
    f(lock(&WATCH).get_or_insert_with(Watch::default))
}

/// A break goes on the timeline and in the log, loudly, and is kept for the self test.
fn said(t: i64, b: Option<Break>) {
    let Some(b) = b else { return };
    let line = b.line();
    nori_model::alog::info(&format!("invariant: {line}"));
    crate::perf_log::note_invariant(t, &line);
    let mut kept = lock(&BREAKS);
    if kept.len() >= MOST_BREAKS {
        kept.remove(0);
    }
    kept.push(format!("{} invariant: {line}", crate::perf_log::clock_words(t)));
}

/// The track's own account of the equalizer screen's shallow buffer (how deep it was made for the output
/// it plays on and why, and any growth after it ran dry): a "tuning" event on the perf timeline. Nothing
/// outside the perf build.
pub fn tuning_said(detail: &str) {
    if on() {
        crate::perf_log::note_output(wall_ms(), "tuning", detail);
    }
}

/// The perf build switches the watch on as it starts; nothing is watched before.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch(on: bool) {
    ON.store(on, Ordering::Relaxed);
}

/// What the engine's thread saw last, while the watch is on: for the self test, which reads the engine
/// through it rather than through doors of its own.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfEngineSeen {
    /// When, wall clock ms.
    pub wall_ms: i64,
    pub playing: bool,
    pub offloaded: bool,
    /// Queue index heard, -1 none.
    pub index: i64,
    pub position_ms: i64,
    pub in_output_ms: i64,
}

/// The engine's thread woke (nori-engine's watch hook, through the Android library).
pub fn engine_seen(now_ms: i64, playing: bool, offloaded: bool, index: Option<usize>, position_ms: i64, in_output_ms: i64) {
    let t = wall_ms();
    *lock(&ENGINE) = Some(PerfEngineSeen { wall_ms: t, playing, offloaded, index: index.map_or(-1, |i| i as i64), position_ms, in_output_ms });
    // The CPU's output is watched where it is written (the track's own writer, which reads the device);
    // the engine's word counts for the offloaded one, whose play head only the engine reads.
    if !offloaded {
        return;
    }
    let pos = position_ms.max(0) as u64;
    let m = Moving { now_ms, playing, offloaded, song: index, written: pos + in_output_ms.max(0) as u64, presented: pos, rate: 1000 };
    let b = with(|w| w.output("engine", &m));
    said(t, b);
}

/// The last thing the engine's thread saw; none before it woke with the watch on.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_engine_seen() -> Option<PerfEngineSeen> {
    *lock(&ENGINE)
}

/// The AudioTrack's writer read the device: `written` frames handed to it and `presented` played, at
/// `rate` a second, `now_ms` by the monotonic clock.
pub fn track_seen(now_ms: i64, playing: bool, written: u64, presented: u64, rate: u32) {
    let m = Moving { now_ms, playing, offloaded: false, song: None, written, presented, rate };
    let b = with(|w| w.output("track", &m));
    said(wall_ms(), b);
}

/// The player service arrived on song `id`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch_heard(wall_ms: i64, id: String) {
    if !on() {
        return;
    }
    let b = with(|w| w.heard(wall_ms, &id));
    said(wall_ms, b);
}

/// The screen's player shows song `id` now.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch_shown(wall_ms: i64, id: Option<String>) {
    if !on() {
        return;
    }
    let b = with(|w| w.shown(wall_ms, id.as_deref()));
    said(wall_ms, b);
}

/// The screen can be seen (the app in the foreground, the screen on), or no longer can.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch_visible(wall_ms: i64, visible: bool) {
    if !on() {
        return;
    }
    let b = with(|w| w.visible(wall_ms, visible));
    said(wall_ms, b);
}

/// Anything else woke the platform's watcher: the screen and the ear compared at this moment too.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch_look(wall_ms: i64) {
    if !on() {
        return;
    }
    let b = with(|w| w.compare(wall_ms));
    said(wall_ms, b);
}

/// The user pressed next or previous on the song at queue place `index`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch_skip(wall_ms: i64, index: i64) {
    if on() {
        with(|w| w.skip(wall_ms, index));
    }
}

/// The player arrived on queue place `index`, by itself (`auto`) or by a jump.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch_arrived(wall_ms: i64, index: i64, auto: bool, shuffled: bool) {
    if !on() {
        return;
    }
    let b = with(|w| w.arrived(wall_ms, index, auto, shuffled));
    said(wall_ms, b);
}

/// Lyrics of song `id` went up on the screen.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch_lyrics(wall_ms: i64, id: String) {
    if !on() {
        return;
    }
    let b = with(|w| w.lyrics(wall_ms, &id));
    said(wall_ms, b);
}

/// The queue changed: the ids of its songs without a length, of `total`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch_queue(wall_ms: i64, missing: Vec<String>, total: u32) {
    if !on() {
        return;
    }
    let auto_mix = nori_settings::settings_store::current().is_some_and(|p| p.auto_mix);
    let b = with(|w| w.queue(auto_mix, &missing, total));
    said(wall_ms, b);
}

/// A second after the settings changed: what the engine shows (whether it asks for offload, whether the
/// sound chain is in the samples' path) against what the settings say it should, over the output the
/// platform sees (`usb`: something USB attached, where offload never goes). Judged only as
/// [`settings_judged`] says: `playing` through an output that is open (`output_open`).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_watch_settings(wall_ms: i64, offload_wanted: bool, chain_in: bool, usb: bool, playing: bool, output_open: bool) {
    if !on() {
        return;
    }
    let since = ENGINE_SINCE.load(Ordering::Relaxed);
    if !settings_judged(wall_ms, playing, output_open, (since != i64::MIN).then_some(since)) {
        return;
    }
    let Some(s) = nori_settings::settings_store::current() else { return };
    let want_offload = s.offload && !usb && crate::perf_log::offload_blocked().is_none();
    let mut pairs = vec![("offload wanted", want_offload, offload_wanted)];
    // With the equalizer on, the chain is in the samples' path; off, it may stay in, flat.
    if s.eq_enabled {
        pairs.push(("sound chain in the path", true, chain_in));
    }
    said(wall_ms, settings_held(&pairs));
}

/// The player service started or ended at `wall_ms` (the perf timeline's engine note).
pub(crate) fn engine_changed(wall_ms: i64) {
    ENGINE_SINCE.store(wall_ms, Ordering::Relaxed);
}

/// The invariant breaks this process said, oldest first, each "21:05:12 invariant: kind: detail".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_invariant_breaks() -> Vec<String> {
    lock(&BREAKS).clone()
}

/// The self test's volume on every output of both players (0 to 1); 1 is the music as it is. A player
/// volume, never the phone's: the other apps and the volume keys are left alone.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_quiet(level: f32) {
    QUIET.store(level.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

/// What every volume an output is set to is multiplied by: 1 outside the self test.
#[inline]
pub fn quiet() -> f32 {
    f32::from_bits(QUIET.load(Ordering::Relaxed))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(now_ms: i64, presented: u64, written: u64) -> Moving {
        Moving { now_ms, playing: true, offloaded: false, song: Some(0), written, presented, rate: 1000 }
    }

    #[test]
    fn an_output_that_holds_music_and_does_not_play_it_is_a_break_once() {
        let mut w = Watch::default();
        assert_eq!(w.output("track", &at(0, 1000, 10_000)), None);
        assert_eq!(w.output("track", &at(1000, 2000, 10_000)), None, "it moved");
        assert_eq!(w.output("track", &at(2500, 2000, 10_000)), None, "still for 1.5 s: not yet");
        let b = w.output("track", &at(3100, 2000, 10_000)).expect("still for 2.1 s");
        assert_eq!(b.kind, "stalled");
        assert!(b.detail.contains("2100 ms") && b.detail.contains("8000 ms written and not presented"), "{}", b.detail);
        assert_eq!(w.output("track", &at(9000, 2000, 10_000)), None, "said once");
        assert_eq!(w.output("track", &at(9100, 2100, 10_000)), None, "moving again");
        assert!(w.output("track", &at(11_200, 2100, 10_000)).is_some(), "a second stall is said again");
    }

    #[test]
    fn standing_still_is_fine_paused_after_a_flush_and_with_nothing_written_ahead() {
        let mut w = Watch::default();
        w.output("track", &at(0, 5000, 5000));
        assert_eq!(w.output("track", &at(9000, 5000, 5000)), None, "everything written was played: waiting for music");
        let paused = Moving { playing: false, ..at(10_000, 5000, 9000) };
        assert_eq!(w.output("track", &paused), None);
        assert_eq!(w.output("track", &at(11_000, 5000, 9000)), None, "the pause started the count again");
        assert_eq!(w.output("track", &at(12_000, 0, 4000)), None, "a flush sets the count back");
        assert_eq!(w.output("track", &at(13_500, 0, 4000)), None);
        let song = Moving { song: Some(1), ..at(16_000, 0, 4000) };
        assert_eq!(w.output("track", &song), None, "a new song starts afresh");
    }

    #[test]
    fn a_starved_offloaded_track_is_named_so() {
        let mut w = Watch::default();
        let m = |t, p| Moving { offloaded: true, ..at(t, p, 20_000) };
        w.output("engine", &m(0, 900));
        let b = w.output("engine", &m(2600, 900)).unwrap();
        assert_eq!(b.kind, "offload-starved");
        assert!(b.detail.starts_with("engine: playing, but the offloaded track"), "{}", b.detail);
        assert_eq!(w.output("track", &at(0, 1, 2)), None, "each output is watched on its own");
    }

    #[test]
    fn a_skip_moves_one_song_and_three_quick_ones_three() {
        let mut w = Watch::default();
        w.skip(0, 4);
        assert_eq!(w.arrived(100, 5, false, false), None);
        w.skip(200, 5);
        w.skip(350, 6);
        assert_eq!(w.arrived(400, 6, false, false), None);
        assert_eq!(w.arrived(500, 7, false, false), None, "three presses from 4, three songs");
        let b = w.arrived(700, 8, false, false).expect("a fourth song for three presses");
        assert_eq!(b.kind, "skip");
        assert_eq!(b.detail, "3 skip presses moved 4 songs, from queue place 4 to 8");
        assert_eq!(w.arrived(800, 9, false, false), None, "said once for the run");
    }

    #[test]
    fn a_song_ending_by_itself_or_a_shuffled_queue_is_not_a_skip() {
        let mut w = Watch::default();
        w.skip(0, 0);
        assert_eq!(w.arrived(100, 3, false, true), None, "shuffled: places say nothing");
        w.skip(1000, 3);
        assert_eq!(w.arrived(1100, 4, true, false), None);
        assert_eq!(w.arrived(1200, 6, false, false), None, "the run ended with the song that ended");
        w.skip(10_000, 6);
        assert_eq!(w.arrived(14_000, 9, false, false), None, "long after the press: not its doing");
        w.skip(20_000, 9);
        assert_eq!(w.arrived(20_100, 8, false, false), None, "previous moves one back");
    }

    #[test]
    fn the_screen_may_trail_the_ear_a_second_and_a_mix_longer() {
        let mut w = Watch::default();
        assert_eq!(w.heard(0, "a"), None);
        assert_eq!(w.shown(10, Some("a")), None);
        assert_eq!(w.heard(1000, "b"), None);
        assert_eq!(w.compare(1900), None, "0.9 s behind");
        assert_eq!(w.shown(1950, Some("b")), None);
        assert_eq!(w.heard(5000, "c"), None);
        let b = w.compare(6100).expect("1.1 s behind");
        assert_eq!(b.kind, "shown-heard");
        assert_eq!(b.detail, "the screen showed b while c was heard, for 1100 ms");
        assert_eq!(w.compare(9000), None, "said once");
        assert_eq!(w.shown(9100, Some("c")), None);
    }

    #[test]
    fn the_screen_is_not_late_while_nobody_can_see_it() {
        let mut w = Watch::default();
        w.heard(0, "a");
        w.shown(10, Some("a"));
        assert_eq!(w.visible(20, false), None, "the screen goes off");
        w.heard(1000, "b");
        assert_eq!(w.compare(60_000), None, "a minute off: nothing is drawn, nothing is late");
        assert_eq!(w.visible(64_000, true), None, "back on: the second to catch up starts now");
        assert_eq!(w.compare(64_900), None);
        let b = w.compare(65_100).expect("still the old song 1.1 s after coming back");
        assert_eq!(b.detail, "the screen showed a while b was heard, for 1100 ms");
        // Caught up in time: nothing said.
        let mut w = Watch::default();
        w.heard(0, "a");
        w.shown(10, Some("a"));
        w.visible(20, false);
        w.heard(1000, "b");
        w.visible(64_000, true);
        assert_eq!(w.shown(64_300, Some("b")), None);
        assert_eq!(w.compare(70_000), None);
    }

    #[test]
    fn lyrics_belong_to_the_song_heard() {
        let mut w = Watch::default();
        assert_eq!(w.lyrics(0, "a"), None, "nothing heard yet");
        w.heard(1000, "a");
        assert_eq!(w.lyrics(1500, "a"), None);
        w.heard(2000, "b");
        assert_eq!(w.lyrics(2500, "a"), None, "a's lyrics arriving just as b started");
        let b = w.lyrics(4000, "a").unwrap();
        assert_eq!(b.kind, "lyrics");
        assert_eq!(b.detail, "lyrics of a shown while b is heard (since 2000 ms)");
    }

    #[test]
    fn automix_wants_every_song_s_length() {
        let mut w = Watch::default();
        let missing = vec!["x".to_string(), "y".to_string()];
        assert_eq!(w.queue(false, &missing, 10), None, "AutoMix off");
        let b = w.queue(true, &missing, 10).unwrap();
        assert_eq!(b.detail, "AutoMix is on and 2 of 10 songs in the queue have no length: x, y");
        assert_eq!(w.queue(true, &missing, 10), None, "the same songs said once");
        assert_eq!(w.queue(true, &[], 10), None);
        assert!(w.queue(true, &missing, 10).is_some(), "again after the queue was whole");
    }

    #[test]
    fn a_setting_is_judged_only_playing_through_an_open_output_and_after_the_service_settled() {
        // The self test's restore: the service had just started with a batch of settings, paused.
        assert!(!settings_judged(1_000, false, false, Some(0)), "paused with no output, just started");
        assert!(!settings_judged(10_000, false, true, Some(0)), "paused: the chain is in no path");
        assert!(!settings_judged(10_000, true, false, Some(0)), "playing, but no output open yet");
        assert!(!settings_judged(2_000, true, true, Some(0)), "the service started two seconds ago");
        assert!(!settings_judged(SETTLE_MS, true, true, Some(0)), "still settling at the edge");
        assert!(settings_judged(SETTLE_MS + 1, true, true, Some(0)));
        assert!(settings_judged(5, true, true, None), "no service start seen: judged");
    }

    #[test]
    fn a_setting_is_in_the_engine_a_second_later() {
        assert_eq!(settings_held(&[("offload wanted", true, true)]), None);
        let b = settings_held(&[("offload wanted", false, true), ("sound chain in the path", true, true)]).unwrap();
        assert_eq!(b.kind, "setting");
        assert_eq!(b.detail, "a second after the settings changed the engine still shows offload wanted: true, expected false");
    }

    #[test]
    fn quiet_is_a_factor_held_in_range() {
        perf_quiet(0.001);
        assert!((quiet() - 0.001).abs() < 1e-6);
        perf_quiet(3.0);
        assert_eq!(quiet(), 1.0);
    }
}
