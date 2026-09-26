//! Which song the ear is on and where, for the app's seek bar and now-playing page:
//! `nori_player::heard` over the player's own word. The player plays through its own transition engine
//! and says itself which song is heard, so no held ending is ever told here: the tracker is given
//! nothing held. Asked every frame the bar is drawn, so the answer is a few numbers, packed into one
//! `i64` for a platform that asks across a language boundary, and nothing is allocated.

use nori_player::engine::Heard;
use nori_player::heard::{HeardTracker, Playhead, Seen};

/// Nothing held and nothing mixing: the player's own word stands.
const NOTHING: Heard = Heard {
    id: None,
    us: 0,
    at_ms: 0,
    until_us: i64::MAX,
    mixing: false,
    next_id: None,
    next_from_us: 0,
    next_rate: 1.0,
    from_id: None,
    audible_us: i64::MAX,
};

const MS_BITS: u32 = 43;

/// Where the ear is: the queue index it is on (none: the player's own word stands), whether that
/// changed since the last question, and the place in ms.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HeardAt {
    pub index: Option<usize>,
    pub changed: bool,
    pub ms: i64,
}

impl HeardAt {
    /// `(index + 1) << 44 | changed << 43 | ms`, index none being 0.
    pub fn pack(self) -> i64 {
        let index = self.index.map_or(0, |i| i as i64 + 1);
        (index << (MS_BITS + 1)) | ((self.changed as i64) << MS_BITS) | self.ms.clamp(0, (1 << MS_BITS) - 1)
    }

    /// [`pack`](Self::pack) read back. Twin of `PlayerConnection.read` (core/.../playback/PlayerConnection.kt).
    pub fn unpack(r: i64) -> HeardAt {
        let index = (r as u64 >> (MS_BITS + 1)) as i64 - 1;
        HeardAt { index: (index >= 0).then_some(index as usize), changed: (r >> MS_BITS) & 1 != 0, ms: r & ((1 << MS_BITS) - 1) }
    }
}

/// Which row of the page's list the now playing page shows while the ear is on another song than the
/// player (a held ending, a mix): `heard` is the heard tracker's row in the core's queue (`queue`), which
/// has already chosen between the copies of a song queued twice; `page` is what the page lists and
/// `playing` the player's own song. As a rule the page lists the core's queue and the row is the
/// tracker's. While the player's list trails the core's after a change the page lists the player's, and
/// the row is the heard song's copy there nearest the tracker's row (the earlier one of two as near).
/// None - the player's own row stands - when there is no heard row, the page does not have the song, or
/// it is the song the player is on.
pub fn shown_row<Q: AsRef<str>, P: AsRef<str>>(heard: Option<usize>, queue: &[Q], page: &[P], playing: Option<&str>) -> Option<usize> {
    let at = heard?;
    let id = queue.get(at)?.as_ref();
    let row = if page.get(at).is_some_and(|p| p.as_ref() == id) {
        at
    } else {
        page.iter().enumerate().filter(|(_, p)| p.as_ref() == id).min_by_key(|(i, _)| i.abs_diff(at))?.0
    };
    (Some(id) != playing).then_some(row)
}

/// [`shown_row`] over the core's own queue, for a platform: `heard` as the heard door gave it (-1: the
/// player's own word stands), `page` the ids the page lists; -1 when the player's own row stands.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn heard_shown_row(heard: i32, page: Vec<String>, playing: Option<String>) -> i32 {
    let heard = usize::try_from(heard).ok();
    crate::playlist::with(|p| shown_row(heard, p.ids(), &page, playing.as_deref())).map_or(-1, |r| r as i32)
}

/// The tracker, and the revision of the core's queue it was last given (crates/queue/src/playlist.rs).
pub struct HeardClock {
    t: HeardTracker,
    rev: u64,
    /// The place the seek bar last showed.
    head: Playhead,
}

impl Default for HeardClock {
    fn default() -> Self {
        HeardClock { t: HeardTracker::new(), rev: u64::MAX, head: Playhead::new() }
    }
}

