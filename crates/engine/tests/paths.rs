//! The engine's other paths end to end on real threads: songs handed as packets to an output that
//! decodes them itself (a simulated offloaded AudioTrack whose play head the test moves), bit-perfect
//! output (every sample reaching the device as the file stores it, at its own rate and depth), a live
//! stream with the station's announcements in it, the offline bridge taking a song the network would not
//! bring, and repeat one reported as loops.
//!
//! The compressed songs are made by ffmpeg on the machine running the tests; without it the tests that
//! need them say so and pass.

use std::collections::VecDeque;
use std::io::{Cursor, Read};
use std::path::Path;
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::Thread;
use std::time::{Duration, Instant};

use nori_engine::{AudioOutput, Body, ByteSource, Coded, Coding, Config, Engine, Event, Feed, Library, Located, OffloadOutput, OutputFacts, OutputFormat, Settings, SharedQueue, Source, State, Support};
use nori_player::dsp::Band;
use nori_player::engine::{Host, Plan};
use nori_player::automix::analysis::Analyzer;
use nori_player::pipeline::{App, Sound};
use nori_player::playlist::REPEAT_ONE;
use nori_player::queue::{OnError, PlaybackError};
use nori_player::sim;
use nori_player::transitions::WindowSong;
use parking_lot::Mutex;

// ---- songs ----

fn ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

/// A tone of `secs` encoded by ffmpeg with `codec` into a file of extension `ext`, its bytes.
fn made(dir: &Path, name: &str, secs: u32, hz: u32, codec: &[&str], ext: &str) -> Vec<u8> {
    let out = dir.join(format!("{name}.{ext}"));
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={hz}:sample_rate=44100:duration={secs}"), "-ac", "2"])
        .args(codec)
        .arg(&out)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "ffmpeg made {name}");
    std::fs::read(out).unwrap()
}

fn dir() -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("nori-paths-{}-{}", std::process::id(), nanos()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn nanos() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
}

fn mp3(dir: &Path, name: &str, secs: u32, hz: u32) -> Vec<u8> {
    made(dir, name, secs, hz, &["-c:a", "libmp3lame", "-b:a", "128k"], "mp3")
}

fn flac(dir: &Path, name: &str, secs: u32, hz: u32) -> Vec<u8> {
    made(dir, name, secs, hz, &["-c:a", "flac"], "flac")
}

/// Samples as a WAV file stores them: `bits` of 16 or 24, interleaved stereo.
fn wav(rate: u32, bits: u16, samples: &[i32]) -> Vec<u8> {
    let width = bits as u32 / 8;
    let data = samples.len() as u32 * width;
    let mut w = Vec::new();
    w.extend_from_slice(b"RIFF");
    w.extend_from_slice(&(36 + data).to_le_bytes());
    w.extend_from_slice(b"WAVEfmt ");
    w.extend_from_slice(&16u32.to_le_bytes());
    w.extend_from_slice(&1u16.to_le_bytes());
    w.extend_from_slice(&2u16.to_le_bytes());
    w.extend_from_slice(&rate.to_le_bytes());
    w.extend_from_slice(&(rate * 2 * width).to_le_bytes());
    w.extend_from_slice(&(2 * width as u16).to_le_bytes());
    w.extend_from_slice(&bits.to_le_bytes());
    w.extend_from_slice(b"data");
    w.extend_from_slice(&data.to_le_bytes());
    for v in samples {
        w.extend_from_slice(&v.to_le_bytes()[..width as usize]);
    }
    w
}

/// Every sample a different value, spread over the whole range of `bits`.
fn ramp(frames: usize, bits: u32, seed: i64) -> Vec<i32> {
    let max = (1i64 << (bits - 1)) - 1;
    (0..frames * 2).map(|i| (((i as i64 * 7919 + seed * 104_729) % (2 * max)) - max) as i32).collect()
}

// ---- where songs come from ----

#[derive(Default)]
struct Server {
    files: Mutex<Vec<(String, Arc<Vec<u8>>)>>,
    /// Songs the network will not bring.
    down: Mutex<Vec<String>>,
}

impl ByteSource for Server {
    fn open(&self, url: &str, from: u64) -> Result<Body, String> {
        if self.down.lock().iter().any(|d| d == url) {
            return Err("the network is gone".into());
        }
        let f = self.files.lock().iter().find(|(u, _)| u == url).map(|(_, f)| f.clone()).ok_or("404")?;
        let len = f.len() as u64;
        Ok(Body { start: from, len: Some(len), reader: Box::new(Cursor::new(f[from as usize..].to_vec())) })
    }
}

/// The songs (id, container, length ms), and the albums some are on (id, album, track number).
struct Songs(Arc<Server>, Vec<(String, String, i64)>, Vec<(String, String, i32)>);

impl Library for Songs {
    fn locate(&mut self, id: &str) -> Result<Located, String> {
        let (_, hint, ms) = self.1.iter().find(|(i, _, _)| i == id).cloned().ok_or("no such song")?;
        Ok(Located { source: Source::Url { url: id.to_string(), bytes: self.0.clone() }, hint: Some(hint), duration_ms: Some(ms) })
    }

    fn about(&self, id: &str) -> WindowSong {
        let ms = self.1.iter().find(|(i, _, _)| i == id).map_or(0, |s| s.2);
        let (album_id, track) = self.2.iter().find(|(i, _, _)| i == id).map_or((None, 0), |a| (Some(a.1.clone()), a.2));
        WindowSong { id: id.into(), title: id.into(), duration_ms: ms, album_id, disc: 1, track, ..Default::default() }
    }
}

// ---- the CPU's output ----

/// A sound card on a fast clock that keeps everything it plays, as float, and the formats it was opened in.
#[derive(Clone, Default)]
struct Card {
    heard: Arc<Mutex<Vec<f32>>>,
    playing: Arc<AtomicBool>,
    opened: Arc<Mutex<Vec<OutputFormat>>>,
    closed: Arc<AtomicBool>,
}

impl AudioOutput for Card {
    fn open(&mut self, want: OutputFormat) -> Result<OutputFormat, String> {
        self.opened.lock().push(want);
        Ok(want)
    }

