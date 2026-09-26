//! Making a seek stick. Asked on a song that is still being fetched, a seek lands on a source that has
//! not been opened yet: the player accepts it, loads the track and starts it from the beginning, and
//! the place the finger asked for is gone. And a remote player (a media session's controller) answers
//! from its own books the moment it is asked, so right after a seek it reports the chosen place whatever
//! really happened; the truth arrives later. So a seek is remembered and looked at again - on the
//! player's events and on a short timer - until it is clearly kept or the song has moved on, and asked
//! for again if the player ended up back where it was.

/// How long a seek is watched for before it is given up on; extended while it is visibly arriving.
pub const KEEP_MS: i64 = 15_000;
/// How often the platform should look while a seek is being watched.
pub const LOOK_EVERY_MS: i64 = 300;
/// Within this of the target counts as there.
const NEAR_MS: i64 = 1_500;
/// How many times a dropped seek is asked for again.
const TRIES: u32 = 3;

/// What the platform should do after a look.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Keep watching.
    Watch,
    /// Done with it: kept, given up on, or the song moved on.
    Forget,
    /// It was dropped: ask the player for this place again (and keep watching).
    SeekAgain(i64),
}

#[derive(Debug, Clone, Default)]
pub struct SeekKeeper {
    wanted: Option<Wanted>,
}

#[derive(Debug, Clone)]
struct Wanted {
    target: i64,
    until: i64,
    tries: u32,
    /// Where the player was: taken when the seek was asked on a ready player, otherwise at the first
    /// sight of a ready one. `low` is the lowest place seen since, `last` the last.
    from: Option<i64>,
    low: Option<i64>,
    last: Option<i64>,
}

impl SeekKeeper {
    pub fn new() -> Self {
        SeekKeeper::default()
    }

    /// The place being watched for, while there is one (a seek bar holds it instead of jumping).
    pub fn pending(&self) -> Option<i64> {
        self.wanted.as_ref().map(|w| w.target)
    }

    /// A seek to `target` asked at `now`. `ready`: the player is playing or paused on an opened source,
    /// with its playhead at `pos` - then where it was means something. On a player that is still
    /// opening the position means nothing (idle reports 0 while a session restores to wherever the
    /// queue was left), so the anchor waits for the first ready look. Already at the target anchors AT
    /// it: a controller answering from its own books can report the asked place itself, and the
    /// direction test would otherwise read a forward seek as a backward one and ask again.
    pub fn ask(&mut self, target: i64, now: i64, ready: bool, pos: i64) {
        let from = ready.then(|| if (pos - target).abs() <= NEAR_MS { target } else { pos });
        self.wanted = Some(Wanted { target, until: now + KEEP_MS, tries: 0, from, low: from, last: None });
    }

    pub fn forget(&mut self) {
        self.wanted = None;
    }

    /// A look at the player: still on the song the seek was asked in, ready, where, and playing.
    pub fn look(&mut self, now: i64, same_song: bool, ready: bool, pos: i64, playing: bool) -> Verdict {
        let v = self.judge(now, same_song, ready, pos, playing);
        if v == Verdict::Forget {
            self.wanted = None;
        }
        v
    }