impl HeardClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// Asked with what the player says now: `on` is its current index in the queue and `next` the one it
    /// goes to next. The queue is the core's own; it is read again only when it has changed. With no
    /// index the player's own word stands, and the place is `position_ms`.
    pub fn at(&mut self, now_ms: i64, playing: bool, on: Option<usize>, next: Option<usize>, position_ms: i64) -> HeardAt {
        let s = self.seen(now_ms, playing, on, next, position_ms);
        at(s, s.ms)
    }

    /// [`HeardClock::at`] for the seek bar itself, whose page shows queue index `shown`: the same answer,
    /// but the place is the one the bar shows - held while the ear has moved to a song the page has not
    /// followed to yet (`nori_player::heard::Playhead`). `engine_ms` is the engine's own place in the song
    /// `on`, read on this side of the controller (negative: none, another song or a seek on its way): what
    /// the bar goes by when there is one, rather than `position_ms`, which through a controller is the
    /// session's last word run on at one times, never put right until the next play, pause or seek.
    #[allow(clippy::too_many_arguments)]
    pub fn position(&mut self, now_ms: i64, playing: bool, on: Option<usize>, next: Option<usize>, position_ms: i64, shown: Option<usize>, engine_ms: i64) -> HeardAt {
        let position_ms = if engine_ms >= 0 { engine_ms } else { position_ms };
        let s = self.seen(now_ms, playing, on, next, position_ms);
        let ms = self.head.show_for(&self.t, s, shown, now_ms, on, position_ms, playing);
        at(s, ms)
    }

    /// The listener asked for a place (a seek): the bar shows the next reading as it is, even a moment
    /// back in the same song (`nori_player::heard::Playhead::jumped`).
    pub fn jumped(&mut self) {
        self.head.jumped();
    }

    /// Where the seek bar is while nothing can be asked (the app reconnecting to the player): the last place
    /// shown, run on from then if the music was `playing`.
    pub fn run_on(&self, now_ms: i64, playing: bool) -> i64 {
        self.head.run_on(now_ms, playing)
    }

    fn seen(&mut self, now_ms: i64, playing: bool, on: Option<usize>, next: Option<usize>, position_ms: i64) -> Seen {
        let rev = crate::playlist::playlist_rev();
        if self.rev != rev {
            self.rev = rev;
            self.t.set_queue(crate::playlist::with(|p| crate::queue::durations(p.ids())));
        }
        self.t.at_index(&NOTHING, now_ms, playing, on, next, position_ms)
    }
}

fn at(s: Seen, ms: i64) -> HeardAt {
    HeardAt { index: s.index, changed: s.changed, ms }
}

#[cfg(test)]
mod tests {
    use super::shown_row;

    const Q: [&str; 5] = ["a", "b", "c", "b", "d"];

    #[test]
    fn the_page_listing_the_core_queue_shows_the_tracker_row() {
        // The tracker chose the second "b": the page shows that copy, not the first.
        assert_eq!(shown_row(Some(3), &Q, &Q, Some("d")), Some(3));
        assert_eq!(shown_row(Some(1), &Q, &Q, Some("c")), Some(1));
    }

    #[test]
    fn nothing_heard_or_the_player_own_song_leaves_the_player_row() {
        assert_eq!(shown_row(None, &Q, &Q, Some("a")), None);
        assert_eq!(shown_row(Some(2), &Q, &Q, Some("c")), None, "the ear is on the player's song");
        assert_eq!(shown_row(Some(9), &Q, &Q, None), None, "past the end of the queue");
    }

    #[test]
    fn a_trailing_page_finds_the_nearest_copy() {
        // The player's list has not had "x" put in front yet: the heard "b" at 3 is at 2 there, and of
        // the two copies of it the one nearest the tracker's.
        let page = ["a", "b", "c", "b", "d"];
        let queue = ["x", "a", "b", "c", "b", "d"];
        assert_eq!(shown_row(Some(4), &queue, &page, Some("d")), Some(3));
        assert_eq!(shown_row(Some(2), &queue, &page, Some("c")), Some(1));
        // Two copies as near: the earlier.
        assert_eq!(shown_row(Some(2), &["b", "a", "b"][..], &["b", "a", "c", "a", "b"][..], None), Some(0));
    }

    #[test]
    fn a_page_without_the_song_leaves_the_player_row() {
        assert_eq!(shown_row(Some(4), &Q, &["a", "b", "c"][..], None), None);
        assert_eq!(shown_row::<&str, &str>(Some(0), &Q, &[], None), None);
    }
}
