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

/// How many of the songs coming up (0 = the one playing) are measured ahead for AutoMix.
pub const MEASURE_AHEAD: usize = 3;

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
    fn previous_and_repeat() {
        assert!(previous_restarts(5_000, true, false));
        assert!(!previous_restarts(1_000, true, false));
        assert!(!previous_restarts(5_000, true, true), "always skips");
        assert!(previous_restarts(5_000, false, true), "nothing before to skip to: restart");
        assert_eq!((next_repeat(0), next_repeat(2), next_repeat(1)), (2, 1, 0));
    }
}
