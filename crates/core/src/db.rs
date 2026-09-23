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

/// One database for the whole app. What belongs to one server - its library, answers, pending writes,
/// downloads, history and the like - carries the server profile's id in `server`, and a connection
/// reads and writes only its own server's rows through `sid()`, the id it was opened for. The equalizer
/// profiles, the AutoEQ index and the settings are the app's, whatever the server.
const PRAGMAS: &str = "
PRAGMA journal_mode=WAL;
PRAGMA synchronous=NORMAL;
PRAGMA temp_store=MEMORY;
PRAGMA busy_timeout=5000;
";

const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS items(rowid INTEGER PRIMARY KEY, server TEXT NOT NULL, kind INTEGER NOT NULL, id TEXT NOT NULL, json TEXT NOT NULL, UNIQUE(server, kind, id));
CREATE VIRTUAL TABLE IF NOT EXISTS fts USING fts5(text, tokenize='unicode61 remove_diacritics 2', prefix='2 3');
CREATE TABLE IF NOT EXISTS cache(server TEXT NOT NULL, key TEXT NOT NULL, body BLOB NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY(server, key)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS kv(server TEXT NOT NULL, key TEXT NOT NULL, value TEXT NOT NULL, PRIMARY KEY(server, key)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS pending(rowid INTEGER PRIMARY KEY, server TEXT NOT NULL, endpoint TEXT NOT NULL, params TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS downloads(server TEXT NOT NULL, id TEXT NOT NULL, json TEXT NOT NULL, ts INTEGER NOT NULL, done INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(server, id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS autoeq(rowid INTEGER PRIMARY KEY, name TEXT NOT NULL, source TEXT NOT NULL, form TEXT NOT NULL, target TEXT NOT NULL, path TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS profiles(name TEXT PRIMARY KEY, json TEXT NOT NULL, outputs TEXT NOT NULL DEFAULT '') WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS app_kv(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS searches(server TEXT NOT NULL, query TEXT NOT NULL, ts INTEGER NOT NULL, PRIMARY KEY(server, query)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS plays(rowid INTEGER PRIMARY KEY, server TEXT NOT NULL, song_id TEXT NOT NULL, started_ms INTEGER NOT NULL, heard_ms INTEGER NOT NULL, duration_ms INTEGER NOT NULL, completed INTEGER NOT NULL, skipped INTEGER NOT NULL, hour INTEGER NOT NULL, day INTEGER NOT NULL);
CREATE INDEX IF NOT EXISTS plays_started ON plays(server, started_ms);
CREATE TABLE IF NOT EXISTS song_stats(server TEXT NOT NULL, song_id TEXT NOT NULL, plays INTEGER NOT NULL DEFAULT 0, skips INTEGER NOT NULL DEFAULT 0, last_played_ms INTEGER NOT NULL DEFAULT 0, heard_ms_total INTEGER NOT NULL DEFAULT 0, taste REAL NOT NULL DEFAULT 0, PRIMARY KEY(server, song_id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS mix_excluded(server TEXT NOT NULL, song_id TEXT NOT NULL, PRIMARY KEY(server, song_id)) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS smart_playlists(server TEXT NOT NULL, id TEXT NOT NULL, name TEXT NOT NULL, json TEXT NOT NULL, updated_ms INTEGER NOT NULL, PRIMARY KEY(server, id)) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS items_genre ON items(server, json_extract(json,'$.genre') COLLATE NOCASE) WHERE kind=2;
CREATE INDEX IF NOT EXISTS items_artist ON items(server, json_extract(json,'$.artistId')) WHERE kind=2;
CREATE INDEX IF NOT EXISTS items_year ON items(server, json_extract(json,'$.year')) WHERE kind=2;
CREATE INDEX IF NOT EXISTS items_starred ON items(server, json_extract(json,'$.starred')) WHERE kind=2 AND json_extract(json,'$.starred')=1;
CREATE INDEX IF NOT EXISTS items_rated ON items(server, json_extract(json,'$.userRating')) WHERE kind=2 AND json_extract(json,'$.userRating')>=4;
CREATE TABLE IF NOT EXISTS track_analysis(server TEXT NOT NULL, song_id TEXT NOT NULL, analysis_version INTEGER NOT NULL, duration_ms INTEGER NOT NULL, bpm REAL NOT NULL, bpm_confidence REAL NOT NULL, beat_offset_ms REAL NOT NULL, stability REAL NOT NULL, downbeat_phase INTEGER NOT NULL, downbeat_confidence REAL NOT NULL, lufs REAL NOT NULL, key INTEGER NOT NULL, key_confidence REAL NOT NULL, silence_start_ms INTEGER NOT NULL, silence_end_ms INTEGER NOT NULL, mixramp_start_ms INTEGER NOT NULL, mixramp_end_ms INTEGER NOT NULL, intro_end_ms INTEGER NOT NULL, outro_start_ms INTEGER NOT NULL, outro_vocal REAL NOT NULL, intro_vocal REAL NOT NULL, outro_centroid REAL NOT NULL, intro_centroid REAL NOT NULL, outro_bpm REAL NOT NULL, outro_bpm_confidence REAL NOT NULL, outro_beat_offset_ms REAL NOT NULL, outro_stability REAL NOT NULL, outro_downbeat_phase INTEGER NOT NULL, intro_bpm REAL NOT NULL, intro_bpm_confidence REAL NOT NULL, intro_beat_offset_ms REAL NOT NULL, intro_stability REAL NOT NULL, intro_downbeat_phase INTEGER NOT NULL, analysed_ms INTEGER NOT NULL, PRIMARY KEY(server, song_id)) WITHOUT ROWID;
";

/// The tables that belong to one server: what goes when its profile is removed.
const SERVER_TABLES: [&str; 11] =
    ["items", "cache", "kv", "pending", "downloads", "searches", "plays", "song_stats", "mix_excluded", "smart_playlists", "track_analysis"];

/// The app's database, opened for `server`'s rows.
pub fn open(path: &str, server: &str) -> rusqlite::Result<Connection> {
    let c = if path.is_empty() { Connection::open_in_memory()? } else { Connection::open(path)? };
    c.execute_batch(PRAGMAS)?;
    let sid = server.to_string();
    c.create_scalar_function("sid", 0, rusqlite::functions::FunctionFlags::SQLITE_UTF8 | rusqlite::functions::FunctionFlags::SQLITE_DETERMINISTIC, move |_| {
        Ok(sid.clone())
    })?;
    c.execute_batch(SCHEMA)?;
    Ok(c)
}

/// The app's database for what is the app's alone (the settings and the app's own values), with no server.
pub fn open_app(path: &str) -> rusqlite::Result<Connection> {
    let c = if path.is_empty() { Connection::open_in_memory()? } else { Connection::open(path)? };
    c.execute_batch(PRAGMAS)?;
    c.execute_batch(
        "CREATE TABLE IF NOT EXISTS settings(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;
         CREATE TABLE IF NOT EXISTS app_kv(key TEXT PRIMARY KEY, value TEXT NOT NULL) WITHOUT ROWID;",
    )?;
    Ok(c)
}

/// The rows of one server gone, for a server profile that was removed.
pub fn forget_server(c: &Connection, server: &str) -> rusqlite::Result<()> {
    c.execute("DELETE FROM fts WHERE rowid IN (SELECT rowid FROM items WHERE server=?1)", [server])?;
    for t in SERVER_TABLES {
        c.execute(&format!("DELETE FROM {t} WHERE server=?1"), [server])?;
    }
    Ok(())
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
        .prepare_cached("SELECT rowid, json FROM items WHERE server=sid() AND kind=?1 AND id=?2")?
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
            c.prepare_cached("INSERT INTO items(server, kind, id, json) VALUES(sid(), ?1, ?2, ?3)")?.execute(params![kind, id, json])?;
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
        "SELECT i.json FROM fts JOIN items i ON i.rowid = fts.rowid WHERE fts MATCH ?1 AND i.server = sid() AND i.kind = ?2 ORDER BY rank LIMIT ?3",
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
    c.query_row("SELECT count(*) FROM items WHERE server=sid() AND kind=?1", [kind], |r| r.get(0))
}

pub fn clear_library(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch(
        "DELETE FROM fts WHERE rowid IN (SELECT rowid FROM items WHERE server=sid()); DELETE FROM items WHERE server=sid();
         DELETE FROM cache WHERE server=sid(); DELETE FROM pending WHERE server=sid(); DELETE FROM kv WHERE server=sid() AND key='queue';",
    )
}

pub fn kv_get(c: &Connection, key: &str) -> rusqlite::Result<Option<String>> {
    c.prepare_cached("SELECT value FROM kv WHERE server=sid() AND key=?1")?.query_row([key], |r| r.get(0)).optional()
}

pub fn kv_put(c: &Connection, key: &str, value: &str) -> rusqlite::Result<()> {
    c.prepare_cached("INSERT OR REPLACE INTO kv(server, key, value) VALUES(sid(), ?1, ?2)")?.execute(params![key, value]).map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(c: &mut Connection, id: &str, title: &str) {
        let s: Song = serde_json::from_str(&format!(r#"{{"id":"{id}","title":"{title}"}}"#)).unwrap();
        index(c, &[], &[], &[s]).unwrap();
    }

    #[test]
    fn each_server_reads_its_own_rows_and_forgetting_one_leaves_the_rest() {
        let dir = std::env::temp_dir().join(format!("nori-one-db-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nori.db").display().to_string();

        let mut def = open(&path, "default").unwrap();
        let mut x1 = open(&path, "x1").unwrap();
        song(&mut def, "a1", "Airbag");
        song(&mut x1, "b1", "Bones");
        assert_eq!(search(&def, "airbag", 10).unwrap().songs.len(), 1);
        assert!(search(&def, "bones", 10).unwrap().songs.is_empty(), "another server's songs are not this one's");
        assert_eq!(search(&x1, "bones", 10).unwrap().songs[0].id, "b1");

        forget_server(&def, "x1").unwrap();
        assert!(search(&x1, "bones", 10).unwrap().songs.is_empty());
        assert_eq!(search(&def, "airbag", 10).unwrap().songs.len(), 1);
        drop((def, x1));
        assert_eq!(search(&open(&path, "default").unwrap(), "airbag", 10).unwrap().songs.len(), 1, "opened again, nothing lost");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