    fn start(&mut self, mut feed: Feed) -> Result<(), String> {
        let closed = Arc::new(AtomicBool::new(false));
        self.closed = closed.clone();
        let (heard, playing) = (self.heard.clone(), self.playing.clone());
        std::thread::spawn(move || {
            let f = feed.format();
            let mut block = vec![0f32; 512 * f.channels];
            let period = Duration::from_secs_f64(512.0 / f.rate as f64 / 20.0);
            while !closed.load(Ordering::Acquire) {
                std::thread::sleep(period);
                if !playing.load(Ordering::Acquire) || (feed.available() < 512 && !feed.ending()) {
                    continue;
                }
                let got = feed.pull(&mut block);
                heard.lock().extend_from_slice(&block[..got * f.channels]);
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
        true
    }

    fn close(&mut self) {
        self.closed.store(true, Ordering::Release);
    }
}

// ---- the output that decodes songs itself ----

#[derive(Debug, Clone, PartialEq)]
enum Call {
    Open(Coded),
    DelayPadding(u32, u32),
    Write(usize, u64),
    EndOfStream,
    Play,
    Pause,
    Flush,
    Volume(f32),
    Close,
}

/// An offloaded AudioTrack on a clock the test moves: it takes what fits in its buffer, plays what it
/// was given as far as the test says, and can be torn down.
#[derive(Default)]
struct Chip {
    support: Vec<(Coding, Support)>,
    calls: Vec<Call>,
    capacity: usize,
    /// What it holds and has not played: bytes and the frames they stand for.
    held: VecDeque<(usize, u64)>,
    written: u64,
    /// Every byte it took.
    bytes: Vec<u8>,
    head: u64,
    /// The frames written up to the last end of stream.
    ended_at: Option<u64>,
    torn: bool,
    open: bool,
    engine: Option<Thread>,
    /// Playing, as Android's track would say: an end of stream is taken only then.
    playing: bool,
    /// Readings the platform gives before its true count, one each time it is asked: none for a call
    /// that failed, or a number that makes no sense.
    readings: VecDeque<Option<u64>>,
    /// Where the platform's count started again from nought (a standby), in the frames it played.
    offset: u64,
    /// The platform says it presented everything, whatever it played (a late word about another song).
    stale_presented: bool,
    /// How many times faster than the music the test moves the play head (none: as fast as it likes).
    pace: Option<f64>,
    /// How many times what it was asked for it takes, as a phone that takes whole songs at once.
    takes: usize,
    /// What each note said.
    notes: Vec<String>,
    /// Ends of stream it refuses though it plays (Android's track still stopping from the one before).
    refuse_eos: u32,
}

#[derive(Clone, Default)]
struct Fake(Arc<Mutex<Chip>>);

impl Fake {
    fn new(support: &[(Coding, Support)]) -> Fake {
        let f = Fake::default();
        f.0.lock().support = support.to_vec();
        f
    }

    /// The chip plays `frames` more of what it holds, and wakes the engine as the platform does.
    fn advance(&self, frames: u64) {
        self.play_on(frames, true);
    }

    /// The chip plays `frames` more without the engine hearing of it: it reads the head when it next
    /// wakes for something else.
    fn advance_quietly(&self, frames: u64) {
        self.play_on(frames, false);
    }

    fn play_on(&self, frames: u64, wake: bool) {
        let mut c = self.0.lock();
        let to = (c.head + frames).min(c.written);
        let mut n = to - c.head;
        c.head = to;
        while n > 0 {
            let Some((bytes, f)) = c.held.front_mut() else { break };
            if *f <= n {
                n -= *f;
                c.held.pop_front();
            } else {
                let part = (*bytes as u128 * n as u128 / *f as u128) as usize;
                *bytes -= part;
                *f -= n;
                n = 0;
            }
        }
        if let Some(t) = c.engine.as_ref().filter(|_| wake) {
            t.unpark();
        }
    }

    fn tear_down(&self) {
        let mut c = self.0.lock();
        c.torn = true;
        if let Some(t) = &c.engine {
            t.unpark();
        }
    }

    fn calls(&self) -> Vec<Call> {
        self.0.lock().calls.iter().filter(|c| !matches!(c, Call::Volume(_))).cloned().collect()
    }

    fn written(&self) -> u64 {
        self.0.lock().written
    }

    /// The next readings of the play head, before its true count again.
    fn read_as(&self, readings: &[Option<u64>]) {
        let mut c = self.0.lock();
        c.readings.extend(readings.iter().copied());
        if let Some(t) = &c.engine {
            t.unpark();
        }
    }

    /// The platform starts counting again from nought, where the head is.
    fn count_again(&self) {
        let mut c = self.0.lock();
        c.offset = c.head;
    }

    fn pace(&self, pace: Option<f64>) {
        self.0.lock().pace = pace;
    }

    fn notes(&self) -> Vec<String> {
        self.0.lock().notes.clone()
    }
}

impl OffloadOutput for Fake {
    fn supports(&mut self, coded: Coded) -> Support {
        self.0.lock().support.iter().find(|(c, _)| *c == coded.coding).map_or(Support::No, |s| s.1)
    }

    fn open(&mut self, coded: Coded, bytes: usize) -> Result<usize, String> {
        let mut c = self.0.lock();
        c.calls.push(Call::Open(coded));
        c.capacity = bytes * c.takes.max(1);
        c.held.clear();
        c.written = 0;
        c.head = 0;
        c.offset = 0;
        c.ended_at = None;
        c.torn = false;
        c.open = true;
        c.playing = false;
        c.engine = Some(std::thread::current());
        Ok(bytes)
    }

    fn write(&mut self, data: &[u8], frames: u64) -> Result<usize, i32> {
        let mut c = self.0.lock();
        if c.torn {
            return Err(-6);
        }
        let held: usize = c.held.iter().map(|h| h.0).sum();
        let n = data.len().min(c.capacity.saturating_sub(held));
        if n > 0 {
            let f = if n == data.len() { frames } else { (frames as u128 * n as u128 / data.len() as u128) as u64 };
            c.held.push_back((n, f));
            c.written += f;
            c.calls.push(Call::Write(n, f));
            c.bytes.extend_from_slice(&data[..n]);
        }
        Ok(n)
    }

    fn delay_padding(&mut self, delay: u32, padding: u32) {
        self.0.lock().calls.push(Call::DelayPadding(delay, padding));
    }

    fn end_of_stream(&mut self) -> bool {
        let mut c = self.0.lock();
        if !c.playing {
            return false;
        }
        if c.refuse_eos > 0 {
            c.refuse_eos -= 1;
            return false;
        }
        c.calls.push(Call::EndOfStream);
        c.ended_at = Some(c.written);
        // What it said of the end of stream before is not about this one.
        c.stale_presented = false;
        true
    }

    fn play(&mut self) {
        let mut c = self.0.lock();
        c.calls.push(Call::Play);
        c.playing = true;
    }

    fn pause(&mut self) {
        let mut c = self.0.lock();
        c.calls.push(Call::Pause);
        c.playing = false;
    }

    fn flush(&mut self) {
        let mut c = self.0.lock();
        c.calls.push(Call::Flush);
        c.held.clear();
        c.written = 0;
        c.head = 0;
        c.offset = 0;
        c.ended_at = None;
    }

    fn set_volume(&mut self, volume: f32) {
        self.0.lock().calls.push(Call::Volume(volume));
    }

    fn head(&mut self) -> Option<u64> {
        let mut c = self.0.lock();
        match c.readings.pop_front() {
            Some(r) => r,
            None => Some(c.head - c.offset),
        }
    }

    fn presented(&mut self) -> bool {
        let c = self.0.lock();
        c.stale_presented || c.ended_at.is_some_and(|e| c.head >= e)
    }

    fn note(&mut self, what: &str) {
        self.0.lock().notes.push(what.to_string());
    }

    fn pace(&self) -> f64 {
        self.0.lock().pace.unwrap_or(1e6)
    }

    fn torn_down(&mut self) -> bool {
        self.0.lock().torn
    }

    fn close(&mut self) {
        let mut c = self.0.lock();
        c.calls.push(Call::Close);
        c.open = false;
        c.playing = false;
    }
}

// ---- the rig ----

struct Rig {
    engine: Engine,
    card: Card,
    queue: SharedQueue,
    events: Arc<Mutex<Vec<Event>>>,
}

impl Rig {
    fn new(server: Arc<Server>, songs: Vec<(String, String, i64)>, app: impl App + Send + 'static, fake: Option<Fake>, settings: Settings) -> Rig {
        Rig::albums(server, songs, Vec::new(), app, fake, settings)
    }

    /// [`Rig::new`], with some of the songs on albums: (id, album, track number).
    fn albums(server: Arc<Server>, songs: Vec<(String, String, i64)>, albums: Vec<(String, String, i32)>, app: impl App + Send + 'static, fake: Option<Fake>, settings: Settings) -> Rig {
        let queue = SharedQueue::default();
        queue.0.lock().set(songs.iter().map(|s| s.0.clone()).collect(), Some(0), false, 0);
        let card = Card::default();
        let events = Arc::new(Mutex::new(Vec::new()));
        let seen = events.clone();
        let config = Config { memory_mb: 256, settings, ..Config::default() };
        let offload = fake.map(|f| Box::new(f) as Box<dyn OffloadOutput>);
        let engine = Engine::start_with(Songs(server, songs, albums), app, queue.clone(), Box::new(card.clone()), offload, config, move |e| seen.lock().push(e));
        engine.queue_changed();
        Rig { engine, card, queue, events }
    }

    fn wait(&self, secs: u64, mut done: impl FnMut(&Rig) -> bool) -> bool {
        let until = Instant::now() + Duration::from_secs(secs);
        while Instant::now() < until {
            if done(self) {
                return true;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        false
    }

    fn heard_song(&self, id: &str) -> bool {
        self.events.lock().iter().any(|e| matches!(e, Event::Song { id: i, .. } if i == id))
    }
}

fn app() -> sim::App {
    let mut a = sim::App::new();
    a.prefs = sim::prefs_off();
    a
}

fn offload() -> Settings {
    Settings { offload: true, ..Settings::default() }
}

fn serve(server: &Server, files: &[(&str, &[u8])]) {
    for (id, f) in files {
        server.files.lock().push((id.to_string(), Arc::new(f.to_vec())));
    }
}

const MP3_ONLY: &[(Coding, Support)] = &[(Coding::Mp3, Support::Gapless)];

/// The tracks opened for the chip.
fn opens(fake: &Fake) -> usize {
    fake.calls().iter().filter(|c| matches!(c, Call::Open(_))).count()
}

/// What was done with the track opened last.
fn after_last_open(calls: &[Call]) -> Vec<&Call> {
    let from = calls.iter().rposition(|c| matches!(c, Call::Open(_))).unwrap_or(0);
    calls[from..].iter().collect()
}

// ---- offload ----

#[test]
fn songs_of_one_format_join_on_one_offloaded_track_each_with_its_delay_and_padding() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let (a, b) = (mp3(&d, "a", 20, 440), mp3(&d, "b", 20, 660));
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a), ("b", &b)]);
    let fake = Fake::new(MP3_ONLY);
    let mut app = app();
    app.gains.insert("b".into(), 0.5);
    let songs = vec![("a".into(), "mp3".into(), 20_000), ("b".into(), "mp3".into(), 20_000)];
    let rig = Rig::new(server, songs, app, Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    // Both songs are short beside the track: all of them goes in at once, the end of the queue closing it.
    assert!(rig.wait(10, |_| fake.calls().iter().filter(|c| **c == Call::EndOfStream).count() == 2), "{:?}", fake.calls());
    let calls = fake.calls();
    let opens: Vec<&Call> = calls.iter().filter(|c| matches!(c, Call::Open(_))).collect();
    assert_eq!(opens.len(), 1, "one track for both: {calls:?}");
    assert!(matches!(opens[0], Call::Open(Coded { coding: Coding::Mp3, rate: 44_100, channels: 2, .. })));
    // Each song's delay and padding before its first packet, and the end of stream between them.
    let marks: Vec<&Call> = calls.iter().filter(|c| matches!(c, Call::DelayPadding(..) | Call::EndOfStream)).collect();
    assert_eq!(marks.len(), 4, "{marks:?}");
    let (Call::DelayPadding(d1, p1), Call::EndOfStream, Call::DelayPadding(d2, p2), Call::EndOfStream) = (marks[0], marks[1], marks[2], marks[3]) else { panic!("{marks:?}") };
    // LAME's numbers as media3 hands them over: the encoder's delay (576), and padding past it.
    assert_eq!((*d1, *d2), (576, 576), "the LAME tag's delay");
    assert!(*p1 > 0 && *p2 > 0);
    // Every frame of both songs, and nothing else.
    let frames = fake.written();
    assert_eq!(frames, 2 * 20 * 44_100, "the music, delay and padding cut: {frames}");
    // The ear reaches the second song: said, at its own volume.
    fake.advance(20 * 44_100 + 100);
    assert!(rig.wait(5, |r| r.heard_song("b")), "{:?}", rig.events.lock());
    assert!(rig.wait(5, |_| fake.0.lock().calls.contains(&Call::Volume(0.5))), "b plays at its ReplayGain volume");
    assert!(rig.engine.status().offloaded);
    let at = rig.engine.status().position_ms;
    assert!((0..100).contains(&at), "at the start of b: {at}");
    // Played out: the end of the queue.
    fake.advance(20 * 44_100);
    assert!(rig.wait(5, |r| r.events.lock().contains(&Event::State(State::Ended))), "{:?}", rig.events.lock());
    assert!(rig.card.opened.lock().is_empty(), "the CPU's output was never opened");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_track_that_is_torn_down_hands_the_music_to_the_cpu_where_the_ear_is() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let a = mp3(&d, "a", 20, 440);
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a)]);
    let fake = Fake::new(MP3_ONLY);
    let rig = Rig::new(server, vec![("a".into(), "mp3".into(), 20_000)], app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |_| fake.written() > 0));
    fake.advance(5 * 44_100);
    assert!(rig.wait(5, |r| r.engine.status().position_ms >= 4_900), "{:?}", rig.engine.status());
    fake.tear_down();
    // The CPU plays on from five seconds in.
    assert!(rig.wait(10, |r| r.card.heard.lock().len() > 44_100), "the CPU took over");
    assert!(fake.calls().contains(&Call::Close), "the torn track was let go");
    let s = rig.engine.status();
    assert!(!s.offloaded && s.position_ms >= 5_000, "{s:?}");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn offload_is_taken_up_where_the_ear_is_once_nothing_touches_the_samples_and_given_up_at_once() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let (a, b) = (mp3(&d, "a", 120, 440), mp3(&d, "b", 60, 660));
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a), ("b", &b)]);
    let fake = Fake::new(MP3_ONLY);
    let songs = vec![("a".into(), "mp3".into(), 120_000), ("b".into(), "mp3".into(), 60_000)];
    // Offload on, and the equalizer with it: the CPU plays, the samples being touched.
    let eq = Sound { bands: vec![Band { kind: 0, freq: 1000.0, gain_db: 3.0, q: 1.0, channel: 0 }], ..Sound::default() };
    let rig = Rig::new(server, songs, app(), Some(fake.clone()), Settings { sound: eq.clone(), ..offload() });
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |r| r.card.heard.lock().len() > 2 * 44_100), "the CPU plays a");
    let (before, asked) = (rig.engine.status().position_ms, Instant::now());
    // The equalizer off: nothing touches the samples any more.
    rig.engine.set_settings(offload());
    // Not at the next song: at once, where the ear is, behind a dip.
    assert!(rig.wait(10, |r| r.engine.status().offloaded), "{:?}", rig.engine.status().pcm_why);
    let s = rig.engine.status();
    // The card plays twenty times faster than the music: what it moved on while the switch was made.
    let moved = asked.elapsed().as_millis() as i64 * 20 + 500;
    assert!(s.index == Some(0) && s.position_ms >= before && s.position_ms <= before + moved, "a from where it was ({before} ms): {s:?}");
    let calls = fake.calls();
    assert!(matches!(calls.iter().find(|c| matches!(c, Call::DelayPadding(..))), Some(Call::DelayPadding(0, _))), "a from part way in, no delay to cut: {calls:?}");
    // The equalizer on: off offload at once, where the ear is.
    fake.advance(10 * 44_100);
    let at = rig.engine.status().position_ms;
    assert!(rig.wait(5, |r| r.engine.status().position_ms >= at + 9_900));
    let heard = rig.card.heard.lock().len();
    rig.engine.set_settings(Settings { sound: eq, ..offload() });
    assert!(rig.wait(10, |r| r.card.heard.lock().len() > heard + 44_100), "the CPU plays a on");
    assert!(fake.calls().contains(&Call::Close));
    assert!(rig.wait(5, |r| !r.engine.status().offloaded), "the status follows once the burst is in");
    let s = rig.engine.status();
    assert!(s.index == Some(0) && s.position_ms >= at + 10_000, "{s:?}");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_song_the_chip_does_not_decode_plays_on_the_cpu_between_songs_it_does() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let (a, b, c) = (mp3(&d, "a", 10, 440), flac(&d, "b", 30, 550), mp3(&d, "c", 10, 660));
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a), ("b", &b), ("c", &c)]);
    let fake = Fake::new(MP3_ONLY);
    let songs = vec![("a".into(), "mp3".into(), 10_000), ("b".into(), "flac".into(), 30_000), ("c".into(), "mp3".into(), 10_000)];
    let rig = Rig::new(server, songs, app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |_| fake.calls().contains(&Call::EndOfStream)), "{:?}", fake.calls());
    assert_eq!(fake.calls().iter().filter(|c| matches!(c, Call::DelayPadding(..))).count(), 1, "the FLAC song is not written to the chip");
    // a played out: the FLAC song on the CPU.
    fake.advance(10 * 44_100);
    assert!(rig.wait(10, |r| r.heard_song("b") && !r.card.heard.lock().is_empty()), "{:?}", rig.events.lock());
    assert!(fake.calls().contains(&Call::Close));
    // And back to the chip for c, once the CPU has played b to its end.
    assert!(rig.wait(20, |r| r.heard_song("c")), "{:?}", rig.events.lock());
    assert!(rig.wait(5, |r| r.engine.status().offloaded), "c is the chip's");
    let heard = rig.card.heard.lock().len() / 2;
    assert!(heard + 44_100 / 2 >= 30 * 44_100, "b whole on the CPU: {heard}");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_seek_on_the_chip_empties_its_track_and_starts_again_at_a_packet() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let a = mp3(&d, "a", 30, 440);
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a)]);
    let fake = Fake::new(MP3_ONLY);
    let rig = Rig::new(server, vec![("a".into(), "mp3".into(), 30_000)], app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    // All of it is written, and its end of stream said, which stops Android's track until it gets there.
    assert!(rig.wait(10, |_| fake.calls().contains(&Call::EndOfStream)), "{:?}", fake.calls());
    rig.engine.seek(12_000);
    // So the seek lets that track go and opens another, as media3 does, rather than flushing it.
    assert!(rig.wait(5, |_| opens(&fake) == 2 && fake.written() > 0), "{:?}", fake.calls());
    let calls = fake.calls();
    assert!(!calls.contains(&Call::Flush), "a stopped track is not flushed: {calls:?}");
    assert!(calls.iter().position(|c| *c == Call::Close) < calls.iter().rposition(|c| matches!(c, Call::Open(_))), "{calls:?}");
    let after = after_last_open(&calls);
    assert!(matches!(after.iter().find(|c| matches!(c, Call::DelayPadding(..))), Some(Call::DelayPadding(0, _))), "no delay to cut part way in: {after:?}");
    // The place is the packet's the seek landed in (a few frames back, for the decoder's reservoir), and
    // what is written is the song from there.
    let at = rig.engine.status().position_ms;
    assert!((11_800..=12_000).contains(&at), "{at}");
    let frames = fake.written() as i64;
    assert!((frames + at * 44_100 / 1000 - 30 * 44_100).abs() <= 1152, "{frames} frames from {at} ms");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn repeat_one_on_the_chip_joins_the_song_to_itself_and_says_each_loop() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let a = mp3(&d, "a", 5, 440);
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a)]);
    let fake = Fake::new(MP3_ONLY);
    let rig = Rig::new(server, vec![("a".into(), "mp3".into(), 5_000)], app(), Some(fake.clone()), offload());
    rig.engine.set_repeat(REPEAT_ONE);
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |_| fake.calls().iter().filter(|c| matches!(c, Call::DelayPadding(..))).count() >= 2), "{:?} {:?}", fake.calls().iter().filter(|c| !matches!(c, Call::Write(..))).collect::<Vec<_>>(), rig.events.lock());
    for _ in 0..2 {
        fake.advance(5 * 44_100);
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(rig.wait(5, |r| r.events.lock().iter().filter(|e| matches!(e, Event::Looped { index: 0, .. })).count() == 2), "{:?}", rig.events.lock());
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_sleep_timer_s_end_of_song_on_the_chip_takes_back_the_song_written_after_it() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let (a, b) = (mp3(&d, "a", 10, 440), mp3(&d, "b", 10, 660));
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a), ("b", &b)]);
    let fake = Fake::new(MP3_ONLY);
    let songs = vec![("a".into(), "mp3".into(), 10_000), ("b".into(), "mp3".into(), 10_000)];
    let rig = Rig::new(server, songs, app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |_| fake.written() == 20 * 44_100), "both written: {}", fake.written());
    fake.advance(3 * 44_100);
    assert!(rig.wait(5, |r| r.engine.status().position_ms >= 2_900));
    rig.engine.pause_at_end(true);
    // b cannot be taken back out of the track: it starts again from where the ear is, a alone, on a new
    // track (the join's end of stream stopped the one before).
    assert!(rig.wait(5, |_| opens(&fake) == 2 && fake.written() > 0), "{:?}", fake.calls());
    let after_flush = |f: &Fake| after_last_open(&f.calls()).into_iter().filter(|c| matches!(c, Call::EndOfStream | Call::DelayPadding(..))).cloned().collect::<Vec<_>>();
    assert!(rig.wait(5, |_| after_flush(&fake).contains(&Call::EndOfStream)), "{:?}", fake.calls());
    assert_eq!(after_flush(&fake).len(), 2, "a's rest, closed: {:?}", after_flush(&fake));
    let frames = fake.written() as i64;
    // From the packet the place lands in, a few frames back for the decoder's reservoir.
    assert!((frames - 7 * 44_100).abs() <= 4 * 1152, "the rest of a only: {frames}");
    fake.advance(frames as u64);
    assert!(rig.wait(5, |r| r.events.lock().contains(&Event::Stopped)), "{:?}", rig.events.lock());
    assert!(rig.wait(5, |r| { let s = r.engine.status(); s.state == State::Paused && s.index == Some(1) && s.position_ms == 0 }), "{:?}", rig.engine.status());
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn opus_goes_to_the_chip_in_ogg_pages_its_header_first() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let a = made(&d, "a", 3, 440, &["-c:a", "libopus", "-b:a", "96k"], "opus");
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a)]);
    let fake = Fake::new(&[(Coding::Opus, Support::Gapless)]);
    let rig = Rig::new(server, vec![("a".into(), "opus".into(), 3_000)], app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |_| fake.calls().contains(&Call::EndOfStream)), "{:?}", fake.calls());
    assert!(matches!(fake.calls()[0], Call::Open(Coded { coding: Coding::Opus, rate: 48_000, channels: 2 })));
    let bytes = fake.0.lock().bytes.clone();
    // Page by page: the stream's header, the comment header, then one packet a page, each stamped with
    // the samples decoded to its end.
    let mut at = 0;
    let mut pages = Vec::new();
    while at < bytes.len() {
        assert_eq!(&bytes[at..at + 4], b"OggS", "a page at {at}");
        let segments = bytes[at + 26] as usize;
        let body: usize = bytes[at + 27..at + 27 + segments].iter().map(|&s| s as usize).sum();
        let granule = u64::from_le_bytes(bytes[at + 6..at + 14].try_into().unwrap());
        let start = at + 27 + segments;
        pages.push((granule, bytes[start..start + body].to_vec()));
        at = start + body;
    }
    assert!(pages[0].1.starts_with(b"OpusHead") && pages[1].1.starts_with(b"OpusTags"));
    assert!(pages[2..].windows(2).all(|w| w[1].0 > w[0].0), "the stamps only grow");
    // Three seconds of music, and the encoder's pre-skip before it (which the header tells the chip).
    let last = pages.last().unwrap().0;
    let skip = u16::from_le_bytes([pages[0].1[10], pages[0].1[11]]) as u64;
    assert!(last >= 3 * 48_000 + skip && last < 3 * 48_000 + skip + 960 * 3, "{last}");
    assert_eq!(fake.written(), 3 * 48_000, "the frames heard, pre-skip and padding cut");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn aac_in_mp4_goes_to_the_chip_with_its_edit_list_as_delay_and_padding() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let a = made(&d, "a", 3, 440, &["-c:a", "aac", "-b:a", "128k"], "m4a");
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a)]);
    let fake = Fake::new(&[(Coding::Aac, Support::Gapless)]);
    let rig = Rig::new(server, vec![("a".into(), "m4a".into(), 3_000)], app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |_| fake.calls().contains(&Call::EndOfStream)), "{:?}", fake.calls());
    let calls = fake.calls();
    assert!(matches!(calls[0], Call::Open(Coded { coding: Coding::Aac, rate: 44_100, channels: 2 })), "{calls:?}");
    let Some(Call::DelayPadding(delay, padding)) = calls.iter().find(|c| matches!(c, Call::DelayPadding(..))) else { panic!("{calls:?}") };
    // ffmpeg's encoder primes 1024 frames (the edit list's start), and cuts the last block short in the
    // sample table itself, so there is no padding past the edit: media3 reads the same.
    assert_eq!((*delay, *padding), (1024, 0));
    assert_eq!(fake.written(), 3 * 44_100, "the frames heard");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

