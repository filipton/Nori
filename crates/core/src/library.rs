//! What the app's repository used to decide for itself between two calls into the core: when a failed
//! refresh is an error, how a favourite that the server refused is taken back, how a mix gets drawn when
//! the index has nothing to draw from, and what picking the server's queue back up plays. Each is one
//! call here, so a second client asks the same question and gets the same answer.

use crate::cache_policy::{Page, Read};
use crate::client::{Client, NetResult, Starrable, Write};
use crate::mixes::board::MixDraw;
use crate::{PlayQueue, Song};

/// How many random songs stand in for a mix the index could not draw (no listening history yet).
pub const MIX_FALLBACK_SONGS: i32 = 50;

/// Today where the phone is, as days since 1970: a mix is drawn once a day (or a week) by the calendar the
/// listener lives by, not by UTC's midnight.
pub fn local_epoch_day() -> i64 {
    let now = crate::db::now_ms() / 1000;
    #[cfg(unix)]
    unsafe {
        let t = now as libc::time_t;
        let mut tm: libc::tm = std::mem::zeroed();
        if !libc::localtime_r(&t, &mut tm).is_null() {
            return (now + tm.tm_gmtoff as i64).div_euclid(86_400);
        }
    }
    now.div_euclid(86_400)
}

/// The lyrics of nothing playing: no lines, untimed, as if the server had said so.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn lyrics_none() -> crate::Lyrics {
    crate::Lyrics { synced: false, word_timed: false, lines: Vec::new(), key: 0 }
}

/// What a press on "Resume from server" does with the queue the server kept.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum ResumePlan {
    /// Nothing was saved: say so.
    Nothing { message: String },
    /// Play `songs` from `index`, then seek to `position_ms`.
    Play { songs: Vec<Song>, index: u32, position_ms: u64 },
}

fn resume_plan(q: PlayQueue) -> ResumePlan {
    if q.songs.is_empty() {
        ResumePlan::Nothing { message: crate::words::words(crate::words::Said::NoServerQueue, String::new()) }
    } else {
        ResumePlan::Play { songs: q.songs, index: q.index, position_ms: q.position_ms }
    }
}

/// The favourites handed to the mixes from the stored answer: whether there was one, and what
/// [`Client::mix_favourites_refresh`] needs to ask the server after it.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct FavouritesHanded {
    pub handed: bool,
    pub digest: Option<u64>,
    /// Young enough that the server is not asked.
    pub fresh: bool,
}

