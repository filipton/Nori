//! What the repository used to decide between two calls, as the client's calls: the favourites refreshed,
//! a star taken back, a mix drawn from the server, the server's queue picked up. The decisions themselves
//! are nori-library's.

use crate::cache_policy::{Page, Read};
use crate::client::{Client, NetResult, Starrable, Write};
use crate::mixes::board::MixDraw;
use crate::{PlayQueue, Song};

pub use nori_library::library::*;

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
pub(crate) mod tests {
    use super::*;
    use crate::client::tests::{block, client};
    use crate::client::NetProfile;

    #[test]
    fn a_failed_refresh_is_only_an_error_with_nothing_stored() {
        let err = || Err(nori_net::transport::NetError::io("down".into()));
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
}
