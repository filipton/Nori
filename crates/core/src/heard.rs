//! Which song the ear is on and where, for the app's seek bar and now-playing page:
//! `nori_player::heard` fed with the transition engine's latest reading. Asked every frame the bar is
//! drawn, so the answer is a few numbers, packed into one `i64` for a platform that asks across a
//! language boundary, and nothing is allocated.

use nori_player::engine::Heard;
use nori_player::heard::{HeardTracker, Playhead, Seen};
use parking_lot::Mutex;

/// The engine's latest word on what the ear has, for [`HeardClock`].
static HEARD: Mutex<Heard> = Mutex::new(Heard {
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
});

/// What the engine says the ear has now, after each call made on it. Asked about on every position
/// query while a hold or mix runs, so it is copied in place, reusing the strings it already has.
pub fn publish(heard: &Heard) {
    let mut shared = HEARD.lock();
    if *shared != *heard {
        shared.assign(heard);
    }
}

/// Whether a mix is being heard right now (for the test bridge and logs).
pub fn mixing() -> bool {
    HEARD.lock().mixing
}

/// Whether the ear is behind the player on a held ending (for logs).
pub fn holding() -> bool {
    HEARD.lock().id.is_some()
}

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

/// Which queue row the now playing page shows while the ear is on another song than the player (a held
/// ending, a mix): the ear's `heard` row, or none when the player's own current row stands. When the
/// queue was just read again (`reread`), the row is found by the song's id in the new list (`ids_now`),
/// since positions moved with the edit; a song no longer there, or one that is the player's own song
/// (`playing`), shows as the player says.
///
/// Twin of the `heardIndex` worked out in `PlayerConnection.publish` (core/.../playback/PlayerConnection.kt),
/// which Android keeps: it runs on each player event, and its lists are the ones Compose draws.
pub fn shown_index<'a>(heard: Option<usize>, reread: bool, ids_before: &[&'a str], ids_now: &[&'a str], playing: Option<&str>) -> Option<usize> {
    let at = heard?;
    let at = if reread { ids_now.iter().position(|id| Some(id) == ids_before.get(at))? } else { at };
    (ids_now.get(at).copied() != playing).then_some(at)
}

/// The tracker, and the revision of the core's queue it was last given (crates/core/src/playlist.rs).
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
    /// followed to yet (`nori_player::heard::Playhead`).
    pub fn position(&mut self, now_ms: i64, playing: bool, on: Option<usize>, next: Option<usize>, position_ms: i64, shown: Option<usize>) -> HeardAt {
        let s = self.seen(now_ms, playing, on, next, position_ms);
        let ms = self.head.show(&self.t, s, shown, now_ms);
        at(s, ms)
    }

    /// Where the seek bar is while nothing can be asked (the app reconnecting to the player): the last place
    /// shown, run on from then if the music was `playing`.
    pub fn run_on(&self, now_ms: i64, playing: bool) -> i64 {
        self.head.run_on(now_ms, playing)
    }

    fn seen(&mut self, now_ms: i64, playing: bool, on: Option<usize>, next: Option<usize>, position_ms: i64) -> Seen {
        let heard = HEARD.lock();
        let rev = crate::playlist::playlist_rev();
        if self.rev != rev {
            self.rev = rev;
            self.t.set_queue(crate::playlist::with(|p| crate::queue::durations(p.ids())));
        }
        self.t.at_index(&heard, now_ms, playing, on, next, position_ms)
    }
}

fn at(s: Seen, ms: i64) -> HeardAt {
    HeardAt { index: s.index, changed: s.changed, ms }
}
