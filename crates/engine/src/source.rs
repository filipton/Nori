//! Bytes of a song over the network, fetched in bursts. A client hands in one door, [`ByteSource`]:
//! open a URL from a byte offset and read. A loader thread per song fills a window ahead of where the
//! demuxer reads - up to the core's `load_control` high mark in one go - then closes the connection and
//! sleeps until the demuxer has come within the low mark of the end of what is there. A song that fits
//! the memory cap (most do) is fetched whole in its first burst, so the radio wakes once per song. The
//! cap is one budget for the songs kept: a song fetched ahead holds what the one playing leaves of it
//! (`Loader::limit`), and the rest in a burst of its own once it plays.
//! Given a stream cache entry to fill, the loader writes each burst into it as it comes; a song heard
//! again then plays from the disk and the network is not asked at all.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::thread::Thread;
use std::time::Duration;

use parking_lot::{Condvar, Mutex};
use symphonia::core::io::MediaSource;

use crate::store::Writer;

/// A response body being read: where in the resource it starts (the offset asked for, or nought when
/// the server would not do ranges), the whole resource's length when known, and the bytes.
pub struct Body {
    pub start: u64,
    pub len: Option<u64>,
    pub reader: Box<dyn Read + Send>,
}

/// The client's HTTP client, for audio: a GET of `url` from byte `from` on (a `Range` request).
/// Blocking is fine: it runs on the loader's own thread.
pub trait ByteSource: Send + Sync {
    fn open(&self, url: &str, from: u64) -> Result<Body, String>;
}

/// How much to keep loaded, as `nori_player::transport::load_control` gives it: fill up to `high`
/// bytes ahead of the reader, start again below `low`, and never hold more than `cap` in memory.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub low: u64,
    pub high: u64,
    pub cap: u64,
}

/// A bitrate to size the window by when the song's is not known: 320 kbps, so the window errs on
/// the side of fetching more.
const BYTES_PER_MS_GUESS: u64 = 40;
/// Read in pieces this big.
const CHUNK: usize = 64 * 1024;
/// A reader this far past what is loaded has seeked: the fetch starts again there.
const FAR: u64 = 1024 * 1024;
/// A song's bytes are ready to be read without waiting when this much is there (or all of it).
pub(crate) const READY: u64 = 256 * 1024;
/// A dropped connection is tried again this many times before the song counts as failed.
const RETRIES: u32 = 3;
/// How long the demuxer waits for bytes before it gives up on the song.
const READ_TIMEOUT: Duration = Duration::from_secs(30);

impl Window {
    /// The window for a song of `duration_ms` and `len` bytes (either may be unknown), from
    /// `load_control` (min buffer ms, max buffer ms, .., .., byte cap).
    pub fn for_song(load: [i64; 5], duration_ms: Option<i64>, len: Option<u64>) -> Window {
        let per_ms = match (duration_ms, len) {
            (Some(d), Some(l)) if d > 0 => (l / d as u64).max(1),
            _ => BYTES_PER_MS_GUESS,
        };
        let cap = load[4].max(1) as u64;
        Window { low: (load[0] as u64 * per_ms).min(cap / 2), high: (load[1] as u64 * per_ms).min(cap), cap }
    }
}

#[derive(Default)]
struct State {
    /// Bytes `base..base + data.len()` of the resource.
    base: u64,
    data: Vec<u8>,
    len: Option<u64>,
    /// Loaded to the end of the resource.
    done: bool,
    error: Option<String>,
    /// Where the demuxer reads.
    reader_at: u64,
    /// The demuxer wants bytes from here, outside what is kept.
    restart: Option<u64>,
    closed: bool,
    /// The engine is waiting for bytes, and is woken when they are there.
    waiter: Option<Thread>,
    /// The demuxer is blocked in a read.
    blocked: bool,
    /// Times the network was opened: one per burst.
    bursts: u32,
    window: Option<Window>,
    /// At most this many bytes held, below the window's own cap: a song fetched ahead gets what the
    /// one playing leaves of the cap (`Loader::limit`).
    budget: Option<u64>,
}

impl State {
    /// The window for the song, as its length and bitrate size it, within the budget.
    fn window(&self, load: [i64; 5], duration_ms: Option<i64>) -> Window {
        let w = self.window.unwrap_or_else(|| Window::for_song(load, duration_ms, self.len));
        match self.budget {
            Some(b) if b < w.cap => Window { low: w.low.min(b / 2), high: w.high.min(b), cap: b },
            _ => w,
        }
    }

