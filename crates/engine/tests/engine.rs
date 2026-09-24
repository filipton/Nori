//! The engine end to end on real threads: songs served as WAV bytes by a fake HTTP client that counts
//! every request, an output that records what a sound card would have played on a clock twenty times
//! faster than real time, and the simulated player (`nori_player::sim`) as the reference - the engine
//! must play exactly what the tested pipeline plays.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nori_engine::{App, AudioOutput, Body, ByteSource, Config, Device, DeviceWatch, Engine, Event, Feed, Library, Located, OutputFacts, OutputFormat, OutputKind, Recent, Settings, Source, State, Store};
use nori_player::automix::analysis::Analyzer;
use nori_player::automix::synth::Rng;
use nori_player::automix::ANALYSIS_VERSION;
use nori_player::engine::{Host, Plan};
use nori_player::playlist::Playlist;
use nori_player::sim::{self, prefs_off, Audio};
use nori_player::transitions::{TransitionPrefs, WindowSong};
use nori_player::types::TrackAnalysis;
use parking_lot::Mutex;

const RATE: u32 = 44_100;

/// Something like music, never the same twice for different seeds: a few partials and a little noise.
fn music(secs: f64, seed: u64) -> Vec<i16> {
    let mut r = Rng(seed);
    let hz = [110.0, 331.0, 1250.0].map(|h| h * (1.0 + seed as f64 * 0.01));
    (0..(secs * RATE as f64) as usize)
        .flat_map(|i| {
            let t = i as f64 / RATE as f64;
            let v = hz.iter().enumerate().map(|(k, h)| (std::f64::consts::TAU * h * t).sin() * 0.2 / (k + 1) as f64).sum::<f64>();
            [((v + 0.02 * r.next()) * 32767.0) as i16, ((v * 0.8 - 0.02 * r.next()) * 32767.0) as i16]
        })
        .collect()
}

fn wav(samples: &[i16]) -> Vec<u8> {
    let data = samples.len() as u32 * 2;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 4).to_le_bytes());
    w.extend_from_slice(&4u16.to_le_bytes());
    w.extend_from_slice(&16u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    w.extend(samples.iter().flat_map(|v| v.to_le_bytes()));
    w
}

/// 24-bit samples as a WAV file stores them.
fn wav24(samples: &[i32]) -> Vec<u8> {
    let data = samples.len() as u32 * 3;
    let mut w = Vec::with_capacity(44 + data as usize);
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&RATE.to_le_bytes());
    w.extend_from_slice(&(RATE * 6).to_le_bytes());
    w.extend_from_slice(&6u16.to_le_bytes());
    w.extend_from_slice(&24u16.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    w.extend(samples.iter().flat_map(|v| v.to_le_bytes()[..3].to_vec()));
    w
}

/// A file's bytes, shared between requests.
struct Bytes(Arc<Vec<u8>>);

impl AsRef<[u8]> for Bytes {
    fn as_ref(&self) -> &[u8] {
        &self.0
    }
}

/// A server with ranges: every request is counted, with where it started.
#[derive(Default)]
struct Server {
    files: Mutex<Vec<(String, Arc<Vec<u8>>)>>,
    requests: Mutex<Vec<(String, u64)>>,
    /// Songs whose answer takes this long to start.
    slow: Mutex<Vec<(String, Duration)>>,
    /// Songs whose connection breaks after this many bytes, and cannot be had again past them.
    cut: Mutex<Vec<(String, u64)>>,
}

/// A body that breaks off: the bytes up to the cut, then an error.
struct Broken(Cursor<Bytes>, u64);

impl std::io::Read for Broken {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let left = self.1.saturating_sub(self.0.position());
        if left == 0 {
            return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "reset"));
        }
        let n = buf.len().min(left as usize);
        self.0.read(&mut buf[..n])
    }
}

impl ByteSource for Server {
    fn open(&self, url: &str, from: u64) -> Result<Body, String> {
        self.requests.lock().push((url.to_string(), from));
        let slow = self.slow.lock().iter().find(|(u, _)| u == url).map(|s| s.1);
        if let Some(d) = slow {
            std::thread::sleep(d);
        }
        let file = self.files.lock().iter().find(|(u, _)| u == url).map(|(_, f)| f.clone()).ok_or("404")?;
        let len = file.len() as u64;
        let mut c = Cursor::new(Bytes(file));
        c.set_position(from);
        if let Some(at) = self.cut.lock().iter().find(|(u, _)| u == url).map(|s| s.1) {
            if from >= at {
                return Err("connection refused".into());
            }
            return Ok(Body { start: from, len: Some(len), reader: Box::new(Broken(c, at)) });
        }
        Ok(Body { start: from, len: Some(len), reader: Box::new(c) })
    }
}

/// The test's queue: a playlist, and the songs the user's settings skip on arrival (explicit ones).
struct TestQueue {
    list: Playlist,
    skip: Vec<String>,
}

impl nori_engine::Queue for TestQueue {
    fn read<R>(&self, f: impl FnOnce(&Playlist) -> R) -> R {
        f(&self.list)
    }

    fn moved_to(&mut self, index: usize) {
        self.list.moved_to(index);
    }

    fn set_repeat(&mut self, mode: u8) {
        self.list.set_repeat(mode);
    }

    fn skips(&self, index: usize) -> bool {
        self.skip.contains(&self.list.ids()[index]) && self.list.next_of(index, self.list.repeat()).is_some()
    }
}

/// What a test sets up beyond the songs.
#[derive(Default)]
struct Extra {
    float: bool,
    skip: Vec<String>,
    server: Arc<Server>,
    /// Songs go through this stream cache.
    store: Option<Arc<Store>>,
    /// Paused this long, the output is let go.
    idle_release_ms: Option<i64>,
    /// How much faster than real time the recorder plays: twenty times unless set.
    pace: Option<f64>,
}

struct Songs {
    server: Arc<Server>,
    lengths: Vec<(String, i64)>,
    store: Option<Arc<Store>>,
}

impl Library for Songs {
    fn locate(&mut self, id: &str) -> Result<Located, String> {
        let duration_ms = self.lengths.iter().find(|(i, _)| i == id).map(|s| s.1);
        let bytes: Arc<dyn ByteSource> = self.server.clone();
        let url = id.to_string();
        let source = match &self.store {
            Some(store) => Source::Cached { url, bytes, store: store.clone(), key: format!("{id}:0") },
            None => Source::Url { url, bytes },
        };
        Ok(Located { source, hint: Some("wav".into()), duration_ms })
    }

    fn about(&self, id: &str) -> WindowSong {
        let duration_ms = self.lengths.iter().find(|(i, _)| i == id).map_or(0, |s| s.1);
        WindowSong { id: id.into(), title: id.into(), duration_ms, ..Default::default() }
    }
}

