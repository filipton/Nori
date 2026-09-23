//! The offline bridge: when the server is gone mid-evening and the next queued song is not on the phone,
//! play from the downloads until the network is back, then return to the queue where it was parked.
//! The queue is the core's (playlist.rs): this chooses the songs and makes the change, and the platform
//! makes the same change to its player.

use nori_player::queue::shuffle;

use crate::playlist::{self, QueueEdit};
use crate::{db, queue, Core, Result, Song};

/// How many downloads a bridge brings in at a time.
pub const BATCH: u32 = 12;

/// How close a download is to the song the evening was on: the same artist counts most, then the same
/// album, the genre and the artist's id, and a starred song a little. Nothing here reaches for the
/// network - the pool is what is on the phone.
fn score(seed: Option<&Song>, s: &Song) -> i32 {
    let mut v = 0;
    if let Some(seed) = seed {
        if !seed.artist.trim().is_empty() && s.artist.to_lowercase() == seed.artist.to_lowercase() {
            v += 4;
        }
        if seed.album_id.is_some() && s.album_id == seed.album_id {
            v += 3;
        }
        if seed.genre.as_deref().is_some_and(|g| !g.trim().is_empty()) && s.genre == seed.genre {
            v += 2;
        }
        if seed.artist_id.is_some() && s.artist_id == seed.artist_id {
            v += 2;
        }
    }
    if s.starred {
        v += 1;
    }
    v
}

/// Up to `n` of `pool` to bridge with after `seed`: the closest first, shuffled among equals, none that
/// is already queued (`exclude`) or a provider's.
pub fn pick(seed: Option<&Song>, pool: &[Song], exclude: &[String], n: usize, rng_seed: u64) -> Vec<Song> {
    let mut scored: Vec<(i32, &Song)> = pool
        .iter()
        .filter(|s| !exclude.contains(&s.id) && !s.is_external && !s.id.starts_with("ext-"))
        .map(|s| (score(seed, s), s))
        .collect();
    // Shuffle first, then a stable sort by score: songs of one score stay in shuffled order.
    shuffle(&mut scored, rng_seed);
    scored.sort_by(|a, b| b.0.cmp(&a.0));
    scored.into_iter().take(n).map(|(_, s)| s.clone()).collect()
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    /// The server cannot be reached for the song playing: downloads to play instead, the closest to it
    /// first, none already queued. The first time the song and what follows are parked behind them;
    /// while bridging, more go in before the parked song. None when there is nothing downloaded to play.
    pub fn bridge_start(&self) -> Result<Option<QueueEdit>> {
        let (current, queued) = playlist::snapshot();
        let pool = self.downloads(true)?;
        let seed = current.and_then(queue::queue_song);
        let picks = pick(seed.as_ref(), &pool, &queued, BATCH as usize, db::now_ms() as u64);
        if picks.is_empty() {
            return Ok(None);
        }
        let ids = picks.iter().map(|s| s.id.clone()).collect();
        queue::queue_register(picks.clone());
        Ok(playlist::edit_splice(|p| p.bridge(ids), picks))
    }

    /// The first song after the playing one, in play order, that is on the phone: where to skip to when
    /// the server is out of reach before bridging at all. -1 for none.
    pub fn bridge_next_downloaded(&self) -> Result<i32> {
        let after: Vec<(usize, String)> = playlist::with(|p| p.upcoming().skip(1).map(|i| (i, p.ids()[i].clone())).collect());
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT 1 FROM downloads WHERE server=sid() AND id=?1 AND done=1")?;
        for (i, id) in after {
            if st.exists([id])? {
                return Ok(i as i32);
            }
        }
        Ok(-1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, artist: &str, album: &str, starred: bool) -> Song {
        Song { id: id.into(), artist: artist.into(), album_id: Some(album.into()), starred, ..Default::default() }
    }

    #[test]
    fn the_closest_downloads_come_first_and_queued_ones_never() {
        let seed = song("s", "Radiohead", "ok", false);
        let pool = [song("a", "Muse", "x", false), song("b", "radiohead", "kid", false), song("c", "Radiohead", "ok", false), song("d", "Muse", "y", true), song("q", "Radiohead", "ok", false)];
        let p = pick(Some(&seed), &pool, &["q".to_string()], 3, 7);
        let ids: Vec<&str> = p.iter().map(|s| s.id.as_str()).collect();
        assert_eq!(&ids[..2], &["c", "b"], "artist and album, then artist");
        assert_eq!(ids[2], "d", "then starred");
        assert!(!ids.contains(&"q"));
    }

    #[test]
    fn a_bridge_is_started_and_undone_over_the_core_queue() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        for id in ["dl1", "dl2"] {
            core.download_queue(vec![song(id, "Muse", "x", false)]).unwrap();
            core.download_done(id.into()).unwrap();
        }
        queue::queue_register(vec![song("on1", "Muse", "y", false), song("on2", "Muse", "y", false)]);
        let _g = crate::playlist::tests::hold(&["on1", "on2"], 0);
        assert_eq!(core.bridge_next_downloaded().unwrap(), -1, "nothing queued after it is on the phone");
        let e = core.bridge_start().unwrap().unwrap();
        assert_eq!((e.at, e.seek, e.songs.len(), e.remove.len()), (0, 0, 2, 0));
        assert!(crate::playlist::playlist_bridge_state().bridging);
        let back = crate::playlist::playlist_unbridge().unwrap();
        assert_eq!((back.remove, back.seek), (vec![0, 2], 0));
        assert_eq!(crate::playlist::playlist_upcoming(5), vec!["on1".to_string(), "on2".to_string()]);
    }
}
