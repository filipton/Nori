//! Covers for a screen: asked for by address and size, answered from memory, the disk or the network, on
//! a few worker threads. Views asking for one cover at one size while it is on its way share the one
//! fetch and decode. A request whose views have all gone (a row scrolled away) is dropped before it
//! starts, or before its decode when the bytes are already coming: those are kept on disk all the same.
//!
//! The newest request is served first. In a list flung past a hundred covers, the ones on screen now
//! were asked for last, and the rows that flew by may be cancelled before a worker reaches them.
//!
//! Workers are started on the first requests, as many as are waited for up to the limit, and sleep on a
//! condition variable when there is nothing to do: an idle loader never wakes.

use std::collections::hash_map::Entry;
use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::path::PathBuf;
use std::pin::pin;
use std::sync::{mpsc, Arc};
use std::task::{Context, Poll, Wake, Waker};
use std::thread::{self, Thread};

use norimusic::covers::is_provider_cover;
use norimusic::transport::{FailureKind, Transport, TransportError};
use parking_lot::{Condvar, Mutex};

use crate::decode::{self, Decoder};
use crate::disk::{DiskCache, Key};
use crate::memory::{Image, MemoryCache, Sized};
use crate::scale::Alpha;

pub struct Config {
    /// Where covers are kept on disk; None keeps none.
    pub dir: Option<PathBuf>,
    pub disk_bytes: u64,
    /// Decoded covers kept in memory, in bytes.
    pub memory_bytes: usize,
    /// At most this many covers fetched and decoded at once.
    pub workers: usize,
    pub alpha: Alpha,
    /// A fetch gives up after this long; 0 is the transport's own timeouts.
    pub timeout_ms: u32,
}

impl Config {
    /// The core's limits (`cover_rules`) for the disk, 64 MB of decoded covers (about 180 at 300x300 and
    /// the player's at full size), and two to four workers: decoding is CPU work, and a fifth thread
    /// makes no cover on screen come sooner.
    pub fn new(dir: impl Into<PathBuf>) -> Config {
        Config {
            dir: Some(dir.into()),
            disk_bytes: norimusic::covers::cover_rules().disk_bytes,
            memory_bytes: 64 << 20,
            workers: thread::available_parallelism().map_or(2, |n| n.get().clamp(2, 4)),
            alpha: Alpha::Straight,
            timeout_ms: 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Error {
    /// The request did not come back.
    Transport { kind: FailureKind, detail: Option<String> },
    /// The server answered with an error, or with nothing.
    Status(u16),
    Decode(decode::Error),
    /// The loader was dropped first.
    Closed,
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Transport { detail, .. } => f.write_str(detail.as_deref().unwrap_or("network error")),
            Error::Status(s) => write!(f, "HTTP {s}"),
            Error::Decode(e) => e.fmt(f),
            Error::Closed => f.write_str("closed"),
        }
    }
}

impl std::error::Error for Error {}

impl From<TransportError> for Error {
    fn from(e: TransportError) -> Error {
        let TransportError::Failed { kind, detail } = e;
        Error::Transport { kind, detail }
    }
}

pub type Done = Box<dyn FnOnce(Result<Arc<Image>, Error>) + Send>;

/// One cover at one size on its way, and who waits for it.
struct Flight {
    url: String,
    started: bool,
    waiters: Vec<(u64, Done)>,
}

#[derive(Default)]
struct Jobs {
    flights: HashMap<Sized, Flight>,
    /// Flights not started yet, newest last. A cancelled one stays here until a worker skips it.
    queue: Vec<Sized>,
    workers: usize,
    idle: usize,
    next_id: u64,
    closed: bool,
}

struct Inner {
    transport: Arc<dyn Transport>,
    disk: Option<DiskCache>,
    memory: MemoryCache,
    jobs: Mutex<Jobs>,
    work: Condvar,
    workers: usize,
    alpha: Alpha,
    timeout_ms: u32,
}

pub struct Loader {
    inner: Arc<Inner>,
}

/// A request's claim on its cover. Dropping it (or [`Ticket::cancel`]) says the view no longer wants the
/// cover, and its callback is not called; [`Ticket::detach`] lets the request run on unwatched.
#[must_use = "dropping a ticket cancels its request"]
pub struct Ticket {
    inner: Option<Arc<Inner>>,
    key: Sized,
    id: u64,
}

impl Ticket {
    pub fn cancel(self) {}

