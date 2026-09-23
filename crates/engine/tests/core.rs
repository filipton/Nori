//! The engine's pieces over the core: downloads run from the core's queue into the store, a downloaded
//! or cached song is found on the disk before the network is asked, and with AutoMix on the songs
//! coming up that are on the disk are measured ahead. The core keeps one active database and one queue
//! per process, so it is all one test.
#![cfg(feature = "core")]

use std::io::Cursor;
use std::sync::Arc;

use nori_engine::core::{CoreLibrary, CoreOrder, Downloader, Measurer};
use nori_engine::{Body, ByteSource, Library, Source, Store};
use norimusic::client::{Client, NetProfile};
use norimusic::transport::{Transport, TransportError, TransportResponse};
use norimusic::{Core, ServerConfig, Song};
use parking_lot::Mutex;

/// No API calls are made here; resolving a song's address needs none.
struct NoApi;

#[async_trait::async_trait]
impl Transport for NoApi {
    async fn get(&self, _url: String, _timeout_ms: u32) -> Result<TransportResponse, TransportError> {
        Ok(TransportResponse { status: 500, body: Vec::new() })
    }

    fn address_changed(&self) {}
}

/// Audio: each song's bytes made up from its id, every request counted, and a connection that breaks
/// half way through the first time it is asked for a song.
#[derive(Default)]
struct Audio {
    requests: Mutex<Vec<(String, u64)>>,
}

const LEN: usize = 300_000;

fn bytes_of(url: &str) -> Vec<u8> {
    let seed = url.bytes().fold(7u8, |a, b| a.wrapping_mul(31).wrapping_add(b));
    (0..LEN).map(|i| (i as u8).wrapping_add(seed)).collect()
}

/// Reads up to `stop`, then fails.
struct Breaks(Cursor<Vec<u8>>, u64);

impl std::io::Read for Breaks {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.1.saturating_sub(self.0.position()) as usize;
        if left == 0 {
            return Err(std::io::Error::other("reset"));
        }
        let n = buf.len().min(left);
        self.0.read(&mut buf[..n])
    }
}

impl ByteSource for Audio {
    fn open(&self, url: &str, from: u64) -> Result<Body, String> {
        let first = !self.requests.lock().iter().any(|(u, _)| u == url);
        self.requests.lock().push((url.to_string(), from));
        let mut c = Cursor::new(bytes_of(url));
        c.set_position(from);
        let reader: Box<dyn std::io::Read + Send> = if first { Box::new(Breaks(c, LEN as u64 / 2)) } else { Box::new(c) };
        Ok(Body { start: from, len: Some(LEN as u64), reader })
    }
}

/// Forty seconds of a steady beat at 120 bpm as a 16-bit stereo WAV file.
fn beat_wav() -> Vec<u8> {
    let rate = 44_100u32;
    let frames = rate as usize * 40;
    let mut samples = Vec::with_capacity(frames * 2);
    for i in 0..frames {
        let in_beat = i % (rate as usize / 2);
        let click = if in_beat < 2000 { (1.0 - in_beat as f64 / 2000.0) * 0.8 } else { 0.0 };
        let tone = (i as f64 * 220.0 * std::f64::consts::TAU / rate as f64).sin() * 0.1;
        let v = (((click * ((i * 7919) % 97) as f64 / 97.0) + tone) * 32767.0) as i16;
        samples.extend([v, v]);
    }
    let data = samples.len() as u32 * 2;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    w.extend(samples.iter().flat_map(|v| v.to_le_bytes()));
    w
}

