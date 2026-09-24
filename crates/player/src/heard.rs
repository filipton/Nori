//! Which song a listener is hearing and where in it, for a seek bar and a now-playing page. Through a
//! transition the player runs ahead of the ear (the held ending counts as played the moment it is
//! decoded, so the next song arrives in time to be mixed in), and the engine's [`Heard`] says what the
//! ear really has. This turns that reading, taken seconds apart with a deep buffer, into a place that
//! moves at one times between readings, follows the ear into the next song the moment it is the louder
//! of the two in the mix (the engine's `until_us`: a fade that starts with the next song silent is
//! still the last song to anyone listening), and does not fall back to the old song in the gap between
//! the engine letting go and the player moving on.

use crate::engine::Heard;

/// What the player itself says, at the moment of asking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayerNow<'a> {
    pub now_ms: i64,
    pub playing: bool,
    /// The song the player is on.
    pub on: Option<&'a str>,
    pub position_ms: i64,
}

/// The answer: which song of the queue is heard (its index in [`HeardTracker::set_queue`]'s order) and
/// the place in it, ms; `None` while the player's own word is the truth. `changed` is whether the heard
/// song changed since the last question, so a page can follow the ear at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Seen {
    pub index: Option<usize>,
    pub ms: i64,
    pub changed: bool,
}

/// Asked every frame a seek bar is drawn, so it allocates nothing: songs are compared in place and an
/// id is only copied when the song heard changes.
#[derive(Debug, Default)]
pub struct HeardTracker {
    queue: Vec<(String, i64)>,
    ear: Ear,
}

#[derive(Debug, Default)]
struct Ear {
    /// The song the page was carried onto, where in it, and when that was.
    carry: Option<(String, i64, i64)>,
    consumed: Option<String>,
    before: Option<String>,
}

/// Points `slot` at `id`, copying it only when it names another song.
fn set_to(slot: &mut Option<String>, id: Option<&str>) {
    if slot.as_deref() != id {
        *slot = id.map(str::to_string);
    }
}

fn duration(queue: &[(String, i64)], id: &str) -> i64 {
    queue.iter().find(|(q, _)| q == id).map_or(i64::MAX, |q| q.1)
}

/// Where in the next song the ear is, `ms_in` after the mix became audible.
fn into_next(queue: &[(String, i64)], h: &Heard, ms_in: i64) -> i64 {
    let Some(id) = &h.next_id else { return 0 };
    (h.next_from_us / 1000 + (ms_in as f64 * h.next_rate as f64) as i64).clamp(0, duration(queue, id))
}

impl HeardTracker {
    pub fn new() -> HeardTracker {
        HeardTracker::default()
    }

    /// The queue in its order, each song with its length in ms: answers are indexes into it, and
    /// places are kept inside the songs.
    pub fn set_queue<I: IntoIterator<Item = (String, i64)>>(&mut self, songs: I) {
        self.queue.clear();
        self.queue.extend(songs);
    }

    pub fn at(&mut self, h: &Heard, p: PlayerNow) -> Seen {
        self.ear.at(&self.queue, h, p)
    }

    /// [`HeardTracker::at`] with the player's song given as its index in the queue (`on`), and the index
    /// the player goes to next. A song can be queued more than once, so the heard one is placed where the
    /// ear can be: the player's next song when that is it, else the nearest earlier copy (the song it just
    /// left), else the last one.
    pub fn at_index(&mut self, h: &Heard, now_ms: i64, playing: bool, on: Option<usize>, next: Option<usize>, position_ms: i64) -> Seen {
        let on_id = on.and_then(|i| self.queue.get(i)).map(|(id, _)| id.as_str());
        let mut seen = self.ear.at(&self.queue, h, PlayerNow { now_ms, playing, on: on_id, position_ms });
        if let Some(first) = seen.index {
            let id = self.queue[first].0.as_str();
            let is = |i: usize| self.queue.get(i).is_some_and(|(q, _)| q == id);
            seen.index = next
                .filter(|&n| is(n))
                .or_else(|| on.and_then(|cur| (0..cur).rev().find(|&i| is(i))))
                .or_else(|| (0..self.queue.len()).rev().find(|&i| is(i)));
        }
        seen
    }

