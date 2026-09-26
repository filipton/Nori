//! A one-time import of what nori music 0.3.4 (the Kotlin/ExoPlayer app) kept on the phone, into this
//! app's database, so someone updating keeps their servers, settings and downloads. Temporary.
//!
//! # How to delete this
//!
//! Once the 0.3.4 users have updated, remove, and nothing else:
//! - this file, `crates/core/src/migrate_034.rs`;
//! - `pub mod migrate_034;` in `crates/core/src/lib.rs`;
//! - the start-up call in `core/src/main/kotlin/dev/nori/music/Nori.kt` (the `init` line marked
//!   "Remove with migrate_034");
//! - its tests, `crates/core/tests/migrate_034.rs`, and their fixtures, `crates/core/testdata/migrate_034/`.
//!
//! What it leaves on phones is harmless once it is gone: the `app_kv` row `migrated034`, and on a phone
//! where something went wrong, 0.3.4's files parked in `files/nori-0.3.4/`.
//!
//! # What 0.3.4 kept, and what comes over
//!
//! - `shared_prefs/nori.xml` (SharedPreferences): every setting and the server profiles (address, second
//!   address, user, password or API key, headers, certificate file name, ...). The keys are the ones the
//!   settings here are stored under, so it goes through `settings::load` as it is, but for two: the left
//!   swipe was `swipeLeft3`, and `ignoreSystemMotion` is dropped (it was written off on every phone; its
//!   successor `animateAnyway` keeps its new default).
//! - `shared_prefs/nori-devices.xml`: the devices never to be offered an AutoEQ curve (`quiet`) come over;
//!   the sound kept from before a device took over (`looseSound`) is a moment's state and does not.
//! - `shared_prefs/nori-outputs.xml`: every output device seen (`known`) comes over.
//! - `files/nori.db` (server "default") and `files/nori-<id>.db`, one SQLite database per server with no
//!   `server` column. Comes over: the downloads table (so downloaded songs show and play offline), the saved
//!   queue, smart playlists, songs kept out of mixes, writes waiting to be sent, and the equalizer profiles
//!   with their devices. Left behind: the library index, answers and covers (fetched again), recent
//!   searches, the AutoEQ list (fetched again), track analyses (measured again), and the play history
//!   (no call here writes a play with its own time; the server keeps its own play counts).
//! - The downloaded audio (media3's SimpleCache in the external files dir, `downloads/`) and media3's
//!   download index (`databases/exoplayer_internal.db`) are read by this app as they are: untouched.
//! - `files/certs/` (client certificates) is where this app reads them too: untouched.
//! - The stream cache and Coil's covers in the cache dir: caches, not carried over.
//!
//! 0.3.4's `files/nori.db` has the same name as this app's database and a layout it cannot use, so it is
//! moved aside before anything opens it: this has to run before the settings are opened.
//!
//! # Once, and safely
//!
//! It runs when 0.3.4's settings file or 0.3.4's database is there, and records `migrated034` in `app_kv`
//! when it is done. The old databases are parked in `files/nori-0.3.4/` while it reads them; when every step
//! worked, that folder and the old settings files are deleted, otherwise they are kept there, out of the
//! way, and the app goes on with what did come over. A run cut short starts again from the parked files
//! at the next start; every step can run twice. Nothing here panics into the app or returns an error: it
//! logs and carries on.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags, OptionalExtension};

use crate::settings::{load, PrefValue, SavedServer, StoredPrefs};
use crate::{alog, background, settings_store, Core, Param, PlayQueue, ServerConfig, Song, SoundProfile};

/// Written to `app_kv` once the import is done.
pub const MARKER: &str = "migrated034";
/// Where 0.3.4's databases wait while they are read, under the files dir.
pub const PARKED: &str = "nori-0.3.4";
/// Where the list of every output device seen is kept (`nori_devices::outputs`, `app_kv`).
const KNOWN_OUTPUTS: &str = "knownOutputs";