/// A sound card on a fast clock that keeps everything it plays. `underruns` counts the times it
/// found too little to play: on a real device each would be a gap.
struct Recorder {
    pace: f64,
    /// The device plays float: what it pulls is kept in `heard_f` instead.
    float: bool,
    heard: Arc<Mutex<Vec<i16>>>,
    heard_f: Arc<Mutex<Vec<f32>>>,
    playing: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    underruns: Arc<AtomicU64>,
    /// Times the device was opened and let go.
    opened: Arc<AtomicU64>,
    shut: Arc<AtomicU64>,
    /// Where the engine hears which device the music goes to.
    watch: Arc<Mutex<Option<DeviceWatch>>>,
    /// Set by a test: the device dies at its next pull and will not open again, and says why here.
    die: Arc<AtomicBool>,
    failure: Arc<Mutex<Option<String>>>,
    /// Times the music the ring held was dropped.
    flushes: Arc<AtomicU64>,
    /// The engine asked for the device to be kept shallow (the equalizer tuned).
    shallow: Arc<AtomicBool>,
}

impl AudioOutput for Recorder {
    fn watch(&mut self, changed: DeviceWatch) {
        *self.watch.lock() = Some(changed);
    }

    fn open(&mut self, want: OutputFormat) -> Result<OutputFormat, String> {
        self.opened.fetch_add(1, Ordering::Relaxed);
        Ok(want)
    }

    fn start(&mut self, mut feed: Feed) -> Result<(), String> {
        self.closed = Arc::default();
        let (pace, heard, playing, closed, underruns) = (self.pace, self.heard.clone(), self.playing.clone(), self.closed.clone(), self.underruns.clone());
        let (float, heard_f) = (self.float, self.heard_f.clone());
        let (die, failure) = (self.die.clone(), self.failure.clone());
        std::thread::spawn(move || {
            let f = feed.format();
            let mut block = vec![0i16; 512 * f.channels];
            let mut floats = vec![0f32; 512 * f.channels];
            let period = Duration::from_secs_f64(512.0 / f.rate as f64 / pace);
            let mut started = false;
            while !closed.load(Ordering::Acquire) {
                std::thread::sleep(period);
                if die.swap(false, Ordering::AcqRel) {
                    *failure.lock() = Some("the sound server died".into());
                    feed.wake_engine();
                    return;
                }
                if !playing.load(Ordering::Acquire) || (!started && feed.available() < 512) {
                    continue;
                }
                started = true;
                // A test machine may be too busy to keep up with a clock this fast: the recorder then
                // waits rather than recording silence, and counts it.
                if feed.available() < 512 && !feed.ending() {
                    underruns.fetch_add(1, Ordering::Relaxed);
                    continue;
                }
                if float {
                    let got = feed.pull(&mut floats);
                    let n = if feed.ending() { got } else { 512 };
                    heard_f.lock().extend_from_slice(&floats[..n * f.channels]);
                    continue;
                }
                let got = feed.pull_i16(&mut block);
                let n = if feed.ending() { got } else { 512 };
                heard.lock().extend_from_slice(&block[..n * f.channels]);
            }
        });
        Ok(())
    }

    fn pause(&mut self) {
        self.playing.store(false, Ordering::Release);
    }

    fn resume(&mut self) {
        self.playing.store(true, Ordering::Release);
    }

    fn latency_us(&self) -> u64 {
        0
    }

    fn takes_float(&mut self) -> bool {
        self.float
    }

    fn failed(&mut self) -> Option<String> {
        self.failure.lock().take()
    }

    fn flush(&mut self) {
        self.flushes.fetch_add(1, Ordering::Relaxed);
    }

    fn shallow(&mut self, on: bool) {
        self.shallow.store(on, Ordering::Relaxed);
    }

    fn close(&mut self) {
        self.shut.fetch_add(1, Ordering::Relaxed);
        self.closed.store(true, Ordering::Release);
    }
}

struct Rig {
    engine: Engine,
    opened: Arc<AtomicU64>,
    shut: Arc<AtomicU64>,
    watch: Arc<Mutex<Option<DeviceWatch>>>,
    heard: Arc<Mutex<Vec<i16>>>,
    heard_f: Arc<Mutex<Vec<f32>>>,
    underruns: Arc<AtomicU64>,
    server: Arc<Server>,
    events: Arc<Mutex<Vec<Event>>>,
    die: Arc<AtomicBool>,
    flushes: Arc<AtomicU64>,
    shallow: Arc<AtomicBool>,
}

impl Rig {
    fn new(songs: &[(&str, &[i16])], prefs: TransitionPrefs, settings: Settings) -> Rig {
        let mut app = sim::App::new();
        app.prefs = prefs;
        Rig::with_app(songs, app, settings)
    }

    fn with_app(songs: &[(&str, &[i16])], app: impl App + Send + 'static, settings: Settings) -> Rig {
        let files = songs.iter().map(|(id, s)| (id.to_string(), wav(s), (s.len() / 2) as i64 * 1000 / RATE as i64)).collect();
        Rig::build(files, app, settings, Extra::default())
    }

    /// Songs as (id, file, length ms), played through a device that takes float or 16-bit samples.
    fn build(files: Vec<(String, Vec<u8>, i64)>, app: impl App + Send + 'static, settings: Settings, extra: Extra) -> Rig {
        let Extra { float, skip, server, store, idle_release_ms, pace } = extra;
        for (id, f, _) in &files {
            server.files.lock().push((id.clone(), Arc::new(f.clone())));
        }
        let lengths = files.iter().map(|(id, _, ms)| (id.clone(), *ms)).collect();
        let mut list = Playlist::default();
        list.set(files.iter().map(|(id, _, _)| id.clone()).collect(), Some(0), false, 0);
        let queue = TestQueue { list, skip };
        let heard = Arc::new(Mutex::new(Vec::new()));
        let heard_f = Arc::new(Mutex::new(Vec::new()));
        let underruns = Arc::new(AtomicU64::new(0));
        let out = Recorder {
            pace: pace.unwrap_or(20.0),
            float,
            heard: heard.clone(),
            heard_f: heard_f.clone(),
            playing: Arc::default(),
            closed: Arc::default(),
            underruns: underruns.clone(),
            opened: Arc::default(),
            shut: Arc::default(),
            watch: Arc::default(),
            die: Arc::default(),
            failure: Arc::default(),
            flushes: Arc::default(),
            shallow: Arc::default(),
        };
        let (opened, shut, watch, die, flushes, shallow) = (out.opened.clone(), out.shut.clone(), out.watch.clone(), out.die.clone(), out.flushes.clone(), out.shallow.clone());
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let library = Songs { server: server.clone(), lengths, store };
        let mut config = Config { memory_mb: 256, settings, ..Config::default() };
        config.idle_release_ms = idle_release_ms.unwrap_or(config.idle_release_ms);
        let engine = Engine::start(library, app, queue, Box::new(out), config, move |e| seen.lock().push(e));
        Rig { engine, opened, shut, watch, heard, heard_f, underruns, server, events, die, flushes, shallow }
    }