// ---- bit-perfect ----

#[test]
fn bit_perfect_hands_every_sample_over_as_the_file_stores_it_at_its_own_rate_and_depth() {
    let a = ramp(44_100, 24, 1);
    let b = ramp(48_000, 16, 2);
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &wav(44_100, 24, &a)), ("b", &wav(48_000, 16, &b))]);
    let mut app = app();
    // ReplayGain and the equalizer both stand down.
    app.gains.insert("a".into(), 0.5);
    app.gains.insert("b".into(), 0.25);
    let eq = Sound { bands: vec![Band { kind: 0, freq: 1000.0, gain_db: 6.0, q: 1.0, channel: 0 }], ..Sound::default() };
    let songs = vec![("a".into(), "wav".into(), 1_000), ("b".into(), "wav".into(), 1_000)];
    let rig = Rig::new(server, songs, app, None, Settings { sound: eq, ..Settings::default() });
    rig.engine.set_output(OutputFacts { usb: true, bit_perfect: true });
    std::thread::sleep(Duration::from_millis(50));
    rig.engine.play_at(0, 0);
    assert!(rig.wait(20, |r| r.events.lock().contains(&Event::State(State::Ended))), "{:?}", rig.events.lock());
    let opened = rig.card.opened.lock().clone();
    assert_eq!(opened.iter().map(|f| (f.rate, f.bits)).collect::<Vec<_>>(), [(44_100, 24), (48_000, 16)], "a device for each song's own format");
    let heard = rig.card.heard.lock().clone();
    let (first, second) = heard.split_at(a.len());
    assert!(first.iter().zip(&a).all(|(v, s)| (v * 8_388_608.0) as i32 == *s), "the 24-bit song, bit for bit");
    assert!(second.iter().zip(&b).all(|(v, s)| (v * 32_768.0) as i32 == *s), "the 16-bit song, bit for bit");
    assert_eq!(second.len(), b.len(), "every sample of it");
    rig.engine.stop();
}

