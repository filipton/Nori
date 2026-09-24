//! The offline bridge: when the server is gone mid-evening and the next queued song is not on the phone,
//! play from the downloads until the network is back, then return to the queue where it was parked.
//! The queue is the core's (playlist.rs): this chooses the songs and makes the change, and the platform
//! makes the same change to its player.

use nori_model::Song;
use nori_player::queue::shuffle;

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
}
