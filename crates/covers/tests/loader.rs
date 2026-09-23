//! The loader on real worker threads over a transport that counts its requests and holds them until the
//! test lets them through.

use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{mpsc, Arc};
use std::time::Duration;

use nori_covers::{Alpha, Config, Error, Image, Key, Loader};
use norimusic::transport::{FailureKind, Transport, TransportError, TransportResponse};
use parking_lot::{Condvar, Mutex};

const PHOTO: &str = "http://s/rest/getCoverArt.view?u=a&t=b&s=c&id=al-1&size=320";

struct Server {
    calls: AtomicUsize,
    open: Mutex<bool>,
    opened: Condvar,
    status: u16,
    body: Vec<u8>,
}

impl Server {
    fn new(status: u16) -> Arc<Server> {
        let body = std::fs::read(format!("{}/testdata/photo.jpg", env!("CARGO_MANIFEST_DIR"))).unwrap();
        Arc::new(Server { calls: AtomicUsize::new(0), open: Mutex::new(true), opened: Condvar::new(), status, body })
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
}

#[async_trait::async_trait]
impl Transport for Server {
    async fn get(&self, _url: String, _timeout_ms: u32) -> Result<TransportResponse, TransportError> {
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

    fn address_changed(&self) {}
}

fn dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("nori-covers-loader-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    d
}

fn config(dir: Option<PathBuf>, workers: usize) -> Config {
    Config { dir, disk_bytes: 1 << 20, memory_bytes: 1 << 20, workers, alpha: Alpha::Straight, timeout_ms: 0 }
}

/// Waits for `n` answers, failing the test rather than hanging it.
fn answers(rx: &mpsc::Receiver<Result<Arc<Image>, Error>>, n: usize) -> Vec<Result<Arc<Image>, Error>> {
    (0..n).map(|_| rx.recv_timeout(Duration::from_secs(10)).expect("an answer")).collect()
}

#[test]
fn views_asking_for_one_cover_share_one_fetch_and_one_decode() {
    let server = Server::new(200);
    server.hold();
    let loader = Loader::new(config(None, 3), server.clone()).unwrap();
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
    let loader = Loader::new(config(None, 1), server.clone()).unwrap();
    let (tx, rx) = mpsc::channel();
    let tx2 = tx.clone();
    // The one worker is held on the first cover; the second waits behind it and is let go.
    let first = loader.request("http://s/a", 8, 8, move |r| tx.send(r).unwrap());
    while server.calls() == 0 {
        std::thread::yield_now();
    }
    let second = loader.request("http://s/b", 8, 8, move |r| tx2.send(r).unwrap());
    second.cancel();
    server.release();
    assert!(answers(&rx, 1)[0].is_ok());
    // Something after it still gets through, and the cancelled one was never asked for.
    assert!(loader.load("http://s/c", 8, 8).is_ok());
    assert_eq!(server.calls(), 2);
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    first.detach();
}

#[test]
fn one_view_letting_go_leaves_the_cover_to_the_others() {
    let server = Server::new(200);
    server.hold();
    let loader = Loader::new(config(None, 1), server.clone()).unwrap();
    let (tx, rx) = mpsc::channel();
    let busy = loader.request("http://s/busy", 8, 8, |_| {});
    while server.calls() == 0 {
        std::thread::yield_now();
    }
    let tx2 = tx.clone();
    let gone = loader.request(PHOTO, 8, 8, move |r| tx.send(r).unwrap());
    let kept = loader.request(PHOTO, 8, 8, move |r| tx2.send(r).unwrap());
    drop(gone);
    server.release();
    assert_eq!(answers(&rx, 1).len(), 1);
    assert!(rx.recv_timeout(Duration::from_millis(100)).is_err());
    drop((busy, kept));
}

#[test]
fn covers_are_kept_on_disk_for_the_next_run_but_a_providers_are_not() {
    let d = dir("disk");
    let provider = "http://s/rest/getCoverArt.view?u=a&id=ext-deezer-1&size=320";
    {
        let loader = Loader::new(config(Some(d.clone()), 2), Server::new(200)).unwrap();
        loader.load(PHOTO, 8, 8).unwrap();
        loader.load(provider, 8, 8).unwrap();
        let disk = loader.disk().unwrap();
        assert!(disk.contains(Key::of(PHOTO)) && disk.path(Key::of(PHOTO)).exists());
        assert!(!disk.contains(Key::of(provider)));
    }
    // A server that is not there: the kept cover still comes, the provider's does not.
    let down = Server::new(0);
    let loader = Loader::new(config(Some(d.clone()), 2), down.clone()).unwrap();
    assert_eq!(loader.load(PHOTO, 8, 8).unwrap().width, 8);
    assert_eq!(down.calls(), 0);
    assert!(matches!(loader.load(provider, 8, 8), Err(Error::Transport { kind: FailureKind::Connect, .. })));
    drop(loader);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn an_error_answer_is_an_error_and_is_not_kept() {
    let d = dir("error");
    let server = Server::new(404);
    let loader = Loader::new(config(Some(d.clone()), 1), server.clone()).unwrap();
    assert_eq!(loader.load(PHOTO, 8, 8), Err(Error::Status(404)));
    assert_eq!(loader.disk().unwrap().bytes(), 0);
    // Not remembered as a failure either: the next ask asks again.
    assert_eq!(loader.load(PHOTO, 8, 8), Err(Error::Status(404)));
    assert_eq!(server.calls(), 2);
    drop(loader);
    std::fs::remove_dir_all(&d).unwrap();
}

#[test]
fn dropping_the_loader_answers_whoever_still_waits() {
    let server = Server::new(200);
    server.hold();
    let loader = Loader::new(config(None, 1), server.clone()).unwrap();
    let (tx, rx) = mpsc::channel();
    let _held = loader.request("http://s/a", 8, 8, |_| {});
    while server.calls() == 0 {
        std::thread::yield_now();
    }
    let _waiting = loader.request("http://s/b", 8, 8, move |r| tx.send(r).unwrap());
    drop(loader);
    assert_eq!(answers(&rx, 1)[0], Err(Error::Closed));
    server.release();
}
