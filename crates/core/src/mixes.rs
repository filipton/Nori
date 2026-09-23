//! Mixes built from the local index and the taste model in `history.rs`; no server round trip.
//!
//! The index can hold 100k songs, so no mix reads it whole. SQL narrows to a candidate pool of a
//! few times `limit` (through `song_stats`, or through the expression indexes on genre, artist,
//! year, starred and rating), and only that pool is parsed, scored and sampled in Rust.
//!
//! Everything random derives from the caller's `seed`: the same seed over the same data gives the
//! same mix, so a screen can be rebuilt without its content changing, and a "refresh" is a new seed.
//! SQLite's `random()` cannot be seeded, so pools are drawn in the order of a seeded linear
//! congruence over `rowid` (`shuffled_order`), which needs no JSON parsing and no temp table.

use std::collections::{HashMap, HashSet, VecDeque};

use rusqlite::{types::Value, Connection, OptionalExtension};

use crate::{db, history, model::Song, Core, Result};

/// The "For you" row built on these draws.
pub mod board;

const DAY_MS: i64 = 86_400_000;

/// splitmix64: tiny, no state to warm up, good enough to shuffle music.
pub(crate) struct Rng(u64);

impl Rng {
    pub(crate) fn new(seed: u64) -> Self {
        Rng(seed)
    }

    pub(crate) fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// In (0, 1], so its logarithm is finite.
    fn unit(&mut self) -> f64 {
        ((self.next() >> 11) + 1) as f64 / (1u64 << 53) as f64
    }

    fn below(&mut self, n: usize) -> usize {
        (self.next() % n.max(1) as u64) as usize
    }
}

const LCG_P: i64 = 2_147_483_647;

/// An `ORDER BY` term that is a seeded permutation of `rowid` (a bijection modulo a prime), and its two parameters.
/// `a` and `b` are the 1-based numbers of those parameters in the statement.
pub(crate) fn shuffled_order(rowid: &str, a: usize, b: usize) -> String {
    format!("(({rowid} % {LCG_P}) * ?{a} + ?{b}) % {LCG_P}")
}

pub(crate) fn shuffled_params(seed: u64) -> [Value; 2] {
    let mut r = Rng::new(seed);
    [Value::Integer(1 + (r.next() % (LCG_P as u64 - 1)) as i64), Value::Integer((r.next() % LCG_P as u64) as i64)]
}

/// This server's songs. Most rows of `items` are songs: saying so keeps the planner from treating
/// `kind=2` as the selective term and ignoring the expression indexes (there is no ANALYZE data on a phone).
pub(crate) const SONGS: &str = "i.server=sid() AND likelihood(i.kind=2, 0.9)";

struct Cand {
    song: Song,
    plays: u32,
    skips: u32,
    taste: f64,
}

enum Order {
    Shuffled(u64),
    Taste,
    Plays,
}

/// Up to `n` songs matching `cond` (which numbers its own parameters from ?1), never an excluded one.
/// `played_only` drives the query from `song_stats`, which is small, instead of from the index.
fn pool(c: &Connection, played_only: bool, cond: &str, mut args: Vec<Value>, order: Order, n: usize, now_ms: i64) -> rusqlite::Result<Vec<Cand>> {
    let from = if played_only { "song_stats s JOIN items i ON s.server=sid() AND i.server=sid() AND i.kind=2 AND i.id=s.song_id" } else { "items i LEFT JOIN song_stats s ON s.server=i.server AND s.song_id=i.id" };
    let order = match order {
        Order::Shuffled(seed) => {
            args.extend(shuffled_params(seed));
            shuffled_order("i.rowid", args.len() - 1, args.len())
        }
        Order::Taste => "s.taste DESC, i.rowid".to_string(),
        Order::Plays => "s.plays DESC, s.heard_ms_total DESC, i.rowid".to_string(),
    };
    args.push(Value::Integer(n as i64));
    let sql = format!(
        "SELECT i.json, coalesce(s.plays,0), coalesce(s.skips,0), coalesce(s.taste,0) FROM {from}
         WHERE {SONGS} AND {cond} AND NOT EXISTS(SELECT 1 FROM mix_excluded e WHERE e.server=i.server AND e.song_id=i.id) ORDER BY {order} LIMIT ?{}",
        args.len()
    );
    let mut st = c.prepare_cached(&sql)?;
    let mut rows = st.query(rusqlite::params_from_iter(args))?;
    let mut out = Vec::new();
    while let Some(r) = rows.next()? {
        let Ok(song) = serde_json::from_str::<Song>(&r.get::<_, String>(0)?) else { continue };
        let taste = history::taste(&song, r.get(3)?, now_ms);
        out.push(Cand { song, plays: r.get(1)?, skips: r.get(2)?, taste });
    }
    Ok(out)
}

