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
    fn sleep_counts_songs() {
        assert_eq!(sleep_after(1, false), (true, 0));
        assert_eq!(sleep_after(3, false), (false, 2));
        assert_eq!(sleep_after(0, true), (true, 0));
        assert_eq!(sleep_song_changed(2), (1, false));
        assert_eq!(sleep_song_changed(1), (0, true));
        assert_eq!(sleep_song_changed(0), (0, false));
        assert_eq!(load_control(256)[4], 48 * 1024 * 1024);
        assert_eq!(load_control(32)[4], 16 * 1024 * 1024);
    }
}