/// What a failed refresh means: nothing when a stored answer is already on screen (offline with something
/// to show is not an error), the failure itself when there is nothing to fall back on.
fn refreshed(got: NetResult<Option<Page>>, had_stored: bool) -> NetResult<Option<Page>> {
    match got {
        Err(_) if had_stored => Ok(None),
        other => other,
    }
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Client {
    /// The second half of a screen's read, after `read_stored` painted what was kept: asks the server
    /// unless that was fresh, and returns its answer only when it differs. A failure is only an error
    /// when nothing was stored (`stored_digest` None); otherwise the stored answer stands.
    pub async fn read_refresh(&self, read: Read, stored_digest: Option<u64>) -> NetResult<Option<Page>> {
        refreshed(self.read_fetch(read, stored_digest).await, stored_digest.is_some())
    }

    /// Sends a favourite whose mark is already up (`star_mark`). Offline, the write is kept for later and
    /// this succeeds; a refusal puts the mark from before (`previous`) back, so the screen stops showing a
    /// favourite the server never took, and the error comes back to say so.
    pub async fn star_send(&self, kind: Starrable, id: String, on: bool, previous: Option<bool>) -> NetResult<()> {
        let sent = self.write(Write::Star { kind, id: id.clone(), on }).await;
        if sent.is_err() {
            crate::stars::star_restore(kind, id, previous);
        }
        sent
    }

    /// The starred songs, as stored for this server and with this session's marks, handed to the mixes
    /// (`mix_favourites`) without crossing to the platform and back. The first half of the favourites
    /// read, as `read_stored` is of a screen's.
    pub fn mix_favourites_stored(&self) -> NetResult<FavouritesHanded> {
        let stored = self.read_stored(Read::StarredItems)?;
        let handed = match stored.page {
            Some(Page::StarredPage { v }) => {
                self.core.mix_favourites(v.songs);
                true
            }
            _ => false,
        };
        Ok(FavouritesHanded { handed, digest: stored.digest, fresh: stored.fresh })
    }

    /// The second half: the server's starred songs handed to the mixes when they differ from the stored
    /// ones (`stored_digest`). True when they were.
    pub async fn mix_favourites_refresh(&self, stored_digest: Option<u64>) -> NetResult<bool> {
        Ok(match self.read_refresh(Read::StarredItems, stored_digest).await? {
            Some(Page::StarredPage { v }) => {
                self.core.mix_favourites(v.songs);
                true
            }
            _ => false,
        })
    }

    /// Every album of the artist `artist_id` (a provider's left out), in order, as one list of songs: the
    /// artist as stored, or asked for when it is not, then [`Client::artist_songs`].
    pub async fn artist_songs_of(&self, artist_id: String) -> NetResult<Vec<Song>> {
        let read = || Read::ArtistById { id: artist_id.clone() };
        let page = match self.read_stored(read())?.page {
            Some(p) => p,
            None => self.read_fetch(read(), None).await?.unwrap_or(Page::Albums { v: Vec::new() }),
        };
        let Page::ArtistPage { v } = page else { return Ok(Vec::new()) };
        Ok(self.artist_songs(v.albums).await)
    }

    /// Draws mix `id` unless today's (or this week's) draw is there already; `again` asks for a different
    /// one. With no listening history the index draws nothing, and the server's random songs stand in.
    /// True when the draw changed, so the tiles and pages read it again.
    pub async fn mix_ensure(&self, id: String, again: bool) -> bool {
        let day = local_epoch_day();
        match self.core.mix_draw(id.clone(), day, again, None) {
            MixDraw::Drawn => true,
            MixDraw::NeedsFallback => {
                self.mix_fallback(id, day, again).await;
                true
            }
            MixDraw::Kept | MixDraw::Unknown => false,
        }
    }

    /// Draws whichever mixes are missing or from the last period. True when any tile changed.
    pub async fn mix_warm_all(&self) -> bool {
        let day = local_epoch_day();
        let warm = self.core.mix_warm(day);
        for id in &warm.needs_fallback {
            self.mix_fallback(id.clone(), day, false).await;
        }
        warm.changed || !warm.needs_fallback.is_empty()
    }

    /// Picks up the queue another device (or the web player) left on the server.
    pub async fn resume_from_server(&self) -> NetResult<ResumePlan> {
        match self.read_now(Read::PullQueue).await? {
            Page::Queue { v } => Ok(resume_plan(v)),
            _ => Ok(resume_plan(PlayQueue { songs: vec![], index: 0, position_ms: 0 })),
        }
    }
}

impl Client {
    async fn mix_fallback(&self, id: String, day: i64, again: bool) {
        let random = self.songs(Read::RandomSongs { size: MIX_FALLBACK_SONGS, genre: None }).await.unwrap_or_default();
        self.core.mix_draw(id, day, again, Some(random));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{block, client};
    use crate::client::NetProfile;

    #[test]
    fn a_failed_refresh_is_only_an_error_with_nothing_stored() {
        let err = || Err(crate::transport::NetError::io("down".into()));
        assert!(matches!(refreshed(err(), true), Ok(None)));
        assert!(refreshed(err(), false).is_err());
        assert!(matches!(refreshed(Ok(None), false), Ok(None)));
    }

    #[test]
    fn a_refused_favourite_takes_its_mark_back() {
        let (c, fake) = client(NetProfile { url: "h".into(), ..Default::default() });
        let marked = crate::stars::star_mark(Starrable::Song, "lib-refused".into(), true);
        fake.answer(r#"{"subsonic-response":{"status":"failed","error":{"code":50,"message":"no"}}}"#);
        assert!(block(c.star_send(Starrable::Song, "lib-refused".into(), true, marked.previous)).is_err());
        assert_eq!(crate::stars::star_marks().songs.get("lib-refused"), None);
    }

    #[test]
    fn nothing_saved_on_the_server_says_so() {
        assert_eq!(resume_plan(PlayQueue { songs: vec![], index: 3, position_ms: 9 }), ResumePlan::Nothing { message: "No queue saved on the server".into() });
        let s = Song { id: "a".into(), ..Default::default() };
        assert_eq!(resume_plan(PlayQueue { songs: vec![s.clone()], index: 0, position_ms: 1200 }), ResumePlan::Play { songs: vec![s], index: 0, position_ms: 1200 });
    }

    #[test]
    fn the_local_day_is_near_utcs() {
        let utc = crate::db::now_ms() / 86_400_000;
        assert!((local_epoch_day() - utc).abs() <= 1);
    }
}