    /// Times the recorder found too little to play (a gap on a real device), for the failure messages.
    fn waits(&self) -> u64 {
        self.underruns.load(Ordering::Relaxed)
    }

    fn wait_for(&self, secs: u64, mut done: impl FnMut(&Rig) -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            if done(self) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn ended(&self) -> bool {
        self.events.lock().contains(&Event::State(State::Ended))
    }
}

/// What the simulated player hears of the same songs with the same settings.
fn reference(songs: &[(&str, &[i16])], prefs: TransitionPrefs) -> Vec<i16> {
    let tracks = songs.iter().map(|(id, s)| sim::Track::new(id, Audio::pcm(RATE, 2, s))).collect();
    let mut p = sim::Player::with_prefs(tracks, prefs);
    p.play_from(0);
    assert!(p.run_to_end(600_000));
    p.sink.heard_samples()
}

fn crossfade(secs: i32) -> TransitionPrefs {
    TransitionPrefs { crossfade_s: secs, keep_albums: true, ..prefs_off() }
}

#[test]
fn a_jump_or_a_seek_while_paused_stays_paused_and_fetches_nothing_until_play() {
    let (a, b, c) = (music(20.0, 91), music(20.0, 92), music(20.0, 93));
    let rig = Rig::new(&[("a", &a), ("b", &b), ("c", &c)], prefs_off(), Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2));
    rig.engine.pause();
    assert!(rig.wait_for(5, |r| r.engine.status().state == State::Paused));
    let asked = rig.server.requests.lock().len();
    rig.engine.go_to(1, 0);
    rig.engine.seek(5_000);
    assert!(rig.wait_for(5, |r| { let s = r.engine.status(); s.index == Some(1) && s.position_ms == 5_000 }), "the screen is told the place at once: {:?}", rig.engine.status());
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.engine.status().state, State::Paused, "still paused");
    assert_eq!(rig.server.requests.lock().len(), asked, "nothing fetched for a place nobody listens to yet");
    let heard = rig.heard.lock().len();
    rig.engine.play();
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > heard + 2 * RATE as usize), "play goes there");
    let played = rig.heard.lock()[heard..heard + 2 * RATE as usize].to_vec();
    let from = 5 * RATE as usize * 2;
    assert!(b[from..from + 2 * RATE as usize] == played[..], "b from five seconds in");
    // The skip button is another matter: paused, it is a request for music.
    rig.engine.pause();
    assert!(rig.wait_for(5, |r| r.engine.status().state == State::Paused));
    rig.engine.next();
    assert!(rig.wait_for(5, |r| { let s = r.engine.status(); s.state == State::Playing && s.index == Some(2) }), "{:?}", rig.engine.status());
}

#[test]
fn paused_at_the_end_of_a_song_it_waits_on_the_next_one() {
    let (a, b) = (music(30.0, 95), music(4.0, 96));
    let rig = Rig::new(&[("a", &a), ("b", &b)], crossfade(2), Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| !r.heard.lock().is_empty()));
    rig.engine.pause_at_end(true);
    assert!(rig.wait_for(10, |r| r.events.lock().contains(&Event::Stopped)), "{:?}", rig.events.lock());
    assert!(rig.wait_for(5, |r| { let s = r.engine.status(); s.state == State::Paused && s.index == Some(1) && s.position_ms == 0 }), "{:?}", rig.engine.status());
    let heard = rig.heard.lock().clone();
    // The recorder may miss the last frame or so as the output pauses under it.
    assert!(heard.len() <= a.len() && heard.len() + 8 >= a.len() && heard[..] == a[..heard.len()], "a to its end, nothing of b and no mix into it: {} of {} samples", heard.len(), a.len());
    rig.engine.play();
    assert!(rig.wait_for(10, Rig::ended), "play goes on with b");
    assert!(rig.heard.lock()[heard.len()..] == b[..], "b from its start");
}

#[test]
fn a_song_slow_to_come_is_said_to_be_buffering_until_it_plays() {
    let a = music(10.0, 81);
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let extra = Extra::default();
    extra.server.slow.lock().push(("a".into(), Duration::from_millis(1_500)));
    let rig = Rig::build(files(&songs), sim::App::new(), Settings::default(), extra);
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize), "the song plays in the end");
    assert!(rig.wait_for(5, |r| r.events.lock().contains(&Event::Buffering(false))), "{:?}", rig.events.lock());
    let events = rig.events.lock().clone();
    let on = events.iter().position(|e| *e == Event::Buffering(true)).unwrap_or_else(|| panic!("said while it waits: {events:?}"));
    let off = events.iter().position(|e| *e == Event::Buffering(false)).unwrap_or_else(|| panic!("and when it comes: {events:?}"));
    assert!(on < off, "{events:?}");
}

#[test]
fn a_device_that_dies_stops_the_engine_and_play_opens_another() {
    let a = music(20.0, 71);
    let rig = Rig::new(&[("a", &a)], prefs_off(), Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2), "music is heard");
    rig.die.store(true, Ordering::Release);
    let stopped = |r: &Rig| {
        let e = r.events.lock();
        e.iter().any(|e| matches!(e, Event::Error { message, .. } if message.contains("the output stopped: the sound server died"))) && e.last() == Some(&Event::State(State::Idle))
    };
    assert!(rig.wait_for(10, stopped), "the engine stops and says so: {:?}", rig.events.lock());
    assert!(rig.wait_for(5, |r| r.shut.load(Ordering::Relaxed) >= 1), "and lets the dead device go");
    let (opened, heard) = (rig.opened.load(Ordering::Relaxed), rig.heard.lock().len());
    rig.engine.play();
    assert!(rig.wait_for(10, |r| r.opened.load(Ordering::Relaxed) > opened && r.heard.lock().len() > heard + RATE as usize), "play opens another and the music goes on");
}

#[test]
fn two_songs_join_sample_for_sample_and_each_is_fetched_once() {
    let whole = music(50.0, 1);
    let cut = RATE as usize * 2 * 23 + 2 * 317;
    let songs: [(&str, &[i16]); 2] = [("a", &whole[..cut]), ("b", &whole[cut..])];
    let rig = Rig::new(&songs, prefs_off(), Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, Rig::ended), "played to the end: {:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    assert_eq!(heard.len(), whole.len(), "not a sample more or less ({} waits)", rig.waits());
    assert!(heard == whole, "the join is exact");
    // Each song's loader runs on a thread of its own, and b's starts as a does: either may ask first.
    let mut requests = rig.server.requests.lock().clone();
    requests.sort();
    assert_eq!(requests, vec![("a".to_string(), 0), ("b".to_string(), 0)], "one request per song, the whole song in one burst");
    let events = rig.events.lock().clone();
    assert!(events.contains(&Event::Song { index: 1, id: "b".into() }), "{events:?}");
}