// ---- a live stream ----

/// An endless stream of 16-bit stereo WAV with ICY announcements every 4 KB: the title changes at
/// 64 KB of music.
struct Station;

struct Live {
    at: u64,
    since: usize,
    header: Vec<u8>,
    /// An announcement still to be handed over, however small the reads.
    meta: Vec<u8>,
}

const EVERY: usize = 4096;

impl Read for Live {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.meta.is_empty() {
            let n = buf.len().min(self.meta.len());
            buf[..n].copy_from_slice(&self.meta[..n]);
            self.meta.drain(..n);
            return Ok(n);
        }
        if !self.header.is_empty() {
            let n = buf.len().min(self.header.len()).min(EVERY - self.since);
            buf[..n].copy_from_slice(&self.header[..n]);
            self.header.drain(..n);
            self.since += n;
            return Ok(n);
        }
        if self.since == EVERY {
            self.since = 0;
            let music = self.at;
            let text = if (60_000..60_000 + EVERY as u64 * 2).contains(&music) { b"StreamTitle='Artist - Song';".to_vec() } else { Vec::new() };
            let blocks = text.len().div_ceil(16);
            self.meta = vec![blocks as u8];
            self.meta.extend_from_slice(&text);
            self.meta.resize(1 + blocks * 16, 0);
            return self.read(buf);
        }
        let n = buf.len().min(EVERY - self.since).min(4096) & !3;
        for (k, b) in buf[..n].iter_mut().enumerate() {
            *b = ((self.at + k as u64) / 4) as u8;
        }
        self.at += n as u64;
        self.since += n;
        std::thread::sleep(Duration::from_micros(200));
        Ok(n)
    }
}

