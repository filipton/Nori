//! nori music core: Subsonic request signing, response parsing and the local
//! SQLite index. No sockets and no threads of its own; every call is coarse
//! (one response, one page) so the FFI crossing stays off the hot path.

uniffi::setup_scaffolding!();

mod api;
mod autoeq;
pub mod automix;
mod db;
mod lyrics;
pub mod dsp;
mod history;
mod m3u;
mod mixes;
mod model;
mod smart;

use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Deserialize;

pub use model::*;

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum CoreError {
    #[error("{reason}")]
    Api { code: i32, reason: String },
    #[error("bad response: {reason}")]
    Parse { reason: String },
    #[error("database: {reason}")]
    Db { reason: String },
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Db { reason: e.to_string() }
    }
}

type Result<T> = std::result::Result<T, CoreError>;

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ServerConfig {
    pub url: String,
    pub user: String,
    pub password: String,
    /// OpenSubsonic API key; when set it is used instead of user and password.
    pub api_key: Option<String>,
    pub legacy_auth: bool,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PendingCall {
    pub row_id: i64,
    pub endpoint: String,
    pub params: Vec<Param>,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct Param {
    pub key: String,
    pub value: String,
}

// ---- wire shapes -----------------------------------------------------------

#[derive(Deserialize, Default)]
#[serde(default)]
struct ApiError {
    code: i32,
    message: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Found {
    artist: Vec<Artist>,
    album: Vec<Album>,
    song: Vec<Song>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct AlbumWire {
    #[serde(flatten)]
    album: Album,
    song: Vec<Song>,
    #[serde(rename = "discTitles")]
    disc_titles: Vec<DiscTitle>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct ArtistWire {
    #[serde(flatten)]
    artist: Artist,
    album: Vec<Album>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct PlaylistWire {
    #[serde(flatten)]
    playlist: Playlist,
    entry: Vec<Song>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Albums {
    album: Vec<Album>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Songs {
    song: Vec<Song>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Index {
    artist: Vec<Artist>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Artists {
    index: Vec<Index>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Playlists {
    playlist: Vec<Playlist>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Genres {
    genre: Vec<Genre>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Stations {
    #[serde(rename = "internetRadioStation")]
    station: Vec<RadioStation>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct ArtistInfoWire {
    biography: Option<String>,
    last_fm_url: Option<String>,
    music_brainz_id: Option<String>,
    large_image_url: Option<String>,
    medium_image_url: Option<String>,
    similar_artist: Vec<Artist>,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct LyricsList {
    structured_lyrics: Vec<lyrics::Structured>,
}


#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Folders {
    music_folder: Vec<MusicFolder>,
}

/// getMusicDirectory mixes folders and songs in one `child` list, told apart by `isDir`.
#[derive(Deserialize, Default)]
#[serde(default)]
struct DirectoryWire {
    #[serde(deserialize_with = "crate::model::id_string")]
    id: String,
    name: String,
    child: Vec<serde_json::Value>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Share {
    url: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Shares {
    share: Vec<Share>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct QueueWire {
    entry: Vec<Song>,
    current: Option<serde_json::Value>,
    position: u64,
}

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Response {
    status: String,
    version: String,
    #[serde(rename = "type")]
    kind: String,
    server_version: String,
    open_subsonic: bool,
    error: Option<ApiError>,
    search_result3: Option<Found>,
    starred2: Option<Found>,
    album: Option<AlbumWire>,
    artist: Option<ArtistWire>,
    playlist: Option<PlaylistWire>,
    album_list2: Option<Albums>,
    artists: Option<Artists>,
    playlists: Option<Playlists>,
    genres: Option<Genres>,
    random_songs: Option<Songs>,
    songs_by_genre: Option<Songs>,
    similar_songs2: Option<Songs>,
    top_songs: Option<Songs>,
    song: Option<Song>,
    internet_radio_stations: Option<Stations>,
    artist_info2: Option<ArtistInfoWire>,
    lyrics_list: Option<LyricsList>,
    play_queue: Option<QueueWire>,
    shares: Option<Shares>,
    music_folders: Option<Folders>,
    indexes: Option<Artists>,
    directory: Option<DirectoryWire>,
}

#[derive(Deserialize)]
struct Envelope {
    #[serde(rename = "subsonic-response")]
    response: Response,
}

fn parse(body: &[u8]) -> Result<Response> {
    let r = serde_json::from_slice::<Envelope>(body).map_err(|e| CoreError::Parse { reason: e.to_string() })?.response;
    if r.status != "ok" {
        let e = r.error.unwrap_or_default();
        return Err(CoreError::Api { code: e.code, reason: if e.message.is_empty() { "request failed".into() } else { e.message } });
    }
    Ok(r)
}

// ---- the object Kotlin holds -----------------------------------------------

#[derive(uniffi::Object)]
pub struct Core {
    db: Mutex<Connection>,
    server: RwLock<api::Server>,
}

#[uniffi::export]
impl Core {
    /// `db_path` empty opens an in-memory index.
    #[uniffi::constructor]
    pub fn new(db_path: String) -> Result<Arc<Self>> {
        let db = db::open(&db_path)?;
        automix::store::migrate(&db)?;
        Ok(Arc::new(Core { db: Mutex::new(db), server: RwLock::new(api::Server::default()) }))
    }

    /// Returns the normalised base url. Each server profile has its own database file, so the
    /// index is only dropped when the same file is pointed at a different server or user.
    pub fn configure(&self, config: ServerConfig) -> Result<String> {
        let auth = match (&config.api_key, config.legacy_auth) {
            (Some(k), _) if !k.is_empty() => api::Auth::ApiKey(k),
            (_, true) => api::Auth::Legacy { user: &config.user, password: &config.password },
            _ => api::Auth::Token { user: &config.user, password: &config.password },
        };
        let s = api::Server::with(&config.url, auth);
        let ident = format!("{}|{}", s.base, config.user);
        let db = self.db.lock();
        if db::kv_get(&db, "server")?.as_deref() != Some(&ident) {
            db::clear_library(&db)?;
            db::kv_put(&db, "server", &ident)?;
        }
        let base = s.base.clone();
        *self.server.write() = s;
        Ok(base)
    }

    /// Points requests at another address of the same server (LAN vs WAN) without touching the index.
    pub fn use_address(&self, url: String) {
        let next = self.server.read().rebased(&url);
        *self.server.write() = next;
    }

    /// getIndexes: the top of the folder tree.
    pub fn parse_indexes(&self, body: Vec<u8>) -> Result<Vec<Artist>> {
        Ok(parse(&body)?.indexes.unwrap_or_default().index.into_iter().flat_map(|i| i.artist).collect())
    }

    pub fn parse_directory(&self, body: Vec<u8>) -> Result<Directory> {
        let d = parse(&body)?.directory.unwrap_or_default();
        let mut out = Directory { id: d.id, name: d.name, ..Default::default() };
        for c in d.child {
            if c.get("isDir").and_then(|v| v.as_bool()).unwrap_or(false) {
                if let Ok(mut a) = serde_json::from_value::<Artist>(c.clone()) {
                    if a.name.is_empty() {
                        a.name = c.get("title").and_then(|t| t.as_str()).unwrap_or_default().to_string();
                    }
                    out.folders.push(a);
                }
            } else if let Ok(s) = serde_json::from_value::<Song>(c) {
                out.songs.push(s);
            }
        }
        Ok(out)
    }

    pub fn parse_music_folders(&self, body: Vec<u8>) -> Result<Vec<MusicFolder>> {
        Ok(parse(&body)?.music_folders.unwrap_or_default().music_folder)
    }

    pub fn url(&self, endpoint: String, params: Vec<Param>) -> String {
        let p: Vec<(String, String)> = params.into_iter().map(|p| (p.key, p.value)).collect();
        self.server.read().url(&endpoint, &p)
    }

    /// Signed url of `endpoint` with no parameters; callers that build many urls (cover art, per list row)
    /// append `&id=..` themselves instead of crossing the FFI for each one.
    pub fn url_prefix(&self, endpoint: String) -> String {
        self.server.read().url(&endpoint, &[])
    }

    /// `max_bit_rate` 0 and empty `format` mean the original file. A transcode is an ffmpeg pipe
    /// with no byte length, which the player reads as unseekable (dead scrub, frozen notification
    /// progress); `estimateContentLength` makes the server declare duration x bitrate so seeks work.
    pub fn stream_url(&self, id: String, max_bit_rate: u32, format: String) -> String {
        let transcode = max_bit_rate > 0 || !format.is_empty();
        let mut p = vec![("id".to_string(), id)];
        if max_bit_rate > 0 {
            p.push(("maxBitRate".into(), max_bit_rate.to_string()));
        }
        if !format.is_empty() {
            p.push(("format".into(), format));
        }
        if transcode {
            p.push(("estimateContentLength".into(), "true".to_string()));
        }
        self.server.read().url("stream", &p)
    }

    pub fn cover_url(&self, id: String, size: u32) -> String {
        let mut p = vec![("id".to_string(), id)];
        if size > 0 {
            p.push(("size".into(), size.to_string()));
        }
        self.server.read().url("getCoverArt", &p)
    }

    // ---- parsing; library items seen on the way are indexed ----

    /// Validates any response; used for ping and for calls with no payload.
    pub fn parse_status(&self, body: Vec<u8>) -> Result<ServerInfo> {
        let r = parse(&body)?;
        Ok(ServerInfo { version: r.version, server_type: r.kind, server_version: r.server_version, open_subsonic: r.open_subsonic })
    }

    pub fn parse_search(&self, body: Vec<u8>) -> Result<SearchResult> {
        let f = parse(&body)?.search_result3.unwrap_or_default();
        db::index(&mut self.db.lock(), &f.artist, &f.album, &f.song)?;
        Ok(SearchResult { artists: f.artist, albums: f.album, songs: f.song })
    }

    /// Library sync: index a search3 page without sending it back over the FFI.
    pub fn ingest_search(&self, body: Vec<u8>) -> Result<IngestStats> {
        let f = parse(&body)?.search_result3.unwrap_or_default();
        let mut st = db::index(&mut self.db.lock(), &f.artist, &f.album, &f.song)?;
        // Callers page until a page comes back empty, so report what was seen, not what changed.
        st.artists = f.artist.len() as u32;
        st.albums = f.album.len() as u32;
        st.songs = f.song.len() as u32;
        Ok(st)
    }

    pub fn parse_starred(&self, body: Vec<u8>) -> Result<Starred> {
        let f = parse(&body)?.starred2.unwrap_or_default();
        db::index(&mut self.db.lock(), &f.artist, &f.album, &f.song)?;
        Ok(Starred { artists: f.artist, albums: f.album, songs: f.song })
    }

    pub fn parse_album(&self, body: Vec<u8>) -> Result<AlbumDetail> {
        let a = parse(&body)?.album.unwrap_or_default();
        db::index(&mut self.db.lock(), &[], std::slice::from_ref(&a.album), &a.song)?;
        Ok(AlbumDetail { album: a.album, songs: a.song, disc_titles: a.disc_titles })
    }

    pub fn parse_artist(&self, body: Vec<u8>) -> Result<ArtistDetail> {
        let a = parse(&body)?.artist.unwrap_or_default();
        db::index(&mut self.db.lock(), std::slice::from_ref(&a.artist), &a.album, &[])?;
        Ok(ArtistDetail { artist: a.artist, albums: a.album })
    }

    pub fn parse_artist_info(&self, body: Vec<u8>) -> Result<ArtistInfo> {
        let i = parse(&body)?.artist_info2.unwrap_or_default();
        Ok(ArtistInfo {
            biography: i.biography.filter(|b| !b.trim().is_empty()),
            image_url: i.large_image_url.or(i.medium_image_url).filter(|u| !u.is_empty()),
            similar: i.similar_artist,
            last_fm_url: i.last_fm_url.filter(|u| u.starts_with("http")),
            music_brainz_id: i.music_brainz_id.filter(|m| !m.is_empty()),
        })
    }

    pub fn parse_album_list(&self, body: Vec<u8>) -> Result<Vec<Album>> {
        let l = parse(&body)?.album_list2.unwrap_or_default().album;
        db::index(&mut self.db.lock(), &[], &l, &[])?;
        Ok(l)
    }

    pub fn parse_artists(&self, body: Vec<u8>) -> Result<Vec<Artist>> {
        let l: Vec<Artist> = parse(&body)?.artists.unwrap_or_default().index.into_iter().flat_map(|i| i.artist).collect();
        db::index(&mut self.db.lock(), &l, &[], &[])?;
        Ok(l)
    }

    /// randomSongs, songsByGenre, similarSongs2, topSongs and getSong all land here.
    pub fn parse_songs(&self, body: Vec<u8>) -> Result<Vec<Song>> {
        let r = parse(&body)?;
        let l = r
            .random_songs
            .or(r.songs_by_genre)
            .or(r.similar_songs2)
            .or(r.top_songs)
            .map(|s| s.song)
            .or(r.song.map(|s| vec![s]))
            .unwrap_or_default();
        db::index(&mut self.db.lock(), &[], &[], &l)?;
        Ok(l)
    }

    pub fn parse_playlists(&self, body: Vec<u8>) -> Result<Vec<Playlist>> {
        Ok(parse(&body)?.playlists.unwrap_or_default().playlist)
    }

    pub fn parse_playlist(&self, body: Vec<u8>) -> Result<PlaylistDetail> {
        let p = parse(&body)?.playlist.unwrap_or_default();
        db::index(&mut self.db.lock(), &[], &[], &p.entry)?;
        Ok(PlaylistDetail { playlist: p.playlist, songs: p.entry })
    }

    pub fn parse_genres(&self, body: Vec<u8>) -> Result<Vec<Genre>> {
        let mut g = parse(&body)?.genres.unwrap_or_default().genre;
        g.sort_by(|a, b| b.song_count.cmp(&a.song_count));
        Ok(g)
    }

    pub fn parse_radio(&self, body: Vec<u8>) -> Result<Vec<RadioStation>> {
        Ok(parse(&body)?.internet_radio_stations.unwrap_or_default().station)
    }

    /// Synced lyrics win over plain ones; words are timed from the server's cues or estimated (see lyrics.rs).
    pub fn parse_lyrics(&self, body: Vec<u8>) -> Result<Lyrics> {
        Ok(lyrics::build(parse(&body)?.lyrics_list.unwrap_or_default().structured_lyrics))
    }

    pub fn parse_share(&self, body: Vec<u8>) -> Result<String> {
        parse(&body)?.shares.and_then(|s| s.share.into_iter().next()).map(|s| s.url).filter(|u| !u.is_empty()).ok_or(CoreError::Parse { reason: "no share in response".into() })
    }

    pub fn parse_play_queue(&self, body: Vec<u8>) -> Result<PlayQueue> {
        let q = parse(&body)?.play_queue.unwrap_or_default();
        let current = q.current.map(|v| v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()));
        let index = current.and_then(|c| q.entry.iter().position(|s| s.id == c)).unwrap_or(0) as u32;
        Ok(PlayQueue { songs: q.entry, index, position_ms: q.position })
    }

    // ---- local index ----

    pub fn local_search(&self, query: String, limit: u32) -> Result<SearchResult> {
        Ok(db::search(&self.db.lock(), &query, limit)?)
    }

    pub fn index_size(&self) -> Result<IngestStats> {
        let c = self.db.lock();
        Ok(IngestStats { artists: db::count(&c, db::ARTIST)?, albums: db::count(&c, db::ALBUM)?, songs: db::count(&c, db::SONG)? })
    }

    // ---- response cache (stale-while-revalidate is driven from Kotlin) ----

    pub fn cache_get(&self, key: String) -> Result<Option<Vec<u8>>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT body FROM cache WHERE key=?1")?;
        Ok(st.query_row([key], |r| r.get(0)).optional()?)
    }

    /// True when `key` was stored less than `max_age_ms` ago: the caller can skip asking the server again.
    pub fn cache_fresh(&self, key: String, max_age_ms: i64) -> Result<bool> {
        let c = self.db.lock();
        let ts: Option<i64> = c.prepare_cached("SELECT ts FROM cache WHERE key=?1")?.query_row([key], |r| r.get(0)).optional()?;
        Ok(ts.is_some_and(|t| db::now_ms() - t < max_age_ms))
    }

    pub fn cache_put(&self, key: String, body: Vec<u8>) -> Result<()> {
        let c = self.db.lock();
        c.prepare_cached("INSERT OR REPLACE INTO cache(key, body, ts) VALUES(?1, ?2, ?3)")?.execute(params![key, body, db::now_ms()])?;
        Ok(())
    }

    /// Drops cached responses whose key starts with `prefix` (after a write to the server).
    pub fn cache_evict(&self, prefix: String) -> Result<()> {
        let c = self.db.lock();
        c.execute("DELETE FROM cache WHERE key >= ?1 AND key < ?1 || x'ff'", [prefix])?;
        Ok(())
    }

    // ---- play queue, survives process death ----

    pub fn save_queue(&self, queue: PlayQueue) -> Result<()> {
        let json = serde_json::json!({ "songs": queue.songs, "index": queue.index, "position": queue.position_ms });
        Ok(db::kv_put(&self.db.lock(), "queue", &json.to_string())?)
    }

    pub fn load_queue(&self) -> Result<PlayQueue> {
        #[derive(Deserialize, Default)]
        #[serde(default)]
        struct Q {
            songs: Vec<Song>,
            index: u32,
            position: u64,
        }
        let q: Q = db::kv_get(&self.db.lock(), "queue")?.and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default();
        Ok(PlayQueue { songs: q.songs, index: q.index, position_ms: q.position })
    }

    // ---- writes made while offline (stars, ratings, playlist edits, scrobbles), replayed in order ----

    pub fn pending_add(&self, endpoint: String, params: Vec<Param>) -> Result<()> {
        let json = serde_json::to_string(&params.iter().map(|p| (&p.key, &p.value)).collect::<Vec<_>>()).unwrap_or_default();
        self.db.lock().execute("INSERT INTO pending(endpoint, params) VALUES(?1, ?2)", params![endpoint, json])?;
        Ok(())
    }

    pub fn pending_list(&self) -> Result<Vec<PendingCall>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT rowid, endpoint, params FROM pending ORDER BY rowid LIMIT 200")?;
        let rows = st.query_map([], |r| Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))?;
        Ok(rows
            .filter_map(|r| r.ok())
            .map(|(row_id, endpoint, json)| {
                let pairs: Vec<(String, String)> = serde_json::from_str(&json).unwrap_or_default();
                PendingCall { row_id, endpoint, params: pairs.into_iter().map(|(key, value)| Param { key, value }).collect() }
            })
            .collect())
    }

    pub fn pending_done(&self, row_id: i64) -> Result<()> {
        self.db.lock().execute("DELETE FROM pending WHERE rowid=?1", [row_id])?;
        Ok(())
    }

    /// The "all songs" list: a sorted, optionally filtered page of the index. `sort` is one of
    /// title, artist, album, year, duration, created, playCount, userRating; anything else means index order.
    pub fn browse_songs(&self, sort: String, descending: bool, starred_only: bool, year_from: u32, year_to: u32, offset: u32, limit: u32) -> Result<Vec<Song>> {
        let key = match sort.as_str() {
            "title" | "artist" | "album" => format!("json_extract(json, '$.{sort}') COLLATE NOCASE"),
            "year" | "duration" | "created" | "playCount" | "userRating" => format!("json_extract(json, '$.{sort}')"),
            _ => "rowid".to_string(),
        };
        let mut sql = String::from("SELECT json FROM items WHERE kind=?1");
        if starred_only {
            sql.push_str(" AND json_extract(json, '$.starred') = 1");
        }
        if year_to > 0 {
            sql.push_str(" AND json_extract(json, '$.year') BETWEEN ?4 AND ?5");
        }
        sql.push_str(&format!(" ORDER BY {key} {} LIMIT ?3 OFFSET ?2", if descending { "DESC" } else { "ASC" }));
        let c = self.db.lock();
        let mut st = c.prepare_cached(&sql)?;
        let map = |r: &rusqlite::Row| r.get::<_, String>(0);
        let rows: Vec<String> = if year_to > 0 {
            st.query_map(params![db::SONG, offset, limit, year_from, year_to], map)?.filter_map(|r| r.ok()).collect()
        } else {
            st.query_map(params![db::SONG, offset, limit], map)?.filter_map(|r| r.ok()).collect()
        };
        Ok(rows.iter().filter_map(|j| serde_json::from_str(j).ok()).collect())
    }

    /// Decades that have songs in the index, newest first, with how many: what "browse by decade" lists.
    pub fn browse_decades(&self) -> Result<Vec<Genre>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT (json_extract(json, '$.year') / 10) * 10 AS d, count(*) FROM items WHERE kind=?1 AND json_extract(json, '$.year') > 0 GROUP BY d ORDER BY d DESC")?;
        let rows = st.query_map([db::SONG], |r| Ok(Genre { name: r.get::<_, i64>(0)?.to_string(), song_count: r.get(1)?, album_count: 0 }))?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// A page of every indexed song, for "download the whole library".
    pub fn indexed_songs(&self, offset: u32, limit: u32) -> Result<Vec<Song>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT json FROM items WHERE kind=?1 ORDER BY rowid LIMIT ?3 OFFSET ?2")?;
        let rows = st.query_map(params![db::SONG, offset, limit], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|j| serde_json::from_str(&j.ok()?).ok()).collect())
    }

    // ---- downloads: the metadata side; media3 owns the bytes ----

    /// Queued, not yet complete; [download_done] makes it show up in [downloads].
    pub fn download_add(&self, song: Song) -> Result<()> {
        let json = serde_json::to_string(&song).unwrap_or_default();
        self.db.lock().execute("INSERT OR IGNORE INTO downloads(id, json, ts) VALUES(?1, ?2, ?3)", params![song.id, json, db::now_ms()])?;
        Ok(())
    }

    pub fn download_done(&self, id: String) -> Result<()> {
        self.db.lock().execute("UPDATE downloads SET done=1 WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn download_remove(&self, id: String) -> Result<()> {
        self.db.lock().execute("DELETE FROM downloads WHERE id=?1", [id])?;
        Ok(())
    }

    pub fn downloads(&self, done: bool) -> Result<Vec<Song>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT json FROM downloads WHERE done=?1 ORDER BY ts DESC")?;
        let rows = st.query_map([done], |r| r.get::<_, String>(0))?;
        Ok(rows.filter_map(|j| serde_json::from_str(&j.ok()?).ok()).collect())
    }

    // ---- AutoEQ headphone database (only fetched when the user opens the browser) ----

    /// The url of the index the caller should download and hand to [autoeq_store].
    pub fn autoeq_index_url(&self) -> String {
        autoeq::INDEX_URL.to_string()
    }

    /// Parses `INDEX.md` into the local table; returns how many headphones it holds.
    pub fn autoeq_store(&self, markdown: String) -> Result<u32> {
        Ok(autoeq::store(&mut self.db.lock(), &markdown)?)
    }

    pub fn autoeq_search(&self, query: String, limit: u32) -> Result<Vec<AutoEqEntry>> {
        Ok(autoeq::search(&self.db.lock(), &query, limit)?)
    }

    /// The curves measured for the headphones behind an output device's own name (a Bluetooth name, a USB
    /// product name), best first; empty when the name says nothing or the index is not downloaded.
    pub fn autoeq_for_device(&self, device: String, limit: u32) -> Result<Vec<AutoEqEntry>> {
        Ok(autoeq::matching(&self.db.lock(), &device, limit)?)
    }

    pub fn autoeq_count(&self) -> Result<u32> {
        Ok(autoeq::count(&self.db.lock())?)
    }

    pub fn autoeq_preset_url(&self, entry: AutoEqEntry) -> String {
        autoeq::preset_url(&entry)
    }

    // ---- saved sound profiles ----

    pub fn profile_save(&self, profile: SoundProfile) -> Result<()> {
        let c = self.db.lock();
        c.prepare_cached("INSERT OR REPLACE INTO profiles(name, json, outputs) VALUES(?1, ?2, ?3)")?
            .execute(params![profile.name, profile.json, profile.outputs.join("\n")])?;
        Ok(())
    }

    pub fn profiles(&self) -> Result<Vec<SoundProfile>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT name, json, outputs FROM profiles ORDER BY name")?;
        let rows = st.query_map([], |r| {
            Ok(SoundProfile {
                name: r.get(0)?,
                json: r.get(1)?,
                outputs: r.get::<_, String>(2)?.lines().filter(|l| !l.is_empty()).map(str::to_string).collect(),
            })
        })?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn profile_delete(&self, name: String) -> Result<()> {
        self.db.lock().execute("DELETE FROM profiles WHERE name=?1", [name])?;
        Ok(())
    }

    /// Binds [output] to the profile [name] and to no other; None leaves it bound to nothing. One device has
    /// one sound, whichever side of the settings it was chosen from.
    pub fn profile_bind(&self, output: String, name: Option<String>) -> Result<()> {
        let mut c = self.db.lock();
        let tx = c.transaction()?;
        let rows: Vec<(String, String)> = {
            let mut st = tx.prepare("SELECT name, outputs FROM profiles")?;
            let r = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.collect::<rusqlite::Result<_>>()?;
            r
        };
        for (profile, outputs) in rows {
            let mut list: Vec<&str> = outputs.lines().filter(|l| !l.is_empty() && *l != output).collect();
            if name.as_deref() == Some(profile.as_str()) {
                list.push(&output);
            }
            let joined = list.join("\n");
            if joined != outputs {
                tx.execute("UPDATE profiles SET outputs=?1 WHERE name=?2", params![joined, profile])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    /// The profile bound to [output], if any: what to apply when that device becomes the active one.
    pub fn profile_for_output(&self, output: String) -> Result<Option<SoundProfile>> {
        Ok(self.profiles()?.into_iter().find(|p| p.outputs.iter().any(|o| *o == output)))
    }

    // ---- search history ----

    pub fn search_remember(&self, query: String) -> Result<()> {
        let c = self.db.lock();
        c.execute("INSERT OR REPLACE INTO searches(query, ts) VALUES(?1, ?2)", params![query.trim(), db::now_ms()])?;
        c.execute("DELETE FROM searches WHERE query NOT IN (SELECT query FROM searches ORDER BY ts DESC LIMIT 20)", [])?;
        Ok(())
    }

    pub fn search_history(&self) -> Result<Vec<String>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT query FROM searches ORDER BY ts DESC")?;
        let rows = st.query_map([], |r| r.get(0))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    pub fn search_forget(&self) -> Result<()> {
        self.db.lock().execute("DELETE FROM searches", [])?;
        Ok(())
    }
}

/// LRC or plain lyrics text, from a third-party provider, into the app's lyrics shape.
#[uniffi::export]
pub fn lyrics_from_lrc(text: String) -> Lyrics {
    lyrics::from_lrc(&text)
}

/// Reads an AutoEQ "ParametricEQ.txt" / Equalizer APO preset:
/// `Preamp: -6.2 dB` and `Filter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70` lines; anything else is ignored.
#[uniffi::export]
pub fn parse_eq_preset(text: String) -> EqPreset {
    let mut preset = EqPreset::default();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let after = |key: &str| t.iter().position(|w| w.eq_ignore_ascii_case(key)).and_then(|i| t.get(i + 1)).and_then(|v| v.parse::<f32>().ok());
        if t.first().is_some_and(|w| w.eq_ignore_ascii_case("preamp:")) {
            preset.preamp_db = t.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        } else if t.first().is_some_and(|w| w.eq_ignore_ascii_case("filter")) {
            let Some(on) = t.iter().position(|w| w.eq_ignore_ascii_case("ON")) else { continue };
            let token = t.get(on + 1).map(|k| k.to_ascii_uppercase()).unwrap_or_default();
            let kind = match token.as_str() {
                "PK" | "PEQ" | "MODAL" => EqKind::Peaking,
                "LS" | "LSC" | "LSQ" => EqKind::LowShelf,
                "HS" | "HSC" | "HSQ" => EqKind::HighShelf,
                "LSC 6DB" | "LS 6DB" | "LS6" => EqKind::LowShelfSlope,
                "HSC 6DB" | "HS 6DB" | "HS6" => EqKind::HighShelfSlope,
                "LP" | "LPQ" => EqKind::LowPass,
                "HP" | "HPQ" => EqKind::HighPass,
                "BP" => EqKind::BandPass,
                "NO" | "NOTCH" => EqKind::Notch,
                "AP" => EqKind::AllPass,
                _ => continue,
            };
            // Only the shelving and peaking kinds carry a gain; a pass filter line has none.
            let Some(freq) = after("Fc") else { continue };
            let gain_db = after("Gain").unwrap_or(0.0);
            if matches!(kind, EqKind::Peaking | EqKind::LowShelf | EqKind::HighShelf | EqKind::LowShelfSlope | EqKind::HighShelfSlope) && after("Gain").is_none() {
                continue;
            }
            preset.bands.push(EqBand { kind, freq, gain_db, q: after("Q").unwrap_or(0.71) });
        }
    }
    preset
}

#[cfg(test)]
mod tests;