#[test]
fn a_crossfade_is_heard_as_the_simulated_player_hears_it() {
    let (a, b) = (music(40.0, 2), music(40.0, 3));
    let songs: [(&str, &[i16]); 2] = [("a", &a), ("b", &b)];
    let expected = reference(&songs, crossfade(6));
    let rig = Rig::new(&songs, crossfade(6), Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, Rig::ended), "played to the end: {:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    assert_eq!(heard.len(), expected.len(), "80 s less the 6 s overlap ({} waits)", rig.waits());
    assert_eq!(heard.len(), a.len() + b.len() - RATE as usize * 2 * 6);
    let first = heard.iter().zip(&expected).position(|(x, y)| x != y);
    assert_eq!(first, None, "the mix starts at the planned sample and sounds the same");
}

#[test]
fn a_seek_lands_on_the_sample_asked_for() {
    let a = music(30.0, 4);
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let rig = Rig::new(&songs, prefs_off(), Settings::default());
    rig.engine.play_at(0, 20_000);
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    assert!(heard[..] == a[RATE as usize * 2 * 20..], "from 20 s to the end");
}

#[test]
fn a_seek_says_it_is_on_its_way_while_the_music_dips_before_it() {
    let a = music(30.0, 22);
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let rig = Rig::new(&songs, prefs_off(), Settings { fade_ms: 400, ..Settings::default() });
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2));
    rig.engine.seek(20_000);
    assert!(rig.wait_for(5, |r| r.engine.status().switching), "the dip is on");
    assert!(rig.engine.status().position_ms < 5_000, "and the place is still the one before the seek");
    assert!(rig.wait_for(5, |r| !r.engine.status().switching && r.engine.status().position_ms >= 20_000), "then it is the one asked for");
}

#[test]
fn pausing_stops_the_music_and_playing_takes_it_up_where_it_was() {
    let a = music(30.0, 5);
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let rig = Rig::new(&songs, prefs_off(), Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2 * 5));
    rig.engine.pause();
    assert!(rig.wait_for(5, |r| r.engine.status().state == State::Paused));
    assert!(!rig.events.lock().contains(&Event::Stopped), "a pause asked for is not one the engine made by itself");
    std::thread::sleep(Duration::from_millis(100));
    let at = rig.heard.lock().len();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.heard.lock().len(), at, "nothing plays while paused");
    // The place is where the ear stopped, though the engine slept through the seconds before.
    let heard_ms = (at / 2) as i64 * 1000 / RATE as i64;
    let place = rig.engine.status().position_now();
    assert!((place - heard_ms).abs() < 300, "paused at {place} ms, the ear at {heard_ms} ms");
    rig.engine.play();
    assert!(rig.wait_for(30, Rig::ended));
    let heard = rig.heard.lock().clone();
    assert!(heard == a, "every sample once, in order");
}

#[test]
fn a_pause_fades_out_and_play_fades_back_in() {
    let a = vec![8000i16; RATE as usize * 2 * 20];
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let rig = Rig::new(&songs, prefs_off(), Settings { fade_ms: 400, ..Settings::default() });
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2 * 4));
    rig.engine.pause();
    assert!(rig.wait_for(5, |r| r.engine.status().state == State::Paused));
    std::thread::sleep(Duration::from_millis(200));
    let at = rig.heard.lock().len();
    let heard = rig.heard.lock().clone();
    // Down to silence over the fade, not cut: the last samples before the pause are quiet, and there
    // is a stretch of samples between full and silent.
    assert!(heard[at - 2].abs() < 100, "faded to silence: {}", heard[at - 2]);
    assert!(heard.iter().any(|&v| v > 2000 && v < 6000), "a ramp down, not a cut");
    rig.engine.play();
    assert!(rig.wait_for(30, Rig::ended));
    let heard = rig.heard.lock().clone();
    assert!(heard[at..at + 200].iter().all(|&v| v < 8000), "back in from silence");
    assert_eq!(*heard.last().unwrap(), 8000);
}