    /// Whether the song heard (`heard`) is another song than the one a page shows (`shown`), both indexes
    /// into the queue. A page showing nothing, or a song the queue does not have, differs from nothing.
    pub fn differs(&self, heard: usize, shown: usize) -> bool {
        match self.queue.get(shown) {
            None => false,
            Some((id, _)) => self.queue.get(heard).is_none_or(|(h, _)| h != id),
        }
    }
}

/// The place a seek bar shows, and when it was taken. The ear changes song a moment before the page
/// follows (on the next tick): until it has, the old song's title must not be shown with the new song's
/// time under it, so the bar holds where it was. And while the app is reconnecting to the player
/// nothing can be asked at all - reporting zero then makes the bar snap to 0:00 and jump back a
/// heartbeat later, so it carries on from where it was, moving if the music was.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Playhead {
    ms: i64,
    at_ms: i64,
}

impl Playhead {
    pub const fn new() -> Self {
        Playhead { ms: 0, at_ms: 0 }
    }

    /// What the bar shows for `seen`, asked at `now_ms` while the page shows queue index `shown`.
    pub fn show(&mut self, t: &HeardTracker, seen: Seen, shown: Option<usize>, now_ms: i64) -> i64 {
        if let (Some(h), Some(s)) = (seen.index, shown) {
            if t.differs(h, s) {
                return self.ms;
            }
        }
        self.ms = seen.ms;
        self.at_ms = now_ms;
        self.ms
    }

    /// Where the bar is at `now_ms` with nothing to ask: the last place, run on at one times if `playing`.
    pub fn run_on(&self, now_ms: i64, playing: bool) -> i64 {
        let elapsed = if playing && self.at_ms > 0 { now_ms - self.at_ms } else { 0 };
        (self.ms + elapsed).max(0)
    }
}

/// The length a page shows for its song: the heard song's own (in seconds, from the queue) while the ear
/// is on a song the player has left, else what the player measured once it knows, else what the song's
/// tags said.
pub fn shown_duration_ms(heard_s: Option<i64>, player_ms: i64, tagged_ms: i64) -> i64 {
    match heard_s {
        Some(s) => s * 1000,
        None if player_ms > 0 => player_ms,
        None => tagged_ms,
    }
}