/// Imports what 0.3.4 kept, once; `files_dir` is the app's files dir (`Context.filesDir`). Costs a file
/// check and a look at the database's layout when there is nothing to do.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn migrate_034(files_dir: String) {
    let old = Old::at(Path::new(&files_dir));
    if !old.there() {
        return;
    }
    alog::info("migrate 0.3.4: importing what 0.3.4 kept");
    match std::panic::catch_unwind(|| old.run()) {
        Ok(problems) if problems.is_empty() => alog::info("migrate 0.3.4: done"),
        Ok(problems) => alog::info(&format!("migrate 0.3.4: done, without: {}", problems.join("; "))),
        Err(_) => alog::info("migrate 0.3.4: stopped by a panic; the app starts with what came over"),
    }
}

/// Where 0.3.4's things are.
struct Old {
    files: PathBuf,
    prefs: PathBuf,
}

impl Old {
    fn at(files: &Path) -> Old {
        let data = files.parent().unwrap_or(files);
        Old { files: files.to_path_buf(), prefs: data.join("shared_prefs") }
    }

    fn db(&self) -> PathBuf {
        self.files.join(crate::db::DB_FILE)
    }

    fn parked(&self, id: &str) -> PathBuf {
        self.files.join(PARKED).join(format!("{id}.db"))
    }

    fn pref_files(&self) -> [PathBuf; 3] {
        ["nori.xml", "nori-devices.xml", "nori-outputs.xml"].map(|n| self.prefs.join(n))
    }

    fn there(&self) -> bool {
        self.prefs.join("nori.xml").exists() || old_layout(&self.db())
    }

    /// Every step, each on its own; what did not work, in words.
    fn run(&self) -> Vec<String> {
        let mut problems = Vec::new();
        let mut note = |what: &str, r: Result<(), String>| {
            if let Err(e) = r {
                problems.push(format!("{what}: {e}"));
            }
        };
        let prefs = match read_prefs(&self.prefs.join("nori.xml")) {
            Ok(raw) => raw.map(|r| load(&renamed(r))),
            Err(e) => {
                note("settings", Err(e));
                None
            }
        };
        let servers: Vec<SavedServer> = prefs.as_ref().map(|p| p.servers.clone()).unwrap_or_default();
        let active = prefs.as_ref().map(|p| p.active_server_id.clone()).unwrap_or_default();

        // 0.3.4's databases out of the way, the "default" one first: it has this app's database's name.
        if old_layout(&self.db()) {
            if let Err(e) = park(&self.db(), &self.parked("default")) {
                // Left in place it is unusable and would be written over: all of it is tried again next start.
                return vec![format!("park nori.db: {e}")];
            }
        }
        for s in servers.iter().filter(|s| s.id != "default" && safe_id(&s.id)) {
            note(&format!("park {}", s.id), park(&self.files.join(format!("nori-{}.db", s.id)), &self.parked(&s.id)));
        }

        let db = self.db().to_string_lossy().into_owned();
        let fresh = match settings_store::settings_open(db.clone()) {
            Ok(now) => now == StoredPrefs::default(),
            Err(e) => {
                // No database to write to: nothing else can come over. The old files stay for another go.
                alog::info(&format!("migrate 0.3.4: the database: {e}"));
                return vec![format!("the database: {e}")];
            }
        };
        if settings_store::app_value(MARKER).is_none() {
            if let Some(p) = prefs.filter(|_| fresh) {
                settings_store::settings_put(p);
            }
            note("devices", self.devices());
            // The server in use first: an equalizer profile saved under the same name in two servers' databases
            // is the one the listener last had in front of them.
            let mut ids: Vec<&str> = vec![active.as_str()];
            ids.extend(servers.iter().map(|s| s.id.as_str()));
            ids.push("default");
            let mut seen = Vec::new();
            for id in ids.into_iter().filter(|id| !id.is_empty() && safe_id(id)) {
                if seen.contains(&id) {
                    continue;
                }
                seen.push(id);
                let server = servers.iter().find(|s| s.id == id);
                note(&format!("server {id}"), self.server(&db, id, server));
            }
            settings_store::keep_app_value(MARKER, "1".into());
        }
        flush();

        if problems.is_empty() {
            let _ = fs::remove_dir_all(self.files.join(PARKED));
            for f in self.pref_files() {
                let _ = fs::remove_file(f);
            }
        } else {
            // Kept for a look, where nothing reads them and this does not run again.
            let dir = self.files.join(PARKED);
            let _ = fs::create_dir_all(&dir);
            for f in self.pref_files() {
                if let Some(name) = f.file_name() {
                    let _ = fs::rename(&f, dir.join(name));
                }
            }
        }
        problems
    }