impl ByteSource for Station {
    fn open(&self, _: &str, _: u64) -> Result<Body, String> {
        Err("a live stream is opened live".into())
    }

    fn open_live(&self, _: &str) -> Result<(Body, Option<usize>), String> {
        let mut header = wav(44_100, 16, &[]);
        // A stream with no end: the largest length a WAV header holds.
        header[4..8].copy_from_slice(&u32::MAX.to_le_bytes());
        header[40..44].copy_from_slice(&(u32::MAX - 36).to_le_bytes());
        Ok((Body { start: 0, len: None, reader: Box::new(Live { at: 0, since: 0, header, meta: Vec::new() }) }, Some(EVERY)))
    }
}

struct Radio;

impl Library for Radio {
    fn locate(&mut self, id: &str) -> Result<Located, String> {
        Ok(Located { source: Source::Live { url: id.into(), bytes: Arc::new(Station) }, hint: Some("wav".into()), duration_ms: None })
    }

    fn about(&self, id: &str) -> WindowSong {
        WindowSong { id: id.into(), title: id.into(), radio: true, ..Default::default() }
    }
}

#[test]
fn a_live_stream_plays_as_it_comes_with_its_announcements_taken_out() {
    let queue = SharedQueue::default();
    queue.0.lock().set(vec!["radio:1".into()], Some(0), false, 0);
    let card = Card::default();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    let engine = Engine::start(Radio, app(), queue, Box::new(card.clone()), Config::default(), move |e| seen.lock().push(e));
    engine.queue_changed();
    engine.play_at(0, 0);
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until && card.heard.lock().len() < 200_000 {
        std::thread::sleep(Duration::from_millis(20));
    }
    let heard = card.heard.lock().clone();
    assert!(heard.len() >= 200_000, "it plays: {} samples", heard.len());
    // The music is the station's bytes with not one byte of announcement in it.
    for (k, v) in heard.iter().enumerate().take(200_000) {
        let at = k as u64 * 2;
        let byte = |i: u64| (i / 4) as u8;
        let want = i16::from_le_bytes([byte(at), byte(at + 1)]) as f32 / 32768.0;
        assert_eq!(*v, want, "sample {k}");
    }
    // Said when the ear reaches it: after what the output held when the reader passed it, which on this
    // card's fast clock is a while in real time.
    let until = Instant::now() + Duration::from_secs(20);
    while Instant::now() < until && !events.lock().contains(&Event::Title("Artist - Song".into())) {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(events.lock().contains(&Event::Title("Artist - Song".into())), "{:?}", events.lock());
    assert_eq!(engine.status().state, State::Playing, "it goes on");
    engine.stop();
}

#[test]
fn an_announcement_reads_its_title() {
    assert_eq!(nori_engine::source::stream_title(b"StreamTitle='Muse - Uprising';StreamUrl='';\0\0"), Some("Muse - Uprising".into()));
    assert_eq!(nori_engine::source::stream_title(b"StreamTitle='';\0"), None);
    assert_eq!(nori_engine::source::stream_title(b"StreamTitle='Sigur R\xf3s - Hopp\xedpolla';"), Some("Sigur Rós - Hoppípolla".into()), "Latin-1");
    assert_eq!(nori_engine::source::stream_title(b"StreamTitle='Guns N' Roses - Patience';"), Some("Guns N' Roses - Patience".into()));
}

// ---- the offline bridge ----

/// The simulated app, with the core's word on a song the network would not bring: the bridge takes it.
struct Bridging(sim::App);

impl Host for Bridging {
    fn plan_for(&mut self, id: &str) -> Option<Plan> {
        self.0.plan_for(id)
    }
    fn wants_analysis(&mut self, id: &str) -> Option<u64> {
        self.0.wants_analysis(id)
    }
    fn analysed(&mut self, id: &str, a: Analyzer, channels: usize, frames: u64, rate: u32) {
        self.0.analysed(id, a, channels, frames, rate)
    }
    fn now_ms(&self) -> i64 {
        self.0.now_ms()
    }
}

impl App for Bridging {
    fn clock(&mut self, now_ms: i64) {
        self.0.clock(now_ms)
    }
    fn auto_mix(&self) -> bool {
        false
    }
    fn window(&mut self, window: Vec<WindowSong>, shuffling: bool) {
        self.0.window(window, shuffling)
    }
    fn measure_ahead<S: nori_player::pipeline::Songs>(&mut self, _: &mut S, _: &[String]) {}
    fn on_error(&mut self, kind: PlaybackError, _: bool) -> Option<OnError> {
        Some(if kind == PlaybackError::Network { OnError::Bridge } else { OnError::Skip })
    }
}

#[test]
fn a_song_the_network_will_not_bring_is_handed_to_the_offline_bridge() {
    let a = ramp(44_100, 16, 3);
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &wav(44_100, 16, &a))]);
    server.down.lock().push("b".into());
    let songs = vec![("a".into(), "wav".into(), 1_000), ("b".into(), "wav".into(), 1_000)];
    let rig = Rig::new(server, songs, Bridging(app()), None, Settings::default());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(30, |r| r.events.lock().contains(&Event::Bridge)), "{:?}", rig.events.lock());
    let events = rig.events.lock().clone();
    assert!(!events.contains(&Event::Stopped), "the bridge takes over: not a stop: {events:?}");
    assert!(rig.wait(5, |r| r.engine.status().state == State::Paused));
    // The bridge's jump (a downloaded song, in the app) plays at once.
    rig.queue.0.lock().set(vec!["a".into(), "b".into(), "a".into()], Some(1), false, 0);
    rig.engine.queue_changed();
    rig.engine.play_at(2, 0);
    assert!(rig.wait(5, |r| r.engine.status().state == State::Playing));
    rig.engine.stop();
}

