//! How the controls sound: fading in on play and out on pause, dipping around a seek or a skip, and
//! when a change to the sound chain may rebuild the output. The platform runs the fades and the
//! rebuilds; this says what they are.

/// What a control does to the music.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    /// A new place in the same song.
    Seek,
    /// Straight to another song of the queue.
    ToSong,
    /// Next or previous.
    Skip,
}

/// Down, then the switch, then back up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dip {
    pub down_ms: i32,
    pub up_ms: i32,
}

/// The dip is quick, so the finger's switch is heard at once.
const DIP_DOWN_MAX_MS: i32 = 150;
/// A jump to another song dips even with fades off: a hard digital cut mid-sound clicks, and out of
/// a mix it snaps texture as well as place.
const TO_SONG_FLOOR_MS: i32 = 120;

/// The dip around a switch made while music plays, `fade_ms` being the user's fade length (0 off).
/// `None`: switch at once. Cutting dead reads as a chop, and out of a blend it lands like the sound
/// dropped out; the configured fade is the way back in.
pub fn switch_dip(fade_ms: i32, switch: Switch, playing: bool) -> Option<Dip> {
    let floor = if switch == Switch::ToSong { TO_SONG_FLOOR_MS } else { 0 };
    let ms = fade_ms.max(floor);
    (ms > 0 && playing).then_some(Dip { down_ms: ms.min(DIP_DOWN_MAX_MS), up_ms: ms })
}

/// Pressing play: from silence up over this long; `None` to start at full volume.
pub fn play_fade(fade_ms: i32, playing: bool) -> Option<i32> {
    (fade_ms > 0 && !playing).then_some(fade_ms)
}

/// Pressing pause: down to silence over this long before pausing; `None` to pause at once.
pub fn pause_fade(fade_ms: i32, playing: bool) -> Option<i32> {
    (fade_ms > 0 && playing).then_some(fade_ms)
}

/// How often a running volume fade moves: one frame at 60 Hz, so the ramp is smooth and ticks only while
/// it lasts.
pub const FADE_TICK_MS: i64 = 16;

/// One tick of a volume fade from `from` to `to` that started at `start_ms` and lasts `ms`: the volume
/// now, and whether the fade is over. A fade of no length is over at once, at `to`. Asked on every tick,
/// so primitives only.
pub fn fade_step(from: f32, to: f32, start_ms: i64, now_ms: i64, ms: i32) -> (f32, bool) {
    if ms <= 0 {
        return (to, true);
    }
    let t = ((now_ms - start_ms) as f32 / ms as f32).clamp(0.0, 1.0);
    (crate::policy::fade(from, to, t), t >= 1.0)
}

/// A switch waiting out its dip. The old sound has to fall before the flush, so the switch runs a
/// heartbeat after the finger - guarded by what was current when it was asked: anything else moving on
/// first (a song ending inside the dip) drops it instead of yanking the queue back. A second switch
/// chains behind the first instead of cancelling it, so the queue steps once per tap, and a pause never
/// swallows the seek it interrupts. The platform keeps the action itself; this keeps whether it may run.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SwitchQueue {
    /// The song and queue index current when the waiting switch was asked for.
    waiting: Option<(Option<String>, i32)>,
}

impl SwitchQueue {
    pub const fn new() -> Self {
        SwitchQueue { waiting: None }
    }

    /// A switch now waits out its dip, asked on `song` at queue index `index`.
    pub fn wait(&mut self, song: Option<&str>, index: i32) {
        self.waiting = Some((song.map(str::to_string), index));
    }

    /// The waiting switch is due (its dip is down, another control was pressed): whether it runs, which it
    /// does only if the player is still where it was asked. Either way it no longer waits.
    pub fn take(&mut self, song: Option<&str>, index: i32) -> bool {
        self.waiting.take().is_some_and(|(s, i)| s.as_deref() == song && i == index)
    }

    /// Stopping drops a switch still waiting: starting over is not continuing it.
    pub fn drop_waiting(&mut self) {
        self.waiting = None;
    }
}

/// A skip asked for while the music is paused is a request for music, not for a different song to sit
/// paused on: the song changes and starts. Only the user's controls follow this - the player's own skips
/// (an explicit song, a song that will not play) leave a paused queue paused.
pub fn skip_plays(play_when_ready: bool) -> bool {
    !play_when_ready
}

/// What the next button does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NextAction {
    /// Skip to the song after.
    Skip,
    /// Nothing after the song playing: refilling the queue may still be fetching, so the press is
    /// remembered and taken when the songs land, instead of dying as a no-op.
    FillThenSkip,
}

