//! The loader on real worker threads over a transport that counts its requests and holds them until the
//! test lets them through.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use nori_covers::{header, Alpha, Config, DecodeError, Decoder, Error, Image, Key, Loader, Paint};
use nori_core::transport::{Exchange, FailureKind, Transport, TransportError, TransportResponse};
use parking_lot::{Condvar, Mutex};

const PHOTO: &str = "http://s/rest/getCoverArt.view?u=a&t=b&s=c&id=al-1&size=320";

struct Server {
    calls: AtomicUsize,
    /// What was asked for, in order.
    asked: Mutex<Vec<String>>,
    open: Mutex<bool>,
    opened: Condvar,
    status: u16,
    body: Vec<u8>,
}

impl Server {
    fn new(status: u16) -> Arc<Server> {
        let body = std::fs::read(format!("{}/testdata/photo.jpg", env!("CARGO_MANIFEST_DIR"))).unwrap();
        Arc::new(Server { calls: AtomicUsize::new(0), asked: Mutex::new(Vec::new()), open: Mutex::new(true), opened: Condvar::new(), status, body })
    }

    fn hold(&self) {
        *self.open.lock() = false;
    }

    fn release(&self) {
        *self.open.lock() = true;
        self.opened.notify_all();
    }

    fn calls(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    /// Waits until `n` requests have reached the server, failing the test rather than hanging it.
    fn wait_calls(&self, n: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while self.calls() < n {
            assert!(std::time::Instant::now() < deadline, "{} requests reached the server, not {n}: {:?}", self.calls(), self.asked.lock());
            std::thread::yield_now();
        }
    }
}

#[async_trait::async_trait]
impl Transport for Server {
    async fn get(&self, url: String, _timeout_ms: u32) -> Result<TransportResponse, TransportError> {
        self.asked.lock().push(url);
        self.calls.fetch_add(1, Ordering::SeqCst);
        let mut open = self.open.lock();
        while !*open {
            self.opened.wait(&mut open);
        }
        if self.status == 0 {
            return Err(TransportError::Failed { kind: FailureKind::Connect, detail: Some("refused".into()) });
        }
        Ok(TransportResponse { status: self.status, body: self.body.clone() })
    }

    async fn send(&self, request: Exchange) -> Result<TransportResponse, TransportError> {
        self.get(request.url, request.timeout_ms).await
    }

