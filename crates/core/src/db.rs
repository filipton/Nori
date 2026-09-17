//! Local index: every library item the app has seen, searchable offline with
//! FTS5, plus the response cache and the small persistent queues. Items are
//! written here straight from the response bytes, so a library sync never
//! materialises objects on the Kotlin side.

use rusqlite::{params, Connection, OptionalExtension};
use serde::{de::DeserializeOwned, Serialize};

use crate::model::*;

pub const ARTIST: i64 = 0;
pub const ALBUM: i64 = 1;
pub const SONG: i64 = 2;

const SCHEMA: &str = "
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA temp_store=MEMORY;
CREATE TABLE IF NOT EXISTS items(rowid INTEGER PRIMARY KEY, kind INTEGER NOT NULL, id TEXT NOT NULL, json TEXT NOT NULL, UNIQUE(kind, id));
CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(text, tokenize='unicode61 remove_diacritics 2', prefix='2 3');
CREATE TABLE IF NOT EXISTS cache(key TEXT PRIMARY KEY, body BLOB NOT NULL, ts INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS kv(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS pending(rowid INTEGER PRIMARY KEY, endpoint TEXT NOT NULL, params TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS downloads(id TEXT PRIMARY KEY, json TEXT NOT NULL, ts INTEGER NOT NULL, done INTEGER NOT NULL DEFAULT 0) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS autoeq(rowid INTEGER PRIMARY KEY, name TEXT NOT NULL, source TEXT NOT NULL, form TEXT NOT NULL, target TEXT NOT NULL, path TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS profiles(name TEXT PRIMARY KEY, json TEXT NOT NULL, outputs TEXT NOT NULL DEFAULT '') WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS searches(query TEXT PRIMARY KEY, ts INTEGER NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS plays(rowid INTEGER PRIMARY KEY, song_id TEXT NOT NULL, started_ms INTEGER NOT NULL, heard_ms INTEGER NOT NULL, duration_ms INTEGER NOT NULL, completed INTEGER NOT NULL, skipped INTEGER NOT NULL, hour INTEGER NOT NULL, day INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS plays_started ON plays(started_ms);
CREATE TABLE IF NOT EXISTS song_stats(song_id TEXT PRIMARY KEY, plays INTEGER NOT NULL DEFAULT 0, skips INTEGER NOT NULL DEFAULT 0, last_played_ms INTEGER NOT NULL DEFAULT 0, heard_ms_total INTEGER NOT NULL DEFAULT 0, taste REAL NOT NULL DEFAULT 0) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS mix_excluded(song_id TEXT PRIMARY KEY) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS smart_playlists(id TEXT PRIMARY KEY, name TEXT NOT NULL, json TEXT NOT NULL, updated_ms INTEGER NOT NULL) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS items_genre ON items(json_extract(json,'$.genre') COLLATE NOCASE) WHERE kind=2;
CREATE INDEX IF NOT EXISTS items_artist ON items(json_extract(json,'$.artistId')) WHERE kind=2;
CREATE INDEX IF NOT EXISTS items_year ON items(json_extract(json,'$.year')) WHERE kind=2;
CREATE INDEX IF NOT EXISTS items_starred ON items(json_extract(json,'$.starred')) WHERE kind=2 AND json_extract(json,'$.starred')=1;
CREATE INDEX IF NOT EXISTS items_rated ON items(json_extract(json,'$.userRating')) WHERE kind=2 AND json_extract(json,'$.userRating')>=4;
";

pub fn open(path: &str) -> rusqlite::Result<Connection> {
    let c = if path.is_empty() { Connection::open_in_memory()? } else { Connection::open(path)? };
    c.execute_batch(SCHEMA)?;
    Ok(c)
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as i64).unwrap_or(0)
}

/// Provider items from octo-fiesta are not library rows: they change id once
/// downloaded, so they are never indexed.
pub fn external(id: &str) -> bool {
    id.starts_with("ext-") || id.starts_with("pl-")
}

fn upsert<T: Serialize>(c: &Connection, kind: i64, id: &str, text: &str, item: &T) -> rusqlite::Result<bool> {
    if id.is_empty() || external(id) {
        return Ok(false);
    }
    let json = serde_json::to_string(item).unwrap_or_default();
    let old: Option<(i64, String)> = c
        .prepare_cached("SELECT rowid, json FROM items WHERE kind=?1 AND id=?2")?
        .query_row(params![kind, id], |r| Ok((r.get(0)?, r.get(1)?)))
        .optional()?;
    match old {
        Some((_, j)) if j == json => Ok(false),
        Some((rowid, _)) => {
            c.prepare_cached("UPDATE items SET json=?1 WHERE rowid=?2")?.execute(params![json, rowid])?;
            c.prepare_cached("INSERT OR REPLACE INTO fts(rowid, text) VALUES(?1, ?2)")?.execute(params![rowid, text])?;
            Ok(true)
        }
        None => {
            c.prepare_cached("INSERT INTO items(kind, id, json) VALUES(?1, ?2, ?3)")?.execute(params![kind, id, json])?;
            let rowid = c.last_insert_rowid();
            c.prepare_cached("INSERT INTO fts(rowid, text) VALUES(?1, ?2)")?.execute(params![rowid, text])?;
            Ok(true)
        }
    }
}

pub fn index(c: &mut Connection, artists: &[Artist], albums: &[Album], songs: &[Song]) -> rusqlite::Result<IngestStats> {
    let tx = c.transaction()?;
    let mut st = IngestStats::default();
    for a in artists {
        st.artists += upsert(&tx, ARTIST, &a.id, &a.name, a)? as u32;
    }
    for a in albums {
        if a.is_external {
            continue;
        }
        st.albums += upsert(&tx, ALBUM, &a.id, &format!("{} {}", a.name, a.artist), a)? as u32;
    }
    for s in songs {
        if s.is_external {
            continue;
        }
        st.songs += upsert(&tx, SONG, &s.id, &format!("{} {} {}", s.title, s.artist, s.album), s)? as u32;
    }
    tx.commit()?;
    Ok(st)
}

/// Every token is a prefix match and all must match: "pin flo" finds Pink Floyd.
fn fts_query(q: &str) -> Option<String> {
    let toks: Vec<String> = q
        .split(|c: char| !c.is_alphanumeric())
        .filter(|t| !t.is_empty())
        .map(|t| format!("\"{t}\"*"))
        .collect();
    (!toks.is_empty()).then(|| toks.join(" "))
}

fn find<T: DeserializeOwned>(c: &Connection, kind: i64, q: &str, limit: u32) -> rusqlite::Result<Vec<T>> {
    let mut st = c.prepare_cached(
        "SELECT i.json FROM fts JOIN items i ON i.rowid = fts.rowid WHERE fts MATCH ?1 AND i.kind = ?2 ORDER BY rank LIMIT ?3",
    )?;
    let rows = st.query_map(params![q, kind, limit], |r| r.get::<_, String>(0))?;
    Ok(rows.filter_map(|j| serde_json::from_str(&j.ok()?).ok()).collect())
}

pub fn search(c: &Connection, query: &str, limit: u32) -> rusqlite::Result<SearchResult> {
    let Some(q) = fts_query(query) else { return Ok(SearchResult::default()) };
    Ok(SearchResult {
        artists: find(c, ARTIST, &q, limit)?,
        albums: find(c, ALBUM, &q, limit)?,
        songs: find(c, SONG, &q, limit)?,
    })
}

pub fn count(c: &Connection, kind: i64) -> rusqlite::Result<u32> {
    c.query_row("SELECT count(*) FROM items WHERE kind=?1", [kind], |r| r.get(0))
}

pub fn clear_library(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("DELETE FROM items; DELETE FROM fts; DELETE FROM cache; DELETE FROM pending; DELETE FROM kv WHERE key='queue';")
}

pub fn kv_get(c: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    c.prepare_cached("SELECT value FROM kv WHERE key=?1")?.query_row([key], |r| r.get(0)).optional()
}

pub fn kv_put(c: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    c.prepare_cached("INSERT OR REPLACE INTO kv(key, value) VALUES(?1, ?2)")?.execute(params![key, value]).map(|_| ())
}