    /// The devices never to be offered a curve, and every device seen.
    fn devices(&self) -> Result<(), String> {
        let set = |file: &str, key: &str| -> Result<Option<Vec<String>>, String> {
            Ok(read_prefs(&self.prefs.join(file))?.and_then(|mut r| match r.remove(key) {
                Some(PrefValue::Texts { v }) => Some(v),
                _ => None,
            }))
        };
        if let Some(quiet) = set("nori-devices.xml", "quiet")? {
            settings_store::keep_app_value(nori_devices::profiles::QUIET, serde_json::to_string(&quiet).unwrap_or_default());
        }
        if let Some(mut known) = set("nori-outputs.xml", "known")? {
            known.sort();
            settings_store::keep_app_value(KNOWN_OUTPUTS, serde_json::to_string(&known).unwrap_or_default());
        }
        Ok(())
    }

    /// One server's rows, from its parked database, through a core opened for it.
    fn server(&self, db: &str, id: &str, profile: Option<&SavedServer>) -> Result<(), String> {
        let core = Core::new(db.to_string(), id.to_string()).map_err(|e| e.to_string())?;
        if let Some(p) = profile {
            // Its address and user kept now, so the first real connection does not take this for a new
            // server and clear its rows (the saved queue among them).
            let config = ServerConfig {
                url: p.url.clone(),
                user: p.user.clone(),
                password: p.password.clone(),
                api_key: Some(p.api_key.clone()).filter(|k| !k.is_empty()),
                legacy_auth: p.legacy_auth,
            };
            core.configure(config).map_err(|e| e.to_string())?;
        }
        let path = self.parked(id);
        if !path.exists() {
            return Ok(());
        }
        let old = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).map_err(|e| e.to_string())?;
        let mut problems = Vec::new();
        for (what, r) in [
            ("equalizer profiles", sound_profiles(&old, &core)),
            ("downloads", downloads(&old, &core)),
            ("queue", queue(&old, &core)),
            ("smart playlists", smart(&old, &core)),
            ("kept out of mixes", excluded(&old, &core)),
            ("waiting writes", pending(&old, &core)),
        ] {
            if let Err(e) = r {
                problems.push(format!("{what}: {e}"));
            }
        }
        if problems.is_empty() { Ok(()) } else { Err(problems.join(", ")) }
    }
}

type Step = Result<(), String>;

fn rows<T>(old: &Connection, sql: &str, f: impl FnMut(&rusqlite::Row) -> rusqlite::Result<T>) -> Result<Vec<T>, String> {
    let mut st = old.prepare(sql).map_err(|e| e.to_string())?;
    let r = st.query_map([], f).map_err(|e| e.to_string())?.collect::<rusqlite::Result<Vec<T>>>().map_err(|e| e.to_string());
    r
}

