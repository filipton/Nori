//! The queue itself: which songs, in which list order, the order they play in, which were added by
//! hand, which one is current, shuffle and repeat. The platform's player mirrors it - every change is
//! made here first and handed to the player as the same change, with the play order written out -
//! so what the queue is, and what comes next, is answered here without asking the player.

use crate::queue::{place, shuffle, shuffle_around};

/// How a song came into the queue.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Hand {
    /// With the list it was played from.
    #[default]
    No,
    /// Play next.
    Next,
    /// Add to queue.
    Last,
    /// Brought in by the offline bridge: a download played while the server is out of reach.
    Bridge,
}

impl Hand {
    /// Added by the user (Play next, Add to queue).
    pub fn by_user(self) -> bool {
        matches!(self, Hand::Next | Hand::Last)
    }
}

/// A change to the list the platform's player makes the same way: the ranges taken out (last first, so
/// each is still where it says), then `count` songs put in at `at`, then, when set, a jump to `seek`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Splice {
    pub remove: Vec<(usize, usize)>,
    pub at: usize,
    pub count: usize,
    pub seek: Option<usize>,
}

/// media3's repeat modes, same numbers.
pub const REPEAT_OFF: u8 = 0;
pub const REPEAT_ONE: u8 = 1;
pub const REPEAT_ALL: u8 = 2;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Playlist {
    ids: Vec<String>,
    hand: Vec<Hand>,
    /// The play order while shuffling (list indexes); empty otherwise.
    order: Vec<usize>,
    shuffling: bool,
    /// Shuffle shown as on: the player's own shuffle, or a list that was put in a shuffled order before it
    /// was queued (a weighted shuffle that the player's would undo).
    lit: bool,
    cur: Option<usize>,
    /// While the offline bridge plays: the song the queue picks up at once the server is back.
    parked: Option<usize>,
    repeat: u8,
    /// Bumped on every change to the list or its order, so a reader knows to look again.
    rev: u64,
    /// Bumped only when the songs listed change (not their play order or the current one), so a reader
    /// that holds the songs already need not copy them again.
    list_rev: u64,
}

impl Playlist {
    pub const fn new() -> Self {
        Playlist { ids: Vec::new(), hand: Vec::new(), order: Vec::new(), shuffling: false, lit: false, cur: None, parked: None, repeat: REPEAT_OFF, rev: 0, list_rev: 0 }
    }

