//! How the queue moves: where songs added by hand go, what shuffle does to the order, what is fetched
//! and measured ahead, and what happens when a song will not play. The platform's player holds the
//! list; this works on its indexes and play order and says what to do.

/// Where songs added by hand go, and the play order afterwards when shuffling.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
    /// The list index the new songs are inserted at.
    pub at: usize,
    /// The shuffled play order afterwards (list indexes, new songs included), when shuffling.
    pub order: Option<Vec<usize>>,
}

/// Play next and Add to queue, the way Apple does them: the songs go right after the playing one ("next",
/// `last` false), or after the songs added by hand before them ("last"), in the order given, and then the
/// queue carries on as it was. In the list itself they sit there too, so turning shuffle off keeps them
/// next. Under shuffle the player would drop each at a random place in the play order, so the order is
/// rebuilt with them where they belong and everything else where it was.
///
/// `n` songs are queued, `cur` is playing, `hand[i]` says song i was added by hand, `order` is the
/// current play order (when shuffling), and `count` songs are being added.
pub fn place(n: usize, cur: usize, hand: &[bool], order: Option<&[usize]>, last: bool, count: usize) -> Placement {
    // The end of the run of hand-added songs after the current one, walking the given order.
    let run_end = |walk: &dyn Fn(usize) -> Option<usize>| {
        let mut end = cur;
        if !last {
            return end;
        }
        let mut i = walk(cur);
        while let Some(x) = i {
            if !hand.get(x).copied().unwrap_or(false) {
                break;
            }
            end = x;
            i = walk(x);
        }
        end
    };
    let in_list = |i: usize| (i + 1 < n).then_some(i + 1);
    let at = run_end(&in_list) + 1;
    let Some(order) = order else { return Placement { at, order: None } };
    let pos_of = |x: usize| order.iter().position(|&o| o == x);
    let in_order = |i: usize| pos_of(i).and_then(|p| order.get(p + 1).copied());
    let end = run_end(&in_order);
    let shift = |i: usize| if i >= at { i + count } else { i };
    let mut next: Vec<usize> = order.iter().map(|&i| shift(i)).collect();
    let after = next.iter().position(|&i| i == shift(end)).map_or(next.len(), |p| p + 1);
    next.splice(after..after, at..at + count);
    Placement { at, order: Some(next) }
}

/// Shuffle turned on: the playing song goes first, the songs added by hand right after it keep their
/// order, and only the rest is shuffled. The player's own order would leave the playing song somewhere in
/// the middle, so the songs before it in that order were never played, and it scatters the hand-added ones.
pub fn shuffle_around(n: usize, cur: usize, hand: &[bool], seed: u64) -> Vec<usize> {
    let mut kept = vec![cur];
    let mut i = cur + 1;
    while i < n && hand.get(i).copied().unwrap_or(false) {
        kept.push(i);
        i += 1;
    }
    let mut rest: Vec<usize> = (0..n).filter(|x| !kept.contains(x)).collect();
    shuffle(&mut rest, seed);
    kept.extend(rest);
    kept
}

/// Fisher-Yates over a small xorshift: an even shuffle, and the same one for the same seed.
pub fn shuffle<T>(items: &mut [T], seed: u64) {
    let mut s = seed | 1;
    for i in (1..items.len()).rev() {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        items.swap(i, (s % (i as u64 + 1)) as usize);
    }
}

/// Which of the songs coming up (0 = the one playing) are fetched ahead, as a range: `count` from the
/// user's setting for this network. The player itself buffers the very next song, so this normally covers
/// the ones after it - except with a transition on: a mix needs that next song decodable a whole
/// crossfade before the player would otherwise want it, and audio that arrives too late to be mixed into
/// leaves a hole where the end of the song should be. Nothing extra is fetched, only earlier. Shuffle moves
/// the goalposts, so nothing is fetched deep into a queue about to be reordered - but the next song is the
/// next song whatever the order. `None`: nothing to fetch.
pub fn precache_range(count: usize, mixing: bool, shuffling: bool) -> Option<(usize, usize)> {
    let first = if mixing { 1 } else { 2 };
    let last = (if shuffling { 0 } else { count }).max(if mixing { 1 } else { 0 });
    (last >= first).then_some((first, last))
}