#[test]
fn each_song_plays_at_its_replay_gain_volume() {
    let (a, b) = (music(12.0, 7), music(12.0, 8));
    let songs: [(&str, &[i16]); 2] = [("a", &a), ("b", &b)];
    let mut app = sim::App::new();
    app.gains.insert("a".into(), 0.5);
    let rig = Rig::with_app(&songs, app, Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    assert_eq!(heard.len(), a.len() + b.len(), "the volume leaves the timing alone");
    // The volume changes on the very sample b starts at.
    let off = heard[..a.len()].iter().zip(&a).position(|(h, s)| (*h as i32 - (*s as f64 / 2.0).round() as i32).abs() > 0);
    assert_eq!(off, None, "a at half its level, -6 dB");
    assert!(heard[a.len()..] == b[..], "b untouched at full volume");
}

/// A 24-bit song: 16-bit music with a low byte of its own under every sample.
fn music24(secs: f64, seed: u64) -> Vec<i32> {
    music(secs, seed).iter().enumerate().map(|(i, v)| ((*v as i32) << 8) | (i as i32 * 37 & 0xFF)).collect()
}

fn loud_eq() -> Settings {
    let bands = vec![nori_player::dsp::Band { kind: nori_player::dsp::PEAKING, freq: 1000.0, gain_db: 6.0, q: 1.0, channel: 0 }];
    Settings { sound: nori_engine::Sound { bands, ..Default::default() }, ..Settings::default() }
}

#[test]
fn high_quality_output_carries_a_24_bit_song_to_a_float_device_untouched() {
    let a = music24(8.0, 9);
    let files = vec![("a".to_string(), wav24(&a), 8_000)];
    // The equalizer is on, and stands aside: nothing may touch the samples.
    let rig = Rig::build(files, sim::App::new(), Settings { hi_res: true, ..loud_eq() }, Extra { float: true, ..Extra::default() });
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard_f.lock().clone();
    assert_eq!(heard.len(), a.len());
    let off = heard.iter().zip(&a).position(|(h, s)| *h != *s as f32 / 8_388_608.0);
    assert_eq!(off, None, "every one of the 24 bits, in float");
}

#[test]
fn a_device_that_takes_16_bit_only_gets_the_16_bit_chain() {
    let a = music24(8.0, 10);
    let files = vec![("a".to_string(), wav24(&a), 8_000)];
    let rig = Rig::build(files, sim::App::new(), Settings { hi_res: true, ..Settings::default() }, Extra::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    let expected: Vec<i16> = a.iter().map(|v| (*v as f32 / 256.0).round_ties_even() as i16).collect();
    assert!(heard == expected, "rounded to 16 bits as the decoder rounds");
    assert!(rig.heard_f.lock().is_empty());
}

fn files(songs: &[(&str, &[i16])]) -> Vec<(String, Vec<u8>, i64)> {
    songs.iter().map(|(id, s)| (id.to_string(), wav(s), (s.len() / 2) as i64 * 1000 / RATE as i64)).collect()
}

#[test]
fn a_song_whose_connection_fails_for_good_is_reported_and_skipped() {
    let (a, b) = (music(10.0, 11), music(6.0, 12));
    let songs: [(&str, &[i16]); 2] = [("a", &a), ("b", &b)];
    let extra = Extra::default();
    // Four seconds of a get through, then the connection breaks and will not open again.
    extra.server.cut.lock().push(("a".into(), 44 + RATE as u64 * 4 * 4));
    let rig = Rig::build(files(&songs), sim::App::new(), Settings::default(), extra);
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(40, Rig::ended), "{:?}", rig.events.lock());
    let events = rig.events.lock().clone();
    assert!(events.iter().any(|e| matches!(e, Event::Error { id, .. } if id == "a")), "{events:?}");
    let heard = rig.heard.lock().clone();
    // All but the last packet the reader was part way into when the bytes stopped.
    let four = RATE as usize * 2 * 39 / 10;
    assert!(heard[..four] == a[..four], "what came of a played: {} heard, {:?}", heard.len(), heard.iter().zip(&a).position(|(h, s)| h != s));
    assert!(heard[heard.len() - b.len()..] == b[..], "then b, whole");
}

#[test]
fn a_song_still_on_its_way_does_not_hold_the_engine_up() {
    let (slow, a) = (music(6.0, 13), music(6.0, 14));
    let songs: [(&str, &[i16]); 2] = [("slow", &slow), ("a", &a)];
    let extra = Extra::default();
    // Slower than the whole test is allowed to take: an engine that waited for it could not finish.
    extra.server.slow.lock().push(("slow".into(), Duration::from_secs(60)));
    let rig = Rig::build(files(&songs), sim::App::new(), Settings::default(), extra);
    rig.engine.play_at(0, 0);
    rig.engine.next();
    assert!(rig.wait_for(40, Rig::ended), "next was taken while slow was opening: {:?}", rig.events.lock());
    assert!(*rig.heard.lock() == a, "a, whole");
}

#[test]
fn an_explicit_song_the_settings_skip_is_never_heard() {
    let (a, x, b) = (music(5.0, 15), music(5.0, 16), music(5.0, 17));
    let songs: [(&str, &[i16]); 3] = [("a", &a), ("x", &x), ("b", &b)];
    let extra = Extra { skip: vec!["x".into()], ..Extra::default() };
    let server = extra.server.clone();
    let rig = Rig::build(files(&songs), sim::App::new(), Settings::default(), extra);
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    assert!(*rig.heard.lock() == [a.clone(), b.clone()].concat(), "a joined straight to b");
    assert!(!server.requests.lock().iter().any(|(u, _)| u == "x"), "x not even fetched");
}

#[test]
fn a_song_heard_again_plays_from_the_cache_without_asking_the_network() {
    let a = music(8.0, 18);
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let dir = std::env::temp_dir().join(format!("nori-cache-{}", std::process::id()));
    let store = Store::open(&dir, 64 << 20, Box::new(Recent::default())).unwrap();
    let first = Extra { store: Some(store.clone()), ..Extra::default() };
    let rig = Rig::build(files(&songs), sim::App::new(), Settings::default(), first);
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    assert_eq!(rig.server.requests.lock().len(), 1);
    drop(rig);
    assert!(store.cached("a:0").is_some(), "kept as it loaded");
    let again = Extra { store: Some(store.clone()), ..Extra::default() };
    let rig = Rig::build(files(&songs), sim::App::new(), Settings::default(), again);
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    assert!(rig.server.requests.lock().is_empty(), "{:?}", rig.server.requests.lock());
    assert!(*rig.heard.lock() == a, "the same song, from the disk");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_long_pause_lets_the_output_go_and_play_opens_it_again_where_it_was() {
    let a = music(12.0, 19);
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let extra = Extra { idle_release_ms: Some(300), ..Extra::default() };
    let rig = Rig::build(files(&songs), sim::App::new(), Settings::default(), extra);
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2 * 3));
    rig.engine.pause();
    assert!(rig.wait_for(5, |r| r.shut.load(Ordering::Relaxed) == 1), "let go after the idle time");
    assert_eq!(rig.engine.status().releases, 1);
    assert_eq!(rig.engine.status().state, State::Paused);
    rig.engine.play();
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    assert_eq!(rig.opened.load(Ordering::Relaxed), 2, "opened again on play");
    let heard = rig.heard.lock().clone();
    // Taken up again at the millisecond it stopped: a few frames at most heard twice.
    let again = heard.len() - a.len();
    assert!(again <= RATE as usize * 2 / 1000, "{again} samples heard again");
    let (head, tail) = (RATE as usize * 2 * 3, a.len() - RATE as usize * 2 * 6);
    assert!(heard[..head] == a[..head] && heard[heard.len() - tail..] == a[a.len() - tail..], "from the start, and on to the end");
}