impl Ear {
    fn at(&mut self, q: &[(String, i64)], h: &Heard, p: PlayerNow) -> Seen {
        let next = h.next_id.as_deref();
        let result: Option<(&str, i64)> = if let Some(id) = h.id.as_deref() {
            let since = if p.playing { p.now_ms - h.at_ms } else { 0 };
            let ms = h.us / 1000 + since;
            let until = h.until_us / 1000;
            if ms < until {
                Some((id, ms.clamp(0, duration(q, id))))
            } else {
                // The mix is audible: from here the next song is heard, at the point the mix entered it,
                // never the old one's last seconds jumped through.
                next.map(|n| (n, into_next(q, h, ms - until)))
            }
        } else if let (Some(n), Some(on)) = (next, p.on) {
            // The next song arrived at once, so the player was never ahead of the ear and its clock is
            // the truth - but past the point the mix is heard, the truth is the next song.
            let until = h.audible_us / 1000;
            (Some(on) == h.from_id.as_deref() && Some(n) != self.consumed.as_deref() && p.position_ms >= until)
                .then(|| (n, into_next(q, h, p.position_ms - until)))
        } else {
            None
        };
        // Once the page is on the next song it stays there until the player has left the old one: the
        // engine letting go and the player moving on are not the same moment, and in between the page
        // would fall back to the player's word - the old song - and flash its cover back.
        let shown: Option<(&str, i64)> = result.or_else(|| {
            let (held, ms, at) = self.carry.as_ref()?;
            (p.on == h.from_id.as_deref() && Some(held.as_str()) != self.consumed.as_deref()).then(|| {
                let since = if p.playing { p.now_ms - at } else { 0 };
                (held.as_str(), (ms + since).clamp(0, duration(q, held)))
            })
        });
        let (shown_id, shown_ms) = shown.map_or((None, p.position_ms), |(id, ms)| (Some(id), ms));
        let index = shown_id.and_then(|id| q.iter().position(|(s, _)| s == id));
        let changed = self.before.as_deref() != shown_id;
        // Copied only when the song heard changes: the one allocation, once per change of song.
        let before = changed.then(|| shown_id.map(str::to_string));
        let carrying = shown_id.is_some() && shown_id == next && p.on != next;
        if let Some(b) = before {
            self.before = b;
        }
        if carrying {
            match &mut self.carry {
                Some((id, ms, at)) if Some(id.as_str()) == next => (*ms, *at) = (shown_ms, p.now_ms),
                c => *c = next.map(|id| (id.to_string(), shown_ms, p.now_ms)),
            }
        }
        // The player has reached the next song: this mix is done with, whatever the engine still holds.
        if next.is_some() && p.on == next {
            set_to(&mut self.consumed, next);
            self.carry = None;
        }
        Seen { index, ms: shown_ms, changed }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const A: usize = 0;
    const B: usize = 1;

    impl Seen {
        fn seen(self) -> Option<(usize, i64)> {
            self.index.map(|i| (i, self.ms))
        }
    }

    fn tracker() -> HeardTracker {
        let mut t = HeardTracker::new();
        t.set_queue([("a".to_string(), 200_000), ("b".to_string(), 180_000)]);
        t
    }

    /// Held on the last seconds of `a`, which mix into `b` from 5 s in; the player is already on `b`.
    fn holding(us: i64, at_ms: i64) -> Heard {
        Heard {
            id: Some("a".into()),
            us,
            at_ms,
            until_us: 194_000_000,
            mixing: false,
            next_id: Some("b".into()),
            next_from_us: 5_000_000,
            next_rate: 1.0,
            from_id: Some("a".into()),
            audible_us: 194_000_000,
        }
    }

    fn now(ms: i64, on: &str, pos: i64) -> PlayerNow<'_> {
        PlayerNow { now_ms: ms, playing: true, on: Some(on), position_ms: pos }
    }

    #[test]
    fn a_song_queued_twice_is_heard_at_the_copy_the_ear_can_be_at() {
        let mut t = HeardTracker::new();
        t.set_queue(["a", "b", "a", "b"].map(|id| (id.to_string(), 200_000)));
        // Held on the ending of `a`; the player is on the second `b` (3), and its next song is none.
        let s = t.at_index(&holding(190_000_000, 0), 1_000, true, Some(3), None, 0);
        assert_eq!(s.index, Some(2), "the nearest earlier copy: the song just left");
        let s = t.at_index(&holding(190_000_000, 0), 1_000, true, Some(1), Some(2), 0);
        assert_eq!(s.index, Some(2), "the player's next song, when it is that one");
    }

    #[test]
    fn the_held_ending_runs_on_between_readings() {
        let mut t = tracker();
        let s = t.at(&holding(190_000_000, 1_000), now(3_000, "b", 800));
        assert_eq!((s.index, s.ms), (Some(A), 192_000));
        assert!(s.changed);
        assert!(!t.at(&holding(190_000_000, 1_000), now(3_100, "b", 900)).changed, "same song, no change");
    }

    #[test]
    fn once_the_mix_is_audible_the_next_song_is_heard() {
        let mut t = tracker();
        let s = t.at(&holding(190_000_000, 1_000), now(6_500, "b", 900));
        // 5.5 s since the reading: 195.5 s into a, 1.5 s past the audible point, so 6.5 s into b.
        assert_eq!((s.index, s.ms), (Some(B), 6_500));
        assert!(s.changed);
    }

    #[test]
    fn a_stretched_mix_runs_through_the_next_song_at_its_rate() {
        let mut t = tracker();
        let h = Heard { next_rate: 0.5, ..holding(194_000_000, 0) };
        assert_eq!(t.at(&h, now(2_000, "b", 0)).seen(), Some((B, 6_000)));
    }