fn artist_key(s: &Song) -> Option<String> {
    match s.artist_id.as_deref() {
        Some(id) if !id.is_empty() => Some(id.to_string()),
        _ => (!s.artist.is_empty()).then(|| s.artist.to_lowercase()),
    }
}

fn album_key(s: &Song) -> Option<String> {
    match s.album_id.as_deref() {
        Some(id) if !id.is_empty() => Some(id.to_string()),
        _ => (!s.album.is_empty()).then(|| format!("{}\0{}", s.artist, s.album).to_lowercase()),
    }
}

/// Weighted sampling without replacement (Efraimidis-Spirakis: the largest `ln(u)/w` win), at most
/// `per_artist` songs of one artist so that one discography cannot fill a mix.
fn sample(cands: Vec<(Song, f64)>, limit: usize, per_artist: usize, rng: &mut Rng) -> Vec<Song> {
    let mut seen = HashSet::new();
    let mut keyed: Vec<(f64, Song)> = cands.into_iter().filter(|(s, _)| seen.insert(s.id.clone())).map(|(s, w)| (rng.unit().ln() / w.max(1e-6), s)).collect();
    keyed.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut by_artist: HashMap<String, usize> = HashMap::new();
    let mut out = Vec::with_capacity(limit.min(keyed.len()));
    for (_, s) in keyed {
        if out.len() == limit {
            break;
        }
        if let Some(k) = artist_key(&s) {
            let n = by_artist.entry(k).or_default();
            if *n >= per_artist {
                continue;
            }
            *n += 1;
        }
        out.push(s);
    }
    out
}

/// A seeded shuffle that keeps songs of one artist apart, and when it can also songs of one album and an
/// artist heard two songs ago. `after` is what plays right before the result.
///
/// Two songs of an artist end up adjacent only when that cannot be avoided, that is when the artist has
/// more than half of what is left: the artist with the most songs left is forced as soon as postponing
/// it would make a collision certain; every other position takes the next song of the plain shuffle that fits.
pub(crate) fn spread(mut songs: Vec<Song>, rng: &mut Rng, after: Option<&Song>) -> Vec<Song> {
    let n = songs.len();
    for i in (1..n).rev() {
        songs.swap(i, rng.below(i + 1));
    }
    let (mut artists, mut albums): (HashMap<String, usize>, HashMap<String, usize>) = (HashMap::new(), HashMap::new());
    let number = |map: &mut HashMap<String, usize>, key: Option<String>, i: usize| {
        // No name: not the same as anything.
        let key = key.unwrap_or_else(|| format!("\0{i}"));
        let next = map.len();
        *map.entry(key).or_insert(next)
    };
    let artist_of: Vec<usize> = songs.iter().enumerate().map(|(i, s)| number(&mut artists, artist_key(s), i)).collect();
    let album_of: Vec<usize> = songs.iter().enumerate().map(|(i, s)| number(&mut albums, album_key(s), i)).collect();
    let mut queues: Vec<VecDeque<usize>> = vec![VecDeque::new(); artists.len()];
    for (i, &a) in artist_of.iter().enumerate() {
        queues[a].push_back(i);
    }
    let mut last_artist = after.and_then(artist_key).and_then(|k| artists.get(&k).copied());
    let mut last_album = after.and_then(album_key).and_then(|k| albums.get(&k).copied());
    let mut prev_artist = None;

    let mut slots: Vec<Option<Song>> = songs.into_iter().map(Some).collect();
    let mut out = Vec::with_capacity(n);
    let mut start = 0;
    for left in (1..=n).rev() {
        let dominant = (0..queues.len()).max_by_key(|&a| queues[a].len()).unwrap_or(0);
        let pick = if queues[dominant].len() * 2 > left && Some(dominant) != last_artist {
            queues[dominant][0]
        } else {
            let (mut fits, mut looked) = (None, 0);
            for i in start..n {
                if slots[i].is_none() {
                    continue;
                }
                let a = Some(artist_of[i]);
                if a != last_artist {
                    if a != prev_artist && Some(album_of[i]) != last_album {
                        fits = Some(i);
                        break;
                    }
                    fits = fits.or(Some(i));
                }
                looked += 1;
                // The nice-to-haves are not worth a quadratic scan of a long queue.
                if looked >= 24 && fits.is_some() {
                    break;
                }
            }
            fits.unwrap_or(start)
        };
        let a = artist_of[pick];
        if let Some(p) = queues[a].iter().position(|&i| i == pick) {
            queues[a].remove(p);
        }
        out.extend(slots[pick].take());
        (prev_artist, last_artist, last_album) = (last_artist, Some(a), Some(album_of[pick]));
        while start < n && slots[start].is_none() {
            start += 1;
        }
    }
    out
}

fn text(s: &str) -> Value {
    Value::Text(s.to_string())
}