    fn end(&self) -> u64 {
        self.base + self.data.len() as u64
    }

    fn ahead(&self) -> u64 {
        self.end().saturating_sub(self.reader_at)
    }

    fn at_end(&self) -> bool {
        self.done || self.len.is_some_and(|l| self.end() >= l)
    }

    fn ready(&self) -> bool {
        self.at_end() || self.error.is_some() || self.ahead() >= READY
    }
}

/// One song's bytes, shared by the loader thread and the song's readers.
struct Loaded {
    state: Mutex<State>,
    cv: Condvar,
}

/// A song being loaded. The library and the song's readers share it; when the last of them lets go
/// the loader stops and its memory goes.
pub struct Loader(Arc<Loaded>);

impl Loader {
    /// Starts loading `url` through `source` on a thread of its own, sized by `load` and the song's
    /// tagged length, writing what it fetches into `keep` when given one.
    pub fn start(source: Arc<dyn ByteSource>, url: String, load: [i64; 5], duration_ms: Option<i64>, keep: Option<Writer>) -> Arc<Loader> {
        Loader::start_within(source, url, load, duration_ms, keep, None)
    }

    /// [`Loader::start`], holding no more than `budget` bytes from its first burst on (`Loader::limit`).
    pub fn start_within(source: Arc<dyn ByteSource>, url: String, load: [i64; 5], duration_ms: Option<i64>, keep: Option<Writer>, budget: Option<u64>) -> Arc<Loader> {
        let loaded = Arc::new(Loaded { state: Mutex::new(State { budget, ..State::default() }), cv: Condvar::new() });
        let l = loaded.clone();
        std::thread::Builder::new()
            .name("nori-load".into())
            .spawn(move || l.run(&*source, &url, load, duration_ms, keep))
            .expect("a thread for loading");
        Arc::new(Loader(loaded))
    }

    /// Times the network was opened for this song: one per burst.
    pub fn bursts(&self) -> u32 {
        self.0.state.lock().bursts
    }

    /// Bytes held in memory.
    pub fn held(&self) -> usize {
        self.0.state.lock().data.len()
    }

    /// Bytes it will hold once its burst is in: the rest of the song from where it keeps it, or what is
    /// here while the length is not known yet; never more than its window's cap.
    pub fn holding(&self) -> u64 {
        let s = self.0.state.lock();
        let cap = s.window.map_or(u64::MAX, |w| w.cap).min(s.budget.unwrap_or(u64::MAX));
        s.len.map_or(s.data.len() as u64, |l| l.saturating_sub(s.base)).min(cap)
    }

    /// Holds at most `bytes` from now on, or as much as its window lets it with none. A song fetched
    /// ahead is limited to what the one playing leaves of the cap, as one player's buffer would be; it
    /// is let have all of it once it is opened to be played, and fetches the rest in its next burst.
    pub fn limit(&self, bytes: Option<u64>) {
        let mut s = self.0.state.lock();
        if s.budget != bytes {
            s.budget = bytes;
            self.0.cv.notify_all();
        }
    }

    /// The whole song is in memory: nothing read from it can wait.
    pub fn complete(&self) -> bool {
        let s = self.0.state.lock();
        s.base == 0 && s.at_end() && s.error.is_none()
    }

    /// Waits, on the caller's own thread, until the whole song is in memory. False when it never will
    /// be: its connection failed, or it is larger than one burst fetches.
    pub(crate) fn wait_whole(&self) -> bool {
        let l = &*self.0;
        let mut s = l.state.lock();
        loop {
            if s.error.is_some() || s.closed {
                return false;
            }
            if s.base == 0 && s.at_end() {
                return true;
            }
            if s.window.is_some_and(|w| s.len.is_some_and(|len| len > w.high)) {
                return false;
            }
            s.blocked = true;
            l.cv.wait_for(&mut s, Duration::from_millis(200));
            s.blocked = false;
        }
    }

    /// Why the connection gave up for good, once it has.
    pub fn error(&self) -> Option<String> {
        self.0.state.lock().error.clone()
    }

