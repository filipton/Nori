//! Keeps the music going past the end of the queue. What arrives is the user's choice twice over - songs
//! or a whole album, chosen by what the server calls similar or by the artist, genre or decade - and
//! every route here reads the library, so this never makes octo-fiesta download a provider track. When
//! to fetch (the last song, or one song left, one fetch at a time) and a next pressed at the end while
//! songs are on the way are `nori_player::queue::Refill`'s, kept here over the core's own queue; the
//! platform fetches when told and appends what comes back.

use std::collections::HashSet;

use nori_player::queue::{refillable, shuffle, Refill};
use parking_lot::Mutex;

use crate::cache_policy::{Page, Read};
use crate::client::Client;
use crate::transport::NetError;
use crate::{queue, Song};

/// How many loose songs one fill adds.
const SONGS: usize = 15;
/// How many candidate albums are tried before settling for a short one.
const ALBUM_TRIES: usize = 6;
/// An album shorter than this is a single: taken only if nothing longer is on offer.
const ALBUM_MIN: usize = 3;

/// `AutoFillKind` and `AutoFillBasis`, by ordinal (see settings.rs).
const ALBUMS: i32 = 1;
const ARTIST: i32 = 1;
const GENRE: i32 = 2;
const ERA: i32 = 3;

type Got<T> = Result<T, NetError>;

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
#[uniffi::export]
pub fn autofill_start() -> bool {
    let (ok, after) = refill_facts();
    REFILL.lock().start(ok, after)
}

/// What a next press does now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
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
#[uniffi::export]
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
#[uniffi::export]
pub fn autofill_arrived(count: u32) -> bool {
    let after = crate::playlist::playlist_after() as usize;
    REFILL.lock().arrived(count as usize, after)
}

/// The songs that arrived are in the queue: whether to take a next that was pressed while they were on
/// the way.
#[uniffi::export]
pub fn autofill_landed() -> bool {
    let (current, has_next) = crate::playlist::with(|p| (p.current_id().map(str::to_string), p.next().is_some()));
    REFILL.lock().landed(current.as_deref(), has_next)
}

pub(crate) fn seed_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

fn shuffled<T>(mut v: Vec<T>) -> Vec<T> {
    shuffle(&mut v, seed_now());
    v
}

/// The decade `seed` belongs to, for the era basis; none when the server gave no year.
fn era(seed: &Song) -> Option<(u32, u32)> {
    (seed.year > 0).then(|| (seed.year / 10 * 10, seed.year / 10 * 10 + 9))
}

impl Client {
    /// The first answer a screen would show: the stored one when there is one, else the server's.
    pub(crate) async fn first(&self, read: Read) -> Got<Page> {
        if let Ok(stored) = self.read_stored(read.clone()) {
            if let Some(p) = stored.page {
                return Ok(p);
            }
        }
        self.read_now(read).await
    }

    pub(crate) async fn songs(&self, read: Read) -> Got<Vec<Song>> {
        match self.read_now(read).await? {
            Page::Songs { v } => Ok(v),
            _ => Ok(Vec::new()),
        }
    }

    async fn artist_albums(&self, seed: &Song) -> Got<Vec<crate::Album>> {
        let Some(id) = seed.artist_id.clone() else { return Ok(Vec::new()) };
        match self.first(Read::ArtistById { id }).await? {
            Page::ArtistPage { v } => Ok(v.albums),
            _ => Ok(Vec::new()),
        }
    }