/// Whether songs meet in a mix, for [`precache_range`]: a crossfade or AutoMix is on and the output
/// allows touching the samples at all.
pub fn mixing(transitions_off: bool, crossfade_s: i32, auto_mix: bool) -> bool {
    !transitions_off && (crossfade_s > 0 || auto_mix)
}

/// How many songs are fetched ahead: the user's setting for the network the phone is on.
pub fn precache_count(metered: bool, wifi: i32, mobile: i32) -> usize {
    (if metered { mobile } else { wifi }).max(0) as usize
}

/// How many of the songs coming up (0 = the one playing) are measured ahead for AutoMix.
pub const MEASURE_AHEAD: usize = 3;

/// How many songs coming up are measured ahead: none while AutoMix is off (nothing plans from them).
pub fn measure_ahead(auto_mix: bool) -> usize {
    if auto_mix {
        MEASURE_AHEAD
    } else {
        0
    }
}

/// A song would not play.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaybackError {
    /// The audio output refused the stream (typically an offloaded one).
    Output,
    /// The server could not be reached.
    Network,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnError {
    /// Stop handing the audio chip compressed audio for good, rebuild on the CPU path, play the same song.
    GiveUpOffload,
    /// Hand it to the offline bridge (a downloaded song still queued, or downloads until the server is back).
    Bridge,
    /// Skip to the next song and play.
    Skip,
    /// Leave it stopped.
    Stop,
}

/// What to do when a song will not play. An output that refuses an offloaded stream would refuse the next
/// song the same way, so the chain goes back to the CPU instead. One unplayable or unreachable song should
/// not end the evening: a network failure goes to the offline bridge when it is on, anything else skips -
/// at most three in a row, then it stops.
pub fn on_error(kind: PlaybackError, offload_refused: bool, bridge: bool, skip_on_error: bool, has_next: bool, errors_in_a_row: u32) -> OnError {
    match kind {
        PlaybackError::Output if !offload_refused => OnError::GiveUpOffload,
        PlaybackError::Network if bridge => OnError::Bridge,
        _ if skip_on_error && has_next && errors_in_a_row < 3 => OnError::Skip,
        _ => OnError::Stop,
    }
}

/// The run of songs that would not play. Only [`on_error`] and a failed bridge skip, and they count; a
/// song that starts, or a bridge that took over, breaks the run.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ErrorRun {
    in_a_row: u32,
}

impl ErrorRun {
    pub const fn new() -> Self {
        ErrorRun { in_a_row: 0 }
    }

    /// A song would not play: [`on_error`] with the run so far, counted when it skips.
    pub fn failed(&mut self, kind: PlaybackError, offload_refused: bool, bridge: bool, skip_on_error: bool, has_next: bool) -> OnError {
        let d = on_error(kind, offload_refused, bridge, skip_on_error, has_next, self.in_a_row);
        if d == OnError::Skip {
            self.in_a_row += 1;
        }
        d
    }

    /// The offline bridge could not take a network failure over: skipped like any other, same limit.
    pub fn bridge_failed(&mut self, skip_on_error: bool, has_next: bool) -> bool {
        let skip = on_error(PlaybackError::Other, true, false, skip_on_error, has_next, self.in_a_row) == OnError::Skip;
        if skip {
            self.in_a_row += 1;
        }
        skip
    }

    /// A song started, or the bridge took over: the run is broken.
    pub fn played(&mut self) {
        self.in_a_row = 0;
    }

    pub fn in_a_row(&self) -> u32 {
        self.in_a_row
    }
}

/// Keeping the music going past the end of the queue. A fetch starts when the queue's end is near - the
/// last song, or up to [`FILL_AHEAD`] still following - so that a fast run of nexts does not hit a wall
/// while similar songs are still on the wire (a server asking Last.fm for them takes seconds), and only
/// one is on the wire at a time. What comes back goes in after the song that was last when the fetch
/// started, and only while it still is: a queue given songs from elsewhere meanwhile is left alone.
///
/// A next pressed with nothing after is remembered, and taken when the songs land - unless the user has
/// moved on since, or the press is older than [`NEXT_KEPT_MS`]: by then the user has settled on the song
/// and a skip out of nowhere would be a surprise (a phone's refill took 4.6 s once, and the song the user
/// had been listening to for four seconds was skipped). The songs still go in; only the skip is dropped.
/// Many presses at the end are one skip, timed from the last of them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Refill {
    in_flight: bool,
    /// The queue's last song (in play order) when the fetch started.
    end: Option<String>,
    /// The song a next was pressed on with nothing after it, and when (the latest press, in ms of the
    /// caller's monotonic clock).
    pending_from: Option<String>,
    pending_at_ms: i64,
}