#[test]
fn a_new_output_device_gets_its_own_sound() {
    let a = vec![8000i16; RATE as usize * 2 * 16];
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let mut app = sim::App::new();
    // Headphones bound to a profile that takes the level down 6 dB.
    let quieter = nori_engine::Sound { preamp_db: -6.0206, ..Default::default() };
    app.device_sounds.insert("Wired headphones".into(), quieter);
    let rig = Rig::with_app(&songs, app, Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2 * 2));
    // Paused, a sound that brings the equalizer into the chain is taken at once (playing, it waits
    // for the next song); either way the test does not race the engine.
    rig.engine.pause();
    assert!(rig.wait_for(10, |r| r.engine.status().state == State::Paused));
    let at = rig.heard.lock().len();
    let watch = rig.watch.lock();
    watch.as_ref().expect("the engine watches the output")(Device { kind: OutputKind::Wired, name: "Jack".into() });
    drop(watch);
    assert!(rig.wait_for(10, |r| r.events.lock().contains(&Event::Output { name: "Wired headphones".into() })));
    rig.engine.play();
    assert!(rig.wait_for(60, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    assert!(heard[..at].iter().all(|&v| v == 8000), "the speaker's sound up to the pause");
    let off = heard[at..].iter().position(|&v| (v - 4000).abs() > 1);
    assert_eq!(off, None, "the headphones' sound from then on");
}

#[test]
fn a_song_is_read_from_after_its_id3_tag_whatever_the_tag_holds() {
    // A tag of a cover's size whose bytes read as MPEG layer I frames, one after another: 32 kbps at
    // 44.1 kHz, 32 bytes a frame. Searched for music, it would be taken for that.
    let frames: Vec<u8> = (0..40_000).flat_map(|_| [[0xff, 0xff, 0x10, 0x00].as_slice(), &[0; 28]].concat()).collect();
    let syncsafe = |n: usize| [(n >> 21) as u8 & 0x7f, (n >> 14) as u8 & 0x7f, (n >> 7) as u8 & 0x7f, n as u8 & 0x7f];
    let a = music(1.0, 1);
    let mut file = [b"ID3".as_slice(), &[3, 0, 0], &syncsafe(frames.len())].concat();
    file.extend_from_slice(&frames);
    file.extend_from_slice(&wav(&a));
    let mut d = nori_engine::demux::Demuxed::open(Box::new(Cursor::new(file)), None, 0, None, nori_player::pcm::Encoding::Pcm16).unwrap();
    let mut out = Vec::new();
    while nori_player::pipeline::Reading::fill(&mut d) {
        out.extend(nori_player::pipeline::Reading::buffer(&d).chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])));
    }
    assert!(out == a, "the song after the tag, sample for sample: {} of {} samples", out.len(), a.len());
}

#[test]
fn play_after_a_song_would_not_play_tries_it_again() {
    let a = music(3.0, 21);
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let extra = Extra::default();
    // The connection will not open, however often it is tried.
    extra.server.cut.lock().push(("a".into(), 0));
    let rig = Rig::build(files(&songs), sim::App::new(), Settings::default(), extra);
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, |r| r.events.lock().iter().any(|e| matches!(e, Event::Error { id, .. } if id == "a"))), "{:?}", rig.events.lock());
    assert!(rig.wait_for(10, |r| r.engine.status().state == State::Paused), "stopped there: {:?}", rig.events.lock());
    assert!(rig.events.lock().contains(&Event::Stopped), "and says it stopped by itself: {:?}", rig.events.lock());
    // The network is back.
    rig.server.cut.lock().clear();
    rig.engine.play();
    assert!(rig.wait_for(20, Rig::ended), "{:?}", rig.events.lock());
    assert!(*rig.heard.lock() == a, "a, whole, not the clock run over nothing");
}

/// `s` at volume `gain`, rounded to 16 bits as the player rounds it.
fn at(s: &[i16], gain: f32) -> Vec<i16> {
    s.iter().map(|&v| (v as f32 * gain).round() as i16).collect()
}

#[test]
fn each_song_s_replay_gain_is_on_its_own_samples_through_a_crossfade() {
    let (a, b) = (music(40.0, 2), music(40.0, 3));
    // The songs turned to their volumes first and mixed after. One volume for the whole output put a's
    // on b through the whole mix, and the music jumped up to b's own where the mix ended.
    let ideal = reference(&[("a", &at(&a, 0.5)), ("b", &b)], crossfade(6));
    let mut app = sim::App::new();
    app.prefs = crossfade(6);
    app.gains.insert("a".into(), 0.5);
    let rig = Rig::with_app(&[("a", &a), ("b", &b)], app, Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    assert_eq!(heard.len(), ideal.len());
    let off = heard.iter().zip(&ideal).position(|(h, i)| h != i);
    assert_eq!(off, None, "every sample as a at half its level mixed into b");
}

// ---- settings changed while music plays ----

/// The simulated app, shared: a test changes what it answers (the planner's settings, the songs'
/// ReplayGain) while the engine plays, as the core's settings change under the phone's player.
#[derive(Clone)]
struct Live(Arc<Mutex<sim::App>>);

impl Live {
    fn new(prefs: TransitionPrefs) -> Live {
        let mut app = sim::App::new();
        app.prefs = prefs;
        Live(Arc::new(Mutex::new(app)))
    }
}

impl Host for Live {
    fn plan_for(&mut self, outgoing_id: &str) -> Option<Plan> {
        self.0.lock().plan_for(outgoing_id)
    }

    fn wants_analysis(&mut self, song_id: &str) -> Option<u64> {
        self.0.lock().wants_analysis(song_id)
    }

    fn analysed(&mut self, song_id: &str, analyzer: Analyzer, channels: usize, frames: u64, rate: u32) {
        self.0.lock().analysed(song_id, analyzer, channels, frames, rate);
    }

    fn log(&mut self, message: &str) {
        self.0.lock().log(message);
    }

    fn now_ms(&self) -> i64 {
        self.0.lock().now_ms()
    }
}

impl App for Live {
    fn clock(&mut self, now_ms: i64) {
        self.0.lock().clock(now_ms);
    }

    fn auto_mix(&self) -> bool {
        self.0.lock().auto_mix()
    }

    fn window(&mut self, window: Vec<WindowSong>, shuffling: bool) {
        self.0.lock().window(window, shuffling);
    }

    fn transitions_off(&mut self, off: bool) {
        self.0.lock().transitions_off(off);
    }

    fn gain(&mut self, index: usize, id: &str) -> f32 {
        self.0.lock().gain(index, id)
    }
}

/// Plays `songs` from the start and waits until two seconds of music were heard.
fn playing(songs: &[(&str, &[i16])], app: impl App + Send + 'static, settings: Settings) -> Rig {
    let rig = Rig::with_app(songs, app, settings);
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2 * 2));
    rig
}

#[test]
fn a_fade_set_while_playing_is_the_next_pause_s() {
    let a = vec![8000i16; RATE as usize * 2 * 20];
    let rig = playing(&[("a", &a)], sim::App::new(), Settings::default());
    // Changed in the settings with the music playing: nothing else happens to the player until the pause.
    rig.engine.set_settings(Settings { fade_ms: 400, ..Settings::default() });
    rig.engine.pause();
    assert!(rig.wait_for(5, |r| r.engine.status().state == State::Paused));
    std::thread::sleep(Duration::from_millis(200));
    let heard = rig.heard.lock().clone();
    assert!(heard[heard.len() - 2].abs() < 100, "faded to silence: {}", heard[heard.len() - 2]);
    assert!(heard.iter().any(|&v| v > 2000 && v < 6000), "a ramp down, not a cut");
}