    /// Loose songs to carry on with. Whatever the basis, the order they come back in is kept.
    async fn next_songs(&self, seed: &Song, basis: i32) -> Got<Vec<Song>> {
        Ok(match basis {
            // The artist's best-known songs first, then the rest of their records, so a long evening
            // does not stop after ten tracks.
            ARTIST => {
                let top = match self.first(Read::TopSongs { artist: seed.artist.clone() }).await? {
                    Page::Songs { v } => v,
                    _ => Vec::new(),
                };
                if !top.is_empty() {
                    return Ok(top);
                }
                let mut all = Vec::new();
                for a in self.artist_albums(seed).await?.into_iter().take(3) {
                    all.extend(self.songs(Read::AlbumSongs { id: a.id }).await?);
                }
                all
            }
            GENRE => match &seed.genre {
                Some(g) => shuffled(self.songs(Read::SongsByGenre { genre: g.clone(), count: 100 }).await?),
                None => Vec::new(),
            },
            // Out of the offline index rather than the server: nothing in Subsonic asks for a decade of songs.
            ERA => match era(seed) {
                Some((from, to)) => shuffled(self.core.browse_songs("playCount".into(), true, false, from, to, 0, 100).unwrap_or_default()),
                None => Vec::new(),
            },
            _ => self.songs(Read::SimilarSongs { id: seed.id.clone(), count: 25 }).await?,
        })
    }

    /// The albums one whole record is picked from, by the same four bases.
    async fn album_candidates(&self, seed: &Song, basis: i32) -> Got<Vec<String>> {
        let ids = |v: Vec<crate::Album>| v.into_iter().filter(|a| !a.is_external).map(|a| a.id).collect::<Vec<_>>();
        let albums = |p: Page| match p {
            Page::Albums { v } => v,
            _ => Vec::new(),
        };
        Ok(match basis {
            ARTIST => ids(self.artist_albums(seed).await?),
            GENRE => match &seed.genre {
                Some(g) => shuffled(ids(albums(
                    self.first(Read::AlbumList { kind: "byGenre".into(), size: 30, offset: 0, genre: Some(g.clone()) }).await?,
                ))),
                None => Vec::new(),
            },
            ERA => match era(seed) {
                Some((from, to)) => shuffled(ids(albums(self.first(Read::AlbumsByYear { from: from as i32, to: to as i32, size: 30, offset: 0 }).await?))),
                None => Vec::new(),
            },
            // The records the songs the server calls similar come from.
            _ => {
                let mut seen = HashSet::new();
                self.songs(Read::SimilarSongs { id: seed.id.clone(), count: 50 })
                    .await?
                    .into_iter()
                    .filter(|s| !s.is_external)
                    .filter_map(|s| s.album_id)
                    .filter(|a| seen.insert(a.clone()))
                    .collect()
            }
        })
    }

    /// One whole album, in its own order, for someone who listens to records. One already in the queue
    /// is passed over, so an evening moves on rather than playing the same record twice. A single is an
    /// album as far as the server is concerned, and stopping the evening on one track is not what
    /// "carry on with albums" means: the first record with a side to it wins, and a short one is only
    /// taken if nothing else is on offer.
    async fn next_album(&self, seed: &Song, basis: i32, queued: &HashSet<&str>, played: &HashSet<String>) -> Vec<Song> {
        let candidates = self.album_candidates(seed, basis).await.unwrap_or_default();
        let mut short = Vec::new();
        for pick in candidates.into_iter().filter(|a| seed.album_id.as_ref() != Some(a) && !played.contains(a)).take(ALBUM_TRIES) {
            let songs: Vec<Song> = self
                .songs(Read::AlbumSongs { id: pick })
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|s| !queued.contains(s.id.as_str()) && !s.is_external)
                .collect();
            if songs.len() >= ALBUM_MIN {
                return songs;
            }
            if songs.len() > short.len() {
                short = songs;
            }
        }
        short
    }
}

#[uniffi::export]
impl Client {
    /// What to append to the queue after the song playing, as the settings say (songs or an album, and
    /// chosen by what). Nothing for a provider's song or one the queue does not know; a failed read is
    /// nothing too.
    pub async fn autofill(&self) -> Vec<Song> {
        let (kind, basis) = crate::settings_store::current().map_or((0, 0), |p| (p.auto_fill_kind, p.auto_fill_basis));
        self.autofill_as(kind, basis).await
    }
}