/// How many songs may still follow the current one when the refill starts.
pub const FILL_AHEAD: usize = 2;
/// How long a next pressed at the end waits for the songs to land. Past this, the press is forgotten.
pub const NEXT_KEPT_MS: i64 = 2_000;

/// Whether a queue may be refilled at all: a song is playing and not a radio stream, repeat is off (a
/// repeating queue has no end) and the user has the setting on.
pub fn refillable(song: bool, radio: bool, repeat: u8, setting: bool) -> bool {
    song && !radio && repeat == crate::playlist::REPEAT_OFF && setting
}

impl Refill {
    pub const fn new() -> Self {
        Refill { in_flight: false, end: None, pending_from: None, pending_at_ms: 0 }
    }

    /// The queue moved (or a next was pressed): whether to start fetching now, `after` songs still
    /// following the current one and `end` last. A true answer means a fetch is on the wire until
    /// [`Refill::arrived`].
    pub fn start(&mut self, refillable: bool, after: usize, end: Option<&str>) -> bool {
        if !refillable || after > FILL_AHEAD || self.in_flight {
            return false;
        }
        self.in_flight = true;
        self.end = end.map(str::to_string);
        true
    }

    /// Next pressed on `current` at `now_ms`: true to skip at once. With nothing after, the press is
    /// remembered - only while the queue can be refilled at all (repeat off, setting on) - and the caller
    /// starts a fetch unless one is out.
    pub fn next(&mut self, has_next: bool, can_refill: bool, current: Option<&str>, now_ms: i64) -> bool {
        if has_next {
            self.pending_from = None;
            return true;
        }
        if can_refill {
            self.pending_from = current.map(str::to_string);
            self.pending_at_ms = now_ms;
        }
        false
    }

    /// The fetch came back with `count` songs while `end` is the queue's last song: whether they go in.
    /// They do not when there are none, or when the queue's end is no longer the one they were fetched
    /// for (songs added, a new queue); the fetch is then over, and so is a waiting next.
    pub fn arrived(&mut self, count: usize, end: Option<&str>) -> bool {
        let keep = count > 0 && end.is_some() && self.end.as_deref() == end;
        if !keep {
            self.pending_from = None;
            self.in_flight = false;
            self.end = None;
        }
        keep
    }

    /// The songs that arrived are in the queue: whether to take the waiting next now, which it is only
    /// if the user is still on the song it was pressed on, the press is recent, and there is somewhere
    /// to go.
    pub fn landed(&mut self, current: Option<&str>, has_next: bool, now_ms: i64) -> bool {
        let waiting = self.skip_waiting(current, now_ms);
        self.in_flight = false;
        self.end = None;
        self.pending_from = None;
        waiting && has_next
    }

    /// Whether a next pressed at the end is waiting for songs on the wire and will be taken if they come
    /// now: what a screen may show as a skip on its way (the next button busy). It turns false by itself
    /// [`NEXT_KEPT_MS`] after the last press.
    pub fn skip_waiting(&self, current: Option<&str>, now_ms: i64) -> bool {
        self.in_flight && self.pending_from.as_deref().is_some_and(|p| Some(p) == current) && now_ms - self.pending_at_ms <= NEXT_KEPT_MS
    }

    pub fn in_flight(&self) -> bool {
        self.in_flight
    }
}

/// What a song the player has just arrived on means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Onto {
    /// An explicit song with the user's setting to skip them, and somewhere to go: straight on.
    Skip,
    /// The same song again under repeat: counted as a play again, nothing else changes.
    Loop,
    /// A new song: its gain, its place saved, what comes after it fetched and refilled.
    Song,
}