/// Songs the user likes and has not heard in the last three days. Empty until there is some history, a star or a rating.
fn quick_picks(c: &Connection, limit: usize, seed: u64, now_ms: i64) -> rusqlite::Result<Vec<Song>> {
    let mut rng = Rng::new(seed);
    let rested = || vec![Value::Integer(now_ms - 3 * DAY_MS)];
    let mut cands = pool(c, true, "s.last_played_ms<?1 AND s.taste>0", rested(), Order::Taste, (limit * 4).max(100), now_ms)?;
    // Two statements because each can only use its own partial index.
    cands.extend(pool(c, false, "json_extract(i.json,'$.starred')=1 AND coalesce(s.last_played_ms,0)<?1", rested(), Order::Shuffled(rng.next()), limit * 2, now_ms)?);
    cands.extend(pool(c, false, "json_extract(i.json,'$.userRating')>=4 AND coalesce(s.last_played_ms,0)<?1", rested(), Order::Shuffled(rng.next()), limit * 2, now_ms)?);
    let picked = sample(cands.into_iter().filter(|c| c.taste > 0.0).map(|c| (c.song, c.taste)).collect(), limit, (limit / 5).max(2), &mut rng);
    Ok(spread(picked, &mut rng, None))
}

/// Recent favourites: played in the last 45 days, the better liked the more likely.
fn listen_again(c: &Connection, limit: usize, seed: u64, now_ms: i64) -> rusqlite::Result<Vec<Song>> {
    let mut rng = Rng::new(seed);
    let cands = pool(c, true, "s.plays>0 AND s.last_played_ms>=?1", vec![Value::Integer(now_ms - 45 * DAY_MS)], Order::Taste, (limit * 4).max(100), now_ms)?;
    let picked = sample(cands.into_iter().filter(|c| c.taste > 0.0).map(|c| (c.song, c.taste * (1.0 + c.plays as f64).ln())).collect(), limit, (limit / 4).max(2), &mut rng);
    Ok(spread(picked, &mut rng, None))
}

/// Most played, in order; the one mix that is a ranking and not a draw.
fn top(c: &Connection, limit: usize, now_ms: i64) -> rusqlite::Result<Vec<Song>> {
    Ok(pool(c, true, "s.plays>0", vec![], Order::Plays, limit, now_ms)?.into_iter().map(|c| c.song).collect())
}

/// Songs never played or played once and never skipped, leaning towards the artists and genres the
/// user plays. With no history it is a random walk through the library.
fn discover(c: &Connection, limit: usize, seed: u64, now_ms: i64) -> rusqlite::Result<Vec<Song>> {
    let mut rng = Rng::new(seed);
    let mut liked = pool(c, true, "1", vec![], Order::Taste, 500, now_ms)?;
    liked.extend(pool(c, false, "json_extract(i.json,'$.starred')=1", vec![], Order::Shuffled(rng.next()), 200, now_ms)?);
    let (mut artists, mut genres): (HashMap<String, f64>, HashMap<String, f64>) = (HashMap::new(), HashMap::new());
    let mut seen = HashSet::new();
    for l in liked.iter().filter(|l| seen.insert(l.song.id.clone())) {
        if let Some(id) = l.song.artist_id.as_deref().filter(|id| !id.is_empty()) {
            *artists.entry(id.to_string()).or_default() += l.taste;
        }
        if let Some(g) = l.song.genre.as_deref().filter(|g| !g.is_empty()) {
            *genres.entry(g.to_lowercase()).or_default() += l.taste;
        }
    }
    let best = |m: &HashMap<String, f64>, n: usize| {
        let mut l: Vec<(String, f64)> = m.iter().filter(|(_, &v)| v > 0.0).map(|(k, &v)| (k.clone(), v)).collect();
        l.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        l.truncate(n);
        l
    };
    let (top_artists, top_genres) = (best(&artists, 8), best(&genres, 4));

    const FRESH: &str = "coalesce(s.plays,0)<=1 AND coalesce(s.skips,0)=0";
    let mut cands = Vec::new();
    for (id, _) in &top_artists {
        cands.extend(pool(c, false, &format!("json_extract(i.json,'$.artistId')=?1 AND {FRESH}"), vec![text(id)], Order::Shuffled(rng.next()), limit, now_ms)?);
    }
    for (g, _) in &top_genres {
        cands.extend(pool(c, false, &format!("json_extract(i.json,'$.genre')=?1 COLLATE NOCASE AND {FRESH}"), vec![text(g)], Order::Shuffled(rng.next()), limit * 2, now_ms)?);
    }
    cands.extend(pool(c, false, FRESH, vec![], Order::Shuffled(rng.next()), limit * 2, now_ms)?);

    // Affinity relative to the favourite, so the weights mean the same for a light and a heavy listener.
    let norm = |m: &HashMap<String, f64>, l: &[(String, f64)], k: Option<String>| -> f64 {
        let max = l.first().map(|x| x.1).unwrap_or(1.0);
        k.and_then(|k| m.get(&k).copied()).map(|v| (v / max).clamp(-1.0, 1.0)).unwrap_or(0.0)
    };
    let weighted = cands
        .into_iter()
        .filter(|c| c.taste >= 0.0)
        .map(|c| {
            let a = norm(&artists, &top_artists, c.song.artist_id.clone());
            let g = norm(&genres, &top_genres, c.song.genre.as_ref().map(|g| g.to_lowercase()));
            let w = 0.3 + 3.0 * a.max(0.0) + 2.0 * g.max(0.0);
            (c.song, if a < 0.0 || g < 0.0 { w * 0.2 } else { w })
        })
        .collect();
    let picked = sample(weighted, limit, (limit / 5).max(2), &mut rng);
    Ok(spread(picked, &mut rng, None))
}