#[test]
fn downloads_the_disk_and_measuring_ahead_over_the_core() {
    let dir = std::env::temp_dir().join(format!("nori-core-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let core = Core::new(dir.join("nori.db").to_string_lossy().into_owned(), "test".into()).unwrap();
    let config = ServerConfig { url: "http://music.test".into(), user: "u".into(), password: "p".into(), api_key: None, legacy_auth: false };
    core.configure(config).unwrap();
    let client = Client::new(core.clone(), Arc::new(NoApi));
    client.set_profile(NetProfile { url: "http://music.test".into(), ..Default::default() });
    let store = Store::open(dir.join("music"), 64 << 20, Box::new(CoreOrder)).unwrap();
    let audio = Arc::new(Audio::default());

    let song = Song { id: "dl-1".into(), title: "One".into(), duration: 3, suffix: "mp3".into(), ..Default::default() };
    core.download_queue(vec![song.clone()]).unwrap();
    let d = Downloader::new(core.clone(), client.clone(), audio.clone(), store.clone());
    d.start(2);
    d.wait();

    let path = store.downloaded("dl-1").expect("downloaded");
    let url = client.resolve("dl-1".into(), true, false).url;
    assert_eq!(std::fs::read(&path).unwrap(), bytes_of(&url), "the whole song, byte for byte");
    let asked: Vec<u64> = audio.requests.lock().iter().map(|r| r.1).collect();
    assert_eq!(asked, [0, LEN as u64 / 2], "taken up where the connection broke");
    assert_eq!(norimusic::transfers::held("dl-1"), 2, "the core has it as finished");
    assert_eq!(norimusic::transfers::download_phase("dl-1".into()), 3);

    norimusic::queue::queue_register(vec![song]);
    let mut library = CoreLibrary { client: client.clone(), bytes: audio.clone(), metered: false, store: Some(store.clone()) };
    match library.locate("dl-1").unwrap().source {
        Source::File(p) => assert_eq!(p, path, "the download, not the network"),
        _ => panic!("a downloaded song is read from the disk"),
    }
    match library.locate("other").unwrap().source {
        Source::Cached { key, .. } => assert_eq!(key, "other:0", "streamed, and kept in the cache"),
        _ => panic!("a song that is not downloaded streams"),
    }

    // AutoMix on: the songs coming up are measured, those on the disk only.
    let mut prefs = norimusic::settings_store::settings_open(dir.join("app.db").to_string_lossy().into_owned()).unwrap();
    prefs.auto_mix = true;
    norimusic::settings_store::settings_put(prefs);
    let on_disk = Song { id: "m-1".into(), title: "Beat".into(), duration: 40, suffix: "wav".into(), ..Default::default() };
    let elsewhere = Song { id: "m-2".into(), title: "Not here".into(), duration: 40, suffix: "wav".into(), ..Default::default() };
    core.download_queue(vec![on_disk.clone()]).unwrap();
    core.download_settle(vec!["m-1".into()], vec![true]).unwrap();
    std::fs::write(store.download_path("m-1"), beat_wav()).unwrap();
    norimusic::queue::queue_register(vec![on_disk, elsewhere]);
    norimusic::playlist::playlist_set(vec!["m-1".into(), "m-2".into(), "ext-3".into()], 0, false);
    let ahead = norimusic::rules::queue_measure();
    assert!(!ahead.contains(&"ext-3".to_string()), "a provider's song is never measured: {ahead:?}");
    let measurer = Measurer::new(core.clone(), client.clone(), store.clone());
    measurer.update(ahead, std::thread::current());
    let until = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while core.analysis_get("m-1".into()).unwrap().is_none() {
        assert!(std::time::Instant::now() < until, "m-1 was measured");
        std::thread::park_timeout(std::time::Duration::from_millis(200));
    }
    let a = core.analysis_get("m-1".into()).unwrap().unwrap();
    assert!((a.bpm - 120.0).abs() < 2.0 || (a.bpm - 60.0).abs() < 1.0 || (a.bpm - 240.0).abs() < 4.0, "the beat heard: {}", a.bpm);
    assert!(core.analysis_get("m-2".into()).unwrap().is_none(), "not on the disk: left for later");
    assert!(audio.requests.lock().iter().all(|(u, _)| !u.contains("m-2")), "and never fetched for it");
    let _ = std::fs::remove_dir_all(dir);
}