/// The player moved onto a song (`song` false: onto nothing). `looped` is the player's own repeat.
pub fn arrival(song: bool, skip_explicit: bool, explicit: bool, has_next: bool, looped: bool) -> Onto {
    if song && skip_explicit && explicit && has_next {
        Onto::Skip
    } else if song && looped {
        Onto::Loop
    } else {
        Onto::Song
    }
}

/// Previous goes back to the start of the song rather than to the one before once this far in (media3's
/// own rule), unless the user asked for it always to skip.
pub const PREVIOUS_REWINDS_AFTER_MS: i64 = 3_000;

/// Whether previous restarts the song playing here (else the player's own previous decides).
pub fn previous_restarts(position_ms: i64, has_previous: bool, always_skips: bool) -> bool {
    !(always_skips && has_previous) && position_ms > PREVIOUS_REWINDS_AFTER_MS
}

/// Repeat off, all, one, then off again.
pub fn next_repeat(mode: u8) -> u8 {
    match mode {
        0 => 2, // off -> all (media3: REPEAT_MODE_ALL = 2)
        2 => 1, // all -> one (REPEAT_MODE_ONE = 1)
        _ => 0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_next_goes_right_after_the_playing_song() {
        let hand = [false, false, true, false, false];
        assert_eq!(place(5, 1, &hand, None, false, 2), Placement { at: 2, order: None });
    }

    #[test]
    fn add_to_queue_goes_after_the_songs_added_by_hand_before() {
        let hand = [false, false, true, true, false];
        assert_eq!(place(5, 1, &hand, None, true, 1).at, 4);
    }

    #[test]
    fn under_shuffle_the_new_songs_follow_the_current_one_in_play_order() {
        // Playing 3; play order 3, 0, 4, 1, 2. Two songs added "next": inserted in the list after 3 (at 4),
        // everything at or past 4 shifts by two, and they come right after 3 in the order.
        let order = [3, 0, 4, 1, 2];
        let p = place(5, 3, &[false; 5], Some(&order), false, 2);
        assert_eq!(p.at, 4);
        assert_eq!(p.order, Some(vec![3, 4, 5, 0, 6, 1, 2]));
    }

    #[test]
    fn shuffle_keeps_the_playing_song_first_and_the_hand_added_after_it() {
        let hand = [false, false, true, true, false, false, false];
        let o = shuffle_around(7, 1, &hand, 42);
        assert_eq!(&o[..3], &[1, 2, 3]);
        let mut rest = o[3..].to_vec();
        rest.sort();
        assert_eq!(rest, vec![0, 4, 5, 6]);
    }

    #[test]
    fn precache_covers_the_songs_after_the_next_unless_mixing() {
        assert_eq!(precache_range(3, false, false), Some((2, 3)));
        assert_eq!(precache_range(3, true, false), Some((1, 3)), "a mix needs the next song early");
        assert_eq!(precache_range(3, false, true), None, "shuffle: nothing deep");
        assert_eq!(precache_range(3, true, true), Some((1, 1)), "but the next song is the next song");
    }

    #[test]
    fn errors_skip_three_then_stop_and_output_refusals_leave_offload() {
        assert_eq!(on_error(PlaybackError::Output, false, false, true, true, 0), OnError::GiveUpOffload);
        assert_eq!(on_error(PlaybackError::Output, true, false, true, true, 0), OnError::Skip, "already off offload");
        assert_eq!(on_error(PlaybackError::Network, false, true, true, true, 0), OnError::Bridge);
        assert_eq!(on_error(PlaybackError::Other, false, true, true, true, 2), OnError::Skip);
        assert_eq!(on_error(PlaybackError::Other, false, true, true, true, 3), OnError::Stop);
        assert_eq!(on_error(PlaybackError::Other, false, false, false, true, 0), OnError::Stop);
    }

    #[test]
    fn the_error_run_counts_skips_and_breaks_on_a_song() {
        let mut r = ErrorRun::new();
        for _ in 0..3 {
            assert_eq!(r.failed(PlaybackError::Other, false, false, true, true), OnError::Skip);
        }
        assert_eq!(r.failed(PlaybackError::Other, false, false, true, true), OnError::Stop, "three in a row, then it stops");
        assert_eq!(r.in_a_row(), 3);
        r.played();
        assert_eq!(r.failed(PlaybackError::Network, false, true, true, true), OnError::Bridge);
        assert_eq!(r.in_a_row(), 0, "handing to the bridge is not a skip");
        assert!(r.bridge_failed(true, true));
        assert!(!r.bridge_failed(false, true), "skip on error off");
        assert!(!r.bridge_failed(true, false), "nothing after");
        assert!(r.bridge_failed(true, true) && r.bridge_failed(true, true));
        assert!(!r.bridge_failed(true, true), "the bridge's skips count toward the same three");
        assert_eq!(r.failed(PlaybackError::Output, false, false, true, true), OnError::GiveUpOffload);
        assert_eq!(r.in_a_row(), 3);
    }

    #[test]
    fn what_is_fetched_and_measured_follows_the_settings() {
        assert!(mixing(false, 4, false) && mixing(false, 0, true));
        assert!(!mixing(false, 0, false), "no transition");
        assert!(!mixing(true, 4, true), "the output forbids it");
        assert_eq!((precache_count(true, 3, 1), precache_count(false, 3, 1), precache_count(false, -1, 1)), (1, 3, 0));
        assert_eq!((measure_ahead(true), measure_ahead(false)), (3, 0));
    }

    #[test]
    fn an_explicit_song_is_skipped_only_with_somewhere_to_go() {
        assert_eq!(arrival(true, true, true, true, false), Onto::Skip);
        assert_eq!(arrival(true, true, true, true, true), Onto::Skip, "even looping: the setting wins");
        assert_eq!(arrival(true, true, true, false, false), Onto::Song, "the last song plays");
        assert_eq!(arrival(true, false, true, true, false), Onto::Song, "setting off");
        assert_eq!(arrival(true, true, false, true, false), Onto::Song);
        assert_eq!(arrival(true, false, false, true, true), Onto::Loop);
        assert_eq!(arrival(false, true, true, true, true), Onto::Song, "onto nothing");
    }

    #[test]
    fn refilling_starts_ahead_and_once() {
        assert!(refillable(true, false, 0, true));
        assert!(!refillable(false, false, 0, true), "nothing playing");
        assert!(!refillable(true, true, 0, true), "a radio stream");
        assert!(!refillable(true, false, 2, true), "repeat all");
        assert!(!refillable(true, false, 1, true), "repeat one");
        assert!(!refillable(true, false, 0, false), "setting off");
        let mut f = Refill::new();
        assert!(!f.start(true, FILL_AHEAD + 1, Some("e")), "plenty left");
        assert!(!f.start(false, 0, Some("e")), "not refillable");
        assert!(f.start(true, FILL_AHEAD, Some("e")), "the end in sight: fetch now");
        assert!(!f.start(true, 0, Some("e")), "already on the wire");
        assert!(f.arrived(5, Some("e")), "the end is where it was, however far the user is from it");
        assert!(!f.landed(Some("a"), true, 0), "no next was waiting");
        assert!(!f.in_flight());
        assert!(f.start(true, 0, Some("e")));
        assert!(!f.arrived(0, Some("e")), "nothing came");
        assert!(f.start(true, 0, Some("e")), "and the next move may try again");
        assert!(!f.arrived(3, Some("added")), "the queue was given songs meanwhile");
        assert!(f.start(true, 1, Some("e")));
        assert!(!f.arrived(3, None), "the queue was emptied");
    }

    #[test]
    fn a_next_at_the_end_is_taken_when_the_songs_land() {
        let mut f = Refill::new();
        assert!(!f.next(false, true, Some("last"), 0));
        assert!(f.start(true, 0, Some("last")));
        assert!(f.skip_waiting(Some("last"), 10), "a screen may show the skip on its way");
        assert!(f.arrived(3, Some("last")));
        assert!(f.landed(Some("last"), true, 800));
        // Moved on meanwhile (previous, a jump): the press is dropped.
        assert!(!f.next(false, true, Some("last"), 1_000));
        assert!(f.start(true, 0, Some("last")));
        assert!(!f.skip_waiting(Some("other"), 1_010));
        assert!(f.arrived(3, Some("last")));
        assert!(!f.landed(Some("other"), true, 1_200));
        // A press while a fetch is out waits for it; a press with a song after clears the waiting one.
        assert!(f.start(true, 1, Some("y")));
        assert!(!f.next(false, true, Some("x"), 2_000));
        assert!(!f.start(true, 0, Some("y")), "one on the wire");
        assert!(f.next(true, true, Some("x"), 2_100), "there is a next now");
        assert!(f.arrived(2, Some("y")));
        assert!(!f.landed(Some("x"), true, 2_200), "that press was already taken");
        // A queue that cannot be refilled remembers nothing.
        assert!(!f.next(false, false, Some("r"), 3_000));
        assert!(!f.skip_waiting(Some("r"), 3_000));
        assert!(f.start(true, 0, Some("r")));
        assert!(f.arrived(1, Some("r")));
        assert!(!f.landed(Some("r"), true, 3_100));
        // Nothing came: the waiting next goes with the fetch.
        assert!(!f.next(false, true, Some("y"), 4_000));
        assert!(f.start(true, 0, Some("y")));
        assert!(!f.arrived(0, Some("y")));
        assert!(f.start(true, 0, Some("y")));
        assert!(f.arrived(1, Some("y")));
        assert!(!f.landed(Some("y"), true, 4_100));
        // Landed with nowhere to go (the songs went in but the player cannot step): no skip.
        assert!(!f.next(false, true, Some("z"), 5_000));
        assert!(f.start(true, 0, Some("z")));
        assert!(f.arrived(1, Some("z")));
        assert!(!f.landed(Some("z"), false, 5_100));
    }

    #[test]
    fn a_next_at_the_end_expires_and_counts_once() {
        // The phone's case: the songs took 4.6 s, the user had settled on the song. They go in; no skip.
        let mut f = Refill::new();
        assert!(!f.next(false, true, Some("fpt"), 10_000));
        assert!(f.start(true, 0, Some("fpt")));
        assert!(f.skip_waiting(Some("fpt"), 10_000 + NEXT_KEPT_MS));
        assert!(!f.skip_waiting(Some("fpt"), 10_001 + NEXT_KEPT_MS), "the wait shows no longer than it holds");
        assert!(f.arrived(15, Some("fpt")), "the songs still go in");
        assert!(!f.landed(Some("fpt"), true, 14_600), "a press 4.6 s old is not taken");
        // A fast answer: taken.
        assert!(!f.next(false, true, Some("a"), 20_000));
        assert!(f.start(true, 0, Some("a")));
        assert!(f.arrived(15, Some("a")));
        assert!(f.landed(Some("a"), true, 20_000 + NEXT_KEPT_MS), "at the edge of the window still");
        // Mashed: six presses, one skip, the window counted from the last of them.
        assert!(!f.next(false, true, Some("b"), 30_000));
        assert!(f.start(true, 0, Some("b")));
        for k in 1..6 {
            assert!(!f.next(false, true, Some("b"), 30_000 + k * 150));
            assert!(!f.start(true, 0, Some("b")), "one fetch for all of them");
        }
        assert!(f.arrived(15, Some("b")));
        assert!(f.landed(Some("b"), true, 30_750 + NEXT_KEPT_MS - 1), "within the window of the last press");
        assert!(!f.landed(Some("b"), true, 30_750 + NEXT_KEPT_MS - 1), "and only once");
        // Mashed and then the answer was slow: nothing.
        assert!(!f.next(false, true, Some("c"), 40_000));
        assert!(f.start(true, 0, Some("c")));
        assert!(!f.next(false, true, Some("c"), 40_300));
        assert!(f.arrived(15, Some("c")));
        assert!(!f.landed(Some("c"), true, 40_301 + NEXT_KEPT_MS));
    }

    #[test]
    fn previous_and_repeat() {
        assert!(previous_restarts(5_000, true, false));
        assert!(!previous_restarts(1_000, true, false));
        assert!(!previous_restarts(5_000, true, true), "always skips");
        assert!(previous_restarts(5_000, false, true), "nothing before to skip to: restart");
        assert_eq!((next_repeat(0), next_repeat(2), next_repeat(1)), (2, 1, 0));
    }
}