/// A draw from `cond`, liked songs more likely and skipped ones less. Shared by the genre, artist and decade mixes.
fn themed(c: &Connection, cond: &str, args: Vec<Value>, limit: usize, per_artist: usize, seed: u64, now_ms: i64) -> rusqlite::Result<Vec<Song>> {
    let mut rng = Rng::new(seed);
    let cands = pool(c, false, cond, args, Order::Shuffled(rng.next()), limit * 3, now_ms)?;
    let picked = sample(cands.into_iter().map(|c| (c.song, affinity(c.taste, c.skips))).collect(), limit, per_artist, &mut rng);
    Ok(spread(picked, &mut rng, None))
}

fn affinity(taste: f64, skips: u32) -> f64 {
    if taste < 0.0 {
        0.2 / (1.0 + skips as f64)
    } else {
        1.0 + taste.min(4.0)
    }
}

fn decade_cond() -> &'static str {
    "json_extract(i.json,'$.year') BETWEEN ?1 AND ?2"
}

/// The seed song, then its neighbourhood: same genre first, then same artist and same decade.
fn instant(c: &Connection, seed_song_id: &str, limit: usize, seed: u64, now_ms: i64) -> rusqlite::Result<Vec<Song>> {
    let first: Option<Song> = c.prepare_cached("SELECT json FROM items WHERE server=sid() AND kind=2 AND id=?1")?.query_row([seed_song_id], |r| r.get::<_, String>(0)).optional()?.and_then(|j| serde_json::from_str(&j).ok());
    let Some(first) = first else { return Ok(Vec::new()) };
    if limit == 0 {
        return Ok(Vec::new());
    }
    let mut rng = Rng::new(seed);
    let decade = (first.year / 10 * 10) as i64;
    let mut cands = Vec::new();
    if let Some(g) = first.genre.as_deref().filter(|g| !g.is_empty()) {
        cands.extend(pool(c, false, "json_extract(i.json,'$.genre')=?1 COLLATE NOCASE", vec![text(g)], Order::Shuffled(rng.next()), limit * 3, now_ms)?);
    }
    if let Some(a) = first.artist_id.as_deref().filter(|a| !a.is_empty()) {
        cands.extend(pool(c, false, "json_extract(i.json,'$.artistId')=?1", vec![text(a)], Order::Shuffled(rng.next()), limit, now_ms)?);
    }
    if first.year > 0 {
        cands.extend(pool(c, false, decade_cond(), vec![Value::Integer(decade), Value::Integer(decade + 9)], Order::Shuffled(rng.next()), limit * 2, now_ms)?);
    }
    let genre = first.genre.as_ref().map(|g| g.to_lowercase());
    let weighted = cands
        .into_iter()
        .filter(|c| c.song.id != first.id && c.taste > -0.5)
        .map(|c| {
            let s = &c.song;
            let mut w = 0.2;
            if genre.is_some() && s.genre.as_ref().map(|g| g.to_lowercase()) == genre {
                w += 3.0;
            }
            if first.artist_id.is_some() && s.artist_id == first.artist_id {
                w += 2.0;
            }
            if first.year > 0 && s.year as i64 / 10 * 10 == decade {
                w += 1.0;
            }
            (c.song, w * affinity(c.taste, c.skips))
        })
        .collect();
    let picked = sample(weighted, limit - 1, (limit / 4).max(3), &mut rng);
    let mut out = Vec::with_capacity(picked.len() + 1);
    let rest = spread(picked, &mut rng, Some(&first));
    out.push(first);
    out.extend(rest);
    Ok(out)
}

