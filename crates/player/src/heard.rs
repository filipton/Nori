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
    /// The song the page was carried onto, where in it, when that was, and the song the player was on.
    carry: Option<(String, i64, i64, Option<String>)>,
    consumed: Option<String>,
    before: Option<String>,
    /// The transition the ear has been taken through, by its takeover point in the old song and the
    /// point it enters the new one (µs): once over it, the ear stays over it. Numbers, not the song's id,
    /// so that asking allocates nothing; a mix let go of forgets it.
    crossed: Option<(i64, i64)>,
}

/// How long after the engine lets a mix go the page may still be carried on the song it moved to while
/// the player catches up. The two are a few milliseconds apart; a seek back into the old song a moment
/// later is the player's word again.
const CARRY_GRACE_MS: i64 = 1_000;

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
///
/// Within one song the place never goes back by itself, nor leaps ahead. A player's clock read through a
/// media session is the player's last word run on at one times, and when the player says its place again
/// (a mix starting, a hold, a pause) that word can be a little behind where the running on had got: the
/// music started a moment after the place was said. And on the ExoPlayer path the ear's own reading takes
/// over from the player's through a held ending, a few hundred ms from it either way. Shown as they come,
/// those are the bar and the lyrics' word fill stepping back, or skipping a piece of a word, in one frame.
/// Up to [`GLIDE_MS`] behind, the place shown runs on at half the music's pace until the reading has
/// caught up with it (stands, paused); up to [`GLIDE_MS`] ahead, at twice it until it has caught the
/// reading. A jump the listener asked for ([`Playhead::jumped`]) is shown as it is, as is another song.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Playhead {
    ms: i64,
    at_ms: i64,
    /// The song the place was shown in: the page's queue index, else the ear's; `None` for neither.
    song: Option<usize>,
    /// A seek was asked for: the next reading is shown as it is, even behind.
    jumped: bool,
}

/// A reading at most this far behind or ahead of the place shown in the same song is glided to, not
/// jumped to: see [`Playhead`].
pub const GLIDE_MS: i64 = 2_000;
/// The most time one question counts for a glide: a bar not asked for a while (the screen was away, the
/// music paused) does not run a glide on over the whole of it.
pub const GLIDE_STEP_MS: i64 = 250;

impl Playhead {
    pub const fn new() -> Self {
        Playhead { ms: 0, at_ms: 0, song: None, jumped: false }
    }

    /// What the bar shows for `seen`, asked at `now_ms` while the page shows queue index `shown`.
    pub fn show(&mut self, t: &HeardTracker, seen: Seen, shown: Option<usize>, now_ms: i64) -> i64 {
        self.show_for(t, seen, shown, now_ms, None, 0, true)
    }

    /// The listener asked for a place (a seek, a tap on a lyric line): the next reading is shown as it
    /// is, even a moment back in the same song.
    pub fn jumped(&mut self) {
        self.jumped = true;
    }

    /// [`Playhead::show`], told the player's own song (`on`, a queue index) and its place in it, and
    /// whether the music plays. The bar holds while the page is a song behind the ear, for the moment it
    /// takes to follow. A page on the player's own song while the ear is still on the ending before it is
    /// not behind: it was put there (a screen come back mid-mix reads the player's song, and the player
    /// has moved on) and shows the player's place in that song - never the place it held from another
    /// song, from before the screen went away, which put the bar ahead and kept it there until the song
    /// before ended.
    #[allow(clippy::too_many_arguments)]
    pub fn show_for(&mut self, t: &HeardTracker, seen: Seen, shown: Option<usize>, now_ms: i64, on: Option<usize>, position_ms: i64, playing: bool) -> i64 {
        if let (Some(h), Some(s)) = (seen.index, shown) {
            if t.differs(h, s) {
                if on.is_none_or(|o| t.differs(o, s)) {
                    return self.ms;
                }
                return self.put(now_ms, shown, position_ms.max(0), playing);
            }
        }
        self.put(now_ms, shown.or(seen.index), seen.ms, playing)
    }