// ---- repeat one on the CPU ----

#[test]
fn repeat_one_on_the_cpu_says_each_loop() {
    let a = ramp(22_050, 16, 4);
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &wav(44_100, 16, &a))]);
    let rig = Rig::new(server, vec![("a".into(), "wav".into(), 500)], app(), None, Settings::default());
    rig.engine.set_repeat(REPEAT_ONE);
    rig.engine.play_at(0, 0);
    assert!(rig.wait(20, |r| r.events.lock().iter().filter(|e| matches!(e, Event::Looped { index: 0, .. })).count() >= 3), "{:?}", rig.events.lock());
    let songs = rig.events.lock().iter().filter(|e| matches!(e, Event::Song { .. })).count();
    assert_eq!(songs, 1, "the song itself is said once; each time round is a loop");
    rig.engine.stop();
}

// ---- why the CPU plays ----

#[test]
fn a_song_on_the_cpu_says_why_the_chip_did_not_take_it() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let (lame, fl) = (mp3(&d, "lame", 10, 440), flac(&d, "fl", 10, 550));
    let cases: [(&[u8], &str, &[(Coding, Support)], Settings, &str); 3] = [
        (&fl, "flac", MP3_ONLY, offload(), "FLAC is not a compression the output decodes"),
        (&lame, "mp3", &[], offload(), "the output does not decode MP3 at 44100 Hz x2"),
        (&lame, "mp3", MP3_ONLY, Settings::default(), "offload is off in the settings"),
    ];
    for (song, ext, support, settings, why) in cases {
        let server = Arc::new(Server::default());
        serve(&server, &[("a", song)]);
        let fake = Fake::new(support);
        let rig = Rig::new(server, vec![("a".into(), ext.into(), 10_000)], app(), Some(fake.clone()), settings);
        rig.engine.play_at(0, 0);
        assert!(rig.wait(10, |r| r.engine.status().pcm_why.is_some_and(|w| w.contains(why))), "{why}: {:?}", rig.engine.status().pcm_why);
        assert!(!rig.engine.status().offloaded);
        assert!(fake.calls().iter().all(|c| !matches!(c, Call::Open(_))), "no track for the chip: {:?}", fake.calls());
        rig.engine.stop();
    }
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn plain_offload_takes_a_song_with_no_gap_to_cut_as_media3_does() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    // No LAME tag: nothing says the song has a delay or padding, so it needs no gapless offload.
    let a = made(&d, "a", 10, 440, &["-c:a", "libmp3lame", "-b:a", "128k", "-write_xing", "0"], "mp3");
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a)]);
    let fake = Fake::new(&[(Coding::Mp3, Support::Plain)]);
    let rig = Rig::new(server, vec![("a".into(), "mp3".into(), 10_000)], app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |r| r.engine.status().offloaded), "{:?}", rig.engine.status().pcm_why);
    assert!(fake.calls().contains(&Call::DelayPadding(0, 0)), "{:?}", fake.calls());
    assert_eq!(rig.engine.status().pcm_why, None);
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

// ---- offload without gapless support ----

const PLAIN_MP3: &[(Coding, Support)] = &[(Coding::Mp3, Support::Plain)];