    pub fn detach(mut self) {
        self.inner = None;
    }
}

impl Drop for Ticket {
    fn drop(&mut self) {
        let Some(inner) = self.inner.take() else { return };
        let mut jobs = inner.jobs.lock();
        let Some(f) = jobs.flights.get_mut(&self.key) else { return };
        f.waiters.retain(|(id, _)| *id != self.id);
        if f.waiters.is_empty() && !f.started {
            jobs.flights.remove(&self.key);
        }
    }
}

impl Loader {
    /// A loader over the client's `transport` (on a desktop, `nori-http`'s). Opening the disk cache reads
    /// its directory once.
    pub fn new(config: Config, transport: Arc<dyn Transport>) -> std::io::Result<Loader> {
        let disk = match &config.dir {
            Some(dir) => Some(DiskCache::open(dir, config.disk_bytes)?),
            None => None,
        };
        let inner = Inner {
            transport,
            disk,
            memory: MemoryCache::new(config.memory_bytes),
            jobs: Mutex::new(Jobs::default()),
            work: Condvar::new(),
            workers: config.workers.max(1),
            alpha: config.alpha,
            timeout_ms: config.timeout_ms,
        };
        Ok(Loader { inner: Arc::new(inner) })
    }

    /// The cover at `width` x `height` if it is decoded and kept: what a view draws right away, with no
    /// placeholder, before it asks.
    pub fn cached(&self, url: &str, width: u32, height: u32) -> Option<Arc<Image>> {
        self.inner.memory.get(&Sized { key: Key::of(url), width, height })
    }

    /// Asks for the cover at `url`, decoded to fill `width` x `height`. `done` gets it on a worker thread
    /// (a GUI posts it to its own), or at once on this one when it is in memory; it is not called if the
    /// ticket is dropped first.
    pub fn request(&self, url: &str, width: u32, height: u32, done: impl FnOnce(Result<Arc<Image>, Error>) + Send + 'static) -> Ticket {
        let key = Sized { key: Key::of(url), width, height };
        let inner = &self.inner;
        if let Some(image) = inner.memory.get(&key) {
            done(Ok(image));
            return Ticket { inner: None, key, id: 0 };
        }
        let mut jobs = inner.jobs.lock();
        jobs.next_id += 1;
        let id = jobs.next_id;
        match jobs.flights.entry(key) {
            Entry::Occupied(mut f) => f.get_mut().waiters.push((id, Box::new(done))),
            Entry::Vacant(v) => {
                // A flight that landed between the look above and the lock left its cover in memory.
                if let Some(image) = inner.memory.get(&key) {
                    drop(jobs);
                    done(Ok(image));
                    return Ticket { inner: None, key, id: 0 };
                }
                v.insert(Flight { url: url.to_owned(), started: false, waiters: vec![(id, Box::new(done))] });
                jobs.queue.push(key);
                if jobs.idle == 0 && jobs.workers < inner.workers {
                    jobs.workers += 1;
                    let inner = inner.clone();
                    thread::Builder::new().name("nori-covers".into()).spawn(move || inner.work()).expect("a thread for covers");
                } else {
                    inner.work.notify_one();
                }
            }
        }
        Ticket { inner: Some(inner.clone()), key, id }
    }

    /// The cover, waiting for it on this thread. Not from a `request` callback: that would wait on the
    /// worker it runs on.
    pub fn load(&self, url: &str, width: u32, height: u32) -> Result<Arc<Image>, Error> {
        let (tx, rx) = mpsc::sync_channel(1);
        let _ticket = self.request(url, width, height, move |r| {
            let _ = tx.send(r);
        });
        rx.recv().unwrap_or(Err(Error::Closed))
    }

