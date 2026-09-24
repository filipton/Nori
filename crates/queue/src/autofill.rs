//! Keeps the music going past the end of the queue. What arrives is the user's choice twice over - songs
//! or a whole album, chosen by what the server calls similar or by the artist, genre or decade - and
//! every route here reads the library, so this never makes octo-fiesta download a provider track. When
//! to fetch (the last song, or one song left, one fetch at a time) and a next pressed at the end while
//! songs are on the way are `nori_player::queue::Refill`'s, kept here over the core's own queue; the
//! platform fetches when told and appends what comes back.

use nori_model::Song;
use nori_net::transport::NetError;
use nori_player::queue::{refillable, shuffle, Refill};
use parking_lot::Mutex;

use crate::queue;

/// How many loose songs one fill adds.
pub const SONGS: usize = 15;
/// How many candidate albums are tried before settling for a short one.
pub const ALBUM_TRIES: usize = 6;
/// An album shorter than this is a single: taken only if nothing longer is on offer.
pub const ALBUM_MIN: usize = 3;

/// `AutoFillKind` and `AutoFillBasis`, by ordinal (see settings.rs).
pub const ALBUMS: i32 = 1;
pub const ARTIST: i32 = 1;
pub const GENRE: i32 = 2;
pub const ERA: i32 = 3;

pub type Got<T> = Result<T, NetError>;

static REFILL: Mutex<Refill> = Mutex::new(Refill::new());

/// Whether the queue may be refilled now, and how many songs follow the current one.
fn refill_facts() -> (bool, usize) {
    let setting = crate::rules::prefs(|p| p.auto_fill);
    crate::playlist::with(|p| {
        let cur = p.current_id();
        (refillable(cur.is_some(), cur.is_some_and(|c| c.starts_with(queue::RADIO_PREFIX)), p.repeat(), setting), p.songs_after())
    })
}

/// The queue moved: whether to fetch songs for its end now ([`Client::autofill`]). A true answer is a
/// fetch on the wire until [`autofill_arrived`].
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn autofill_start() -> bool {
    let (ok, after) = refill_facts();
    REFILL.lock().start(ok, after)
}

/// What a next press does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum FillNext {
    /// There is a song after: skip to it.
    Skip,
    /// Nothing after: fetch, and the skip is taken when the songs land ([`autofill_landed`]).
    Fetch,
    /// Nothing to do now: songs are already on the way (the press waits for them), or the queue cannot
    /// be refilled.
    Wait,
}

/// Next pressed with the queue's end in sight.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn autofill_next() -> FillNext {
    let setting = crate::rules::prefs(|p| p.auto_fill);
    let (has_next, repeat_off, current) =
        crate::playlist::with(|p| (p.next().is_some(), p.repeat() == nori_player::playlist::REPEAT_OFF, p.current_id().map(str::to_string)));
    if REFILL.lock().next(has_next, setting && repeat_off, current.as_deref()) {
        return FillNext::Skip;
    }
    if !(setting && repeat_off) {
        return FillNext::Wait;
    }
    if autofill_start() {
        FillNext::Fetch
    } else {
        FillNext::Wait
    }
}

/// The fetch came back with `count` songs: whether they go in (see `Refill::arrived`).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn autofill_arrived(count: u32) -> bool {
    let after = crate::playlist::playlist_after() as usize;
    REFILL.lock().arrived(count as usize, after)
}

/// The songs that arrived are in the queue: whether to take a next that was pressed while they were on
/// the way.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn autofill_landed() -> bool {
    let (current, has_next) = crate::playlist::with(|p| (p.current_id().map(str::to_string), p.next().is_some()));
    REFILL.lock().landed(current.as_deref(), has_next)
}

pub fn seed_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

pub fn shuffled<T>(mut v: Vec<T>) -> Vec<T> {
    shuffle(&mut v, seed_now());
    v
}

/// The decade `seed` belongs to, for the era basis; none when the server gave no year.
pub fn era(seed: &Song) -> Option<(u32, u32)> {
    (seed.year > 0).then(|| (seed.year / 10 * 10, seed.year / 10 * 10 + 9))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn when_to_refill_is_read_off_the_queue() {
        let _g = crate::playlist::tests::hold(&["rf1", "rf2", "rf3"], 0);
        *REFILL.lock() = Refill::new();
        assert!(!autofill_start(), "two songs still follow");
        assert_eq!(autofill_next(), FillNext::Skip);
        crate::playlist::playlist_moved_to(2);
        assert_eq!(autofill_next(), FillNext::Fetch, "the last song: fetch, and skip when they land");
        assert!(!autofill_start(), "one fetch at a time");
        assert_eq!(autofill_next(), FillNext::Wait, "a second press waits for the same fetch");
        assert!(autofill_arrived(2));
        crate::playlist::playlist_take(3, vec!["rf4".into(), "rf5".into()], vec![nori_player::playlist::Hand::No; 2]);
        assert!(autofill_landed(), "still on the song the press was made on");
        crate::playlist::playlist_repeat(2);
        crate::playlist::playlist_moved_to(4);
        assert!(!autofill_start(), "a repeating queue has no end");
        crate::playlist::playlist_repeat(0);
        crate::playlist::playlist_set(vec!["radio:1".into()], 0, false);
        assert!(!autofill_start(), "a radio stream is not refilled");
    }
}
