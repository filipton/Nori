//! Cover files on disk, in a directory the client names, under a size limit: the least recently used go
//! first. Each file is named by the hash of its address and holds the server's bytes as they came.
//!
//! The index is only in memory, rebuilt from the directory when it is opened: it is what is in the
//! directory, not state of the app's (so it stays out of the app's database), and a file's modification
//! time is its last use, set again on every read, so the order survives a restart.
//!
//! What changes the directory (a file renamed into place, a file deleted) is done under the index's lock,
//! together with the index's own change, so the two cannot disagree: a trim or a remove cannot delete a
//! file a concurrent put has just put in place, nor a put's file be indexed at another put's size.
//! Writing a file and reading one are done outside it; they are the slow part.

use std::collections::{BTreeMap, HashMap};
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use md5::{Digest, Md5};
use parking_lot::Mutex;

/// A cover's address as a file's name: its MD5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Key(pub [u8; 16]);

impl Key {
    pub fn of(url: &str) -> Key {
        Key(Md5::digest(url.as_bytes()).into())
    }

    fn name(&self) -> [u8; 32] {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut s = [0u8; 32];
        for (i, b) in self.0.iter().enumerate() {
            s[2 * i] = HEX[(b >> 4) as usize];
            s[2 * i + 1] = HEX[(b & 15) as usize];
        }
        s
    }

    fn parse(name: &str) -> Option<Key> {
        let b = name.as_bytes();
        if b.len() != 32 {
            return None;
        }
        let mut k = [0u8; 16];
        for (i, pair) in b.chunks_exact(2).enumerate() {
            let hex = |c: u8| (c as char).to_digit(16).map(|d| d as u8);
            k[i] = hex(pair[0])? << 4 | hex(pair[1])?;
        }
        Some(Key(k))
    }
}

struct Entry {
    bytes: u64,
    used: u64,
    /// Which put wrote the file (the clock then): a read that fails forgets the file only if it is still
    /// the one it tried to read.
    written: u64,
}

/// Which files there are and in which order they were last used.
#[derive(Default)]
struct Index {
    files: HashMap<Key, Entry>,
    /// Last use (a counter, oldest first) to the file.
    order: BTreeMap<u64, Key>,
    bytes: u64,
    clock: u64,
}

impl Index {
    /// Counts a use of the file `key`; which put wrote it, or None when it is not kept.
    fn touch(&mut self, key: Key) -> Option<u64> {
        let e = self.files.get_mut(&key)?;
        self.order.remove(&e.used);
        self.clock += 1;
        e.used = self.clock;
        self.order.insert(self.clock, key);
        Some(e.written)
    }

    fn insert(&mut self, key: Key, bytes: u64) {
        self.remove(key);
        self.clock += 1;
        self.files.insert(key, Entry { bytes, used: self.clock, written: self.clock });
        self.order.insert(self.clock, key);
        self.bytes += bytes;
    }

    fn remove(&mut self, key: Key) -> bool {
        let Some(e) = self.files.remove(&key) else { return false };
        self.order.remove(&e.used);
        self.bytes -= e.bytes;
        true
    }

    /// The least recently used files to delete for the rest to fit `limit`.
    fn over(&mut self, limit: u64, out: &mut Vec<Key>) {
        while self.bytes > limit {
            let Some((_, key)) = self.order.pop_first() else { break };
            let e = self.files.remove(&key).expect("every file in the order is indexed");
            self.bytes -= e.bytes;
            out.push(key);
        }
    }
}

pub struct DiskCache {
    dir: PathBuf,
    limit: u64,
    index: Mutex<Index>,
}

/// Makes each writer's temporary file its own.
static WRITES: AtomicU64 = AtomicU64::new(0);

impl DiskCache {
    /// Opens (creating it if need be) the cache in `dir`, keeping at most `limit` bytes. Files left half
    /// written by a crash are deleted, and so is anything over the limit.
    pub fn open(dir: impl Into<PathBuf>, limit: u64) -> io::Result<DiskCache> {
        let dir = dir.into();
        fs::create_dir_all(&dir)?;
        let mut found: Vec<(SystemTime, Key, u64)> = Vec::new();
        for e in fs::read_dir(&dir)? {
            let e = e?;
            let name = e.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(meta) = e.metadata() else { continue };
            match Key::parse(name) {
                Some(key) if meta.is_file() => found.push((meta.modified().unwrap_or(UNIX_EPOCH), key, meta.len())),
                _ if name.ends_with(".tmp") => {
                    let _ = fs::remove_file(e.path());
                }
                _ => {}
            }
        }
        found.sort_unstable();
        let mut index = Index::default();
        for (_, key, bytes) in found {
            index.insert(key, bytes);
        }
        let cache = DiskCache { dir, limit, index: Mutex::new(Index::default()) };
        cache.trim(&mut index);
        *cache.index.lock() = index;
        Ok(cache)
    }