    /// Shows `ms` in `song` at `now_ms`, never back by itself within the song: see [`Playhead`].
    fn put(&mut self, now_ms: i64, song: Option<usize>, ms: i64, playing: bool) -> i64 {
        let same = !self.jumped && self.at_ms > 0 && self.song == song;
        let step = (now_ms - self.at_ms).max(0);
        // Where the place shown would be, run on at one times since it was shown.
        let run = self.ms + step;
        let shown = if same && ms < self.ms && self.ms - ms <= GLIDE_MS {
            // Behind: half the pace, never back.
            if playing { ms.max(self.ms + step.min(GLIDE_STEP_MS) / 2) } else { self.ms }
        } else if same && playing && ms > run && ms - run <= GLIDE_MS {
            // Ahead: twice the pace, never past the reading.
            ms.min(run + step.min(GLIDE_STEP_MS))
        } else {
            ms
        };
        self.jumped = false;
        self.song = song;
        self.ms = shown;
        self.at_ms = now_ms;
        shown
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
    /// Whether the ear has already been taken over the takeover point `until_us` into `next`. The
    /// reading moves on between the engine's readings at one times, and a fresh reading can land a few
    /// milliseconds behind where that had got (an output's clock is read in steps, and corrected): the
    /// page must not go back to the song it has just left for the moment in between, which is the old
    /// cover flashing up after the new one.
    fn over(&self, h: &Heard, until_us: i64) -> bool {
        self.crossed == Some((until_us, h.next_from_us))
    }

    fn at(&mut self, q: &[(String, i64)], h: &Heard, p: PlayerNow) -> Seen {
        let next = h.next_id.as_deref();
        let result: Option<(&str, i64)> = if let Some(id) = h.id.as_deref() {
            let since = if p.playing { p.now_ms - h.at_ms } else { 0 };
            let ms = h.us / 1000 + since;
            let until = h.until_us / 1000;
            match next {
                // The mix is audible: from here the next song is heard, at the point the mix entered it,
                // never the old one's last seconds jumped through.
                Some(n) if ms >= until || self.over(h, h.until_us) => Some((n, into_next(q, h, (ms - until).max(0)))),
                Some(_) => Some((id, ms.clamp(0, duration(q, id)))),
                None if ms < until => Some((id, ms.clamp(0, duration(q, id)))),
                None => None,
            }
        } else if let (Some(n), Some(on)) = (next, p.on) {
            // The next song arrived at once, so the player was never ahead of the ear and its clock is
            // the truth - but past the point the mix is heard, the truth is the next song.
            let until = h.audible_us / 1000;
            (Some(on) == h.from_id.as_deref() && Some(n) != self.consumed.as_deref() && (p.position_ms >= until || self.over(h, h.audible_us)))
                .then(|| (n, into_next(q, h, (p.position_ms - until).max(0))))
        } else {
            None
        };
        match (result, next) {
            (Some((shown, _)), Some(n)) if shown == n => {
                self.crossed = Some((if h.id.is_some() { h.until_us } else { h.audible_us }, h.next_from_us));
            }
            (_, None) => self.crossed = None,
            _ => {}
        }
        // Once the page is on the next song it stays there until the player has left the old one: the
        // engine letting go and the player moving on are not the same moment, and in between the page
        // would fall back to the player's word - the old song - and flash its cover back. The engine
        // forgets which song the mix left as it lets go, so the carry remembers it itself.
        let shown: Option<(&str, i64)> = result.or_else(|| {
            let (held, ms, at, from) = self.carry.as_ref()?;
            let left = p.on == h.from_id.as_deref() || h.from_id.is_none() && p.on == from.as_deref() && p.now_ms - at < CARRY_GRACE_MS;
            (left && Some(held.as_str()) != self.consumed.as_deref()).then(|| {
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
                Some((id, ms, at, from)) if Some(id.as_str()) == next => {
                    (*ms, *at) = (shown_ms, p.now_ms);
                    set_to(from, p.on);
                }
                c => *c = next.map(|id| (id.to_string(), shown_ms, p.now_ms, p.on.map(str::to_string))),
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
    fn a_reading_behind_the_takeover_does_not_take_the_page_back() {
        let mut t = tracker();
        // Run on from a reading at 193.9 s, the page crosses into b at 194 s.
        assert_eq!(t.at(&holding(193_900_000, 0), now(150, "b", 0)).seen(), Some((B, 5_050)));
        // The next reading, taken a little later, finds the ending 30 ms short of the takeover (an
        // output's clock read in steps, or corrected): the page stays on b, at the point it entered.
        let s = t.at(&holding(193_970_000, 200), now(200, "b", 0));
        assert_eq!(s.seen(), Some((B, 5_000)));
        assert!(!s.changed, "no second change of song");
        // And moves on with it from there.
        assert_eq!(t.at(&holding(193_970_000, 200), now(300, "b", 0)).seen(), Some((B, 5_070)));
    }

    #[test]
    fn a_player_clock_behind_the_takeover_does_not_take_the_page_back() {
        let mut t = tracker();
        let direct = Heard { id: None, ..holding(0, 0) };
        assert_eq!(t.at(&direct, now(0, "a", 194_010)).seen(), Some((B, 5_010)));
        let s = t.at(&direct, now(16, "a", 193_990));
        assert_eq!(s.seen(), Some((B, 5_000)), "the player's clock stepped back 20 ms: still b");
        assert!(!s.changed);
    }

    #[test]
    fn a_mix_let_go_of_entirely_still_does_not_flash_the_old_song() {
        let mut t = tracker();
        let direct = Heard { id: None, ..holding(0, 0) };
        assert_eq!(t.at(&direct, now(0, "a", 196_000)).seen(), Some((B, 7_000)));
        // The engine lets go of the mix and forgets which song it left, a moment before the player
        // moves on: the page stays on b.
        let released = Heard { next_id: None, from_id: None, ..direct.clone() };
        let s = t.at(&released, now(40, "a", 196_040));
        assert_eq!(s.seen(), Some((B, 7_040)));
        assert!(!s.changed);
        let s = t.at(&released, now(60, "b", 7_060));
        assert_eq!((s.index, s.ms), (None, 7_060), "the player's own word once it is on b");
        // Not for long, though: a player still on a a second later has been sent back there.
        let mut t = tracker();
        t.at(&direct, now(0, "a", 196_000));
        assert_eq!(t.at(&released, now(1_500, "a", 10_000)).seen(), None);
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
        assert_eq!(p.show(&t, seen(Some(A), 6_000), Some(A), 1_100), 6_000, "heard on the song shown");
        assert_eq!(p.show(&t, seen(Some(2), 7_000), Some(A), 2_100), 7_000, "another copy of the same song is the same song");
        assert_eq!(p.show(&t, seen(Some(B), 1_000), Some(A), 2_200), 7_000, "the ear moved to b, the page is still on a: hold");
        assert_eq!(p.show(&t, seen(Some(B), 1_100), None, 2_300), 1_100, "a page showing nothing does not hold");
        assert_eq!(p.show(&t, seen(Some(B), 1_200), Some(9), 2_400), 1_200, "nor one the queue does not have");
        assert_eq!(p.run_on(3_400, true), 2_200, "reconnecting while playing: runs on from when it was taken");
        assert_eq!(p.run_on(3_400, false), 1_200, "paused: stands");
        assert_eq!(Playhead::new().run_on(5_000, true), 0, "never shown: zero, not the clock");
    }

    #[test]
    fn the_length_shown_is_the_heard_songs() {
        assert_eq!(shown_duration_ms(Some(180), 200_000, 199_000), 180_000, "the ear is a song behind the player");
        assert_eq!(shown_duration_ms(None, 200_123, 199_000), 200_123, "measured");
        assert_eq!(shown_duration_ms(None, 0, 199_000), 199_000, "not measured yet");
        assert_eq!(shown_duration_ms(None, i64::MIN + 1, 199_000), 199_000, "unknown (media3's TIME_UNSET)");
    }

    #[test]
    fn a_page_come_back_on_the_player_s_song_mid_mix_shows_that_song_s_place() {
        let mut t = HeardTracker::new();
        t.set_queue([("a".to_string(), 200_000), ("b".to_string(), 180_000)]);
        let mut p = Playhead::new();
        let seen = |index, ms| Seen { index, ms, changed: false };
        // The bar last drawn on a, 150 s in; the screen goes away.
        assert_eq!(p.show_for(&t, seen(Some(A), 150_000), Some(A), 1_000, Some(A), 150_000, true), 150_000);
        // It comes back 45 s later mid-mix: the ear on a's ending, the player on b and 2 s into it, and the
        // page put on b (the player's song). The bar shows b's place, not a's held from before.
        assert_eq!(p.show_for(&t, seen(Some(A), 195_000), Some(B), 46_000, Some(B), 2_000, true), 2_000);
        assert_eq!(p.show_for(&t, seen(Some(A), 195_100), Some(B), 46_100, Some(B), 2_100, true), 2_100, "and runs on with it");
        // The page catching up to the ear holds for a moment only, as before.
        assert_eq!(p.show_for(&t, seen(Some(A), 195_200), Some(A), 46_200, Some(B), 2_200, true), 195_200);
        assert_eq!(p.show_for(&t, seen(Some(B), 1_000), Some(A), 46_300, Some(B), 1_000, true), 195_200, "the ear moved to b, the page still on a: held");
        // A page on a song that is neither the ear's nor the player's holds too.
        let mut q = Playhead::new();
        q.show_for(&t, seen(Some(A), 10_000), Some(A), 1_000, Some(A), 10_000, true);
        assert_eq!(q.show_for(&t, seen(Some(A), 50_000), Some(B), 41_000, Some(A), 50_000, true), 10_000);
    }

    /// A mix starts: the player says its place again, a quarter of a second behind where the session had
    /// run its last word on to (the music started a moment after that word). Frame by frame the heard
    /// song's place - the seek bar's and the lyrics' - never goes back, and catches up with the truth
    /// within twice the step. The mix ending moves the page to the next song, whose place is shown as it is.
    #[test]
    fn mix_starts_the_heard_song_s_position_never_goes_backwards() {
        let t = tracker();
        let mut p = Playhead::new();
        let player = |index, ms| Seen { index, ms, changed: false };
        let mut last = 0;
        let mut caught = None;
        for now in (1_000..6_000).step_by(16) {
            // The session's clock: 190 s at 1 s, run on at one times; at 3 s the mix starts and the player's
            // word puts it 250 ms back, from where it runs on again.
            let reading = 189_000 + now - if now >= 3_000 { 250 } else { 0 };
            let shown = p.show_for(&t, player(None, reading), Some(A), now, Some(A), reading, true);
            assert!(shown >= last, "back from {last} to {shown} at {now} ms (reading {reading})");
            assert!(shown - reading <= 250, "never further ahead than the step");
            if now >= 3_000 && shown == reading && caught.is_none() {
                caught = Some(now);
            }
            last = shown;
        }
        let caught = caught.expect("caught up with the reading");
        assert!(caught - 3_000 <= 520, "caught up within twice the step, at {caught}");
        // The same on the ExoPlayer path, the ear on a's held ending while the player is on b: a reading of
        // the ending a moment behind the last one (an output's clock read in steps) does not step back.
        let mut t = tracker();
        let mut p = Playhead::new();
        let mut last = 0;
        for (i, at) in (60_000..61_000).step_by(16).enumerate() {
            let us = 190_000_000 + (at - 60_000) * 1000 - if i >= 30 { 120_000 } else { 0 };
            let seen = t.at(&holding(us, at), now(at, "b", 0));
            let shown = p.show_for(&t, seen, Some(A), at, Some(B), 0, true);
            assert!(shown >= last, "back from {last} to {shown} at {at} ms");
            last = shown;
        }
        // The mix over, the page on b: b's place at once, not a's held.
        assert_eq!(p.show_for(&t, player(None, 5_300), Some(B), 61_100, Some(B), 5_300, true), 5_300);
    }

    #[test]
    fn a_seek_back_is_shown_at_once_and_paused_the_place_stands() {
        let t = tracker();
        let mut p = Playhead::new();
        let player = |ms| Seen { index: None, ms, changed: false };
        assert_eq!(p.show_for(&t, player(50_000), Some(A), 1_000, Some(A), 50_000, true), 50_000);
        // A tap on the lyric line that began 800 ms ago: the listener asked for it.
        p.jumped();
        assert_eq!(p.show_for(&t, player(49_200), Some(A), 1_016, Some(A), 49_200, true), 49_200);
        assert_eq!(p.show_for(&t, player(49_216), Some(A), 1_032, Some(A), 49_216, true), 49_216);
        // Paused, with the place said again a little behind: it stands where it was until the music moves.
        assert_eq!(p.show_for(&t, player(49_100), Some(A), 1_100, Some(A), 49_100, false), 49_216);
        assert_eq!(p.show_for(&t, player(49_100), Some(A), 9_000, Some(A), 49_100, false), 49_216);
        // A step back further than a glide is a jump (another controller's seek): shown as it is.
        assert_eq!(p.show_for(&t, player(30_000), Some(A), 9_016, Some(A), 30_000, true), 30_000);
        // Resumed after a long pause a little behind the place shown: the glide runs on one step's worth,
        // not over the whole pause.
        assert_eq!(p.show_for(&t, player(29_900), Some(A), 60_000, Some(A), 29_900, true), 30_000 + GLIDE_STEP_MS / 2);
    }

    /// A held ending on the ExoPlayer path: the ear's reading takes over from the player's 400 ms ahead of
    /// it, and gives the page back to it 8 s later. Neither is a leap: the place shown catches up at twice
    /// the pace, and waits for the player's at half, every frame moving on.
    #[test]
    fn the_ear_taking_over_from_the_player_neither_leaps_ahead_nor_steps_back() {
        let t = tracker();
        let mut p = Playhead::new();
        let mut last = 0;
        let mut caught = None;
        for now in (60_000..70_000).step_by(16) {
            let player = 180_000 + now - 60_000;
            let seen = if (61_000..69_000).contains(&now) { Seen { index: Some(A), ms: player + 400, changed: false } } else { Seen { index: None, ms: player, changed: false } };
            let shown = p.show_for(&t, seen, Some(A), now, Some(A), player, true);
            assert!(shown >= last, "back from {last} to {shown} at {now} ms");
            assert!(last == 0 || shown - last <= 2 * 16 + 1, "leapt from {last} to {shown} at {now} ms");
            if now >= 61_000 && shown == seen.ms && caught.is_none() {
                caught = Some(now);
            }
            last = shown;
        }
        assert!(caught.is_some_and(|c| c - 61_000 <= 420), "caught up with the ear within the step: {caught:?}");
        assert_eq!(last, 180_000 + 9_984, "back on the player's clock by the end");
    }
}