pub fn next_action(has_next: bool) -> NextAction {
    if has_next {
        NextAction::Skip
    } else {
        NextAction::FillThenSkip
    }
}

/// A few seconds into a song it has been fetched and the radio is still up: the songs after it are
/// fetched ahead then.
pub const PRECACHE_AFTER_MS: i64 = 6_000;
/// The queue was edited: the new next song is measured for AutoMix after this, so a burst of edits
/// measures once.
pub const MEASURE_AFTER_EDIT_MS: i64 = 2_000;
/// The sound settings changed: AutoMix may just have been switched on mid-song, and the songs coming up
/// are measured after this, so the next boundary can already be mixed.
pub const MEASURE_AFTER_SETTINGS_MS: i64 = 1_000;
/// The queue is saved this long after it last changed, so a burst of changes is written once.
pub const SAVE_AFTER_MS: i64 = 1_500;

/// Facts about a settings change, for [`rebuild`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ChainChange {
    /// The audio chip is decoding the song playing.
    pub offloaded: bool,
    /// Offload is wanted from now on (`AudioPolicy::offload`).
    pub offload: bool,
    pub offload_changed: bool,
    /// Something USB is attached: the chip cannot reach it, so an offloaded song plays silence there.
    pub usb: bool,
    /// The output refused an offloaded song once.
    pub offload_refused: bool,
    pub tempo_changed: bool,
    /// A processor joins or leaves the chain.
    pub processor_changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rebuild {
    None,
    /// The current path is broken or silent: rebuild now, cut and all.
    Now,
    /// Mid-song a rebuild cuts the music, so it waits for the next song boundary, where it is inaudible.
    AtBoundary,
}

/// Whether the output must be rebuilt for a change, and when.
pub fn rebuild(c: ChainChange) -> Rebuild {
    // An offloaded song routed to USB plays nothing while reporting itself fine; and the chip plays
    // what it was given at 1x, so a new speed cannot wait either.
    if (c.offloaded && !c.offload && (c.usb || c.offload_refused)) || (c.tempo_changed && c.offloaded) {
        return Rebuild::Now;
    }
    if !(c.processor_changed || (c.offload_changed && c.offloaded)) {
        return Rebuild::None;
    }
    // Coming off offload: the offloaded path is the one ending, and it cannot take the new chain.
    if c.offloaded && !c.offload {
        Rebuild::Now
    } else {
        Rebuild::AtBoundary
    }
}

/// When the output is rebuilt for the equalizer screen (a shallow buffer, so a band moves audibly within
/// half a second instead of up to the deep buffer's ten) and back. Rebuilding mid-song is a stop and a
/// gap, so while music plays the swap waits for the next boundary or the next pause; bursts turn off at
/// once so the output stops being topped up in multi-second bursts.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Chain {
    /// The equalizer screen is open: shallow buffer, no bursts.
    pub tuning: bool,
    /// A rebuild is waiting for the next song boundary.
    pub swap_pending: bool,
    /// The buffer depth changes at the next pause (paused is silent, so it needs no boundary).
    pub deep_at_next_pause: bool,
}

/// What the output should do now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChainAct {
    Nothing,
    /// Rebuild now.
    Rebuild,
}

impl Chain {
    /// The equalizer screen opened (`on`) or closed. `eq` whether the equalizer is in the chain at all,
    /// `idle` nothing loaded, `playing` playback wanted. Bursts follow [`Chain::tuning`] at once.
    pub fn tuning(&mut self, on: bool, eq: bool, idle: bool, playing: bool) -> ChainAct {
        if on && !self.tuning && eq {
            self.tuning = true;
            if !idle && playing {
                // No cut while playing; the shallow buffer arrives at the next pause or song.
                self.swap_pending = true;
                self.deep_at_next_pause = true;
                ChainAct::Nothing
            } else if !idle {
                ChainAct::Rebuild
            } else {
                ChainAct::Nothing
            }
        } else if !on && self.tuning {
            self.tuning = false;
            self.deep_at_next_pause = true;
            if !idle && playing {
                self.swap_pending = true;
                ChainAct::Nothing
            } else if !idle {
                ChainAct::Rebuild
            } else {
                ChainAct::Nothing
            }
        } else {
            ChainAct::Nothing
        }
    }

    /// A settings change needs a rebuild that can wait for the boundary. Returns whether it was not
    /// already waiting (so the platform says so once).
    pub fn defer(&mut self) -> bool {
        !std::mem::replace(&mut self.swap_pending, true)
    }