fn sound_profiles(old: &Connection, core: &Core) -> Step {
    let have: Vec<String> = core.profiles().map_err(|e| e.to_string())?.into_iter().map(|p| p.name).collect();
    for (name, json, outputs) in rows(old, "SELECT name, json, outputs FROM profiles", |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get::<_, String>(2)?)))? {
        if have.contains(&name) {
            continue;
        }
        let outputs = outputs.lines().filter(|l| !l.is_empty()).map(str::to_string).collect();
        core.profile_save(SoundProfile { name, json, outputs }).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn downloads(old: &Connection, core: &Core) -> Step {
    let all = rows(old, "SELECT json, done FROM downloads ORDER BY ts, id", |r| Ok((r.get::<_, String>(0)?, r.get::<_, bool>(1)?)))?;
    let (songs, done): (Vec<Song>, Vec<bool>) = all.into_iter().filter_map(|(j, d)| Some((serde_json::from_str::<Song>(&j).ok()?, d))).unzip();
    let finished: Vec<String> = songs.iter().zip(&done).filter(|(_, d)| **d).map(|(s, _)| s.id.clone()).collect();
    core.download_queue(songs).map_err(|e| e.to_string())?;
    let n = finished.len();
    core.download_settle(finished, vec![true; n]).map_err(|e| e.to_string())
}

fn queue(old: &Connection, core: &Core) -> Step {
    #[derive(serde::Deserialize, Default)]
    #[serde(default)]
    struct Q {
        songs: Vec<Song>,
        index: u32,
        position: u64,
    }
    let json: Option<String> = old.query_row("SELECT value FROM kv WHERE key='queue'", [], |r| r.get(0)).optional().map_err(|e| e.to_string())?;
    let Some(q) = json.and_then(|j| serde_json::from_str::<Q>(&j).ok()).filter(|q| !q.songs.is_empty()) else { return Ok(()) };
    core.save_queue(PlayQueue { songs: q.songs, index: q.index, position_ms: q.position }).map_err(|e| e.to_string())
}

fn smart(old: &Connection, core: &Core) -> Step {
    let mut skipped = 0;
    for (id, name, json) in rows(old, "SELECT id, name, json FROM smart_playlists ORDER BY updated_ms", |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, String>(2)?)))? {
        // One whose rules no longer read is left out, not the rest with it.
        if core.smart_save(id, name, json).is_err() {
            skipped += 1;
        }
    }
    if skipped == 0 { Ok(()) } else { Err(format!("{skipped} no longer read")) }
}

fn excluded(old: &Connection, core: &Core) -> Step {
    for id in rows(old, "SELECT song_id FROM mix_excluded", |r| r.get::<_, String>(0))? {
        core.mix_excluded_set(id, true).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn pending(old: &Connection, core: &Core) -> Step {
    // Added once: a second run would send them twice.
    if !core.pending_list().map_err(|e| e.to_string())?.is_empty() {
        return Ok(());
    }
    for (endpoint, json) in rows(old, "SELECT endpoint, params FROM pending ORDER BY rowid", |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))? {
        let pairs: Vec<(String, String)> = serde_json::from_str(&json).unwrap_or_default();
        core.pending_add(endpoint, pairs.into_iter().map(|(key, value)| Param { key, value }).collect()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Whatever the core's background thread was handed before this has been written.
fn flush() {
    let (tx, rx) = std::sync::mpsc::channel();
    background::run(move || {
        let _ = tx.send(());
    });
    let _ = rx.recv_timeout(Duration::from_secs(10));
}

/// A server id as 0.3.4 made them (eight hex digits, or "default"): safe in a file name.
fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
}

/// Whether `path` is a 0.3.4 database: an `items` table without the `server` column.
fn old_layout(path: &Path) -> bool {
    if !path.exists() {
        return false;
    }
    let Ok(c) = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX) else { return false };
    let columns: Vec<String> = match c.prepare("SELECT name FROM pragma_table_info('items')") {
        Ok(mut st) => st.query_map([], |r| r.get(0)).map(|r| r.filter_map(Result::ok).collect()).unwrap_or_default(),
        Err(_) => return false,
    };
    !columns.is_empty() && !columns.iter().any(|c| c == "server")
}

/// Moves a database aside with its write-ahead log folded in first, so the one file is all of it. One
/// already parked (a run cut short) stays as it is.
fn park(from: &Path, to: &Path) -> Step {
    if !from.exists() {
        return Ok(());
    }
    if let Ok(c) = Connection::open(from) {
        let _ = c.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()));
    }
    fs::create_dir_all(to.parent().unwrap_or(Path::new("."))).map_err(|e| e.to_string())?;
    for suffix in ["-wal", "-shm", ""] {
        let (f, t) = (PathBuf::from(format!("{}{suffix}", from.display())), PathBuf::from(format!("{}{suffix}", to.display())));
        if f.exists() {
            fs::rename(&f, &t).map_err(|e| format!("{}: {e}", f.display()))?;
        }
    }
    Ok(())
}

