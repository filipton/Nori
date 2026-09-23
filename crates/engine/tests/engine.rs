//! The engine end to end on real threads: songs served as WAV bytes by a fake HTTP client that counts
//! every request, an output that records what a sound card would have played on a clock twenty times
//! faster than real time, and the simulated player (`nori_player::sim`) as the reference - the engine
//! must play exactly what the tested pipeline plays.

use std::io::Cursor;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use nori_engine::{AudioOutput, Body, ByteSource, Config, Engine, Event, Feed, Library, Located, OutputFormat, Settings, Source, State};
use nori_player::automix::synth::Rng;
use nori_player::playlist::Playlist;
use nori_player::sim::{self, prefs_off, Audio};
use nori_player::transitions::{TransitionPrefs, WindowSong};
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
}

impl ByteSource for Server {
    fn open(&self, url: &str, from: u64) -> Result<Body, String> {
        self.requests.lock().push((url.to_string(), from));
        let file = self.files.lock().iter().find(|(u, _)| u == url).map(|(_, f)| f.clone()).ok_or("404")?;
        let len = file.len() as u64;
        let mut c = Cursor::new(Bytes(file));
        c.set_position(from);
        Ok(Body { start: from, len: Some(len), reader: Box::new(c) })
    }
}

struct Songs {
    server: Arc<Server>,
    lengths: Vec<(String, i64)>,
}

impl Library for Songs {
    fn locate(&mut self, id: &str) -> Result<Located, String> {
        let duration_ms = self.lengths.iter().find(|(i, _)| i == id).map(|s| s.1);
        let bytes: Arc<dyn ByteSource> = self.server.clone();
        Ok(Located { source: Source::Url { url: id.to_string(), bytes }, hint: Some("wav".into()), duration_ms })
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
    heard: Arc<Mutex<Vec<i16>>>,
    playing: Arc<AtomicBool>,
    closed: Arc<AtomicBool>,
    underruns: Arc<AtomicU64>,
}

impl AudioOutput for Recorder {
    fn open(&mut self, want: OutputFormat) -> Result<OutputFormat, String> {
        Ok(want)
    }

    fn start(&mut self, mut feed: Feed) -> Result<(), String> {
        let (pace, heard, playing, closed, underruns) = (self.pace, self.heard.clone(), self.playing.clone(), self.closed.clone(), self.underruns.clone());
        std::thread::spawn(move || {
            let f = feed.format();
            let mut block = vec![0i16; 512 * f.channels];
            let period = Duration::from_secs_f64(512.0 / f.rate as f64 / pace);
            let mut started = false;
            while !closed.load(Ordering::Acquire) {
                std::thread::sleep(period);
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

    fn close(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

struct Rig {
    engine: Engine,
    heard: Arc<Mutex<Vec<i16>>>,
    underruns: Arc<AtomicU64>,
    server: Arc<Server>,
    events: Arc<Mutex<Vec<Event>>>,
}

impl Rig {
    fn new(songs: &[(&str, &[i16])], prefs: TransitionPrefs, settings: Settings) -> Rig {
        let server = Arc::new(Server::default());
        for (id, s) in songs {
            server.files.lock().push((id.to_string(), Arc::new(wav(s))));
        }
        let lengths = songs.iter().map(|(id, s)| (id.to_string(), (s.len() / 2) as i64 * 1000 / RATE as i64)).collect();
        let mut queue = Playlist::default();
        queue.set(songs.iter().map(|(id, _)| id.to_string()).collect(), Some(0), false, 0);
        let mut app = sim::App::new();
        app.prefs = prefs;
        let heard = Arc::new(Mutex::new(Vec::new()));
        let underruns = Arc::new(AtomicU64::new(0));
        let out = Recorder { pace: 20.0, heard: heard.clone(), playing: Arc::default(), closed: Arc::default(), underruns: underruns.clone() };
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let library = Songs { server: server.clone(), lengths };
        let engine = Engine::start(library, app, queue, Box::new(out), Config { memory_mb: 256, settings }, move |e| seen.lock().push(e));
        Rig { engine, heard, underruns, server, events }
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
    let requests = rig.server.requests.lock().clone();
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
fn pausing_stops_the_music_and_playing_takes_it_up_where_it_was() {
    let a = music(30.0, 5);
    let songs: [(&str, &[i16]); 1] = [("a", &a)];
    let rig = Rig::new(&songs, prefs_off(), Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait_for(10, |r| r.heard.lock().len() > RATE as usize * 2 * 5));
    rig.engine.pause();
    assert!(rig.wait_for(5, |r| r.engine.status().state == State::Paused));
    std::thread::sleep(Duration::from_millis(100));
    let at = rig.heard.lock().len();
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(rig.heard.lock().len(), at, "nothing plays while paused");
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
