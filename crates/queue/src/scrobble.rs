//! Counting plays. A play counts once the configured share of the song (or four minutes) has actually
//! been heard. There is no timer: listening time is summed from play and pause edges and judged when
//! the song is left, a moment the radio is awake anyway. The song left is recorded in the local history
//! (what the taste model, the mixes and the smart playlists feed on) here, in the background; the
//! platform is told what to send the server.

use nori_db as db;
use nori_db::background;
use nori_model::Song;
use parking_lot::Mutex;

use crate::queue;

struct Scrobbler {
    /// The song playing, kept from when it started: the queue may be replaced before it is left.
    song: Option<Song>,
    started_at: i64,
    heard_ms: i64,
    playing_since: i64,
}

static SCROBBLER: Mutex<Scrobbler> = Mutex::new(Scrobbler { song: None, started_at: 0, heard_ms: 0, playing_since: 0 });

/// What to tell the server when a song is left.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ScrobbleSend {
    /// The song just left, if it was heard long enough to count, and when it started.
    pub submit_id: Option<String>,
    pub submit_at: i64,
    /// The song now playing, for the server's "now playing".
    pub now_playing_id: Option<String>,
}

fn edge(s: &mut Scrobbler, playing: bool, now: i64) {
    if playing && s.playing_since == 0 {
        s.playing_since = now;
    }
    if !playing && s.playing_since != 0 {
        s.heard_ms += now - s.playing_since;
        s.playing_since = 0;
    }
}

/// Playback started or stopped at `now_ms` (a monotonic clock).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn scrobble_playing(playing: bool, now_ms: i64) {
    edge(&mut SCROBBLER.lock(), playing, now_ms);
}

/// How long of `duration_s` has to be heard for a play to count: `percent` (10..100) of it, at most
/// four minutes, at least ten seconds.
pub fn needed_ms(duration_s: i64, percent: i32) -> i64 {
    (duration_s * 10 * percent.clamp(10, 100) as i64).min(240_000).max(10_000)
}

/// Why the player's song changed, for [`scrobble_track`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum TrackChange {
    /// Onto another song (or onto nothing).
    Moved,
    /// The same song again under repeat: a play of its own.
    Looped,
    /// Playback came to its end.
    Ended,
}

/// What the scrobbler follows after a change to `id`: nothing once playback ended, and a radio stream
/// only when it loops (a stream moved onto is not a song to count or to announce).
fn followed(id: Option<String>, why: TrackChange) -> Option<String> {
    match why {
        TrackChange::Ended => None,
        TrackChange::Looped => id,
        TrackChange::Moved => id.filter(|i| !i.starts_with(queue::RADIO_PREFIX)),
    }
}

/// The player's song changed to `id` (`why`), playing or not, at `now_ms` (monotonic) and `wall_ms`.
/// Records the song left in the history when the taste model is on, and says what to send when
/// scrobbling is, counting a play at the share of the song the settings ask for.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn scrobble_track(id: Option<String>, why: TrackChange, playing: bool, now_ms: i64, wall_ms: i64, tz_offset_ms: i32) -> ScrobbleSend {
    let (taste_model, scrobble, percent) = nori_settings::settings_store::with_prefs(|p| (p.taste_model, p.scrobble, p.scrobble_percent)).unwrap_or((true, true, 50));
    track(followed(id, why), playing, now_ms, wall_ms, tz_offset_ms, taste_model, scrobble, percent)
}

#[allow(clippy::too_many_arguments)]
fn track(next: Option<String>, playing: bool, now_ms: i64, wall_ms: i64, tz_offset_ms: i32, taste_model: bool, scrobble: bool, percent: i32) -> ScrobbleSend {
    let (done, heard, at) = {
        let mut s = SCROBBLER.lock();
        edge(&mut s, false, now_ms);
        let done = std::mem::replace(&mut s.song, next.clone().and_then(queue::queue_song));
        let heard = std::mem::take(&mut s.heard_ms);
        let at = std::mem::replace(&mut s.started_at, wall_ms);
        edge(&mut s, playing, now_ms);
        (done, heard, at)
    };
    if let (Some(song), true) = (done.clone(), taste_model) {
        background::run(move || {
            if let Some(db) = nori_db::active() {
                let _ = nori_library::history::record(&mut db.lock(), &song, at, heard, tz_offset_ms, db::now_ms());
            }
        });
    }
    if !scrobble {
        return ScrobbleSend { submit_id: None, submit_at: 0, now_playing_id: None };
    }
    let submit_id = done.filter(|s| heard >= needed_ms(s.duration as i64, percent)).map(|s| s.id);
    ScrobbleSend { submit_id, submit_at: at, now_playing_id: next }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_play_counts_after_its_share_or_four_minutes() {
        assert_eq!(needed_ms(200, 50), 100_000);
        assert_eq!(needed_ms(600, 50), 240_000, "four minutes at most");
        assert_eq!(needed_ms(10, 50), 10_000, "ten seconds at least");
        assert_eq!(needed_ms(200, 5), 20_000, "the share is at least 10 %");
    }

    #[test]
    fn what_a_change_is_followed_as() {
        let id = |s: &str| Some(s.to_string());
        assert_eq!(followed(id("s1"), TrackChange::Moved), id("s1"));
        assert_eq!(followed(id("radio:4"), TrackChange::Moved), None, "a stream moved onto");
        assert_eq!(followed(id("radio:4"), TrackChange::Looped), id("radio:4"), "a loop keeps whatever it is");
        assert_eq!(followed(id("s1"), TrackChange::Ended), None);
        assert_eq!(followed(None, TrackChange::Moved), None);
    }

    #[test]
    fn scrobbling_off_sends_nothing() {
        let sent = track(Some("next".into()), true, 1_000, 1_000, 0, false, false, 50);
        assert_eq!(sent, ScrobbleSend { submit_id: None, submit_at: 0, now_playing_id: None });
        let sent = track(Some("after".into()), true, 2_000, 2_000, 0, false, true, 50);
        assert_eq!(sent.now_playing_id.as_deref(), Some("after"));
        assert_eq!(sent.submit_id, None, "a song the queue does not know is not counted");
    }
}