    /// Where the cover `key` is kept, whether or not it is there.
    pub fn path(&self, key: Key) -> PathBuf {
        let name = key.name();
        self.dir.join(std::str::from_utf8(&name).expect("hex is ASCII"))
    }

    pub fn contains(&self, key: Key) -> bool {
        self.index.lock().files.contains_key(&key)
    }

    /// Reads the cover `key` into `buf` (cleared first); false when it is not kept. A read counts as a
    /// use.
    pub fn read(&self, key: Key, buf: &mut Vec<u8>) -> bool {
        let Some(written) = self.index.lock().touch(key) else { return false };
        buf.clear();
        let path = self.path(key);
        let read = File::open(&path).and_then(|mut f| {
            f.read_to_end(buf)?;
            // One more syscall on a file already open, so that the order holds after a restart.
            let _ = f.set_modified(SystemTime::now());
            Ok(())
        });
        if read.is_err() {
            // Deleted behind the cache's back: forget it, unless a put has put a new one in its place
            // since.
            let mut index = self.index.lock();
            if index.files.get(&key).is_some_and(|e| e.written == written) {
                index.remove(key);
            }
            return false;
        }
        true
    }

    /// Keeps `bytes` as the cover `key`, then deletes the least recently used covers over the limit. A
    /// file larger than the whole limit is not kept at all.
    pub fn put(&self, key: Key, bytes: &[u8]) -> io::Result<()> {
        if bytes.len() as u64 > self.limit {
            return Ok(());
        }
        let path = self.path(key);
        let tmp = path.with_extension(format!("{}-{}.tmp", std::process::id(), WRITES.fetch_add(1, Ordering::Relaxed)));
        let written = File::create(&tmp).and_then(|mut f| f.write_all(bytes)).and_then(|_| {
            let mut index = self.index.lock();
            fs::rename(&tmp, &path)?;
            index.insert(key, bytes.len() as u64);
            self.trim(&mut index);
            Ok(())
        });
        if written.is_err() {
            let _ = fs::remove_file(&tmp);
        }
        written
    }

    pub fn remove(&self, key: Key) {
        let mut index = self.index.lock();
        if index.remove(key) {
            let _ = fs::remove_file(self.path(key));
        }
    }

    /// Deletes every cover, for a user who asked for the space back.
    pub fn clear(&self) {
        let mut index = self.index.lock();
        for key in std::mem::take(&mut *index).files.into_keys() {
            let _ = fs::remove_file(self.path(key));
        }
    }