/// All mixes are read-only and take a few milliseconds; call them off the main thread like every other
/// core call. `seed` picks the draw: keep it to get the same mix again, change it to refresh.
#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    pub fn mix_quick_picks(&self, limit: u32, seed: u64) -> Result<Vec<Song>> {
        Ok(quick_picks(&self.db.lock(), limit as usize, seed, db::now_ms())?)
    }

    pub fn mix_discover(&self, limit: u32, seed: u64) -> Result<Vec<Song>> {
        Ok(discover(&self.db.lock(), limit as usize, seed, db::now_ms())?)
    }

    pub fn mix_listen_again(&self, limit: u32, seed: u64) -> Result<Vec<Song>> {
        Ok(listen_again(&self.db.lock(), limit as usize, seed, db::now_ms())?)
    }

    pub fn mix_top(&self, limit: u32) -> Result<Vec<Song>> {
        Ok(top(&self.db.lock(), limit as usize, db::now_ms())?)
    }

    /// `genre` is matched without regard to (ASCII) case.
    pub fn mix_genre(&self, genre: String, limit: u32, seed: u64) -> Result<Vec<Song>> {
        let limit = limit as usize;
        Ok(themed(&self.db.lock(), "json_extract(i.json,'$.genre')=?1 COLLATE NOCASE", vec![Value::Text(genre)], limit, (limit / 5).max(2), seed, db::now_ms())?)
    }

    pub fn mix_artist(&self, artist_id: String, limit: u32, seed: u64) -> Result<Vec<Song>> {
        Ok(themed(&self.db.lock(), "json_extract(i.json,'$.artistId')=?1", vec![Value::Text(artist_id)], limit as usize, usize::MAX, seed, db::now_ms())?)
    }

    /// `decade_start_year` 1990 means 1990..=1999.
    pub fn mix_decade(&self, decade_start_year: u32, limit: u32, seed: u64) -> Result<Vec<Song>> {
        let (limit, from) = (limit as usize, decade_start_year as i64);
        Ok(themed(&self.db.lock(), decade_cond(), vec![Value::Integer(from), Value::Integer(from + 9)], limit, (limit / 5).max(2), seed, db::now_ms())?)
    }

    /// Empty when the seed song is not in the index (a provider track).
    pub fn mix_instant(&self, seed_song_id: String, limit: u32, seed: u64) -> Result<Vec<Song>> {
        Ok(instant(&self.db.lock(), &seed_song_id, limit as usize, seed, db::now_ms())?)
    }

    /// An excluded song never appears in a mix; it still plays, searches and counts like any other.
    pub fn mix_excluded_set(&self, song_id: String, excluded: bool) -> Result<()> {
        let c = self.db.lock();
        if excluded {
            c.execute("INSERT OR IGNORE INTO mix_excluded(server, song_id) VALUES(sid(), ?1)", [song_id])?;
        } else {
            c.execute("DELETE FROM mix_excluded WHERE server=sid() AND song_id=?1", [song_id])?;
        }
        Ok(())
    }

    pub fn mix_excluded_clear(&self) -> Result<()> {
        self.db.lock().execute("DELETE FROM mix_excluded WHERE server=sid()", [])?;
        Ok(())
    }

    /// For the settings screen that lets the user take an exclusion back.
    pub fn mix_excluded_list(&self) -> Result<Vec<Song>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT i.json FROM mix_excluded e JOIN items i ON i.server=sid() AND i.kind=2 AND i.id=e.song_id WHERE e.server=sid() ORDER BY i.rowid")?;
        let rows = st.query_map([], |r| r.get::<_, String>(0))?;
        Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.iter().filter_map(|j| serde_json::from_str(j).ok()).collect())
    }
}