    /// Lets the decoded covers go, for a client told memory is short. The disk keeps them.
    pub fn trim_memory(&self) {
        self.inner.memory.clear();
    }

    pub fn disk(&self) -> Option<&DiskCache> {
        self.inner.disk.as_ref()
    }
}

impl Drop for Loader {
    /// Stops the workers once they finish what they are on; whoever still waits gets `Closed`.
    fn drop(&mut self) {
        let flights = {
            let mut jobs = self.inner.jobs.lock();
            jobs.closed = true;
            jobs.queue.clear();
            std::mem::take(&mut jobs.flights)
        };
        self.inner.work.notify_all();
        for (_, f) in flights {
            for (_, done) in f.waiters {
                done(Err(Error::Closed));
            }
        }
    }
}

/// Wakes the worker parked on a future the transport has not finished.
struct Unpark(Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }

    fn wake_by_ref(self: &Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `f` to its end on this thread. A desktop transport finishes on its first poll; one that does not
/// wakes this thread when it can go on.
fn block_on<F: Future>(f: F, waker: &Waker) -> F::Output {
    let mut cx = Context::from_waker(waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        thread::park();
    }
}

/// What a worker keeps from cover to cover.
struct Worker {
    decoder: Decoder,
    bytes: Vec<u8>,
    waker: Waker,
}

impl Inner {
    fn work(self: Arc<Self>) {
        let mut w = Worker { decoder: Decoder::new(), bytes: Vec::new(), waker: Waker::from(Arc::new(Unpark(thread::current()))) };
        loop {
            let (key, url) = {
                let mut jobs = self.jobs.lock();
                loop {
                    if jobs.closed {
                        jobs.workers -= 1;
                        return;
                    }
                    if let Some(key) = jobs.queue.pop() {
                        match jobs.flights.get_mut(&key) {
                            Some(f) if !f.started => {
                                f.started = true;
                                break (key, std::mem::take(&mut f.url));
                            }
                            _ => continue,
                        }
                    }
                    jobs.idle += 1;
                    self.work.wait(&mut jobs);
                    jobs.idle -= 1;
                }
            };
            let result = self.fetch(&key, &url, &mut w);
            let waiters = self.jobs.lock().flights.remove(&key).map(|f| f.waiters).unwrap_or_default();
            for (_, done) in waiters {
                done(result.clone());
            }
        }
    }

    fn fetch(&self, key: &Sized, url: &str, w: &mut Worker) -> Result<Arc<Image>, Error> {
        let from_disk = self.disk.as_ref().is_some_and(|d| d.read(key.key, &mut w.bytes));
        if !from_disk {
            let r = block_on(self.transport.get(url.to_owned(), self.timeout_ms), &w.waker)?;
            if !(200..300).contains(&r.status) || r.body.is_empty() {
                return Err(Error::Status(r.status));
            }
            w.bytes = r.body;
            // Provider artwork is redrawn under the same address once the item is in the library.
            if !is_provider_cover(url) {
                if let Some(d) = &self.disk {
                    let _ = d.put(key.key, &w.bytes);
                }
            }
        }
        if self.jobs.lock().flights.get(key).is_none_or(|f| f.waiters.is_empty()) {
            return Err(Error::Closed);
        }
        let (width, height) = (key.width as usize, key.height as usize);
        let pixels = match w.decoder.decode(&w.bytes, width, height, self.alpha) {
            Ok(px) => px,
            Err(e) => {
                // A file that does not decode is fetched again next time rather than kept.
                if let Some(d) = &self.disk {
                    d.remove(key.key);
                }
                return Err(Error::Decode(e));
            }
        };
        let image = Arc::new(Image { width: key.width, height: key.height, pixels: pixels.into_boxed_slice() });
        self.memory.put(*key, image.clone());
        Ok(image)
    }
}