    /// Whether the next read can be answered at once; if not, `engine` is woken when it can.
    pub(crate) fn ready_or_wake(&self, engine: &Thread) -> bool {
        let mut s = self.0.state.lock();
        if s.ready() {
            return true;
        }
        s.waiter = Some(engine.clone());
        false
    }

    pub fn reader(self: &Arc<Self>) -> LoadedReader {
        LoadedReader { loader: self.clone(), pos: 0 }
    }
}

impl Drop for Loader {
    fn drop(&mut self) {
        self.0.state.lock().closed = true;
        self.0.cv.notify_all();
    }
}

impl Loaded {
    fn run(&self, source: &dyn ByteSource, url: &str, load: [i64; 5], duration_ms: Option<i64>, mut keep: Option<Writer>) {
        let mut body: Option<Box<dyn Read + Send>> = None;
        let mut chunk = vec![0u8; CHUNK];
        let mut failures = 0;
        loop {
            let from = {
                let mut s = self.state.lock();
                loop {
                    if s.closed {
                        return;
                    }
                    if let Some(at) = s.restart.take() {
                        body = None;
                        s.base = at;
                        s.data.clear();
                        s.done = false;
                        s.error = None;
                        // A jump past what was loaded leaves a gap the cache entry cannot have.
                        keep = None;
                    }
                    let w = s.window(load, duration_ms);
                    if s.at_end() {
                        // The whole song is here: the connection goes, the network sleeps, and the cache
                        // has it for next time.
                        body = None;
                        s.done = true;
                        if let (Some(k), Some(len), None) = (keep.take(), s.len, &s.error) {
                            k.finish(len);
                        }
                        self.wake(&mut s);
                    } else if body.is_some() && s.ahead() < w.high {
                        break;
                    } else if body.is_none() && s.ahead() < w.low.max(1) {
                        break;
                    } else if body.is_some() {
                        // Up to the high mark: close the connection and leave the network alone until
                        // the reader comes within the low mark.
                        body = None;
                    }
                    self.cv.wait(&mut s);
                }
                // What the reader has left behind goes, once more than the cap is held.
                let w = s.window(load, duration_ms);
                let keep_from = s.reader_at.saturating_sub(FAR);
                if s.data.len() as u64 > w.cap && keep_from > s.base {
                    let drop = (keep_from - s.base) as usize;
                    s.data.drain(..drop);
                    s.base = keep_from;
                }
                s.end()
            };
            if body.is_none() {
                match source.open(url, from) {
                    Ok(b) => {
                        let mut reader = b.reader;
                        // A server that would not do ranges sends the whole thing: skip to where we are.
                        if b.start < from {
                            let _ = io::copy(&mut (&mut reader).take(from - b.start), &mut io::sink());
                        }
                        let mut s = self.state.lock();
                        s.len = s.len.or(b.len);
                        s.window = Some(Window::for_song(load, duration_ms, s.len));
                        s.bursts += 1;
                        // The burst's bytes in one piece, made once: grown by doubling, a song's memory
                        // was copied at every step and held up to twice its size.
                        if let Some(len) = s.len {
                            let want = len.saturating_sub(s.base).min(s.window(load, duration_ms).high) as usize;
                            let more = want.saturating_sub(s.data.len());
                            s.data.reserve_exact(more);
                        }
                        body = Some(reader);
                    }
                    Err(e) => {
                        failures += 1;
                        if failures > RETRIES {
                            let mut s = self.state.lock();
                            s.error = Some(e);
                            s.done = true;
                            self.wake(&mut s);
                            continue;
                        }
                        std::thread::sleep(Duration::from_millis(500 << failures));
                        continue;
                    }
                }
            }
            let got = body.as_mut().expect("a body is open").read(&mut chunk);
            let mut s = self.state.lock();
            let mut took = 0;
            match got {
                Ok(0) => {
                    body = None;
                    s.done = true;
                    let end = s.end();
                    s.len = Some(s.len.unwrap_or(end));
                }
                Ok(n) => {
                    failures = 0;
                    if s.end() == from && s.restart.is_none() {
                        s.data.extend_from_slice(&chunk[..n]);
                        took = n;
                    }
                }
                Err(_) => body = None,
            }
            self.wake(&mut s);
            drop(s);
            // Written with the lock let go: the demuxer reads on meanwhile.
            if took > 0 && keep.as_mut().is_some_and(|k| !k.write(from, &chunk[..took])) {
                keep = None;
            }
        }
    }