impl Client {
    async fn autofill_as(&self, kind: i32, basis: i32) -> Vec<Song> {
        let (current, ids) = crate::playlist::snapshot();
        let Some(seed) = current.and_then(queue::queue_song) else { return Vec::new() };
        if seed.is_external {
            return Vec::new();
        }
        let queued: HashSet<&str> = ids.iter().map(String::as_str).collect();
        let fresh = if kind == ALBUMS {
            let played: HashSet<String> = queue::queue_albums(ids.clone()).into_iter().collect();
            self.next_album(&seed, basis, &queued, &played).await
        } else {
            self.next_songs(&seed, basis)
                .await
                .unwrap_or_default()
                .into_iter()
                .filter(|s| !queued.contains(s.id.as_str()) && !s.is_external)
                .take(SONGS)
                .collect()
        };
        fresh
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{block, client};
    use crate::client::NetProfile;

    fn song(id: &str, album: &str) -> Song {
        Song { id: id.into(), album_id: Some(album.into()), artist: "A".into(), artist_id: Some("ar".into()), ..Default::default() }
    }

    fn songs_json(ids: &[(&str, &str)]) -> String {
        let s: Vec<String> = ids.iter().map(|(i, a)| format!(r#"{{"id":"{i}","title":"{i}","albumId":"{a}","isDir":false}}"#)).collect();
        format!(r#"{{"subsonic-response":{{"status":"ok","similarSongs2":{{"song":[{}]}}}}}}"#, s.join(","))
    }

    fn album_json(id: &str, songs: &[&str]) -> String {
        let s: Vec<String> = songs.iter().map(|i| format!(r#"{{"id":"{i}","title":"{i}","albumId":"{id}","isDir":false}}"#)).collect();
        format!(r#"{{"subsonic-response":{{"status":"ok","album":{{"id":"{id}","name":"{id}","song":[{}]}}}}}}"#, s.join(","))
    }

    #[test]
    fn similar_songs_skip_what_is_queued_and_keep_their_order() {
        let (c, fake) = client(NetProfile { url: "h".into(), ..Default::default() });
        queue::queue_register(vec![song("af-seed", "al0"), song("af-q", "al0")]);
        fake.answer(&songs_json(&[("af-q", "x"), ("s2", "x"), ("s1", "y")]));
        let _g = crate::playlist::tests::hold(&["af-seed", "af-q"], 0);
        let got = block(c.autofill_as(0, 0));
        assert_eq!(got.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["s2", "s1"]);
        assert!(fake.asked.lock()[0].0.contains("getSimilarSongs2"));
    }

    #[test]
    fn an_album_prefers_one_with_a_side_to_it_and_skips_the_seeds() {
        let (c, fake) = client(NetProfile { url: "h".into(), ..Default::default() });
        queue::queue_register(vec![song("af2-seed", "mine")]);
        // Similar songs from the seed's own album, a single and a full record.
        fake.answer(&songs_json(&[("x1", "mine"), ("x2", "single"), ("x3", "record")]));
        fake.answer(&album_json("single", &["t1"]));
        fake.answer(&album_json("record", &["r1", "r2", "r3"]));
        let _g = crate::playlist::tests::hold(&["af2-seed"], 0);
        let got = block(c.autofill_as(ALBUMS, 0));
        assert_eq!(got.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["r1", "r2", "r3"]);
        assert_eq!(fake.asked.lock().len(), 3, "the seed's own album is never fetched");
    }

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

    #[test]
    fn nothing_for_a_song_the_queue_does_not_know() {
        let (c, fake) = client(NetProfile { url: "h".into(), ..Default::default() });
        let _g = crate::playlist::tests::hold(&["af-unknown"], 0);
        assert!(block(c.autofill_as(0, 0)).is_empty());
        assert!(fake.asked.lock().is_empty());
    }
}
