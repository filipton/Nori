//! Songs fetched whole into the stream cache ahead of their turn, as Android's precacher fetches them
//! for the ExoPlayer path: which ones and how many is the core's (`Client::precache_targets`: the
//! user's count for the network the device is on, none on a metered one by default, never a provider's
//! song or one the downloads have), asked as a song starts ([`crate::Library::ahead`]), a moment the
//! network is awake for the song after it anyway. When their turn comes they play from the disk and the
//! network stays asleep.
//!
//! Burst-y, like everything that touches the network here: each song is fetched in one go, as fast as
//! it comes, one after another on a thread that lives only while there is something to fetch, and then
//! nothing until the next song starts. Never a trickle. The songs asked for replace what was asked
//! before: a song no longer wanted is left half way (its partial file goes), one still wanted carries on
//! where it is. A song whose entry someone else is writing (the player loading it) is left to them.

use std::collections::HashSet;
use std::io::Read;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::source::ByteSource;
use crate::store::Store;

/// How much is read at a time: the loader's own chunk.
const CHUNK: usize = 256 * 1024;

/// The fetching ahead, kept by the [`Store`] it fetches into.
#[derive(Default)]
pub(crate) struct Ahead {
    plan: Mutex<Plan>,
    /// Moves whenever the songs asked for change: read per chunk without the lock.
    asked: AtomicU64,
}

#[derive(Default)]
struct Plan {
    /// Address and cache key of each song, in order.
    songs: Vec<(String, String)>,
    bytes: Option<Arc<dyn ByteSource>>,
    /// Keys given up on for this list (would not open, broke off): not tried again until it changes.
    failed: HashSet<String>,
    running: bool,
}

impl Ahead {
    pub(crate) fn ask(&self, store: &Arc<Store>, bytes: Arc<dyn ByteSource>, songs: Vec<(String, String)>) {
        let mut plan = self.plan.lock();
        if plan.songs == songs {
            return;
        }
        plan.songs = songs;
        plan.bytes = Some(bytes);
        plan.failed.clear();
        self.asked.fetch_add(1, Ordering::AcqRel);
        if plan.running || plan.songs.is_empty() {
            return;
        }
        plan.running = true;
        drop(plan);
        let store = store.clone();
        if std::thread::Builder::new().name("nori-precache".into()).spawn(move || store.ahead.run(&store)).is_err() {
            self.plan.lock().running = false;
        }
    }

    pub(crate) fn busy(&self) -> bool {
        self.plan.lock().running
    }

    /// The next song to fetch: the first asked for that is not on the disk, being written by someone
    /// else, or given up on. None ends the thread.
    fn next(&self, store: &Store) -> Option<(String, String, Arc<dyn ByteSource>)> {
        let mut plan = self.plan.lock();
        let found = plan.songs.iter().find(|(_, key)| !plan.failed.contains(key) && store.peek(key).is_none() && !store.writing(key)).cloned();
        match (found, plan.bytes.clone()) {
            (Some((url, key)), Some(bytes)) => Some((url, key, bytes)),
            _ => {
                plan.running = false;
                None
            }
        }
    }

    fn wanted(&self, key: &str) -> bool {
        self.plan.lock().songs.iter().any(|(_, k)| k == key)
    }

    fn run(&self, store: &Arc<Store>) {
        while let Some((url, key, bytes)) = self.next(store) {
            // Left half way because it is no longer wanted, it is not asked for any more either.
            if self.fetch(store, &*bytes, &url, &key) != Some(true) {
                self.plan.lock().failed.insert(key);
            }
        }
    }

    /// `key` fetched whole into the store: whether it was kept, or None when it stopped being wanted.
    fn fetch(&self, store: &Arc<Store>, bytes: &dyn ByteSource, url: &str, key: &str) -> Option<bool> {
        let Some(mut writer) = store.writer(key) else { return Some(false) };
        let Ok(body) = bytes.open(url, 0) else { return Some(false) };
        if body.start != 0 {
            return Some(false);
        }
        let mut reader = body.reader;
        let mut chunk = vec![0u8; CHUNK];
        let mut asked = self.asked.load(Ordering::Acquire);
        loop {
            let now = self.asked.load(Ordering::Acquire);
            if now != asked {
                if !self.wanted(key) {
                    return None;
                }
                asked = now;
            }
            match reader.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => {
                    if !writer.write(writer.written(), &chunk[..n]) {
                        return Some(false);
                    }
                }
                Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                Err(_) => return Some(false),
            }
        }
        let len = writer.written();
        if body.len.is_some_and(|l| l != len) {
            return Some(false);
        }
        Some(writer.finish(len))
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::sync::mpsc::{channel, Receiver, Sender};
    use std::time::{Duration, Instant};

    use super::*;
    use crate::source::Body;
    use crate::store::Recent;

    const LEN: usize = 600_000;