    fn judge(&mut self, now: i64, same_song: bool, ready: bool, pos: i64, playing: bool) -> Verdict {
        let Some(w) = self.wanted.as_mut() else { return Verdict::Forget };
        // The song changed under it: there is nothing left to keep.
        if !same_song {
            return Verdict::Forget;
        }
        if !ready {
            // A source that never opens still ends the watch; a slow one gets its full window.
            return if now > w.until { Verdict::Forget } else { Verdict::Watch };
        }
        let Some(from) = w.from else {
            // First sight of a ready player that was still opening when asked: this is what "where
            // it was" means. The seek may have landed already (then pos is the target) or been
            // dropped (then the checks below ask again); either way it is measured from truth now.
            let at = if (pos - w.target).abs() <= NEAR_MS { w.target } else { pos };
            (w.from, w.low, w.until) = (Some(at), Some(at), now + KEEP_MS);
            return Verdict::Watch;
        };
        if now > w.until {
            return Verdict::Forget;
        }
        let target = w.target;
        let near = (pos - target).abs() <= NEAR_MS;
        // Paused where the finger asked: landed - a paused player moves for nothing else.
        if near && !playing {
            return Verdict::Forget;
        }
        let low = w.low.map_or(pos, |l| l.min(pos));
        w.low = Some(low);
        // Still converging on the target (a transcode lands seeks in stages, seconds apart): give it its
        // window rather than giving up, so the bar keeps holding the asked place throughout.
        let converging = w.last.is_some_and(|last| (pos - target).abs() + 250 < (last - target).abs());
        w.last = Some(pos);
        if converging {
            w.until = now + KEEP_MS;
            return Verdict::Watch;
        }
        let came_down = low < from - 500;
        let again = |w: &mut Wanted| {
            if w.tries >= TRIES {
                Verdict::Forget
            } else {
                w.tries += 1;
                Verdict::SeekAgain(target)
            }
        };
        if target >= from {
            // Forward: played past it (the anchor is truthful, so this cannot misfire on a restored place).
            if pos > target + 400 {
                return Verdict::Forget;
            }
            if near {
                return Verdict::Watch; // playing through it: wait for the proof above
            }
            // Still at the start: dropped or not yet applied - ask again, briefly. Otherwise it is
            // playing on without it and the recovery window has passed.
            return if pos <= from + NEAR_MS { again(w) } else { Verdict::Forget };
        }
        // Backward: pos > target is the starting condition, not proof of anything. Proof is having come
        // down from the start and reached the target's neighbourhood while playing on.
        if playing && came_down && pos >= target - NEAR_MS {
            return Verdict::Forget;
        }
        if near {
            return Verdict::Watch; // may be arriving (paused-near already kept above)
        }
        // Never moved: dropped - ask again, briefly. Anything else (overshot, partial) is stale.
        if !came_down && pos >= from - NEAR_MS {
            again(w)
        } else {
            Verdict::Forget
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_forward_seek_that_is_dropped_is_asked_again_then_kept_once_played_past() {
        let mut k = SeekKeeper::new();
        k.ask(60_000, 0, true, 5_000);
        assert_eq!(k.look(300, true, true, 5_300, true), Verdict::SeekAgain(60_000), "still at the start");
        assert_eq!(k.look(600, true, true, 60_100, true), Verdict::Watch, "playing through it");
        assert_eq!(k.look(900, true, true, 60_500, true), Verdict::Forget, "past it: kept");
        assert_eq!(k.pending(), None);
    }

    #[test]
    fn a_dropped_seek_is_asked_again_only_three_times() {
        let mut k = SeekKeeper::new();
        k.ask(60_000, 0, true, 5_000);
        for t in 1..=3 {
            assert_eq!(k.look(t * 300, true, true, 5_000, true), Verdict::SeekAgain(60_000));
        }
        assert_eq!(k.look(1_200, true, true, 5_000, true), Verdict::Forget);
    }

    #[test]
    fn a_seek_asked_while_opening_anchors_on_the_first_ready_look() {
        let mut k = SeekKeeper::new();
        k.ask(90_000, 0, false, 0);
        assert_eq!(k.look(300, true, false, 0, false), Verdict::Watch, "not ready yet");
        // The session restored to 40 s and dropped the seek: anchored there, then asked again.
        assert_eq!(k.look(600, true, true, 40_000, true), Verdict::Watch);
        assert_eq!(k.look(900, true, true, 40_300, true), Verdict::SeekAgain(90_000));
    }

    #[test]
    fn paused_near_the_target_is_landed_and_the_song_changing_ends_it() {
        let mut k = SeekKeeper::new();
        k.ask(30_000, 0, true, 10_000);
        assert_eq!(k.look(300, true, true, 30_200, false), Verdict::Forget);
        k.ask(30_000, 0, true, 10_000);
        assert_eq!(k.look(300, false, true, 0, true), Verdict::Forget);
    }

    #[test]
    fn a_backward_seek_is_kept_once_it_has_come_down_and_plays_on() {
        let mut k = SeekKeeper::new();
        k.ask(20_000, 0, true, 120_000);
        assert_eq!(k.look(300, true, true, 120_300, true), Verdict::SeekAgain(20_000), "never moved");
        assert_eq!(k.look(600, true, true, 20_100, true), Verdict::Watch, "a jump that close is still arriving");
        assert_eq!(k.look(900, true, true, 20_400, true), Verdict::Forget, "came down to it and plays on");
    }

    #[test]
    fn a_seek_arriving_in_stages_keeps_its_window() {
        let mut k = SeekKeeper::new();
        k.ask(200_000, 0, true, 10_000);
        let mut t = 0;
        for pos in [10_000, 80_000, 150_000] {
            t += 1_000;
            let v = k.look(t, true, true, pos, true);
            assert!(v != Verdict::Forget, "{pos}: {v:?}");
        }
        assert_eq!(k.look(t + 14_000, true, true, 150_100, true), Verdict::Forget, "played on without it after converging");
    }

    #[test]
    fn it_gives_up_after_its_window() {
        let mut k = SeekKeeper::new();
        k.ask(60_000, 0, false, 0);
        assert_eq!(k.look(KEEP_MS + 1, true, false, 0, false), Verdict::Forget);
    }
}
