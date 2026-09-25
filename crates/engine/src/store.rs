//! Songs kept on disk, in a directory the client names: the stream cache (songs as they were streamed,
//! under the core's cache keys, `<id>:<quality>`) and downloads (whole songs kept for good, by id).
//! Plain files through `std::fs`, so any platform with a file system has them.
//!
//! A streamed song is written into the cache while it loads (the loader tees each burst into it) and is
//! only an entry once all of it is there: a half-written one is a `.part` file that nothing reads, and
//! goes when the next attempt starts. The cache is held to a size; what leaves it first when it is over
//! is the [`Order`]'s call (the core's `stream_cache` for a client that links it: never used by this
//! run first, then least recently used), one whole song at a time.

use std::collections::{HashMap, HashSet};
use std::fs::{self, File};
use std::io::{self, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use parking_lot::Mutex;

/// Which cached song goes first when the cache is over its limit. Told of every use, told once of what
/// an earlier run left, and asked for one key at a time.
pub trait Order: Send + Sync {
    /// `key` was read or written just now.
    fn touch(&self, key: &str);
    /// What the cache held when it was first looked at.
    fn seed(&self, held: &[String]);
    /// The next key to drop, forgotten as it is handed out.
    fn next(&self) -> Option<String>;
    /// The cache was emptied.
    fn clear(&self);
}

/// Least recently used first, and what an earlier run left before anything used since: the core's rule
/// (`stream_cache`), for a client without the core.
#[derive(Default)]
pub struct Recent(Mutex<(HashMap<String, i64>, i64, i64)>);

impl Order for Recent {
    fn touch(&self, key: &str) {
        let mut o = self.0.lock();
        o.1 += 1;
        let t = o.1;
        o.0.insert(key.to_string(), t);
    }

    fn seed(&self, held: &[String]) {
        let mut o = self.0.lock();
        for k in held {
            if !o.0.contains_key(k) {
                o.2 -= 1;
                let t = o.2;
                o.0.insert(k.clone(), t);
            }
        }
    }

    fn next(&self) -> Option<String> {
        let mut o = self.0.lock();
        let key = o.0.iter().min_by_key(|(_, t)| **t).map(|(k, _)| k.clone())?;
        o.0.remove(&key);
        Some(key)
    }

    fn clear(&self) {
        self.0.lock().0.clear();
    }
}

const STREAM: &str = "stream";
const DOWNLOADS: &str = "downloads";
const PART: &str = ".part";

/// A key as a file name: letters, digits, `-`, `_` and `.` as they are, everything else as `%XX`, so
/// every key has a name of its own on every file system.
fn file_name(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 8);
    for b in key.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_') || (b == b'.' && !out.is_empty()) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

/// The key a file name stands for; none for anything that is not an entry.
fn key_of(name: &str) -> Option<String> {
    if name.ends_with(PART) {
        return None;
    }
    let b = name.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            out.push(u8::from_str_radix(name.get(i + 1..i + 3)?, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

struct Held {
    /// Bytes the cache holds, once it has been counted (the first time something had to go).
    bytes: Option<u64>,
    limit: u64,
    /// Keys an entry is being written for now: one writer each, or the second truncates the first's file.
    writing: HashSet<String>,
}

/// The songs on disk. Shared by the engine's loaders, the downloader and whatever measures songs ahead.
pub struct Store {
    dir: PathBuf,
    order: Box<dyn Order>,
    held: Mutex<Held>,
    /// The songs fetched ahead of their turn ([`Store::fetch_ahead`]).
    pub(crate) ahead: crate::ahead::Ahead,
    /// Told whenever a streamed song has become whole in the cache ([`Store::on_whole`]).
    whole: Mutex<Vec<Box<dyn Fn() + Send + Sync>>>,
}

impl Store {
    /// The store in `dir` (made if it is not there), the stream cache held to `limit` bytes.
    pub fn open(dir: impl Into<PathBuf>, limit: u64, order: Box<dyn Order>) -> io::Result<Arc<Store>> {
        let dir = dir.into();
        fs::create_dir_all(dir.join(STREAM))?;
        fs::create_dir_all(dir.join(DOWNLOADS))?;
        Ok(Arc::new(Store { dir, order, held: Mutex::new(Held { bytes: None, limit, writing: HashSet::new() }), ahead: Default::default(), whole: Mutex::new(Vec::new()) }))
    }

    /// `told` hears of every streamed song that becomes whole in the cache from now on, on the thread
    /// that wrote its last bytes: what measures songs ahead looks again then, never by polling.
    pub fn on_whole(&self, told: Box<dyn Fn() + Send + Sync>) {
        self.whole.lock().push(told);
    }

    fn stream_path(&self, key: &str) -> PathBuf {
        self.dir.join(STREAM).join(file_name(key))
    }

    /// Where a download of `id` is kept once it is whole.
    pub fn download_path(&self, id: &str) -> PathBuf {
        self.dir.join(DOWNLOADS).join(file_name(id))
    }

    /// Where a download of `id` is written while it comes in.
    pub fn download_part(&self, id: &str) -> PathBuf {
        let mut p = self.download_path(id).into_os_string();
        p.push(PART);
        p.into()
    }

    /// The whole downloaded song `id`, if it is here.
    pub fn downloaded(&self, id: &str) -> Option<PathBuf> {
        let p = self.download_path(id);
        p.is_file().then_some(p)
    }

    /// The whole streamed song under `key`, if the cache has it; a use, for the order.
    pub fn cached(&self, key: &str) -> Option<PathBuf> {
        let p = self.stream_path(key);
        if !p.is_file() {
            return None;
        }
        self.order.touch(key);
        Some(p)
    }

    /// The whole streamed song under `key`, if the cache has it, without counting as a use: for
    /// reading it in the background (measuring it ahead), which says nothing about what is listened to.
    pub fn peek(&self, key: &str) -> Option<PathBuf> {
        let p = self.stream_path(key);
        p.is_file().then_some(p)
    }

    /// A new entry for `key`, written as the song loads; none when the file cannot be made, or while
    /// another writer has it (the player loading the song the precacher is fetching, or the other way
    /// round): the one there first keeps it.
    pub fn writer(self: &Arc<Self>, key: &str) -> Option<Writer> {
        if !self.held.lock().writing.insert(key.to_string()) {
            return None;
        }
        let mut part = self.stream_path(key).into_os_string();
        part.push(PART);
        let part = PathBuf::from(part);
        let Ok(file) = File::create(&part) else {
            self.held.lock().writing.remove(key);
            return None;
        };
        Some(Writer { store: self.clone(), key: key.to_string(), part, file: Some(file), at: 0 })
    }

    /// Whether an entry for `key` is being written now.
    pub fn writing(&self, key: &str) -> bool {
        self.held.lock().writing.contains(key)
    }

    /// Fetches `songs` (address and cache key, in order) whole into the stream cache ahead of their
    /// turn, one after another through `bytes`, each in one go; see [`crate::ahead`]. Called again, the
    /// new list replaces the old one; an empty one stops the fetching.
    pub fn fetch_ahead(self: &Arc<Self>, bytes: Arc<dyn crate::source::ByteSource>, songs: Vec<(String, String)>) {
        self.ahead.ask(self, bytes, songs);
    }

    /// Whether songs are being fetched ahead now: for a test to wait until they are.
    pub fn fetching_ahead(&self) -> bool {
        self.ahead.busy()
    }

    /// The stream cache's limit from now on, and whatever is over it dropped.
    pub fn set_limit(&self, limit: u64) {
        self.held.lock().limit = limit;
        self.trim(0);
    }

    /// Drops the streamed copies `keys` (a song that is now downloaded is the same bytes twice).
    pub fn drop_cached(&self, keys: &[String]) {
        for k in keys {
            self.remove(k);
        }
    }

    /// Empties the stream cache; downloads stay.
    pub fn clear_cache(&self) {
        for (k, _) in self.entries() {
            self.remove(&k);
        }
        self.order.clear();
        self.held.lock().bytes = Some(0);
    }

    /// Bytes the stream cache holds.
    pub fn cache_bytes(&self) -> u64 {
        self.counted()
    }

    fn entries(&self) -> Vec<(String, u64)> {
        let Ok(dir) = fs::read_dir(self.dir.join(STREAM)) else { return Vec::new() };
        dir.filter_map(|e| {
            let e = e.ok()?;
            let key = key_of(e.file_name().to_str()?)?;
            Some((key, e.metadata().ok()?.len()))
        })
        .collect()
    }

    /// The cache's size, counted from the disk the first time it is asked (and the order told what an
    /// earlier run left), kept from then on.
    fn counted(&self) -> u64 {
        if let Some(b) = self.held.lock().bytes {
            return b;
        }
        let entries = self.entries();
        let keys: Vec<String> = entries.iter().map(|e| e.0.clone()).collect();
        self.order.seed(&keys);
        let total = entries.iter().map(|e| e.1).sum();
        let mut h = self.held.lock();
        *h.bytes.get_or_insert(total)
    }

    fn remove(&self, key: &str) {
        let p = self.stream_path(key);
        let Ok(len) = fs::metadata(&p).map(|m| m.len()) else { return };
        if fs::remove_file(&p).is_ok() {
            if let Some(b) = self.held.lock().bytes.as_mut() {
                *b = b.saturating_sub(len);
            }
        }
    }

    /// `added` bytes came in: whole songs go, in the order's order, until the cache fits again.
    fn trim(&self, added: u64) {
        let total = self.counted();
        let limit = {
            let mut h = self.held.lock();
            let b = h.bytes.get_or_insert(total);
            *b += added;
            if *b <= h.limit {
                return;
            }
            h.limit
        };
        while self.held.lock().bytes.is_some_and(|b| b > limit) {
            let Some(key) = self.order.next() else { return };
            self.remove(&key);
        }
    }

    /// The whole of `key` is written: it is an entry now.
    fn finished(&self, key: &str, part: &Path, len: u64) -> bool {
        // Counted before it joins, so it is counted once.
        self.counted();
        if fs::rename(part, self.stream_path(key)).is_err() {
            let _ = fs::remove_file(part);
            return false;
        }
        self.order.touch(key);
        self.trim(len);
        for told in self.whole.lock().iter() {
            told();
        }
        true
    }
}

/// A stream cache entry being written as its song loads. Only bytes that follow on from what it holds
/// are taken; a gap (the listener seeked past what was loaded) ends it, and it is let go unfinished.
pub struct Writer {
    store: Arc<Store>,
    key: String,
    part: PathBuf,
    file: Option<File>,
    at: u64,
}

impl Writer {
    /// Bytes `from..` of the song; false once the entry was given up.
    pub fn write(&mut self, from: u64, bytes: &[u8]) -> bool {
        let Some(f) = self.file.as_mut() else { return false };
        if from != self.at || f.write_all(bytes).is_err() {
            self.file = None;
            return false;
        }
        self.at += bytes.len() as u64;
        true
    }

    /// The song ended at `len` bytes: kept, when all of it was written.
    pub fn finish(mut self, len: u64) -> bool {
        let Some(mut f) = self.file.take() else { return false };
        if self.at != len || f.flush().is_err() || f.seek(SeekFrom::End(0)).map_or(true, |e| e != len) {
            return false;
        }
        drop(f);
        let part = std::mem::take(&mut self.part);
        self.store.finished(&self.key, &part, len)
    }

    /// Where it has got to.
    pub fn written(&self) -> u64 {
        self.at
    }
}

impl Drop for Writer {
    fn drop(&mut self) {
        if !self.part.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.part);
        }
        self.store.held.lock().writing.remove(&self.key);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A few kilobytes each, in a directory of the test's own, gone when the test is.
    fn dir(name: &str) -> nori_testdir::TempDir {
        nori_testdir::TempDir::new(name)
    }

    fn put(s: &Arc<Store>, key: &str, len: usize) {
        let mut w = s.writer(key).unwrap();
        assert!(w.write(0, &vec![7u8; len]));
        assert!(w.finish(len as u64));
    }

    #[test]
    fn keys_become_file_names_and_back() {
        for k in ["a1:0", "x:192opus", "dl:s/1", ".hidden", "é"] {
            assert_eq!(key_of(&file_name(k)).as_deref(), Some(k));
        }
        assert!(!file_name("../up").contains('/'));
        assert_eq!(key_of("a%3A0.part"), None, "half written is not an entry");
    }

    #[test]
    fn a_song_is_an_entry_only_once_all_of_it_is_written() {
        let d = dir("store-whole");
        let s = Store::open(d.path(), 1 << 20, Box::new(Recent::default())).unwrap();
        let mut w = s.writer("a:0").unwrap();
        assert!(w.write(0, &[1; 100]));
        assert!(s.cached("a:0").is_none(), "not while it loads");
        assert!(!w.write(150, &[1; 10]), "a gap gives it up");
        assert!(!w.finish(110));
        assert!(s.cached("a:0").is_none());
        put(&s, "a:0", 100);
        assert_eq!(fs::read(s.cached("a:0").unwrap()).unwrap().len(), 100);
        let w = s.writer("b:0").unwrap();
        assert!(s.writer("b:0").is_none() && s.writing("b:0"), "one writer at a time: a second would truncate the first's file");
        drop(w);
        assert!(!s.writing("b:0"));
        assert!(fs::read_dir(s.dir.join(STREAM)).unwrap().count() == 1, "an entry let go leaves nothing behind");
    }

    #[test]
    fn over_its_limit_the_cache_lets_the_least_recently_used_go_and_what_an_earlier_run_left_first() {
        let d = dir("store-limit");
        let s = Store::open(d.path(), 1000, Box::new(Recent::default())).unwrap();
        put(&s, "old:0", 300);
        drop(s);
        let s = Store::open(d.path(), 1000, Box::new(Recent::default())).unwrap();
        put(&s, "a:0", 300);
        put(&s, "b:0", 300);
        put(&s, "c:0", 300);
        assert!(s.cached("old:0").is_none(), "what this run never used went first");
        assert!(s.cached("b:0").is_some() && s.cached("c:0").is_some());
        put(&s, "d:0", 300);
        assert!(s.cached("a:0").is_none(), "then a, the least recently used");
        assert!(s.cached("b:0").is_some() && s.cached("c:0").is_some() && s.cached("d:0").is_some());
        s.set_limit(400);
        assert_eq!(s.cache_bytes(), 300);
        assert!(s.cached("d:0").is_some(), "the latest used stays");
        s.clear_cache();
        assert_eq!(s.cache_bytes(), 0);
    }
}