    fn address_changed(&self) {}
}

/// A directory of the test's own, gone when the test is.
fn dir(name: &str) -> nori_testdir::TempDir {
    nori_testdir::TempDir::new(&format!("covers-loader-{name}"))
}

fn config(dir: Option<PathBuf>, workers: usize) -> Config {
    Config { dir, disk_bytes: 1 << 20, memory_bytes: 1 << 20, workers, alpha: Alpha::Straight, timeout_ms: 0, idle: Duration::from_secs(20) }
}

/// Waits for `n` answers, failing the test rather than hanging it.
fn answers(rx: &mpsc::Receiver<Result<Arc<Image>, Error>>, n: usize) -> Vec<Result<Arc<Image>, Error>> {
    (0..n).map(|_| rx.recv_timeout(Duration::from_secs(10)).expect("an answer")).collect()
}

#[test]
fn views_asking_for_one_cover_share_one_fetch_and_one_decode() {
    let server = Server::new(200);
    server.hold();
    let loader = Loader::new(config(None, 3), server.clone());
    let (tx, rx) = mpsc::channel();
    let tickets: Vec<_> = (0..5)
        .map(|_| {
            let tx = tx.clone();
            loader.request(PHOTO, 16, 16, move |r| tx.send(r).unwrap())
        })
        .collect();
    server.release();
    let got = answers(&rx, 5);
    assert_eq!(server.calls(), 1);
    let first = got[0].as_ref().unwrap();
    assert_eq!((first.width, first.height, first.pixels.len()), (16, 16, 16 * 16 * 4));
    assert!(got.iter().all(|r| Arc::ptr_eq(r.as_ref().unwrap(), first)));
    // Kept decoded: asked again, it is there at once, without a fetch.
    assert!(Arc::ptr_eq(&loader.cached(PHOTO, 16, 16).unwrap(), first));
    assert!(Arc::ptr_eq(&loader.load(PHOTO, 16, 16).unwrap(), first));
    assert_eq!(server.calls(), 1);
    // Another size is another decode, from the same address.
    assert_eq!(loader.load(PHOTO, 8, 8).unwrap().width, 8);
    drop(tickets);
}

#[test]
fn a_cancelled_request_is_never_fetched_and_never_answered() {
    let server = Server::new(200);
    server.hold();
    let loader = Loader::new(config(None, 1), server.clone());
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    // The one worker is held on the first cover; the second waits behind it and is let go.
    let first = loader.request("http://s/a", 8, 8, move |r| tx.send(r).unwrap());
    server.wait_calls(1);
    let second = loader.request("http://s/b", 8, 8, move |r| tx2.send(r).unwrap());
    second.cancel();
    server.release();
    assert!(answers(&rx, 1)[0].is_ok());
    // Something after it still gets through, and the cancelled one was never asked for.
    assert!(loader.load("http://s/c", 8, 8).is_ok());
    assert_eq!(*server.asked.lock(), ["http://s/a", "http://s/c"], "b was never fetched");
    // The one worker took c after anything queued before it: b would have been answered by now.
    assert!(rx.try_recv().is_err(), "the cancelled request was answered");
    first.detach();
}

#[test]
fn one_view_letting_go_leaves_the_cover_to_the_others() {
    let server = Server::new(200);
    server.hold();
    let loader = Loader::new(config(None, 1), server.clone());
    let (tx, rx) = mpsc::channel();
    let busy = loader.request("http://s/busy", 8, 8, |_| {});
    server.wait_calls(1);
    let tx2 = tx.clone();
    let gone = loader.request(PHOTO, 8, 8, move |r| tx.send(r).unwrap());
    let kept = loader.request(PHOTO, 8, 8, move |r| tx2.send(r).unwrap());
    drop(gone);
    server.release();
    let got = answers(&rx, 1).remove(0).expect("the view still there gets the cover");
    assert_eq!((got.width, got.height), (8, 8));
    // The one worker answers a cover's views together, and has moved on to another: the view that went
    // would have been called back by now.
    loader.load("http://s/after", 8, 8).unwrap();
    assert!(rx.try_recv().is_err(), "the view that let go was called back");
    assert_eq!(*server.asked.lock(), ["http://s/busy", PHOTO, "http://s/after"], "one fetch for the two views");
    drop((busy, kept));
}

#[test]
fn covers_are_kept_on_disk_for_the_next_run_but_a_providers_are_not() {
    let d = dir("disk");
    let provider = "http://s/rest/getCoverArt.view?u=a&id=ext-deezer-1&size=320";
    {
        let loader = Loader::new(config(Some(d.to_path_buf()), 2), Server::new(200));
        loader.load(PHOTO, 8, 8).unwrap();
        loader.load(provider, 8, 8).unwrap();
        let disk = loader.disk().unwrap();
        assert!(disk.contains(Key::of(PHOTO)) && disk.path(Key::of(PHOTO)).exists());
        assert!(!disk.contains(Key::of(provider)));
    }
    // A server that is not there: the kept cover still comes, the provider's does not.
    let down = Server::new(0);
    let loader = Loader::new(config(Some(d.to_path_buf()), 2), down.clone());
    assert_eq!(loader.load(PHOTO, 8, 8).unwrap().width, 8);
    assert_eq!(down.calls(), 0);
    assert!(matches!(loader.load(provider, 8, 8), Err(Error::Transport { kind: FailureKind::Connect, .. })));
    drop(loader);
}

/// A playlist's cover, which Navidrome names `pl-<id>_<when it changed>` (octo-fiesta's provider
/// playlists are `pl-<provider>-<id>`), is kept on disk like an album's and comes offline; and any cover
/// comes offline asked for under another token and salt (a new password), or at the server's other
/// address (offline, the app has fallen back to it), but not at another size.
#[test]
fn a_playlists_cover_is_kept_and_comes_offline_whatever_signs_it_and_at_either_address() {
    let d = dir("playlist");
    nori_core::covers::cover_address_alike("http://lan.test:4533", "https://wan.test");
    let id = "pl-6b2d0c1e-5f7a-4e21-9d3c-0a1b2c3d4e5f_65f0a1b2";
    let at = |base: &str, t: &str, s: &str, size: u32| format!("{base}/rest/getCoverArt?u=a&t={t}&s={s}&v=1.16.1&c=nori&f=json&id={id}&size={size}");
    let first = at("http://lan.test:4533", "tok1", "salt1", 320);
    {
        let loader = Loader::new(config(Some(d.to_path_buf()), 2), Server::new(200));
        loader.load(&first, 8, 8).unwrap();
        assert!(loader.disk().unwrap().contains(Key::of(&first)), "the playlist's cover is kept");
    }
    let down = Server::new(0);
    let loader = Loader::new(config(Some(d.to_path_buf()), 2), down.clone());
    for url in [first.clone(), at("http://lan.test:4533", "tok2", "salt2", 320), at("https://wan.test", "tok1", "salt1", 320), at("http://wan.test", "tok3", "salt3", 320)] {
        assert_eq!(loader.load(&url, 8, 8).map(|p| p.width), Ok(8), "{url}");
    }
    assert_eq!(down.calls(), 0);
    assert!(loader.load(&at("http://lan.test:4533", "tok1", "salt1", 800), 8, 8).is_err(), "another size is another file");
    assert!(loader.load(&at("http://elsewhere.test", "tok1", "salt1", 320), 8, 8).is_err(), "another server's is its own");
    // A provider's playlist is still never kept.
    let provider = "http://lan.test:4533/rest/getCoverArt?u=a&id=pl-deezer-9&size=320";
    let up = Loader::new(config(Some(d.to_path_buf()), 1), Server::new(200));
    up.load(provider, 8, 8).unwrap();
    assert!(!up.disk().unwrap().contains(Key::of(provider)));
    drop((loader, up));
}

/// A cover kept before keys left the signature out (under its whole address) is still found, offline,
/// and moved under its new key.
#[test]
fn a_cover_kept_under_its_whole_address_is_still_found() {
    let d = dir("legacy");
    let bytes = std::fs::read(format!("{}/testdata/photo.jpg", env!("CARGO_MANIFEST_DIR"))).unwrap();
    nori_covers::DiskCache::open(d.to_path_buf(), 1 << 20).unwrap().put(Key::of_address(PHOTO), &bytes).unwrap();
    let down = Server::new(0);
    let loader = Loader::new(config(Some(d.to_path_buf()), 1), down.clone());
    assert_eq!(loader.load(PHOTO, 8, 8).map(|p| p.width), Ok(8));
    assert_eq!(down.calls(), 0);
    let disk = loader.disk().unwrap();
    assert!(disk.contains(Key::of(PHOTO)) && !disk.contains(Key::of_address(PHOTO)));
    drop(loader);
}

#[test]
fn an_error_answer_is_an_error_and_is_not_kept() {
    let d = dir("error");
    let server = Server::new(404);
    let loader = Loader::new(config(Some(d.to_path_buf()), 1), server.clone());
    assert_eq!(loader.load(PHOTO, 8, 8), Err(Error::Status(404)));
    assert_eq!(loader.disk().unwrap().bytes(), 0);
    // Not remembered as a failure either: the next ask asks again.
    assert_eq!(loader.load(PHOTO, 8, 8), Err(Error::Status(404)));
    assert_eq!(server.calls(), 2);
    drop(loader);
}

#[test]
fn dropping_the_loader_answers_whoever_still_waits() {
    let server = Server::new(200);
    server.hold();
    let loader = Loader::new(config(None, 1), server.clone());
    let (tx, rx) = mpsc::channel();
    let _held = loader.request("http://s/a", 8, 8, |_| {});
    server.wait_calls(1);
    let _waiting = loader.request("http://s/b", 8, 8, move |r| tx.send(r).unwrap());
    drop(loader);
    assert_eq!(answers(&rx, 1)[0], Err(Error::Closed));
    server.release();
}

/// A client's own picture: what was asked for and what came out, and how many decodes there were.
struct Counted(Arc<AtomicUsize>);

#[derive(Debug, Clone, PartialEq)]
struct Picture {
    asked: (u32, u32),
    got: (usize, usize),
}

impl Paint for Counted {
    type Picture = Arc<Picture>;