    /// A song boundary (`repeat_one`: the same song looping, which should restart seamlessly, so the
    /// swap waits for a real boundary). A planned mix into this song dies with the rebuild: the setting
    /// wins over one mix.
    pub fn boundary(&mut self, repeat_one: bool) -> ChainAct {
        if self.swap_pending && !repeat_one {
            // The rebuild carries whatever else was waiting, the deep buffer's return included.
            self.swap_pending = false;
            self.deep_at_next_pause = false;
            ChainAct::Rebuild
        } else {
            ChainAct::Nothing
        }
    }

    /// Playback paused by the user (not merely buffering): the deep buffer can come back at once.
    pub fn paused(&mut self) -> ChainAct {
        if self.deep_at_next_pause {
            self.deep_at_next_pause = false;
            self.swap_pending = false;
            ChainAct::Rebuild
        } else {
            ChainAct::Nothing
        }
    }

    /// Whether the output is fed in bursts: not while offloaded (the chip sleeps by itself), not while tuning.
    pub fn bursting(&self, offloaded: bool) -> bool {
        !offloaded && !self.tuning
    }
}

/// The sleep timer "after N songs": whether to pause at the end of the song playing now, and how many
/// song changes are still to go before that. "End of track" and one song are the same thing.
pub fn sleep_after(songs: u32, end_of_track: bool) -> (bool, u32) {
    (end_of_track || songs == 1, if songs <= 1 { 0 } else { songs - 1 })
}

/// A song change with `left` changes to go: how many remain, and whether to pause at the end of this one.
pub fn sleep_song_changed(left: u32) -> (u32, bool) {
    match left {
        0 => (0, false),
        n => (n - 1, n == 1),
    }
}

/// The sleep timer in minutes, as a delay; with a little slack so the phone may batch the wake-up.
pub fn sleep_delay_ms(minutes: u32) -> (i64, i64) {
    (minutes as i64 * 60_000, 15_000)
}

/// What the sleep timer shows once set: when it fires (0: no timer by the clock), and whether it waits
/// for a song to end ("end of track", or a number of songs).
pub fn sleep_shown(minutes: u32, end_of_track: bool, songs: u32, now_ms: i64) -> (i64, bool) {
    let at = if minutes > 0 { now_ms + sleep_delay_ms(minutes).0 } else { 0 };
    let (pause, left) = sleep_after(songs, end_of_track);
    (at, pause || left > 0)
}

/// A pause this long lets the output go: the player keeps the queue and the place in the song but closes
/// the audio track and stops its own once-a-second tick, so a phone left paused sleeps. Pressing play
/// opens it again from the local cache, which takes a moment only after a pause this long.
pub const IDLE_RELEASE_MS: i64 = 5 * 60_000;