    /// Bytes kept.
    pub fn bytes(&self) -> u64 {
        self.index.lock().bytes
    }

    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// Deletes what is over the limit, under the lock `index` is held by: a file or two after a put.
    fn trim(&self, index: &mut Index) {
        let mut gone = Vec::new();
        index.over(self.limit, &mut gone);
        for key in gone {
            let _ = fs::remove_file(self.path(key));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("nori-covers-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn a_key_is_its_file_name_and_back() {
        let k = Key::of("http://x/rest/getCoverArt?id=1&size=320");
        assert_eq!(Key::parse(std::str::from_utf8(&k.name()).unwrap()), Some(k));
        assert_eq!(Key::parse("nope"), None);
        assert_eq!(Key::parse("zz000000000000000000000000000000"), None);
    }

    #[test]
    fn the_least_recently_used_go_first_and_a_read_is_a_use() {
        let d = dir("lru");
        let c = DiskCache::open(&d, 30).unwrap();
        let (a, b, x) = (Key::of("a"), Key::of("b"), Key::of("c"));
        c.put(a, &[1; 10]).unwrap();
        c.put(b, &[2; 10]).unwrap();
        c.put(x, &[3; 10]).unwrap();
        let mut buf = Vec::new();
        assert!(c.read(a, &mut buf));
        assert_eq!(buf, [1; 10]);
        // Over the limit: b is the oldest now, a having been read.
        c.put(Key::of("d"), &[4; 10]).unwrap();
        assert!(!c.contains(b) && !c.path(b).exists());
        assert!(c.contains(a) && c.contains(x));
        assert_eq!(c.bytes(), 30);
        // Larger than the whole cache: not kept, nothing else lost.
        c.put(Key::of("e"), &[5; 31]).unwrap();
        assert!(!c.contains(Key::of("e")) && c.contains(a));
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn opening_again_finds_the_files_in_the_order_they_were_used_and_trims_to_a_new_limit() {
        let d = dir("reopen");
        {
            let c = DiskCache::open(&d, 100).unwrap();
            for (i, name) in ["a", "b", "c"].iter().enumerate() {
                c.put(Key::of(name), &[i as u8; 10]).unwrap();
                // Modification times a clear step apart, whatever the file system's resolution.
                File::options().write(true).open(c.path(Key::of(name))).unwrap().set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1000 + i as u64)).unwrap();
            }
            File::create(d.join("0123.5-1.tmp")).unwrap();
        }
        let c = DiskCache::open(&d, 20).unwrap();
        assert!(!c.contains(Key::of("a")) && c.contains(Key::of("b")) && c.contains(Key::of("c")));
        assert!(!d.join("0123.5-1.tmp").exists());
        assert_eq!(c.bytes(), 20);
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn a_file_deleted_behind_its_back_is_a_miss() {
        let d = dir("gone");
        let c = DiskCache::open(&d, 100).unwrap();
        c.put(Key::of("a"), &[1; 4]).unwrap();
        fs::remove_file(c.path(Key::of("a"))).unwrap();
        assert!(!c.read(Key::of("a"), &mut Vec::new()));
        assert_eq!(c.bytes(), 0);
        fs::remove_dir_all(&d).unwrap();
    }

    /// Whether the index and the directory say the same: every file kept is indexed at its size, and
    /// nothing indexed is missing.
    fn agree(c: &DiskCache) -> Result<(), String> {
        let index = c.index.lock();
        let mut on_disk = HashMap::new();
        for e in fs::read_dir(&c.dir).unwrap() {
            let e = e.unwrap();
            if let Some(key) = e.file_name().to_str().and_then(Key::parse) {
                on_disk.insert(key, e.metadata().unwrap().len());
            }
        }
        for (key, e) in &index.files {
            if on_disk.get(key) != Some(&e.bytes) {
                return Err(format!("{key:?} indexed at {} bytes, on disk {:?}", e.bytes, on_disk.get(key)));
            }
        }
        if on_disk.len() != index.files.len() {
            return Err(format!("{} files on disk, {} indexed", on_disk.len(), index.files.len()));
        }
        if index.bytes != on_disk.values().sum::<u64>() {
            return Err(format!("{} bytes indexed, {} on disk", index.bytes, on_disk.values().sum::<u64>()));
        }
        Ok(())
    }

    #[test]
    fn threads_putting_reading_and_trimming_the_same_covers_leave_the_index_and_the_directory_agreeing() {
        let d = dir("race");
        let c = std::sync::Arc::new(DiskCache::open(&d, 60).unwrap());
        let keys: Vec<Key> = (0..3).map(|i| Key::of(&i.to_string())).collect();
        // Short rounds, each checked once its threads are done: a race that leaves the two disagreeing
        // is mended by a later put of the same cover, so one long run would hide it.
        for round in 0..300 {
            let go = std::sync::Arc::new(std::sync::Barrier::new(4));
            let threads: Vec<_> = (0..4)
                .map(|t| {
                    let (c, keys, go) = (c.clone(), keys.clone(), go.clone());
                    std::thread::spawn(move || {
                        let mut buf = Vec::new();
                        go.wait();
                        for i in 0..40usize {
                            let key = keys[(i * 7 + t + round) % keys.len()];
                            match (i + t) % 5 {
                                0..=2 => c.put(key, &vec![t as u8; 10 + t * 10]).unwrap(),
                                3 => {
                                    c.read(key, &mut buf);
                                }
                                _ => c.remove(key),
                            }
                        }
                    })
                })
                .collect();
            for t in threads {
                t.join().unwrap();
            }
            if let Err(e) = agree(&c) {
                fs::remove_dir_all(&d).unwrap();
                panic!("round {round}: {e}");
            }
        }
        fs::remove_dir_all(&d).unwrap();
    }

    #[test]
    fn clearing_deletes_every_cover_and_the_cache_goes_on() {
        let d = dir("clear");
        let c = DiskCache::open(&d, 100).unwrap();
        c.put(Key::of("a"), &[1; 4]).unwrap();
        c.put(Key::of("b"), &[2; 4]).unwrap();
        c.clear();
        assert_eq!((c.bytes(), fs::read_dir(&d).unwrap().count()), (0, 0));
        c.put(Key::of("c"), &[3; 4]).unwrap();
        assert!(c.contains(Key::of("c")) && c.bytes() == 4);
        fs::remove_dir_all(&d).unwrap();
    }
}