    fn paint(&self, _: &mut Decoder, bytes: &[u8], width: u32, height: u32) -> Result<Arc<Picture>, DecodeError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        let h = header(bytes)?;
        Ok(Arc::new(Picture { asked: (width, height), got: h.fill(width as usize, height as usize) }))
    }

    fn bytes(_: &Arc<Picture>) -> usize {
        100
    }
}

#[test]
fn a_clients_painter_decodes_once_and_every_waiter_is_called_back_with_the_one_picture() {
    let server = Server::new(200);
    server.hold();
    let painted = Arc::new(AtomicUsize::new(0));
    let loader = Loader::with_paint(Config { memory_bytes: 0, ..config(None, 2) }, server.clone(), Counted(painted.clone()));
    let (tx, rx) = mpsc::channel();
    let tickets: Vec<_> = (0..3)
        .map(|i| {
            let tx = tx.clone();
            loader.request(PHOTO, 30, 30, move |r| tx.send((i, r, std::thread::current().name().map(String::from))).unwrap())
        })
        .collect();
    server.release();
    let mut got: Vec<_> = (0..3).map(|_| rx.recv_timeout(Duration::from_secs(10)).expect("a callback")).collect();
    got.sort_by_key(|(i, ..)| *i);
    assert_eq!(got.iter().map(|(i, ..)| *i).collect::<Vec<_>>(), [0, 1, 2], "one call back each");
    let first = got[0].1.as_ref().unwrap();
    assert!(got.iter().all(|(_, r, _)| Arc::ptr_eq(r.as_ref().unwrap(), first)));
    // Called on a worker, never on the thread that asked.
    assert!(got.iter().all(|(.., thread)| thread.as_deref() == Some("nori-covers")));
    // photo.jpg is 40x30: a 30x30 view is filled at 30x30.
    assert_eq!(**first, Picture { asked: (30, 30), got: (30, 30) });
    assert_eq!(painted.load(Ordering::SeqCst), 1);
    // Nothing kept in memory when the client keeps its own: asked again, it is decoded again.
    assert!(loader.cached(PHOTO, 30, 30).is_none());
    loader.load(PHOTO, 30, 30).unwrap();
    assert_eq!(painted.load(Ordering::SeqCst), 2);
    // A view bigger than the picture gets it at the picture's size, and 0 x 0 is its own size.
    assert_eq!(loader.load(PHOTO, 300, 300).unwrap().got, (30, 30));
    assert_eq!(loader.load(PHOTO, 0, 0).unwrap().got, (40, 30));
    drop(tickets);
}