#[test]
fn replay_gain_changed_while_playing_reaches_the_music_already_on_its_way() {
    let a = music(30.0, 31);
    let live = Live::new(prefs_off());
    let rig = playing(&[("a", &a)], live.clone(), Settings::default());
    live.0.lock().gains.insert("a".into(), 0.5);
    let asked = rig.heard.lock().len();
    rig.engine.gain_changed();
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    assert_eq!(heard.len(), a.len());
    let quiet = at(&a, 0.5);
    // Up to one place the song as it is, from there at half its level: what the output still held was
    // turned down where it lay, not left to play out at the old level for the ten seconds it lasts.
    let k = heard.iter().zip(&a).position(|(h, s)| h != s).expect("the level changed");
    // What the recorder pulled while the ring was being turned down may hold either level: the rescale
    // walks forward from where the output reads, and a busy machine can pause it for a few blocks before it
    // gets ahead. Nobody hears 100 ms at the old level; after that it must be the new one to the end.
    let settle = RATE as usize * 2 / 10;
    assert!(heard[k + settle..] == quiet[k + settle..], "at the new level from there to the end");
    // The recorder runs twenty times faster than the music: a second of it is 50 ms of the test's time.
    assert!(k < asked + RATE as usize * 2 * 2, "heard within two seconds of the change, not ten: {} s after", (k as f64 - asked as f64) / 2.0 / RATE as f64);
}

#[test]
fn bit_perfect_switched_on_while_playing_takes_replay_gain_off_the_next_song() {
    let (a, b) = (music(20.0, 32), music(10.0, 33));
    let mut app = sim::App::new();
    app.gains.insert("a".into(), 0.5);
    app.gains.insert("b".into(), 0.5);
    let rig = playing(&[("a", &a), ("b", &b)], app, Settings::default());
    rig.engine.set_output(OutputFacts { bit_perfect: true, ..OutputFacts::default() });
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    assert!(heard[heard.len() - b.len()..] == b[..], "b untouched, at its own level");
}

#[test]
fn a_longer_crossfade_set_while_playing_is_the_next_mix_s() {
    let (a, b) = (music(30.0, 34), music(20.0, 35));
    let live = Live::new(crossfade(2));
    let rig = playing(&[("a", &a), ("b", &b)], live.clone(), Settings::default());
    live.0.lock().prefs = crossfade(6);
    rig.engine.replan();
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    assert_eq!(rig.heard.lock().len(), a.len() + b.len() - RATE as usize * 2 * 6, "six seconds of overlap, not two");
}

#[test]
fn a_crossfade_switched_off_while_playing_leaves_the_songs_to_join_gaplessly() {
    let (a, b) = (music(30.0, 36), music(10.0, 37));
    let live = Live::new(crossfade(6));
    let rig = playing(&[("a", &a), ("b", &b)], live.clone(), Settings::default());
    live.0.lock().prefs = prefs_off();
    rig.engine.replan();
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    assert!(heard.len() == a.len() + b.len() && heard[..a.len()] == a[..] && heard[a.len()..] == b[..], "a then b, every sample");
}

/// A song measured as steady music at `bpm` from end to end.
fn measured(id: &str, bpm: f64, ms: i64) -> TrackAnalysis {
    TrackAnalysis {
        song_id: id.into(),
        analysis_version: ANALYSIS_VERSION,
        duration_ms: ms,
        bpm,
        bpm_confidence: 1.0,
        beat_offset_ms: 250.0,
        stability: 1.0,
        downbeat_confidence: 1.0,
        lufs: -14.0,
        silence_end_ms: ms,
        mixramp_end_ms: ms,
        intro_end_ms: 250,
        outro_start_ms: ms - 16_000,
        outro_bpm: bpm,
        outro_bpm_confidence: 1.0,
        outro_beat_offset_ms: 250.0,
        outro_stability: 1.0,
        intro_bpm: bpm,
        intro_bpm_confidence: 1.0,
        intro_beat_offset_ms: 250.0,
        intro_stability: 1.0,
        ..Default::default()
    }
}

#[test]
fn automix_switched_on_while_playing_mixes_out_of_the_song_playing() {
    let (a, b) = (music(40.0, 38), music(40.0, 39));
    let live = Live::new(prefs_off());
    let rig = playing(&[("a", &a), ("b", &b)], live.clone(), Settings::default());
    {
        let mut app = live.0.lock();
        app.analyses.insert("a".into(), measured("a", 120.0, 40_000));
        app.analyses.insert("b".into(), measured("b", 120.0, 40_000));
        app.prefs = TransitionPrefs { auto_mix: true, auto_mix_max_s: 12, echo_out: false, ..prefs_off() };
    }
    rig.engine.replan();
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let log = live.0.lock().log.clone();
    assert!(log.iter().any(|l| l.contains("transition a -> b: BeatMatched")), "{log:?}");
    assert!(log.iter().any(|l| l.contains("mixing: the next track arrived")), "{log:?}");
    assert!(rig.heard.lock().len() < a.len() + b.len(), "the songs overlap");
}

#[test]
fn a_speed_set_while_playing_is_heard() {
    let a = music(30.0, 40);
    let rig = playing(&[("a", &a)], sim::App::new(), Settings::default());
    rig.engine.set_settings(Settings { speed: 2.0, ..Settings::default() });
    assert!(rig.wait_for(5, |r| r.engine.status().speed == 2.0), "{:?}", rig.engine.status());
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    // Whatever the output held when it changed plays at the old speed: the rest at twice it.
    let heard = rig.heard.lock().len();
    assert!(heard < a.len() * 3 / 4, "{:.1} s of 30", heard as f64 / 2.0 / RATE as f64);
}

#[test]
fn silence_skipping_switched_on_while_playing_skips_the_silence_ahead() {
    let mut a = music(40.0, 41);
    let (from, to) = (RATE as usize * 2 * 24, RATE as usize * 2 * 34);
    a[from..to].fill(0);
    let rig = playing(&[("a", &a)], sim::App::new(), Settings::default());
    rig.engine.set_settings(Settings { skip_silence: true, ..Settings::default() });
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().len();
    assert!(heard < a.len() - RATE as usize * 2 * 6, "ten seconds of silence mostly skipped: {:.1} s of 40", heard as f64 / 2.0 / RATE as f64);
}

#[test]
fn an_equalizer_switched_on_while_playing_is_heard() {
    let a = music(30.0, 42);
    let rig = playing(&[("a", &a)], sim::App::new(), Settings::default());
    rig.engine.set_settings(loud_eq());
    assert!(rig.wait_for(5, |r| r.engine.status().chain), "{:?}", rig.engine.status());
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard.lock().clone();
    // Made again from where the ear was: to the millisecond, but for what the recorder, twenty times
    // faster than the music, pulled between the clock's reading and the flush, at the dip's silence.
    assert!(heard.len().abs_diff(a.len()) <= RATE as usize * 2 / 20, "{} samples of {}", heard.len(), a.len());
    // Changed two seconds in: as it is before, through the equalizer from there on.
    let end = RATE as usize * 2;
    assert!(heard[..end] == a[..end], "as it is before");
    let end = RATE as usize * 2 * 5;
    assert!(heard[heard.len() - end..] != a[a.len() - end..], "through the equalizer after");
}