/// Shuffle for a play queue: seeded, and songs of one artist (and, when possible, of one album) are kept apart.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn weighted_shuffle(songs: Vec<Song>, seed: u64) -> Vec<Song> {
    spread(songs, &mut Rng::new(seed), None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::{listen, skip, song, DAY, NOW};

    fn ids(l: &[Song]) -> Vec<&str> {
        l.iter().map(|s| s.id.as_str()).collect()
    }

    fn adjacent_artists(l: &[Song]) -> usize {
        l.windows(2).filter(|w| w[0].artist == w[1].artist).count()
    }

    /// 4 genres x 5 artists x 10 songs, years spread over four decades.
    fn library(core: &Core) -> Vec<Song> {
        let genres = ["Rock", "Jazz", "Electronic", "Folk"];
        let mut all = Vec::new();
        for (g, genre) in genres.iter().enumerate() {
            for a in 0..5 {
                for t in 0..10 {
                    let artist = format!("{genre} Artist {a}");
                    all.push(song(&format!("{g}-{a}-{t}"), &format!("Track {t}"), &artist, &format!("{artist} LP{}", t / 5), genre, 1970 + (g as u32) * 10 + t as u32));
                }
            }
        }
        db::index(&mut core.db.lock(), &[], &[], &all).unwrap();
        all
    }

    #[test]
    fn rng_is_deterministic_and_uniformish() {
        let (mut a, mut b) = (Rng::new(7), Rng::new(7));
        assert_eq!((0..5).map(|_| a.next()).collect::<Vec<_>>(), (0..5).map(|_| b.next()).collect::<Vec<_>>());
        assert_ne!(Rng::new(1).next(), Rng::new(2).next());
        let mut r = Rng::new(0);
        let mean = (0..10_000).map(|_| r.unit()).sum::<f64>() / 10_000.0;
        assert!((mean - 0.5).abs() < 0.02, "{mean}");
        assert!((0..1000).all(|_| r.below(3) < 3));
        assert_eq!(r.below(0), 0);
    }

    #[test]
    fn shuffle_keeps_artists_apart_when_it_can() {
        let mut l = Vec::new();
        for (artist, n) in [("A", 10), ("B", 6), ("C", 3), ("D", 1)] {
            for i in 0..n {
                l.push(song(&format!("{artist}{i}"), "t", artist, &format!("{artist}-album-{}", i % 2), "", 0));
            }
        }
        for seed in 0..50 {
            let out = weighted_shuffle(l.clone(), seed);
            assert_eq!(out.len(), l.len());
            let mut sorted = ids(&out);
            sorted.sort();
            sorted.dedup();
            assert_eq!(sorted.len(), l.len(), "a permutation");
            assert_eq!(adjacent_artists(&out), 0, "seed {seed}: {:?}", ids(&out));
        }
        assert_eq!(ids(&weighted_shuffle(l.clone(), 3)), ids(&weighted_shuffle(l.clone(), 3)));
        assert_ne!(ids(&weighted_shuffle(l.clone(), 3)), ids(&weighted_shuffle(l, 4)));
    }

    #[test]
    fn shuffle_at_the_limit_of_what_is_possible() {
        // exactly half plus one: only A_A_A_A works
        let mut l: Vec<Song> = (0..4).map(|i| song(&format!("a{i}"), "t", "A", "", "", 0)).collect();
        l.extend((0..3).map(|i| song(&format!("b{i}"), "t", ["B", "C", "B"][i], "", "", 0)));
        for seed in 0..50 {
            assert_eq!(adjacent_artists(&weighted_shuffle(l.clone(), seed)), 0, "seed {seed}");
        }
        // impossible: 5 of 6 by one artist. Still a permutation, and the odd one out splits the run.
        let mut l: Vec<Song> = (0..5).map(|i| song(&format!("a{i}"), "t", "A", "", "", 0)).collect();
        l.push(song("b", "t", "B", "", "", 0));
        for seed in 0..20 {
            let out = weighted_shuffle(l.clone(), seed);
            assert_eq!(out.len(), 6);
            assert!(out[0].artist == "A" && out[5].artist == "A", "{:?}", ids(&out));
        }
    }

    #[test]
    fn shuffle_edge_cases() {
        assert!(weighted_shuffle(vec![], 1).is_empty());
        let one = vec![song("x", "t", "A", "", "", 0)];
        assert_eq!(weighted_shuffle(one.clone(), 1), one);
        // no artist at all: nothing is "the same artist"
        let anon: Vec<Song> = (0..5).map(|i| Song { id: i.to_string(), ..Default::default() }).collect();
        assert_eq!(weighted_shuffle(anon, 9).len(), 5);
        // artists that differ only by id stay distinct, same name without id is one artist
        let mut l: Vec<Song> = (0..3).map(|i| Song { id: format!("n{i}"), artist: "Björk".into(), ..Default::default() }).collect();
        l.extend((0..3).map(|i| Song { id: format!("m{i}"), artist: "BJÖRK".into(), ..Default::default() }));
        l.extend((0..6).map(|i| Song { id: format!("o{i}"), artist: "Other".into(), ..Default::default() }));
        let out = weighted_shuffle(l, 5);
        assert_eq!(out.windows(2).filter(|w| w[0].artist.to_lowercase() == w[1].artist.to_lowercase()).count(), 0);
    }

    #[test]
    fn shuffle_avoids_the_same_album_too() {
        // two artists cannot avoid alternating; albums within can
        let mut l = Vec::new();
        for artist in ["A", "B", "C"] {
            for i in 0..6 {
                l.push(song(&format!("{artist}{i}"), "t", artist, &format!("{artist}{}", i % 3), "", 0));
            }
        }
        let out = weighted_shuffle(l, 11);
        assert_eq!(adjacent_artists(&out), 0);
        assert_eq!(out.windows(2).filter(|w| w[0].album == w[1].album).count(), 0);
    }

    #[test]
    fn a_long_queue_shuffles_quickly() {
        let l: Vec<Song> = (0..20_000).map(|i| song(&i.to_string(), "t", &format!("artist {}", i % 1500), "", "", 0)).collect();
        let out = weighted_shuffle(l, 1);
        assert_eq!((out.len(), adjacent_artists(&out)), (20_000, 0));
    }

    #[test]
    fn every_mix_is_empty_on_an_empty_index() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        assert!(core.mix_quick_picks(20, 1).unwrap().is_empty());
        assert!(core.mix_discover(20, 1).unwrap().is_empty());
        assert!(core.mix_listen_again(20, 1).unwrap().is_empty());
        assert!(core.mix_top(20).unwrap().is_empty());
        assert!(core.mix_genre("Rock".into(), 20, 1).unwrap().is_empty());
        assert!(core.mix_artist("ar1".into(), 20, 1).unwrap().is_empty());
        assert!(core.mix_decade(1990, 20, 1).unwrap().is_empty());
        assert!(core.mix_instant("nope".into(), 20, 1).unwrap().is_empty());
        assert!(core.mix_excluded_list().unwrap().is_empty());
    }

    #[test]
    fn quick_picks_are_liked_and_rested() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let all = library(&core);
        let (loved, today, hated) = (&all[0], &all[1], &all[2]);
        for d in 4..8 {
            listen(&core, loved, NOW - d * DAY);
        }
        listen(&core, today, NOW - DAY);
        skip(&core, hated, NOW - 5 * DAY);
        let mut starred = all[60].clone();
        starred.starred = true;
        let mut rated = all[61].clone();
        rated.user_rating = 5;
        db::index(&mut core.db.lock(), &[], &[], &[starred, rated]).unwrap();

        let c = core.db.lock();
        let picks = quick_picks(&c, 10, 1, NOW).unwrap();
        let mut got = ids(&picks);
        got.sort();
        assert_eq!(got, vec![loved.id.as_str(), all[60].id.as_str(), all[61].id.as_str()]);
        assert_eq!(ids(&picks), ids(&quick_picks(&c, 10, 1, NOW).unwrap()));
        // three days later today's song has rested
        assert!(ids(&quick_picks(&c, 10, 1, NOW + 3 * DAY).unwrap()).contains(&today.id.as_str()));
        assert!(quick_picks(&c, 0, 1, NOW).unwrap().is_empty());
    }

    #[test]
    fn listen_again_and_top() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let all = library(&core);
        for (i, s) in all.iter().step_by(10).take(5).enumerate() {
            for d in 0..=i as i64 {
                listen(&core, s, NOW - d * DAY);
            }
        }
        listen(&core, &all[55], NOW - 100 * DAY);
        let c = core.db.lock();
        assert_eq!(ids(&top(&c, 3, NOW).unwrap()), vec!["0-4-0", "0-3-0", "0-2-0"]);
        assert_eq!(top(&c, 50, NOW).unwrap().len(), 6);
        let again = listen_again(&c, 10, 2, NOW).unwrap();
        assert_eq!(again.len(), 5, "the one from 100 days ago is not recent");
        assert_eq!(ids(&again), ids(&listen_again(&c, 10, 2, NOW).unwrap()));
    }

    #[test]
    fn discover_prefers_liked_artists_and_genres_and_skips_the_known() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let all = library(&core);
        // a jazz listener, mostly Jazz Artist 0
        for s in all.iter().filter(|s| s.artist == "Jazz Artist 0").take(4) {
            for d in 0..3 {
                listen(&core, s, NOW - d * DAY);
            }
        }
        skip(&core, &all[0], NOW);
        let c = core.db.lock();
        let (mut jazz, mut total) = (0, 0);
        for seed in 0..20 {
            let mix = discover(&c, 20, seed, NOW).unwrap();
            assert_eq!(mix.len(), 20);
            assert_eq!(adjacent_artists(&mix), 0);
            assert!(mix.iter().all(|s| s.id != all[0].id), "skipped songs are not a discovery");
            assert!(mix.iter().all(|s| !(s.artist == "Jazz Artist 0" && s.title.as_str() < "Track 4")), "played songs are not a discovery");
            assert!(mix.iter().filter(|s| s.artist == "Jazz Artist 0").count() <= 4, "per-artist cap");
            jazz += mix.iter().filter(|s| s.genre.as_deref() == Some("Jazz")).count();
            total += mix.len();
        }
        assert!(jazz * 2 > total, "{jazz}/{total} jazz; a quarter of the library is");
        assert_eq!(ids(&discover(&c, 20, 5, NOW).unwrap()), ids(&discover(&c, 20, 5, NOW).unwrap()));
        assert_ne!(ids(&discover(&c, 20, 5, NOW).unwrap()), ids(&discover(&c, 20, 6, NOW).unwrap()));
    }

    #[test]
    fn discover_without_history_is_a_random_walk() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        library(&core);
        let mix = core.mix_discover(30, 1).unwrap();
        assert_eq!(mix.len(), 30);
        assert!(mix.iter().map(|s| s.genre.clone()).collect::<HashSet<_>>().len() > 1);
    }

    #[test]
    fn genre_artist_and_decade_mixes() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        library(&core);
        let rock = core.mix_genre("rock".into(), 15, 1).unwrap();
        assert_eq!(rock.len(), 15);
        assert!(rock.iter().all(|s| s.genre.as_deref() == Some("Rock")));
        assert_eq!(ids(&rock), ids(&core.mix_genre("rock".into(), 15, 1).unwrap()));
        assert_ne!(ids(&rock), ids(&core.mix_genre("rock".into(), 15, 2).unwrap()));
        assert!(core.mix_genre("Polka".into(), 15, 1).unwrap().is_empty());

        let artist = core.mix_artist("ar-jazz artist 3".into(), 50, 1).unwrap();
        assert_eq!(artist.len(), 10, "the whole artist, no per-artist cap");
        assert!(artist.iter().all(|s| s.artist == "Jazz Artist 3"));

        let eighties = core.mix_decade(1980, 50, 1).unwrap();
        assert!(!eighties.is_empty() && eighties.iter().all(|s| (1980..1990).contains(&s.year)));
        assert!(core.mix_decade(1900, 50, 1).unwrap().is_empty());
    }

    #[test]
    fn instant_mix_starts_with_its_seed_and_stays_close() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let all = library(&core);
        let seed_song = all.iter().find(|s| s.id == "2-1-3").unwrap();
        let mix = core.mix_instant(seed_song.id.clone(), 25, 1).unwrap();
        assert_eq!(mix.len(), 25);
        assert_eq!(&mix[0], seed_song);
        assert_eq!(mix.iter().filter(|s| s.id == seed_song.id).count(), 1);
        assert_ne!(mix[1].artist, seed_song.artist, "spread continues from the seed");
        let near = mix.iter().filter(|s| s.genre == seed_song.genre || s.year / 10 == seed_song.year / 10).count();
        assert_eq!(near, 25);
        assert!(mix.iter().filter(|s| s.genre == seed_song.genre).count() > 12);
        assert_eq!(ids(&mix), ids(&core.mix_instant(seed_song.id.clone(), 25, 1).unwrap()));
        assert_eq!(core.mix_instant(seed_song.id.clone(), 1, 1).unwrap(), vec![seed_song.clone()]);
        assert!(core.mix_instant(seed_song.id.clone(), 0, 1).unwrap().is_empty());

        // a song with nothing to go on is a mix of one
        let bare = Song { id: "bare".into(), title: "Ünïcödé".into(), ..Default::default() };
        db::index(&mut core.db.lock(), &[], &[], std::slice::from_ref(&bare)).unwrap();
        assert_eq!(core.mix_instant("bare".into(), 10, 1).unwrap(), vec![bare]);
    }

    #[test]
    fn excluded_songs_stay_out_of_every_mix() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let all = library(&core);
        let out: Vec<&Song> = all.iter().filter(|s| s.artist == "Rock Artist 0").collect();
        for s in &out {
            listen(&core, s, NOW - 10 * DAY);
            core.mix_excluded_set(s.id.clone(), true).unwrap();
        }
        core.mix_excluded_set(out[0].id.clone(), true).unwrap();
        assert_eq!(core.mix_excluded_list().unwrap().len(), 10);
        let c = core.db.lock();
        assert!(quick_picks(&c, 50, 1, NOW).unwrap().is_empty());
        assert!(listen_again(&c, 50, 1, NOW).unwrap().is_empty());
        assert!(top(&c, 50, NOW).unwrap().is_empty());
        drop(c);
        assert_eq!(core.mix_genre("Rock".into(), 100, 1).unwrap().len(), 40);
        assert!(core.mix_artist("ar-rock artist 0".into(), 100, 1).unwrap().is_empty());

        core.mix_excluded_set(out[0].id.clone(), false).unwrap();
        assert_eq!(ids(&core.mix_artist("ar-rock artist 0".into(), 100, 1).unwrap()), vec![out[0].id.as_str()]);
        core.mix_excluded_clear().unwrap();
        assert_eq!(core.mix_artist("ar-rock artist 0".into(), 100, 1).unwrap().len(), 10);
    }

    #[test]
    fn pools_use_the_expression_indexes() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let c = core.db.lock();
        let plan = |cond: &str| -> String {
            let sql = format!("EXPLAIN QUERY PLAN SELECT i.json FROM items i LEFT JOIN song_stats s ON s.server=i.server AND s.song_id=i.id WHERE {SONGS} AND {cond}");
            let mut st = c.prepare(&sql).unwrap();
            let rows = st.query_map([], |r| r.get::<_, String>(3)).unwrap();
            rows.map(|r| r.unwrap()).collect::<Vec<_>>().join("\n")
        };
        assert!(plan("json_extract(i.json,'$.genre')='x' COLLATE NOCASE").contains("items_genre"));
        assert!(plan("json_extract(i.json,'$.artistId')='x'").contains("items_artist"));
        assert!(plan("json_extract(i.json,'$.year') BETWEEN 1 AND 2").contains("items_year"));
        assert!(plan("json_extract(i.json,'$.starred')=1").contains("items_starred"));
        assert!(plan("json_extract(i.json,'$.userRating')>=4").contains("items_rated"));
    }
}