#[test]
fn a_cover_let_go_of_while_fetched_and_asked_for_again_comes() {
    // The player skipping away from a song and back while its cover is on the wire: the second view gets
    // the picture, whether it asked while the first fetch was out or after it ended with nobody waiting.
    let d = dir("again");
    let server = Server::new(200);
    server.hold();
    let loader = Loader::new(Config { memory_bytes: 0, ..config(Some(d.path().to_path_buf()), 1) }, server.clone());
    let (tx, rx) = mpsc::channel();
    let first = loader.request(PHOTO, 8, 8, |_| {});
    server.wait_calls(1);
    drop(first);
    let tx2 = tx.clone();
    let back = loader.request(PHOTO, 8, 8, move |r| tx2.send(r).unwrap());
    server.release();
    assert!(answers(&rx, 1)[0].is_ok(), "joined the fetch that was out");
    drop(back);
    server.hold();
    let gone = loader.request("http://s/other", 8, 8, |_| {});
    server.wait_calls(2);
    drop(gone);
    server.release();
    // The fetch nobody waited for has ended; asked again, the cover comes from the disk.
    loader.load("http://s/next", 8, 8).unwrap();
    let again = loader.request("http://s/other", 8, 8, move |r| tx.send(r).unwrap());
    assert!(answers(&rx, 1)[0].is_ok(), "asked again after the flight ended with nobody waiting");
    assert_eq!(server.calls(), 3, "the second ask was answered from the disk");
    drop(again);
}

#[test]
fn a_view_that_leaves_while_its_cover_is_fetched_is_never_called_back_nor_decoded_for() {
    let server = Server::new(200);
    server.hold();
    let painted = Arc::new(AtomicUsize::new(0));
    let loader = Loader::with_paint(Config { memory_bytes: 0, ..config(None, 1) }, server.clone(), Counted(painted.clone()));
    let (tx, rx) = mpsc::channel::<()>();
    let ticket = loader.request(PHOTO, 8, 8, move |_| tx.send(()).unwrap());
    server.wait_calls(1);
    // The bytes are on their way; the view goes.
    drop(ticket);
    server.release();
    // The worker is free again (so done with the cover that was left), and it never decoded it.
    loader.load("http://s/next", 8, 8).unwrap();
    assert!(rx.try_recv().is_err(), "the view that left was called back");
    assert_eq!(painted.load(Ordering::SeqCst), 1, "decoded only the cover still wanted");
}