    /// Bytes arrived: a blocked reader reads on, and a waiting engine is told once there is enough.
    fn wake(&self, s: &mut State) {
        if s.blocked {
            self.cv.notify_all();
        }
        if s.ready() {
            if let Some(t) = s.waiter.take() {
                t.unpark();
            }
        }
    }
}

/// A song's bytes as a seekable stream for the demuxer: reads block until the bytes are there.
pub struct LoadedReader {
    loader: Arc<Loader>,
    pos: u64,
}

impl Read for LoadedReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let l = &*self.loader.0;
        let mut s = l.state.lock();
        loop {
            let end = s.end();
            if self.pos >= s.base && self.pos < end {
                let from = (self.pos - s.base) as usize;
                let n = buf.len().min(s.data.len() - from);
                buf[..n].copy_from_slice(&s.data[from..from + n]);
                self.pos += n as u64;
                s.reader_at = self.pos;
                // Within the low mark of the end of what is here: the loader's next burst is due.
                if !s.at_end() && s.window.is_some_and(|w| s.ahead() < w.low) {
                    l.cv.notify_all();
                }
                return Ok(n);
            }
            if s.at_end() && self.pos >= end && s.restart.is_none() {
                if let Some(e) = &s.error {
                    return Err(io::Error::other(e.clone()));
                }
                return Ok(0);
            }
            if self.pos < s.base || self.pos > end + FAR {
                s.restart = Some(self.pos);
            }
            s.reader_at = self.pos;
            s.blocked = true;
            l.cv.notify_all();
            let timed_out = l.cv.wait_for(&mut s, READ_TIMEOUT).timed_out();
            s.blocked = false;
            if timed_out {
                return Err(io::Error::new(io::ErrorKind::TimedOut, "the song's bytes did not come"));
            }
        }
    }
}

impl Seek for LoadedReader {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let len = self.loader.0.state.lock().len;
        self.pos = match to {
            SeekFrom::Start(p) => p,
            SeekFrom::Current(d) => self.pos.checked_add_signed(d).ok_or_else(|| io::Error::other("seek before the start"))?,
            SeekFrom::End(d) => len.ok_or_else(|| io::Error::other("length unknown"))?.checked_add_signed(d).ok_or_else(|| io::Error::other("seek before the start"))?,
        };
        Ok(self.pos)
    }
}

impl MediaSource for LoadedReader {
    fn is_seekable(&self) -> bool {
        self.loader.0.state.lock().len.is_some()
    }

    fn byte_len(&self) -> Option<u64> {
        self.loader.0.state.lock().len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    /// A server that makes up `len` bytes (byte `i` is `i as u8`) and counts what it is asked for.
    struct Counting {
        len: u64,
        opens: Mutex<Vec<u64>>,
        served: Arc<AtomicU64>,
    }

    struct Made {
        at: u64,
        len: u64,
        served: Arc<AtomicU64>,
    }

    impl Read for Made {
        fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
            let n = buf.len().min((self.len - self.at) as usize);
            for (k, b) in buf[..n].iter_mut().enumerate() {
                *b = (self.at + k as u64) as u8;
            }
            self.at += n as u64;
            self.served.fetch_add(n as u64, Ordering::Relaxed);
            Ok(n)
        }
    }

    impl ByteSource for Counting {
        fn open(&self, _: &str, from: u64) -> Result<Body, String> {
            self.opens.lock().push(from);
            Ok(Body { start: from, len: Some(self.len), reader: Box::new(Made { at: from, len: self.len, served: self.served.clone() }) })
        }
    }

    fn server(len: u64) -> Arc<Counting> {
        Arc::new(Counting { len, opens: Mutex::new(Vec::new()), served: Arc::new(AtomicU64::new(0)) })
    }

    /// Waits until the loader has stopped fetching: nothing served for a while.
    fn settled(s: &Counting) -> u64 {
        let until = Instant::now() + Duration::from_secs(10);
        let mut last = u64::MAX;
        while Instant::now() < until {
            let now = s.served.load(Ordering::Relaxed);
            if now == last {
                return now;
            }
            last = now;
            std::thread::sleep(Duration::from_millis(50));
        }
        panic!("the loader never rested");
    }

    fn read(r: &mut LoadedReader, n: usize) -> Vec<u8> {
        let mut out = vec![0u8; n];
        r.read_exact(&mut out).unwrap();
        out
    }