/// `a` at `db` of pre-amplification: the sound chain on, doing nothing but turn the music down.
fn quieter(db: f64) -> Settings {
    Settings { sound: nori_engine::Sound { preamp_db: db, ..Default::default() }, ..Settings::default() }
}

/// Where `heard[from..]` goes on as `song` does, sample for sample, until the end of what was heard: the
/// first sample of that run, and how far from it the song's own is (samples; a frame is two).
fn as_the_song(heard: &[i16], song: &[i16], from: usize) -> Option<(usize, isize)> {
    let tail = 2 * RATE as usize / 10;
    let end = heard.len() - heard.len() % 2;
    let probe = &heard[end - tail..end];
    let at = (0..=song.len() - tail).step_by(2).find(|&k| song[k..k + tail] == *probe)?;
    let shift = at as isize - (end - tail) as isize;
    let mut k = end - tail;
    while k > from && song.get((k as isize - 2 + shift) as usize..(k as isize + shift) as usize) == Some(&heard[k - 2..k]) {
        k -= 2;
    }
    Some((k, shift))
}

#[test]
fn an_equalizer_switched_off_while_playing_is_heard_at_once_where_the_ear_is() {
    let a = music(20.0, 45);
    // A recorder at the music's own pace: what is heard when is what a phone would play then.
    let files = vec![("a".to_string(), wav(&a), 20_000)];
    let rig = Rig::build(files, sim::App::new(), quieter(-12.0), Extra { pace: Some(1.0), ..Extra::default() });
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2 * 2), "it plays");
    let (asked, waits) = (rig.heard.lock().len(), rig.waits());
    rig.engine.set_settings(Settings::default());
    std::thread::sleep(Duration::from_millis(1_000));
    let heard = rig.heard.lock().clone();
    let (k, shift) = as_the_song(&heard, &a, asked).expect("the song itself, untouched, after the change");
    let ms = |samples: usize| samples as f64 * 1000.0 / 2.0 / RATE as f64;
    // Ten seconds of the old sound were on their way in the ring: made again behind a 30 ms dip instead.
    assert!(ms(k - asked) <= 200.0, "heard {:.0} ms after the change", ms(k - asked));
    assert!(shift.unsigned_abs() <= 2 * RATE as usize / 1000 * 2, "on from where the ear was: {:.1} ms off", ms(shift.unsigned_abs()));
    assert!(rig.waits() <= waits + 2, "no gap past the dip: {} waits", rig.waits() - waits);
    // Before the change, quieter by 12 dB.
    let before = &heard[asked - 2_000..asked];
    let loud = |s: &[i16]| s.iter().map(|&v| (v as f64).powi(2)).sum::<f64>().sqrt();
    let ratio = loud(before) / loud(&a[asked - 2_000..asked]);
    assert!((ratio - 0.251).abs() < 0.02, "{ratio}");
    rig.engine.stop();
}

#[test]
fn a_slider_dragged_while_playing_is_made_heard_once_per_moment() {
    let a = music(20.0, 46);
    let files = vec![("a".to_string(), wav(&a), 20_000)];
    let rig = Rig::build(files, sim::App::new(), quieter(-3.0), Extra { pace: Some(1.0), ..Extra::default() });
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2));
    let flushes = rig.flushes.load(Ordering::Relaxed);
    // Ten steps in a tenth of a second, as a finger drags the pre-amp.
    let started = Instant::now();
    for k in 0..10 {
        rig.engine.set_settings(quieter(-4.0 - k as f64));
        std::thread::sleep(Duration::from_millis(10));
    }
    let took = started.elapsed().as_millis() as u64;
    std::thread::sleep(Duration::from_millis(1_500));
    let made = rig.flushes.load(Ordering::Relaxed) - flushes;
    assert!(made >= 1 && made <= took / 150 + 2, "{made} times made again for {took} ms of changes");
    // What is heard now is the last step's, 13 dB down: the music's level hardly moves from one half
    // second to the next.
    let heard = rig.heard.lock().clone();
    let loud = |s: &[i16]| (s.iter().map(|&v| (v as f64).powi(2)).sum::<f64>() / s.len() as f64).sqrt();
    let n = heard.len() - heard.len() % 2;
    let ratio = loud(&heard[n - RATE as usize..n]) / loud(&a);
    assert!((ratio - 10f64.powf(-13.0 / 20.0)).abs() < 0.012, "{ratio}");
    rig.engine.stop();
}

#[test]
fn high_quality_output_switched_on_while_playing_takes_the_next_song_untouched() {
    // a long enough that b is opened well after the change.
    let (a, b) = (music24(30.0, 43), music24(8.0, 44));
    let files = vec![("a".to_string(), wav24(&a), 30_000), ("b".to_string(), wav24(&b), 8_000)];
    let rig = Rig::build(files, sim::App::new(), Settings::default(), Extra { float: true, ..Extra::default() });
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard_f.lock().len() > RATE as usize * 2 * 2));
    rig.engine.set_settings(Settings { hi_res: true, ..Settings::default() });
    assert!(rig.wait_for(30, Rig::ended), "{:?}", rig.events.lock());
    let heard = rig.heard_f.lock().clone();
    let tail = &heard[heard.len() - b.len()..];
    let off = tail.iter().zip(&b).position(|(h, s)| *h != *s as f32 / 8_388_608.0);
    assert_eq!(off, None, "b in all its 24 bits, in float");
    assert!(heard[..RATE as usize * 2].iter().zip(&a).all(|(h, s)| *h == (*s as f32 / 256.0).round_ties_even() / 32768.0), "a as 16 bits before");
}

#[test]
fn the_equalizer_screen_makes_the_output_shallow_at_once_and_deep_again_as_it_closes() {
    let a = music(30.0, 47);
    let files = vec![("a".to_string(), wav(&a), 30_000)];
    let rig = Rig::build(files, sim::App::new(), loud_eq(), Extra { pace: Some(1.0), ..Extra::default() });
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2));
    let (flushes, waits) = (rig.flushes.load(Ordering::Relaxed), rig.waits());
    rig.engine.set_tuning(true);
    // Not at the next song: at once, the music made again behind a dip.
    assert!(rig.wait_for(2, |r| r.shallow.load(Ordering::Relaxed) && r.flushes.load(Ordering::Relaxed) > flushes), "shallow at once");
    std::thread::sleep(Duration::from_millis(1_500));
    assert!(rig.waits() <= waits + 2, "a shallow ring kept up with: {} waits", rig.waits() - waits);
    rig.engine.set_tuning(false);
    assert!(rig.wait_for(2, |r| !r.shallow.load(Ordering::Relaxed)), "deep again as the screen closes");
    rig.engine.stop();
}
