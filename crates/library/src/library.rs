//! What the app's repository used to decide for itself between two calls into the core: when a failed
//! refresh is an error, how a favourite that the server refused is taken back, how a mix gets drawn when
//! the index has nothing to draw from, and what picking the server's queue back up plays. Each is one
//! call here, so a second client asks the same question and gets the same answer.

use nori_model::{PlayQueue, Song};

/// How many random songs stand in for a mix the index could not draw (no listening history yet).
pub const MIX_FALLBACK_SONGS: i32 = 50;

/// Today where the phone is, as days since 1970: a mix is drawn once a day (or a week) by the calendar the
/// listener lives by, not by UTC's midnight.
pub fn local_epoch_day() -> i64 {
    let now = nori_db::now_ms() / 1000;
    (now + local_offset_s(now)).div_euclid(86_400)
}

/// How far the phone's clock is ahead of UTC at `at_s` seconds since 1970, in seconds (summer time
/// included); 0 where the C library cannot say.
#[cfg_attr(not(unix), allow(unused_variables))]
pub fn local_offset_s(at_s: i64) -> i64 {
    #[cfg(unix)]
    unsafe {
        let t = at_s as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&t, &mut tm).is_null() {
            return tm.tm_gmtoff as i64;
        }
    }
    0
}

/// The lyrics of nothing playing: no lines, untimed, as if the server had said so.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn lyrics_none() -> nori_model::Lyrics {
    nori_model::Lyrics { synced: false, word_timed: false, lines: Vec::new(), key: 0 }
}

/// What a press on "Resume from server" does with the queue the server kept.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum ResumePlan {
    /// Nothing was saved: the client says so.
    Nothing,
    /// Play `songs` from `index`, then seek to `position_ms`.
    Play { songs: Vec<Song>, index: u32, position_ms: u64 },
}

/// What the server's queue `q` plays when it is picked back up.
pub fn resume_plan(q: PlayQueue) -> ResumePlan {
    if q.songs.is_empty() {
        ResumePlan::Nothing
    } else {
        ResumePlan::Play { songs: q.songs, index: q.index, position_ms: q.position_ms }
    }
}

/// The favourites handed to the mixes from the stored answer: whether there was one, and what
/// the client's `mix_favourites_refresh` needs to ask the server after it.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct FavouritesHanded {
    pub handed: bool,
    pub digest: Option<u64>,
    /// Young enough that the server is not asked.
    pub fresh: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_saved_on_the_server_says_so() {
        assert_eq!(resume_plan(PlayQueue { songs: vec![], index: 3, position_ms: 9 }), ResumePlan::Nothing);
        let s = Song { id: "a".into(), ..Default::default() };
        assert_eq!(resume_plan(PlayQueue { songs: vec![s.clone()], index: 0, position_ms: 1200 }), ResumePlan::Play { songs: vec![s], index: 0, position_ms: 1200 });
    }

    #[test]
    fn the_local_day_is_near_utcs() {
        let utc = nori_db::now_ms() / 86_400_000;
        assert!((local_epoch_day() - utc).abs() <= 1);
    }
}