#[test]
fn a_warm_up_fetches_onto_the_disk_once_and_decodes_nothing() {
    let d = dir("warm");
    let server = Server::new(200);
    let painted = Arc::new(AtomicUsize::new(0));
    let loader = Loader::with_paint(Config { memory_bytes: 0, ..config(Some(d.to_path_buf()), 1) }, server.clone(), Counted(painted.clone()));
    server.hold();
    // The one worker is busy with a view; a warm-up and another view queue behind it.
    let busy = loader.request("http://s/busy", 8, 8, |_| {});
    server.wait_calls(1);
    loader.warm(PHOTO);
    loader.warm("http://s/rest/getCoverArt.view?u=a&id=ext-deezer-1&size=320");
    let late = loader.request("http://s/late", 8, 8, |_| {});
    server.release();
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !loader.disk().unwrap().contains(Key::of(PHOTO)) {
        assert!(std::time::Instant::now() < deadline, "warmed");
        std::thread::sleep(Duration::from_millis(5));
    }
    // The view asked for after the warm-up went first, the provider's cover was not fetched, and the
    // warmed one was not decoded.
    assert_eq!(*server.asked.lock(), ["http://s/busy", "http://s/late", PHOTO]);
    assert_eq!(painted.load(Ordering::SeqCst), 2);
    // Already on the disk: warmed again, it is not fetched again. Warm-ups are taken in turn, so once a
    // new one warmed after it is on the disk, that one has been seen to.
    loader.warm(PHOTO);
    let then = "http://s/rest/getCoverArt.view?u=a&id=al-2&size=320";
    loader.warm(then);
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !loader.disk().unwrap().contains(Key::of(then)) {
        assert!(std::time::Instant::now() < deadline, "the second warm-up came");
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(*server.asked.lock(), ["http://s/busy", "http://s/late", PHOTO, then], "the warmed cover not fetched again");
    // Read back as bytes, as a page's colours are worked out from them.
    let mut bytes = Vec::new();
    loader.read(PHOTO, &mut bytes).unwrap();
    assert_eq!(bytes, server.body);
    assert_eq!(server.calls(), 4, "read from the disk, not fetched: {:?}", server.asked.lock());
    drop((busy, late));
    drop(loader);
}

/// A painter that panics on one size, as a decoder might on one hostile file.
struct Fragile;

impl Paint for Fragile {
    type Picture = (usize, usize);

    fn paint(&self, _: &mut Decoder, bytes: &[u8], width: u32, height: u32) -> Result<(usize, usize), DecodeError> {
        assert!(width != 13, "a decoder bug");
        Ok(header(bytes)?.fill(width as usize, height as usize))
    }

    fn bytes(_: &(usize, usize)) -> usize {
        100
    }
}

#[test]
fn a_cover_that_panics_is_its_own_error_and_every_cover_after_it_still_comes() {
    let d = dir("panic");
    let server = Server::new(200);
    let loader = Loader::with_paint(Config { memory_bytes: 0, ..config(Some(d.to_path_buf()), 1) }, server.clone(), Fragile);
    for _ in 0..3 {
        assert!(matches!(loader.load(PHOTO, 13, 13), Err(Error::Panicked(why)) if why == "a decoder bug"));
        // Not kept: the file may be what broke it.
        assert!(!loader.disk().unwrap().contains(Key::of(PHOTO)));
        assert_eq!(loader.load(PHOTO, 8, 8), Ok((8, 8)));
    }
    // A call back that panics costs its own cover, not the worker.
    let (tx, rx) = mpsc::channel();
    let t = loader.request(PHOTO, 9, 9, move |_| {
        tx.send(()).unwrap();
        panic!("a client bug")
    });
    rx.recv_timeout(Duration::from_secs(10)).expect("the call back ran");
    drop(t);
    assert_eq!(loader.load(PHOTO, 10, 10), Ok((10, 10)));
    drop(loader);
}

/// A painter that counts the worker threads alive (each marks itself on its first cover, and is counted
/// out when it ends) and how many times it was told to rest.
struct Threads {
    alive: Arc<AtomicUsize>,
    rests: Arc<AtomicUsize>,
}

/// Counts its thread out of `alive` when the thread ends.
struct Alive(Arc<AtomicUsize>);

impl Drop for Alive {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

thread_local! {
    static ALIVE: std::cell::RefCell<Option<Alive>> = const { std::cell::RefCell::new(None) };
}

impl Paint for Threads {
    type Picture = ();

    fn paint(&self, _: &mut Decoder, _: &[u8], _: u32, _: u32) -> Result<(), DecodeError> {
        ALIVE.with_borrow_mut(|a| {
            if a.is_none() {
                self.alive.fetch_add(1, Ordering::SeqCst);
                *a = Some(Alive(self.alive.clone()));
            }
        });
        Ok(())
    }

    fn bytes(_: &()) -> usize {
        0
    }

    fn rest(&self) {
        self.rests.fetch_add(1, Ordering::SeqCst);
    }
}

/// Waits for `n` of `count`, failing the test rather than hanging it.
fn until(count: &AtomicUsize, n: usize) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while count.load(Ordering::SeqCst) != n {
        assert!(std::time::Instant::now() < deadline, "{} rather than {n}", count.load(Ordering::SeqCst));
        std::thread::sleep(Duration::from_millis(2));
    }
}

/// Three covers at once, held at the server until all three workers have one: three threads.
fn three_at_once(loader: &Loader<Threads>, server: &Server) {
    server.hold();
    let calls = server.calls();
    let (tx, rx) = mpsc::channel();
    let tickets: Vec<_> = (0..3u32)
        .map(|i| {
            let tx = tx.clone();
            loader.request(&format!("http://s/{i}"), 8, 8, move |r| tx.send(r).unwrap())
        })
        .collect();
    server.wait_calls(calls + 3);
    server.release();
    for _ in 0..3 {
        rx.recv_timeout(Duration::from_secs(10)).expect("an answer").unwrap();
    }
    drop(tickets);
}

#[test]
fn a_resting_loader_ends_its_threads_and_the_next_cover_starts_one_again() {
    let server = Server::new(200);
    let (alive, rests) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let loader = Loader::with_paint(Config { memory_bytes: 0, ..config(None, 3) }, server.clone(), Threads { alive: alive.clone(), rests: rests.clone() });
    three_at_once(&loader, &server);
    assert_eq!(alive.load(Ordering::SeqCst), 3);
    loader.rest();
    until(&alive, 0);
    assert_eq!(rests.load(Ordering::SeqCst), 1);
    loader.load("http://s/after", 8, 8).unwrap();
    assert_eq!(alive.load(Ordering::SeqCst), 1);
    // Idle for less than the loader's while: the thread stays, nothing rests.
    loader.load("http://s/again", 8, 8).unwrap();
    assert_eq!((alive.load(Ordering::SeqCst), rests.load(Ordering::SeqCst)), (1, 1));
}

#[test]
fn a_loader_no_cover_is_asked_of_for_a_while_rests() {
    let server = Server::new(200);
    let (alive, rests) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let config = Config { memory_bytes: 0, idle: Duration::from_millis(500), ..config(None, 3) };
    let loader = Loader::with_paint(config, server.clone(), Threads { alive: alive.clone(), rests: rests.clone() });
    three_at_once(&loader, &server);
    // Covers asked for closer together than that (a tenth of it: a machine this busy is not a phone) keep
    // the threads.
    for i in 0..5 {
        std::thread::sleep(Duration::from_millis(50));
        loader.load(&format!("http://s/soon-{i}"), 8, 8).unwrap();
    }
    assert_eq!((alive.load(Ordering::SeqCst), rests.load(Ordering::SeqCst)), (3, 0));
    until(&alive, 0);
    assert_eq!(rests.load(Ordering::SeqCst), 1);
    loader.load("http://s/after", 8, 8).unwrap();
    assert_eq!(alive.load(Ordering::SeqCst), 1);
}

#[test]
fn out_of_sight_the_loader_rests_and_a_cover_asked_for_then_keeps_no_thread() {
    let server = Server::new(200);
    let (alive, rests) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    let loader = Loader::with_paint(Config { memory_bytes: 0, ..config(None, 3) }, server.clone(), Threads { alive: alive.clone(), rests: rests.clone() });
    three_at_once(&loader, &server);
    loader.show(false);
    until(&alive, 0);
    assert_eq!(rests.load(Ordering::SeqCst), 1);
    // A notification's cover while the screen is off: its thread ends with it, rather than waiting.
    loader.load("http://s/notification", 8, 8).unwrap();
    until(&alive, 0);
    loader.show(true);
    loader.load("http://s/shown", 8, 8).unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!((alive.load(Ordering::SeqCst), rests.load(Ordering::SeqCst)), (1, 1));
}