/// 0.3.4's keys as the settings here read them.
fn renamed(mut raw: HashMap<String, PrefValue>) -> HashMap<String, PrefValue> {
    // An older left swipe, which 0.3.4 no longer read.
    raw.remove("swipeLeft");
    if let Some(v) = raw.remove("swipeLeft3") {
        raw.insert("swipeLeft".into(), v);
    }
    raw.remove("ignoreSystemMotion");
    // Before server profiles there was one server in three keys; 0.3.4 read it as the profile "default".
    let text = |k: &str| match raw.get(k) {
        Some(PrefValue::Text { v }) => v.clone(),
        _ => String::new(),
    };
    if !raw.contains_key("servers") && !text("serverUrl").is_empty() {
        let one = serde_json::json!([{ "id": "default", "url": text("serverUrl"), "user": text("user"), "password": text("password") }]);
        raw.insert("servers".into(), PrefValue::Text { v: one.to_string() });
        raw.entry("activeServerId".into()).or_insert(PrefValue::Text { v: "default".into() });
    }
    raw
}

/// A SharedPreferences file as Android writes it (`XmlUtils.writeMapXml`); none when there is no file.
pub fn read_prefs(path: &Path) -> Result<Option<HashMap<String, PrefValue>>, String> {
    match fs::read(path) {
        Ok(bytes) => parse_prefs(&String::from_utf8(bytes).map_err(|_| format!("{}: not text", path.display()))?).map(Some),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(format!("{}: {e}", path.display())),
    }
}

/// `<map>` of `<boolean|int|long|float name=".." value=".." />`, `<string name="..">text</string>` and
/// `<set name=".."><string>..</string></set>`.
pub fn parse_prefs(xml: &str) -> Result<HashMap<String, PrefValue>, String> {
    let bad = |what: &str| format!("not a preferences file ({what})");
    let start = xml.find("<map").ok_or_else(|| bad("no map"))?;
    let (open, mut rest) = xml[start..].split_once('>').ok_or_else(|| bad("map"))?;
    let mut out = HashMap::new();
    if open.ends_with('/') {
        return Ok(out);
    }
    loop {
        rest = rest.trim_start();
        if rest.starts_with("</map>") {
            return Ok(out);
        }
        let body = rest.strip_prefix('<').ok_or_else(|| bad("text between entries"))?;
        let (tag, after) = body.split_once('>').ok_or_else(|| bad("an open tag"))?;
        rest = after;
        let closed = tag.ends_with('/');
        let tag = tag.trim_end_matches('/').trim();
        let (kind, attrs) = tag.split_once(char::is_whitespace).unwrap_or((tag, ""));
        let name = attr(attrs, "name").ok_or_else(|| bad("a nameless entry"))?;
        let value = || attr(attrs, "value").ok_or_else(|| bad("a valueless entry"));
        let num = |e: &str| bad(&format!("{name}: {e}"));
        let v = match kind {
            "boolean" => PrefValue::Flag { v: value()? == "true" },
            "int" => PrefValue::Number { v: value()?.parse().map_err(|_| num("int"))? },
            "long" => PrefValue::Big { v: value()?.parse().map_err(|_| num("long"))? },
            "float" => PrefValue::Decimal { v: value()?.parse().map_err(|_| num("float"))? },
            "null" => continue,
            "string" if closed => PrefValue::Text { v: String::new() },
            "string" => {
                let (text, after) = rest.split_once("</string>").ok_or_else(|| bad("an open string"))?;
                rest = after;
                PrefValue::Text { v: unescape(text) }
            }
            "set" if closed => PrefValue::Texts { v: Vec::new() },
            "set" => {
                let (inner, after) = rest.split_once("</set>").ok_or_else(|| bad("an open set"))?;
                rest = after;
                let items = inner.split("</string>").filter_map(|s| s.split_once("<string>").map(|(_, t)| unescape(t)));
                PrefValue::Texts { v: items.collect() }
            }
            other => return Err(bad(other)),
        };
        out.insert(name, v);
    }
}

fn attr(attrs: &str, key: &str) -> Option<String> {
    let at = attrs.find(&format!("{key}=\""))? + key.len() + 2;
    let len = attrs[at..].find('"')?;
    Some(unescape(&attrs[at..at + len]))
}

/// XML's entities back into characters (`&amp;`, `&quot;`, `&#10;`, ...).
fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';') else { break };
        let entity = &rest[1..end];
        let c = match entity {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => entity
                .strip_prefix("#x")
                .map(|h| u32::from_str_radix(h, 16))
                .or_else(|| entity.strip_prefix('#').map(str::parse::<u32>))
                .and_then(Result::ok)
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}