    /// Every song is `LEN` bytes; each request is counted; a song named in `held` stops after its first
    /// chunk until the test lets it go on.
    #[derive(Default)]
    struct Net {
        asked: Mutex<Vec<String>>,
        held: Mutex<Option<(String, Receiver<()>)>>,
    }

    struct Held(Cursor<Vec<u8>>, Option<Receiver<()>>);

    impl Read for Held {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            if self.0.position() > 0 {
                if let Some(go) = self.1.take() {
                    let _ = go.recv();
                }
            }
            let n = buf.len().min(CHUNK);
            self.0.read(&mut buf[..n])
        }
    }

    impl ByteSource for Net {
        fn open(&self, url: &str, from: u64) -> Result<Body, String> {
            self.asked.lock().push(url.to_string());
            let gate = self.held.lock().take_if(|(u, _)| u == url).map(|(_, r)| r);
            let mut c = Cursor::new(vec![3u8; LEN]);
            c.set_position(from);
            Ok(Body { start: from, len: Some(LEN as u64), reader: Box::new(Held(c, gate)) })
        }
    }

    /// A store in a directory of the test's own, gone with the guard.
    fn store(name: &str) -> (nori_testdir::TempDir, Arc<Store>) {
        let d = nori_testdir::TempDir::new(&format!("ahead-{name}"));
        let s = Store::open(d.path(), 64 << 20, Box::new(Recent::default())).unwrap();
        (d, s)
    }

    fn settle(s: &Store) {
        let until = Instant::now() + Duration::from_secs(30);
        while s.fetching_ahead() {
            assert!(Instant::now() < until, "the fetching ends");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    fn songs(ids: &[&str]) -> Vec<(String, String)> {
        ids.iter().map(|id| (format!("http://m/{id}"), format!("{id}:0"))).collect()
    }

    #[test]
    fn the_songs_asked_for_are_fetched_whole_once_each_and_one_being_written_is_left_to_its_writer() {
        let (_dir, s) = store("whole");
        let net = Arc::new(Net::default());
        let mut w = s.writer("c:0").unwrap();
        assert!(w.write(0, &[1; 10]));
        s.fetch_ahead(net.clone(), songs(&["a", "b", "c"]));
        settle(&s);
        assert_eq!(std::fs::metadata(s.peek("a:0").unwrap()).unwrap().len(), LEN as u64, "whole");
        assert!(s.peek("b:0").is_some());
        assert!(s.peek("c:0").is_none(), "the player loading c keeps it");
        assert_eq!(*net.asked.lock(), ["http://m/a", "http://m/b"], "one request a song: one burst each");
        drop(w);
        // The same songs asked again (every song start asks): nothing more.
        s.fetch_ahead(net.clone(), songs(&["a", "b", "c"]));
        settle(&s);
        assert_eq!(net.asked.lock().len(), 2);
        // The next song start: what is on the disk is not fetched again.
        s.fetch_ahead(net.clone(), songs(&["b", "c", "d"]));
        settle(&s);
        assert_eq!(net.asked.lock()[2..], ["http://m/c", "http://m/d"]);
    }

    #[test]
    fn a_song_no_longer_wanted_is_left_half_way_and_one_still_wanted_goes_on() {
        let (_dir, s) = store("moved");
        let net = Arc::new(Net::default());
        let (go, wait): (Sender<()>, Receiver<()>) = channel();
        *net.held.lock() = Some(("http://m/a".into(), wait));
        s.fetch_ahead(net.clone(), songs(&["a", "b"]));
        let until = Instant::now() + Duration::from_secs(30);
        while !s.writing("a:0") {
            assert!(Instant::now() < until);
            std::thread::sleep(Duration::from_millis(5));
        }
        // The queue moved on: a is not wanted now.
        s.fetch_ahead(net.clone(), songs(&["b", "x"]));
        go.send(()).unwrap();
        settle(&s);
        assert!(s.peek("a:0").is_none() && !s.writing("a:0"), "left, its partial file gone");
        assert!(s.peek("b:0").is_some() && s.peek("x:0").is_some());

        // Still wanted after the change: it carries on, fetched once.
        let (go, wait) = channel();
        *net.held.lock() = Some(("http://m/y".into(), wait));
        s.fetch_ahead(net.clone(), songs(&["y", "z"]));
        while !s.writing("y:0") {
            std::thread::sleep(Duration::from_millis(5));
        }
        s.fetch_ahead(net.clone(), songs(&["y"]));
        go.send(()).unwrap();
        settle(&s);
        assert!(s.peek("y:0").is_some() && s.peek("z:0").is_none());
        assert_eq!(net.asked.lock().iter().filter(|u| u.ends_with("/y")).count(), 1);
        // Nothing asked for: nothing runs.
        s.fetch_ahead(net.clone(), Vec::new());
        assert!(!s.fetching_ahead());
    }
}