    /// A million bytes for ten seconds of music: a hundred bytes a millisecond, so a window of one
    /// to four seconds is 100 kB to 400 kB.
    const LOAD: [i64; 5] = [1_000, 4_000, 0, 0, 1 << 30];

    #[test]
    fn it_fetches_up_to_the_high_mark_then_leaves_the_network_alone_until_the_low_mark() {
        let s = server(1_000_000);
        let l = Loader::start(s.clone(), "song".into(), LOAD, Some(10_000), None);
        let mut r = l.reader();
        let first = settled(&s);
        assert!((400_000..400_000 + CHUNK as u64).contains(&first), "one burst to the high mark: {first}");
        assert_eq!(*s.opens.lock(), vec![0]);
        // Playing on with more than the low mark still ahead: not a byte fetched.
        let got = read(&mut r, 250_000);
        assert!(got.iter().enumerate().all(|(i, &b)| b == i as u8), "the bytes are the song's");
        std::thread::sleep(Duration::from_millis(200));
        assert_eq!(s.served.load(Ordering::Relaxed), first, "the network sleeps between bursts");
        assert_eq!(s.opens.lock().len(), 1);
        // Within the low mark of the end of what is there: the next burst, from where the last stopped.
        let at = first as usize - 90_000;
        read(&mut r, at - 250_000);
        let second = settled(&s);
        assert_eq!(s.opens.lock().clone(), vec![0, first], "a second request, picking up where the first stopped");
        assert!(second - first >= 300_000, "a whole burst, not a top-up: {}", second - first);
        let rest = read(&mut r, 1_000_000 - at);
        assert!(rest.iter().enumerate().all(|(i, &b)| b == (at + i) as u8));
        assert_eq!(r.read(&mut [0u8; 16]).unwrap(), 0, "and then the end");
        assert_eq!(l.bursts() as usize, s.opens.lock().len());
    }

    #[test]
    fn a_song_that_fits_the_window_is_fetched_whole_in_one_request() {
        let s = server(300_000);
        let l = Loader::start(s.clone(), "song".into(), LOAD, Some(3_000), None);
        assert_eq!(settled(&s), 300_000);
        let mut r = l.reader();
        let all = read(&mut r, 300_000);
        assert!(all.iter().enumerate().all(|(i, &b)| b == i as u8));
        assert_eq!(*s.opens.lock(), vec![0]);
    }

    #[test]
    fn a_song_fetched_ahead_holds_its_budget_and_the_rest_once_it_plays() {
        let s = server(600_000);
        let l = Loader::start_within(s.clone(), "song".into(), LOAD, Some(6_000), None, Some(200_000));
        let ahead = settled(&s);
        assert!(ahead <= 200_000 + CHUNK as u64, "no more than its budget while it waits: {ahead}");
        assert!(l.held() >= 100_000, "but its start is at hand: {}", l.held());
        assert_eq!(l.holding(), 200_000);
        // Played now: the whole cap. The rest comes when the reader nears the end of what is there.
        l.limit(None);
        let mut r = l.reader();
        let all = read(&mut r, 600_000);
        assert!(all.iter().enumerate().all(|(i, &b)| b == i as u8), "every byte, in order");
        assert_eq!(*s.opens.lock(), vec![0, ahead], "one more request, from where the budget stopped it");
    }

    #[test]
    fn a_song_s_bytes_are_held_in_one_piece_the_size_of_the_song() {
        let s = server(300_000);
        let l = Loader::start(s.clone(), "song".into(), LOAD, Some(3_000), None);
        settled(&s);
        assert_eq!(l.0.state.lock().data.capacity(), 300_000, "made once, not grown by doubling");
    }

    #[test]
    fn a_seek_past_what_is_loaded_fetches_from_there() {
        let s = server(5_000_000);
        let l = Loader::start(s.clone(), "song".into(), LOAD, Some(50_000), None);
        settled(&s);
        let mut r = l.reader();
        r.seek(SeekFrom::Start(4_000_000)).unwrap();
        let got = read(&mut r, 1000);
        assert!(got.iter().enumerate().all(|(i, &b)| b == (4_000_000 + i) as u8));
        assert_eq!(*s.opens.lock(), vec![0, 4_000_000], "not the megabytes in between");
    }
}