/// Half-minute MP3s by LAME, which tags each with its encoder delay and padding, served under `ids`: long
/// enough that the next song is not read while the one before has just begun.
fn lame_songs(d: &Path, server: &Server, ids: &[&str]) -> Vec<(String, String, i64)> {
    for (k, id) in ids.iter().enumerate() {
        let f = mp3(d, id, 30, 440 + 110 * k as u32);
        serve(server, &[(id, &f)]);
    }
    ids.iter().map(|id| (id.to_string(), "mp3".to_string(), 30_000)).collect()
}

fn on(albums: &[(&str, &str, i32)]) -> Vec<(String, String, i32)> {
    albums.iter().map(|(id, album, track)| (id.to_string(), album.to_string(), *track)).collect()
}

#[test]
fn without_gapless_offload_songs_of_different_albums_are_offloaded_their_delay_left_in() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let server = Arc::new(Server::default());
    let songs = lame_songs(&d, &server, &["x", "y", "z"]);
    let fake = Fake::new(PLAIN_MP3);
    // A shuffle of three albums: no song follows another on its album.
    let rig = Rig::albums(server, songs, on(&[("x", "X", 3), ("y", "Y", 7), ("z", "Z", 1)]), app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |r| r.engine.status().offloaded), "{:?}", rig.engine.status().pcm_why);
    let why = rig.engine.status().pcm_why.unwrap_or_default();
    assert!(why.contains("near silence at its ends") && why.contains("does not do gapless offload"), "{why}");
    // Each played out, the next goes on the same track from its start: no join to make gapless.
    for next in ["y", "z"] {
        assert!(rig.wait(10, |_| fake.written() > 0));
        fake.advance(1 << 40);
        assert!(rig.wait(10, |r| r.heard_song(next) && r.engine.status().offloaded), "{next}: {:?}", rig.events.lock());
    }
    let calls = fake.calls();
    assert_eq!(calls.iter().filter(|c| matches!(c, Call::Open(_))).count(), 1, "one track: {calls:?}");
    assert_eq!(calls.iter().filter(|c| matches!(c, Call::DelayPadding(576, _))).count(), 3, "every song from its start: {calls:?}");
    assert!(rig.card.opened.lock().is_empty(), "the CPU's output was never opened");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn without_gapless_offload_an_album_in_order_stays_on_the_cpu() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let server = Arc::new(Server::default());
    let songs = lame_songs(&d, &server, &["a1", "a2"]);
    let fake = Fake::new(PLAIN_MP3);
    let rig = Rig::albums(server, songs, on(&[("a1", "A", 1), ("a2", "A", 2)]), app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    let why = "joins a song of its album without a gap, which needs gapless offload, and the output does not do it";
    assert!(rig.wait(10, |r| r.engine.status().pcm_why.is_some_and(|w| w.starts_with("MP3 with an encoder delay of 576") && w.contains(why))), "{:?}", rig.engine.status().pcm_why);
    assert!(rig.wait(30, |r| r.heard_song("a2")));
    assert!(!rig.engine.status().offloaded);
    assert!(fake.calls().iter().all(|c| !matches!(c, Call::Open(_))), "no track for the chip: {:?}", fake.calls());
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn without_gapless_offload_the_cpu_takes_an_album_over_at_its_first_song_and_hands_back_after_its_last() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let server = Arc::new(Server::default());
    let songs = lame_songs(&d, &server, &["x", "a1", "a2", "y"]);
    let fake = Fake::new(PLAIN_MP3);
    let albums = on(&[("x", "X", 5), ("a1", "A", 1), ("a2", "A", 2), ("y", "Y", 9)]);
    let rig = Rig::albums(server, songs, albums, app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |r| r.engine.status().offloaded && fake.written() > 0), "x on the chip: {:?}", rig.engine.status().pcm_why);
    fake.advance(1 << 40);
    // a1 joins a2 without a gap: the CPU from a1's start.
    assert!(rig.wait(10, |r| r.heard_song("a1") && !r.engine.status().offloaded && !r.card.heard.lock().is_empty()), "{:?}", rig.events.lock());
    assert_eq!(fake.calls().iter().filter(|c| matches!(c, Call::DelayPadding(..))).count(), 1, "only x went to the chip: {:?}", fake.calls());
    assert_eq!(fake.written(), 0, "and nothing of a1 after x");
    // After a2, y needs no gap cut: the chip again, from y's start, a2 heard to its end on the CPU.
    assert!(rig.wait(20, |r| r.heard_song("y") && r.engine.status().offloaded), "{:?}", rig.events.lock());
    let heard = rig.card.heard.lock().len() / 2;
    assert!(heard + 44_100 / 2 >= 2 * 30 * 44_100, "a1 and a2 whole on the CPU: {heard} frames");
    assert_eq!(fake.calls().iter().filter(|c| matches!(c, Call::DelayPadding(576, _))).count(), 2, "x and y from their starts: {:?}", fake.calls());
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

// ---- a play head that makes no sense ----

/// Two songs of `secs` on one gapless track, playing, both written, a second heard; the play head moved
/// by the test only as fast as the clock (`pace` 1), until a test lets it run.
fn two_on_the_chip(d: &Path, secs: u32) -> Option<(Rig, Fake)> {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return None;
    }
    let (a, b) = (mp3(d, "a", secs, 440), mp3(d, "b", secs, 660));
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a), ("b", &b)]);
    let fake = Fake::new(MP3_ONLY);
    fake.pace(Some(1.0));
    let ms = secs as i64 * 1000;
    let rig = Rig::new(server, vec![("a".into(), "mp3".into(), ms), ("b".into(), "mp3".into(), ms)], app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |_| fake.written() == 2 * secs as u64 * 44_100), "both written: {}", fake.written());
    // A second of music, as a second goes by.
    std::thread::sleep(Duration::from_millis(1_200));
    fake.advance(44_100);
    assert!(rig.wait(5, |r| r.engine.status().position_ms >= 990), "{:?}", rig.engine.status());
    Some((rig, fake))
}

/// Waits for the engine to have read what the play head was set to say, and a little more.
fn read_all(rig: &Rig, fake: &Fake) {
    assert!(rig.wait(5, |_| fake.0.lock().readings.is_empty()), "the engine read the play head");
    std::thread::sleep(Duration::from_millis(400));
}