/// How much the player reads ahead: at least a minute, up to ten, playback starting after a second and
/// resuming after two - and at most a quarter of the app's memory class, between 16 and 48 MB. A song is
/// fetched in seconds and then played from memory, so the network sleeps for most of it.
pub fn load_control(memory_class_mb: u32) -> [i64; 5] {
    let mb = (memory_class_mb / 4).min(48).max(16) as i64;
    [60_000, 600_000, 1_000, 2_000, mb * 1024 * 1024]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switches_dip_quickly_and_come_back_at_the_users_pace() {
        assert_eq!(switch_dip(600, Switch::Seek, true), Some(Dip { down_ms: 150, up_ms: 600 }));
        assert_eq!(switch_dip(0, Switch::Seek, true), None, "fades off");
        assert_eq!(switch_dip(0, Switch::ToSong, true), Some(Dip { down_ms: 120, up_ms: 120 }), "another song always dips a little");
        assert_eq!(switch_dip(600, Switch::Skip, false), None, "paused: nothing to dip");
    }

    #[test]
    fn play_and_pause_fade_only_when_they_change_something() {
        assert_eq!(play_fade(400, false), Some(400));
        assert_eq!(play_fade(400, true), None);
        assert_eq!(pause_fade(400, true), Some(400));
        assert_eq!(pause_fade(0, true), None);
    }

    #[test]
    fn a_rebuild_waits_for_the_boundary_unless_the_path_is_broken() {
        assert_eq!(rebuild(ChainChange { processor_changed: true, ..Default::default() }), Rebuild::AtBoundary);
        assert_eq!(rebuild(ChainChange::default()), Rebuild::None);
        let silent = ChainChange { offloaded: true, offload: false, usb: true, ..Default::default() };
        assert_eq!(rebuild(silent), Rebuild::Now, "an offloaded song on usb plays silence");
        assert_eq!(rebuild(ChainChange { offloaded: true, offload: true, tempo_changed: true, ..Default::default() }), Rebuild::Now);
        assert_eq!(rebuild(ChainChange { offloaded: true, offload: false, processor_changed: true, ..Default::default() }), Rebuild::Now, "leaving offload");
        assert_eq!(rebuild(ChainChange { offloaded: false, offload: true, offload_changed: true, ..Default::default() }), Rebuild::None, "offload waits for its next song");
    }

    #[test]
    fn tuning_waits_for_a_boundary_while_playing() {
        let mut c = Chain::default();
        assert_eq!(c.tuning(true, true, false, true), ChainAct::Nothing);
        assert!(c.tuning && c.swap_pending && c.deep_at_next_pause && !c.bursting(false));
        assert_eq!(c.boundary(true), ChainAct::Nothing, "a repeat-one loop restarts seamlessly");
        assert_eq!(c.boundary(false), ChainAct::Rebuild);
        assert_eq!(c.paused(), ChainAct::Nothing, "the rebuild carried the deep buffer too");
        assert_eq!(c.tuning(false, true, false, false), ChainAct::Rebuild, "paused: at once");
        assert!(c.deep_at_next_pause);
        assert_eq!(c.paused(), ChainAct::Rebuild);
        assert!(c.defer() && !c.defer(), "said once");
        let mut off = Chain::default();
        assert_eq!(off.tuning(true, false, false, true), ChainAct::Nothing, "no equalizer in the chain: nothing to tune");
        assert!(!off.tuning);
    }

    #[test]
    fn a_fade_steps_to_its_end_and_says_so() {
        assert_eq!(fade_step(1.0, 0.0, 100, 100, 200), (1.0, false));
        assert_eq!(fade_step(1.0, 0.0, 100, 200, 200), (0.5, false));
        assert_eq!(fade_step(1.0, 0.0, 100, 300, 200), (0.0, true));
        assert_eq!(fade_step(1.0, 0.0, 100, 900, 200), (0.0, true), "late ticks stay at the end");
        assert_eq!(fade_step(0.2, 0.8, 100, 50, 200), (0.2, false), "a clock before the start holds");
        assert_eq!(fade_step(1.0, 0.3, 100, 100, 0), (0.3, true), "no length: there at once");
    }

    #[test]
    fn a_waiting_switch_runs_only_where_it_was_asked() {
        let mut q = SwitchQueue::new();
        assert!(!q.take(Some("a"), 0), "nothing waits");
        q.wait(Some("a"), 0);
        assert!(q.take(Some("a"), 0));
        assert!(!q.take(Some("a"), 0), "taken once");
        q.wait(Some("a"), 0);
        assert!(!q.take(Some("b"), 1), "the song moved on inside the dip: dropped");
        assert!(!q.take(Some("a"), 0));
        q.wait(Some("a"), 0);
        assert!(!q.take(Some("a"), 2), "the same song queued twice is another place");
        q.wait(None, -1);
        assert!(q.take(None, -1));
        q.wait(Some("a"), 0);
        q.drop_waiting();
        assert!(!q.take(Some("a"), 0), "stopping drops it");
    }

    #[test]
    fn controls_ask_for_music() {
        assert!(skip_plays(false) && !skip_plays(true));
        assert_eq!(next_action(true), NextAction::Skip);
        assert_eq!(next_action(false), NextAction::FillThenSkip);
        assert_eq!((PRECACHE_AFTER_MS, MEASURE_AFTER_EDIT_MS, MEASURE_AFTER_SETTINGS_MS, SAVE_AFTER_MS, FADE_TICK_MS), (6_000, 2_000, 1_000, 1_500, 16));
    }

    #[test]
    fn sleep_counts_songs() {
        assert_eq!(sleep_after(1, false), (true, 0));
        assert_eq!(sleep_after(3, false), (false, 2));
        assert_eq!(sleep_after(0, true), (true, 0));
        assert_eq!(sleep_song_changed(2), (1, false));
        assert_eq!(sleep_song_changed(1), (0, true));
        assert_eq!(sleep_song_changed(0), (0, false));
        assert_eq!(sleep_shown(30, false, 0, 1_000), (1_000 + 1_800_000, false));
        assert_eq!(sleep_shown(0, false, 0, 1_000), (0, false), "cancelled");
        assert_eq!(sleep_shown(0, true, 0, 1_000), (0, true));
        assert_eq!(sleep_shown(0, false, 1, 1_000), (0, true));
        assert_eq!(sleep_shown(0, false, 3, 1_000), (0, true));
        assert_eq!(load_control(256)[4], 48 * 1024 * 1024);
        assert_eq!(load_control(32)[4], 16 * 1024 * 1024);
    }
}