    #[test]
    fn no_flash_back_while_the_player_is_still_on_the_old_song() {
        let mut t = tracker();
        // Heard on b already, the player not yet moved on from a.
        assert_eq!(t.at(&holding(195_000_000, 0), now(0, "a", 195_000)).seen(), Some((B, 6_000)));
        // The engine let go (no hold any more) but the player is still on a: the page stays on b, moving.
        let released = Heard { id: None, next_id: None, ..holding(0, 0) };
        assert_eq!(t.at(&released, now(2_000, "a", 197_000)).seen(), Some((B, 8_000)));
        // The player reached b: its own word is the truth again.
        let s = t.at(&Heard { id: None, ..holding(0, 0) }, now(2_100, "b", 8_100));
        assert_eq!((s.index, s.ms), (None, 8_100), "the player's own position");
        assert!(s.changed);
    }

    #[test]
    fn when_the_player_was_never_ahead_its_clock_decides() {
        let mut t = tracker();
        let direct = Heard { id: None, ..holding(0, 0) };
        assert_eq!(t.at(&direct, now(0, "a", 190_000)).seen(), None, "before the mix is audible");
        assert_eq!(t.at(&direct, now(0, "a", 196_000)).seen(), Some((B, 7_000)));
        // After the player moved on to b, a later visit to a is not mistaken for the mix.
        t.at(&direct, now(0, "b", 7_000));
        assert_eq!(t.at(&direct, now(0, "a", 196_000)).seen(), None);
    }

    #[test]
    fn paused_it_stands_still_and_stays_inside_the_song() {
        let mut t = tracker();
        let p = PlayerNow { playing: false, ..now(60_000, "b", 0) };
        assert_eq!(t.at(&holding(190_000_000, 0), p).seen(), Some((A, 190_000)));
        let late = Heard { until_us: i64::MAX, ..holding(199_000_000, 0) };
        assert_eq!(t.at(&late, now(60_000, "b", 0)).seen(), Some((A, 200_000)), "clamped to the song's length");
    }

    #[test]
    fn the_bar_holds_while_the_page_is_a_song_behind() {
        let mut t = HeardTracker::new();
        t.set_queue([("a".to_string(), 200_000), ("b".to_string(), 180_000), ("a".to_string(), 200_000)]);
        let mut p = Playhead::new();
        let seen = |index, ms| Seen { index, ms, changed: false };
        assert_eq!(p.show(&t, seen(None, 5_000), Some(A), 100), 5_000, "the player's own word");
        assert_eq!(p.show(&t, seen(Some(A), 6_000), Some(A), 200), 6_000, "heard on the song shown");
        assert_eq!(p.show(&t, seen(Some(2), 7_000), Some(A), 300), 7_000, "another copy of the same song is the same song");
        assert_eq!(p.show(&t, seen(Some(B), 1_000), Some(A), 400), 7_000, "the ear moved to b, the page is still on a: hold");
        assert_eq!(p.show(&t, seen(Some(B), 1_100), None, 500), 1_100, "a page showing nothing does not hold");
        assert_eq!(p.show(&t, seen(Some(B), 1_200), Some(9), 600), 1_200, "nor one the queue does not have");
        assert_eq!(p.run_on(1_600, true), 2_200, "reconnecting while playing: runs on from when it was taken");
        assert_eq!(p.run_on(1_600, false), 1_200, "paused: stands");
        assert_eq!(Playhead::new().run_on(5_000, true), 0, "never shown: zero, not the clock");
    }

    #[test]
    fn the_length_shown_is_the_heard_songs() {
        assert_eq!(shown_duration_ms(Some(180), 200_000, 199_000), 180_000, "the ear is a song behind the player");
        assert_eq!(shown_duration_ms(None, 200_123, 199_000), 200_123, "measured");
        assert_eq!(shown_duration_ms(None, 0, 199_000), 199_000, "not measured yet");
        assert_eq!(shown_duration_ms(None, i64::MIN + 1, 199_000), 199_000, "unknown (media3's TIME_UNSET)");
    }
}