#[test]
fn a_play_head_that_could_not_be_read_or_read_nought_skips_no_song() {
    let d = dir();
    let Some((rig, fake)) = two_on_the_chip(&d, 20) else { return };
    // A call that failed, nought (what a failed call was read as, and what a gapless join starts the
    // count again from), another failure: a second into a, with b placed after it on the track.
    fake.read_as(&[None, Some(0), None]);
    read_all(&rig, &fake);
    let s = rig.engine.status();
    assert!(!rig.heard_song("b") && s.index == Some(0) && s.offloaded, "still a, on the chip: {s:?} {:?}", rig.events.lock());
    assert!((990..=1_100).contains(&s.position_ms), "where the ear was: {s:?}");
    let notes = fake.notes();
    assert!(notes.iter().any(|n| n.contains("could not be read")), "{notes:?}");
    assert!(notes.iter().any(|n| n.contains("read 0 after 44100, not at a join")), "{notes:?}");
    // The true count back, b comes where a ends.
    fake.pace(None);
    fake.advance(19 * 44_100 + 100);
    assert!(rig.wait(5, |r| r.heard_song("b")), "{:?}", rig.events.lock());
    let at = rig.engine.status().position_ms;
    assert!((0..100).contains(&at), "at the start of b: {at}");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_play_head_counting_again_from_nought_after_a_pause_skips_no_song() {
    let d = dir();
    let Some((rig, fake)) = two_on_the_chip(&d, 20) else { return };
    rig.engine.pause();
    assert!(rig.wait(5, |r| r.engine.status().state == State::Paused));
    // The platform went to standby while paused: its count starts again from nought.
    fake.count_again();
    rig.engine.play();
    assert!(rig.wait(5, |r| r.engine.status().state == State::Playing));
    std::thread::sleep(Duration::from_millis(400));
    fake.advance(13_230);
    std::thread::sleep(Duration::from_millis(400));
    fake.read_as(&[]);
    std::thread::sleep(Duration::from_millis(100));
    let s = rig.engine.status();
    assert!(!rig.heard_song("b") && s.index == Some(0) && s.offloaded, "still a: {s:?} {:?}", rig.events.lock());
    assert!((1_250..=1_450).contains(&s.position_ms), "a second and 300 ms into a: {s:?}");
    assert!(fake.notes().iter().any(|n| n.contains("counts again from nought")), "{:?}", fake.notes());
    // And a ends where it ends.
    fake.pace(None);
    fake.advance(19 * 44_100 - 13_230 + 100);
    assert!(rig.wait(5, |r| r.heard_song("b")), "{:?}", rig.events.lock());
    let at = rig.engine.status().position_ms;
    assert!((0..100).contains(&at), "at the start of b: {at}");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_play_head_ahead_of_the_clock_hands_the_song_to_the_cpu_where_the_ear_is() {
    let d = dir();
    let Some((rig, fake)) = two_on_the_chip(&d, 20) else { return };
    // Ten seconds on in a moment (a count in other units, a jump), a failed call, the jump again.
    fake.read_as(&[Some(11 * 44_100), None, Some(11 * 44_100)]);
    assert!(rig.wait(10, |r| !r.engine.status().offloaded && r.card.heard.lock().len() > 44_100), "the CPU took over: {:?}", rig.engine.status());
    let s = rig.engine.status();
    assert!(!rig.heard_song("b") && s.index == Some(0), "a, on the CPU: {s:?} {:?}", rig.events.lock());
    assert!(s.position_ms >= 990 && s.position_ms < 11_000, "from where the ear was, not where the head said: {s:?}");
    let why = s.pcm_why.unwrap_or_default();
    assert!(why.contains("could not be followed") && why.contains("ahead of the clock"), "{why}");
    assert!(fake.notes().iter().any(|n| n.starts_with("offload given up")), "{:?}", fake.notes());
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_late_word_that_everything_was_presented_ends_no_song() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let a = mp3(&d, "a", 20, 440);
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a)]);
    let fake = Fake::new(MP3_ONLY);
    let rig = Rig::new(server, vec![("a".into(), "mp3".into(), 20_000)], app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |_| fake.calls().contains(&Call::EndOfStream)), "{:?}", fake.calls());
    // A seek near the end: a new track, which takes its end of stream only at the third time of asking
    // (still stopping, as Android's is after one), and meanwhile the platform says it presented
    // everything: about the track before.
    fake.0.lock().refuse_eos = 2;
    rig.engine.seek(18_000);
    assert!(rig.wait(5, |_| opens(&fake) == 2 && fake.written() > 0), "{:?}", fake.calls());
    fake.0.lock().stale_presented = true;
    fake.read_as(&[]);
    std::thread::sleep(Duration::from_millis(300));
    assert!(!rig.events.lock().contains(&Event::State(State::Ended)), "not ended by a word about another end of stream");
    assert!(rig.wait(5, |_| fake.calls().iter().filter(|c| **c == Call::EndOfStream).count() == 2), "said again until taken: {:?}", fake.calls());
    std::thread::sleep(Duration::from_millis(200));
    let s = rig.engine.status();
    assert!(s.state == State::Playing && s.offloaded, "{s:?}");
    // Played to its end: ended, and the perf notes say how.
    fake.advance(3 * 44_100);
    assert!(rig.wait(5, |r| r.events.lock().contains(&Event::State(State::Ended))), "{:?}", rig.events.lock());
    let notes = fake.notes();
    assert!(notes.iter().any(|n| n.contains("would not take the end of stream while its track played (2 of 3)")), "{notes:?}");
    assert!(notes.iter().any(|n| n.starts_with("a ended by the play head")), "{notes:?}");
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn an_end_of_stream_the_platform_keeps_refusing_hands_the_song_to_the_cpu() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let a = mp3(&d, "a", 10, 440);
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a)]);
    let fake = Fake::new(MP3_ONLY);
    fake.0.lock().refuse_eos = 100;
    let rig = Rig::new(server, vec![("a".into(), "mp3".into(), 10_000)], app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |r| !r.engine.status().offloaded && r.card.heard.lock().len() > 44_100), "the CPU took over: {:?}", rig.engine.status());
    let why = rig.engine.status().pcm_why.unwrap_or_default();
    assert!(why.contains("would not take the end of stream"), "{why}");
    assert!(!fake.calls().contains(&Call::EndOfStream));
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn the_track_holds_four_minutes_at_most_whatever_the_platform_would_take() {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed: nothing to offload");
        return;
    }
    let d = dir();
    let a = mp3(&d, "a", 300, 440);
    let server = Arc::new(Server::default());
    serve(&server, &[("a", &a)]);
    let fake = Fake::new(MP3_ONLY);
    // A phone that takes four times what it was asked for: all of a five-minute song at once.
    fake.0.lock().takes = 4;
    let rig = Rig::new(server, vec![("a".into(), "mp3".into(), 300_000)], app(), Some(fake.clone()), offload());
    rig.engine.play_at(0, 0);
    assert!(rig.wait(10, |_| fake.written() >= 240 * 44_100), "{}", fake.written());
    std::thread::sleep(Duration::from_millis(300));
    let written = fake.written();
    // Four minutes, and the last write's quarter megabyte past them (16 s at 128 kbps).
    assert!(written <= 257 * 44_100, "four minutes ahead at most: {} s", written / 44_100);
    assert!(!fake.calls().contains(&Call::EndOfStream), "the song is not written to its end yet");
    // Under half a minute left in the track: the rest is written.
    fake.advance(written - 20 * 44_100);
    assert!(rig.wait(5, |_| fake.written() == 300 * 44_100), "{}", fake.written());
    assert!(rig.wait(5, |_| fake.calls().contains(&Call::EndOfStream)));
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}

#[test]
fn a_pause_after_the_engine_slept_through_the_music_keeps_the_ear_where_it_is() {
    let d = dir();
    let Some((rig, fake)) = two_on_the_chip(&d, 20) else { return };
    // The engine sleeps while the chip plays on; the pause comes before it looked again.
    std::thread::sleep(Duration::from_millis(1_500));
    fake.advance_quietly(44_100);
    rig.engine.pause();
    assert!(rig.wait(5, |r| r.engine.status().state == State::Paused));
    std::thread::sleep(Duration::from_millis(300));
    rig.engine.play();
    fake.read_as(&[]);
    read_all(&rig, &fake);
    let s = rig.engine.status();
    assert!(s.offloaded && s.index == Some(0) && s.position_ms >= 1_990, "two seconds into a, still on the chip: {s:?}");
    assert!(!fake.notes().iter().any(|n| n.contains("ahead of the clock")), "{:?}", fake.notes());
    rig.engine.stop();
    let _ = std::fs::remove_dir_all(d);
}