    pub fn ids(&self) -> &[String] {
        &self.ids
    }
    pub fn len(&self) -> usize {
        self.ids.len()
    }
    pub fn is_empty(&self) -> bool {
        self.ids.is_empty()
    }
    pub fn current(&self) -> Option<usize> {
        self.cur
    }
    pub fn current_id(&self) -> Option<&str> {
        self.cur.and_then(|i| self.ids.get(i)).map(String::as_str)
    }
    pub fn shuffling(&self) -> bool {
        self.shuffling
    }
    pub fn lit(&self) -> bool {
        self.lit || self.shuffling
    }
    pub fn repeat(&self) -> u8 {
        self.repeat
    }
    pub fn rev(&self) -> u64 {
        self.rev
    }
    pub fn list_rev(&self) -> u64 {
        self.list_rev
    }
    pub fn hand(&self, i: usize) -> Hand {
        self.hand.get(i).copied().unwrap_or_default()
    }
    /// While the offline bridge plays: the song the queue picks up at once the server is back.
    pub fn parked_id(&self) -> Option<&str> {
        self.parked.and_then(|i| self.ids.get(i)).map(String::as_str)
    }
    pub fn by_hand(&self) -> impl Iterator<Item = usize> + '_ {
        self.hand.iter().enumerate().filter(|(_, h)| h.by_user()).map(|(i, _)| i)
    }

    /// The shuffled order, when shuffling: what the player is told to walk.
    pub fn shuffle_order(&self) -> Option<&[usize]> {
        self.shuffling.then_some(self.order.as_slice())
    }

    /// The list indexes in the order they play.
    pub fn play_order(&self) -> impl Iterator<Item = usize> + '_ {
        let (walk, n) = if self.shuffling { (Some(&self.order), self.order.len()) } else { (None, self.ids.len()) };
        (0..n).map(move |k| walk.map_or(k, |o| o[k]))
    }

    fn position(&self, i: usize) -> Option<usize> {
        if self.shuffling {
            self.order.iter().position(|&o| o == i)
        } else {
            (i < self.ids.len()).then_some(i)
        }
    }

    fn at_position(&self, p: usize) -> Option<usize> {
        if self.shuffling {
            self.order.get(p).copied()
        } else {
            (p < self.ids.len()).then_some(p)
        }
    }

    /// The song after `i` in play order, repeat as given (media3's `getNextWindowIndex`).
    pub fn next_of(&self, i: usize, repeat: u8) -> Option<usize> {
        if repeat == REPEAT_ONE {
            return Some(i);
        }
        let p = self.position(i)?;
        match self.at_position(p + 1) {
            Some(n) => Some(n),
            None if repeat == REPEAT_ALL => self.at_position(0),
            None => None,
        }
    }

    /// The song before `i` in play order.
    pub fn previous_of(&self, i: usize, repeat: u8) -> Option<usize> {
        if repeat == REPEAT_ONE {
            return Some(i);
        }
        let p = self.position(i)?;
        match p.checked_sub(1).and_then(|q| self.at_position(q)) {
            Some(n) => Some(n),
            None if repeat == REPEAT_ALL => self.at_position(self.ids.len().checked_sub(1)?),
            None => None,
        }
    }

    /// What next and previous skip to: repeat-one skips like repeat-all.
    pub fn next(&self) -> Option<usize> {
        self.next_of(self.cur?, if self.repeat == REPEAT_ONE { REPEAT_ALL } else { self.repeat })
    }
    pub fn previous(&self) -> Option<usize> {
        self.previous_of(self.cur?, if self.repeat == REPEAT_ONE { REPEAT_ALL } else { self.repeat })
    }

    /// How many songs still follow the current one in play order, repeat left out.
    pub fn songs_after(&self) -> usize {
        match self.cur.and_then(|c| self.position(c)) {
            Some(p) => self.len() - 1 - p,
            None => 0,
        }
    }

    /// The songs coming up, the current one first, in play order, repeat left out.
    pub fn upcoming(&self) -> impl Iterator<Item = usize> + '_ {
        let from = self.cur.and_then(|c| self.position(c)).unwrap_or(self.len());
        (from..self.len()).filter_map(move |p| self.at_position(p))
    }

    /// A new queue, `start` playing (none: wherever shuffle starts). Returns the song to start at.
    pub fn set(&mut self, ids: Vec<String>, start: Option<usize>, shuffling: bool, seed: u64) -> Option<usize> {
        let n = ids.len();
        self.ids = ids;
        self.list_rev += 1;
        self.hand = vec![Hand::No; n];
        self.parked = None;
        self.shuffling = shuffling && n > 0;
        self.lit = shuffling;
        self.cur = match (n, start) {
            (0, _) => None,
            (_, Some(s)) => Some(s.min(n - 1)),
            (_, None) if shuffling => {
                let mut all: Vec<usize> = (0..n).collect();
                shuffle(&mut all, seed);
                Some(all[0])
            }
            (_, None) => Some(0),
        };
        self.order = match (self.shuffling, self.cur) {
            (true, Some(c)) => shuffle_around(n, c, &vec![false; n], seed),
            _ => Vec::new(),
        };
        self.rev += 1;
        self.cur
    }

    /// A list already in the order it should play, shown as shuffled.
    pub fn set_ordered(&mut self, ids: Vec<String>) -> Option<usize> {
        let at = self.set(ids, Some(0), false, 0);
        self.lit = true;
        at
    }

    /// Songs added by hand (Play next, Add to queue): where they went in the list. An empty queue takes
    /// them as its list.
    pub fn add(&mut self, ids: Vec<String>, hand: Hand) -> usize {
        let count = ids.len();
        let Some(cur) = self.cur.filter(|_| !self.ids.is_empty()) else {
            let at = self.ids.len();
            self.insert(at, ids, hand);
            if self.cur.is_none() && !self.ids.is_empty() {
                self.cur = Some(0);
            }
            return at;
        };
        let flags: Vec<bool> = self.hand.iter().map(|h| h.by_user()).collect();
        let p = place(self.ids.len(), cur, &flags, self.shuffling.then_some(self.order.as_slice()), hand == Hand::Last, count);
        self.splice(p.at, ids, hand);
        if let Some(o) = p.order {
            self.order = o;
        }
        self.rev += 1;
        p.at
    }

    /// Songs a controller hands over at `at`, each marked with how it came (`hands`, one per song). Songs
    /// that were all added by hand, to a queue that already has songs, go where Play next or Add to queue
    /// puts them (the first one's say decides); anything else is the controller's own insert at `at`.
    /// Returns where they went in the list.
    pub fn take(&mut self, at: usize, ids: Vec<String>, hands: &[Hand]) -> usize {
        if !self.ids.is_empty() && !hands.is_empty() && hands.iter().all(|h| h.by_user()) {
            return self.add(ids, hands[0]);
        }
        let at = at.min(self.ids.len());
        self.insert(at, ids, Hand::No);
        at
    }

    /// Shuffle shown as on or off because the user asked for it, before the change itself reaches the
    /// queue; the change then says the same.
    pub fn show_shuffle(&mut self, on: bool) {
        self.lit = on;
    }

    /// Songs inserted at `at` (a controller's own insert): under shuffle they play after the rest.
    pub fn insert(&mut self, at: usize, ids: Vec<String>, hand: Hand) {
        let at = at.min(self.ids.len());
        let count = ids.len();
        self.splice(at, ids, hand);
        if self.shuffling {
            for o in self.order.iter_mut() {
                if *o >= at {
                    *o += count;
                }
            }
            self.order.extend(at..at + count);
        }
        self.rev += 1;
    }

    fn splice(&mut self, at: usize, ids: Vec<String>, hand: Hand) {
        let count = ids.len();
        self.ids.splice(at..at, ids);
        self.list_rev += 1;
        self.hand.splice(at..at, std::iter::repeat_n(hand, count));
        for i in [self.cur.as_mut(), self.parked.as_mut()].into_iter().flatten() {
            if *i >= at {
                *i += count;
            }
        }
    }

    /// Songs `from..to` taken out. The current one going, the song after it in play order is current
    /// (the one before, when it was the last; none when nothing is left) - media3 moves on the same way.
    pub fn remove(&mut self, from: usize, to: usize) {
        let to = to.min(self.ids.len());
        if from >= to {
            return;
        }
        let gone = |i: usize| (from..to).contains(&i);
        let shift = |i: usize| if i >= to { i - (to - from) } else { i };
        if let Some(c) = self.cur {
            if gone(c) {
                let order: Vec<usize> = self.play_order().collect();
                let p = order.iter().position(|&o| o == c).unwrap_or(0);
                let after = order[p..].iter().find(|&&o| !gone(o)).or_else(|| order[..p].iter().rev().find(|&&o| !gone(o)));
                self.cur = after.map(|&i| shift(i));
            } else {
                self.cur = Some(shift(c));
            }
        }
        self.parked = self.parked.filter(|&h| !gone(h)).map(shift);
        self.ids.drain(from..to);
        self.list_rev += 1;
        self.hand.drain(from..to);
        self.order.retain(|&o| !gone(o));
        for o in self.order.iter_mut() {
            *o = shift(*o);
        }
        if self.ids.is_empty() {
            self.cur = None;
            self.shuffling = false;
            self.order.clear();
        }
        self.rev += 1;
    }

    /// Songs `from..to` moved so the first lands at `new_index` (media3's `moveMediaItems`). The play
    /// order under shuffle keeps each song where it was.
    pub fn move_range(&mut self, from: usize, to: usize, new_index: usize) {
        let n = self.ids.len();
        let to = to.min(n);
        if from >= to || from == new_index {
            return;
        }
        let count = to - from;
        let new_index = new_index.min(n - count);
        // Where each old index goes.
        let mut map: Vec<usize> = (0..n).collect();
        let mut list: Vec<usize> = (0..n).collect();
        let moved: Vec<usize> = list.drain(from..to).collect();
        list.splice(new_index..new_index, moved);
        for (new, &old) in list.iter().enumerate() {
            map[old] = new;
        }
        let ids = std::mem::take(&mut self.ids);
        let hand = std::mem::take(&mut self.hand);
        let mut slots: Vec<Option<(String, Hand)>> = ids.into_iter().zip(hand).map(Some).collect();
        for &old in &list {
            let (id, h) = slots[old].take().expect("each index moves once");
            self.ids.push(id);
            self.hand.push(h);
        }
        self.list_rev += 1;
        self.cur = self.cur.map(|c| map[c]);
        self.parked = self.parked.map(|h| map[h]);
        for o in self.order.iter_mut() {
            *o = map[*o];
        }
        self.rev += 1;
    }

    /// Shuffle turned on or off. On: the current song first, the songs added by hand after it, the rest
    /// shuffled. Off: the list order again.
    pub fn set_shuffle(&mut self, on: bool, seed: u64) {
        self.lit = on;
        if on == self.shuffling {
            return;
        }
        self.shuffling = on && !self.ids.is_empty();
        let flags: Vec<bool> = self.hand.iter().map(|h| h.by_user()).collect();
        self.order = match (self.shuffling, self.cur) {
            (true, Some(c)) => shuffle_around(self.ids.len(), c, &flags, seed),
            (true, None) => {
                let mut all: Vec<usize> = (0..self.ids.len()).collect();
                shuffle(&mut all, seed);
                all
            }
            _ => Vec::new(),
        };
        self.rev += 1;
    }

    /// The player's list as it is, for a change made there rather than here. Songs still at the same
    /// place keep how they came in.
    pub fn adopt(&mut self, ids: Vec<String>, current: Option<usize>, order: Option<Vec<usize>>) {
        let hand = ids.iter().enumerate().map(|(i, id)| if self.ids.get(i) == Some(id) { self.hand(i) } else { Hand::No }).collect();
        let n = ids.len();
        if self.ids != ids {
            self.list_rev += 1;
        }
        self.ids = ids;
        self.hand = hand;
        self.parked = None;
        self.cur = current.filter(|&c| c < n);
        self.shuffling = order.as_ref().is_some_and(|o| o.len() == n && n > 0);
        self.order = if self.shuffling { order.unwrap_or_default() } else { Vec::new() };
        self.rev += 1;
    }

    /// The offline bridge is playing: downloads stand in for the queue until the server is back.
    pub fn bridging(&self) -> bool {
        self.hand.contains(&Hand::Bridge)
    }

    /// Whether the song after the current one is where the parked queue picks up: the bridge has run
    /// out of downloads to play before it.
    pub fn next_is_parked(&self) -> bool {
        self.parked.is_some() && self.cur.and_then(|c| self.next_of(c, REPEAT_OFF)) == self.parked
    }

    /// Downloads to play while the server is out of reach. The first time, the song that would not play
    /// and everything after it are parked behind them and the first of them plays now; while bridging,
    /// more go in just before the parked song. They play in the order given, whatever the shuffle.
    pub fn bridge(&mut self, ids: Vec<String>) -> Option<Splice> {
        if ids.is_empty() {
            return None;
        }
        let (before, starting) = match (self.parked, self.cur) {
            (Some(h), _) => (h, false),
            (None, Some(c)) => (c, true),
            (None, None) => return None,
        };
        let count = ids.len();
        let pos = self.position(before);
        self.splice(before, ids, Hand::Bridge);
        if self.shuffling {
            for o in self.order.iter_mut() {
                if *o >= before {
                    *o += count;
                }
            }
            let pos = pos.unwrap_or(self.order.len());
            self.order.splice(pos..pos, before..before + count);
        }
        if starting {
            self.parked = Some(before + count);
            self.cur = Some(before);
        }
        self.rev += 1;
        Some(Splice { remove: Vec::new(), at: before, count, seek: starting.then_some(before) })
    }

    /// The server is back: the bridge's songs go and the parked song plays.
    pub fn unbridge(&mut self) -> Option<Splice> {
        if !self.bridging() {
            return None;
        }
        let mut runs: Vec<(usize, usize)> = Vec::new();
        for (i, h) in self.hand.iter().enumerate() {
            if *h == Hand::Bridge {
                match runs.last_mut() {
                    Some((_, to)) if *to == i => *to = i + 1,
                    _ => runs.push((i, i + 1)),
                }
            }
        }
        runs.reverse();
        let head = self.parked;
        // The parked song is made current first, so taking the bridge out does not move it on.
        if let Some(h) = head {
            self.cur = Some(h);
        }
        for &(from, to) in &runs {
            self.remove(from, to);
        }
        let seek = self.parked.take().or(if self.ids.is_empty() { None } else { Some(0) });
        self.cur = seek;
        self.rev += 1;
        Some(Splice { remove: runs, at: 0, count: 0, seek })
    }

    pub fn set_repeat(&mut self, mode: u8) {
        self.repeat = mode;
    }

    /// The player moved to `index` by itself (a song ended, a controller's seek). A song that was added by
    /// hand and has now played is part of the queue like any other.
    pub fn moved_to(&mut self, index: usize) {
        if index < self.ids.len() && self.cur != Some(index) {
            self.cur = Some(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    fn list(p: &Playlist) -> Vec<&str> {
        p.ids().iter().map(String::as_str).collect()
    }

    fn played(p: &Playlist) -> Vec<&str> {
        p.play_order().map(|i| p.ids()[i].as_str()).collect()
    }

    #[test]
    fn a_plain_queue_plays_in_list_order() {
        let mut p = Playlist::default();
        assert_eq!(p.set(ids(&["a", "b", "c"]), Some(1), false, 0), Some(1));
        assert_eq!((p.next(), p.previous(), p.songs_after()), (Some(2), Some(0), 1));
        assert_eq!(p.upcoming().collect::<Vec<_>>(), [1, 2]);
        p.set_repeat(REPEAT_ALL);
        p.moved_to(2);
        assert_eq!(p.next(), Some(0));
        p.set_repeat(REPEAT_ONE);
        assert_eq!(p.next_of(2, REPEAT_ONE), Some(2), "a song ending under repeat-one plays again");
        assert_eq!(p.next(), Some(0), "but next still skips");
    }

    #[test]
    fn shuffle_keeps_the_current_song_first() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b", "c", "d", "e"]), Some(2), false, 0);
        p.set_shuffle(true, 7);
        assert_eq!(played(&p)[0], "c");
        assert_eq!(p.songs_after(), 4);
        p.set_shuffle(false, 0);
        assert_eq!(played(&p), ["a", "b", "c", "d", "e"]);
        assert!(!p.lit());
    }

    #[test]
    fn a_shuffled_start_picks_its_own_first_song() {
        let mut p = Playlist::default();
        let start = p.set(ids(&["a", "b", "c", "d"]), None, true, 99).unwrap();
        assert_eq!(p.play_order().next(), Some(start));
        assert!(p.lit() && p.shuffling());
    }

    #[test]
    fn play_next_and_add_to_queue_under_shuffle() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b", "c", "d"]), Some(1), true, 3);
        assert_eq!(played(&p)[0], "b");
        p.add(ids(&["x"]), Hand::Last);
        p.add(ids(&["y"]), Hand::Last);
        p.add(ids(&["z"]), Hand::Next);
        assert_eq!(&played(&p)[..4], ["b", "z", "x", "y"]);
        assert_eq!(list(&p)[..5], ["a", "b", "z", "x", "y"]);
        assert_eq!(p.by_hand().collect::<Vec<_>>(), [2, 3, 4]);
        assert_eq!(p.current_id(), Some("b"));
    }

    #[test]
    fn songs_handed_over_go_where_they_were_marked_for() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b", "c"]), Some(0), false, 0);
        assert_eq!(p.take(99, ids(&["x", "y"]), &[Hand::Last, Hand::Last]), 1, "added by hand: after the playing song");
        assert_eq!(p.take(99, ids(&["n"]), &[Hand::Next]), 1, "play next: right after it, ahead of the others");
        assert_eq!(p.take(99, ids(&["l"]), &[Hand::Last]), 4, "add to queue: after the ones added before");
        assert_eq!(p.take(1, ids(&["m"]), &[Hand::Next, Hand::No]), 1, "not all by hand: the controller's own insert");
        assert_eq!(p.take(99, ids(&["e"]), &[Hand::No]), 8, "an insert past the end lands at the end");
        let mut empty = Playlist::default();
        assert_eq!(empty.take(5, ids(&["q"]), &[Hand::Next]), 0, "an empty queue takes them as its list");
        assert_eq!(empty.hand(0), Hand::No);
    }

    #[test]
    fn shuffle_shown_follows_the_ask_and_the_queue() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b"]), Some(0), false, 0);
        p.show_shuffle(true);
        assert!(p.lit() && !p.shuffling(), "asked for before the queue changed");
        p.set_ordered(ids(&["b", "a"]));
        assert!(p.lit() && !p.shuffling(), "a weighted shuffle stays lit with the player's shuffle off");
        p.add(ids(&["c"]), Hand::Next);
        assert!(p.lit(), "editing the queue keeps it lit");
        p.show_shuffle(false);
        p.set_shuffle(false, 0);
        assert!(!p.lit(), "turned off");
        p.set(ids(&["a"]), Some(0), false, 0);
        assert!(!p.lit(), "a plain play");
    }

    #[test]
    fn removing_the_current_song_moves_on() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b", "c"]), Some(1), false, 0);
        p.remove(1, 2);
        assert_eq!((list(&p), p.current_id()), (vec!["a", "c"], Some("c")));
        p.remove(1, 2);
        assert_eq!(p.current_id(), Some("a"), "the last one gone: the one before");
        p.remove(0, 1);
        assert_eq!((p.current(), p.shuffling()), (None, false));
    }

    #[test]
    fn removing_under_shuffle_keeps_the_rest_of_the_order() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b", "c", "d", "e"]), Some(0), true, 11);
        let before: Vec<String> = played(&p).iter().map(|s| s.to_string()).collect();
        p.remove(2, 3);
        let expect: Vec<&str> = before.iter().map(String::as_str).filter(|s| *s != "c").collect();
        assert_eq!(played(&p), expect);
    }

    #[test]
    fn moving_songs_like_media3() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b", "c", "d"]), Some(0), false, 0);
        p.move_range(0, 1, 2);
        assert_eq!((list(&p), p.current_id()), (vec!["b", "c", "a", "d"], Some("a")));
        p.move_range(3, 4, 0);
        assert_eq!(list(&p), ["d", "b", "c", "a"]);
        assert_eq!(p.current(), Some(3));
    }

    #[test]
    fn an_empty_queue_takes_added_songs_as_its_list() {
        let mut p = Playlist::default();
        assert_eq!(p.add(ids(&["a", "b"]), Hand::Last), 0);
        assert_eq!((list(&p), p.current()), (vec!["a", "b"], Some(0)));
    }

    #[test]
    fn the_bridge_parks_the_queue_and_gives_it_back() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b", "c", "d"]), Some(1), false, 0);
        // "b" would not play: two downloads stand in, "b" and what follows are parked behind them.
        assert_eq!(p.bridge(ids(&["x", "y"])), Some(Splice { remove: vec![], at: 1, count: 2, seek: Some(1) }));
        assert_eq!((list(&p), p.current_id()), (vec!["a", "x", "y", "b", "c", "d"], Some("x")));
        assert!(p.bridging() && !p.next_is_parked());
        assert!(p.by_hand().next().is_none(), "bridge songs are not the user's");
        p.moved_to(2);
        assert!(p.next_is_parked());
        // Still offline: more go in before the parked song, and the song playing stays.
        assert_eq!(p.bridge(ids(&["z"])), Some(Splice { remove: vec![], at: 3, count: 1, seek: None }));
        assert_eq!((list(&p), p.current_id()), (vec!["a", "x", "y", "z", "b", "c", "d"], Some("y")));
        // Back online: the bridge goes, the parked song plays.
        assert_eq!(p.unbridge(), Some(Splice { remove: vec![(1, 4)], at: 0, count: 0, seek: Some(1) }));
        assert_eq!((list(&p), p.current_id(), p.bridging()), (vec!["a", "b", "c", "d"], Some("b"), false));
        assert_eq!(p.unbridge(), None);
    }

    #[test]
    fn the_bridge_plays_next_under_shuffle_too() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b", "c", "d"]), Some(2), true, 9);
        let parked = played(&p).iter().map(|s| s.to_string()).collect::<Vec<_>>();
        p.bridge(ids(&["x", "y"]));
        let now = played(&p);
        assert_eq!(&now[..3], ["x", "y", "c"], "the bridge, then the parked song, then the rest as before");
        assert_eq!(now[3..].iter().map(|s| s.to_string()).collect::<Vec<_>>(), parked[1..]);
    }

    #[test]
    fn a_controllers_insert_under_shuffle_plays_after_the_rest() {
        let mut p = Playlist::default();
        p.set(ids(&["a", "b", "c"]), Some(0), true, 5);
        p.insert(1, ids(&["x"]), Hand::No);
        assert_eq!(list(&p), ["a", "x", "b", "c"]);
        assert_eq!(*played(&p).last().unwrap(), "x");
        assert_eq!(p.current_id(), Some("a"));
    }
}
