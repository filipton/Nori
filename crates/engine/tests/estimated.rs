//! A transcoding server that promises an estimated length (Navidrome's `estimateContentLength`) longer
//! than what it sends, and answers a range from past the real end with a 416: the song plays whole, its
//! length and seeks are the real ones, and neither the answer past the end nor a clean end short of the
//! promise counts as a failure. A network that drops in the middle still does, and is asked again.
//!
//! The Ogg Opus songs are made by ffmpeg on the machine running the tests; without it the tests say so
//! and pass. The engine runs on a clock the test moves (`common::Virtual`).

mod common;

use std::io::Read;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use common::card::{Card, Pull};
use common::{Stepper, Virtual};
use nori_engine::{Body, ByteSource, Config, Engine, Event, Library, Located, OpenError, SharedQueue, Source, State};
use nori_player::sim;
use nori_player::transitions::WindowSong;
use parking_lot::Mutex;

fn ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

/// A tone of `secs` as Ogg Opus at 192 kbps, as the server transcodes for a phone on mobile data.
fn opus(dir: &Path, name: &str, secs: u32, hz: u32) -> Vec<u8> {
    let out = dir.join(format!("{name}.opus"));
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={hz}:sample_rate=48000:duration={secs}"), "-ac", "2"])
        .args(["-c:a", "libopus", "-b:a", "192k"])
        .arg(&out)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "ffmpeg made {name}");
    std::fs::read(out).unwrap()
}

/// How a body from the server ends.
#[derive(Clone, Copy, PartialEq, Debug)]
enum Ends {
    /// Cleanly, where the real bytes do.
    Clean,
    /// With an error there: a body shorter than its Content-Length, as OkHttp reads it.
    Broken,
}

/// The transcoding server. Every answer promises `extra` bytes more than there are; a range from the
/// real end on is a 416, saying the real length when `says`. A song's first body sends its first
/// `hold` bytes at once and the rest only a moment later, so the container reader looks for the end
/// while the song is still on its way; `cut`: the named song's first body breaks off with an error at
/// that byte, once.
struct Transcoder {
    files: Vec<(String, Arc<Vec<u8>>)>,
    extra: u64,
    says: bool,
    ends: Ends,
    hold: usize,
    cut: Mutex<Option<(String, usize)>>,
    requests: Mutex<Vec<(String, u64)>>,
}

struct Sent {
    file: Arc<Vec<u8>>,
    at: usize,
    hold: Option<usize>,
    cut: Option<usize>,
    ends: Ends,
}

impl Read for Sent {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.hold.is_some_and(|h| self.at >= h) {
            // The rest comes a moment later, in real time: the engine's clock stands still meanwhile.
            std::thread::sleep(Duration::from_millis(300));
            self.hold = None;
        }
        let stop = self.cut.unwrap_or(self.file.len()).min(self.hold.unwrap_or(usize::MAX));
        if self.at >= stop {
            if self.cut.is_some() || self.ends == Ends::Broken {
                return Err(std::io::Error::new(std::io::ErrorKind::ConnectionReset, "unexpected end of stream"));
            }
            return Ok(0);
        }
        let n = buf.len().min(stop - self.at);
        buf[..n].copy_from_slice(&self.file[self.at..self.at + n]);
        self.at += n;
        Ok(n)
    }
}

impl ByteSource for Transcoder {
    fn open(&self, url: &str, from: u64) -> Result<Body, OpenError> {
        self.requests.lock().push((url.to_string(), from));
        let file = self.files.iter().find(|(u, _)| u == url).map(|(_, f)| f.clone()).ok_or("404")?;
        let real = file.len() as u64;
        if from >= real {
            return Err(OpenError::PastEnd { len: self.says.then_some(real) });
        }
        let hold = (from == 0).then_some(self.hold);
        let mut cut = self.cut.lock();
        let cut = if from == 0 && cut.as_ref().is_some_and(|c| c.0 == url) { cut.take().map(|c| c.1) } else { None };
        Ok(Body { start: from, len: Some(real + self.extra), reader: Box::new(Sent { file, at: from as usize, hold, cut, ends: self.ends }) })
    }
}

impl Transcoder {
    fn asked(&self, id: &str) -> Vec<u64> {
        self.requests.lock().iter().filter(|(u, _)| u == id).map(|r| r.1).collect()
    }
}

struct Songs(Arc<Transcoder>, Vec<(String, i64)>);

impl Library for Songs {
    fn locate(&mut self, id: &str) -> Result<Located, String> {
        let ms = self.1.iter().find(|(i, _)| i == id).map(|s| s.1).ok_or("no such song")?;
        Ok(Located { source: Source::Url { url: id.to_string(), bytes: self.0.clone() }, hint: Some("opus".into()), duration_ms: Some(ms) })
    }

    fn about(&self, id: &str) -> WindowSong {
        let ms = self.1.iter().find(|(i, _)| i == id).map_or(0, |s| s.1);
        WindowSong { id: id.into(), title: id.into(), duration_ms: ms, ..Default::default() }
    }
}

struct Rig {
    engine: Engine,
    time: Stepper<Pull>,
    card: Card,
    events: Arc<Mutex<Vec<Event>>>,
    server: Arc<Transcoder>,
    _dir: nori_testdir::TempDir,
}

/// Seconds of song `a`, and of `b` and `c` after it. `a` is long enough that an Ogg reader's look for its
/// last page lands further past what has come than the loader reads on to: the fetch starts again
/// there, as it did on a phone for a 6 MB transcode. `b` is short: the look waits for the bytes.
const A_SECS: u32 = 60;
const B_SECS: u32 = 5;

/// `a`, `b` and `c` from a [`Transcoder`], each promised `extra` bytes too many (well past one Ogg page, so an
/// Ogg reader looking for the last page looks past the real end). None without ffmpeg.
fn rig(extra: u64, says: bool, ends: Ends, cut: Option<(&str, usize)>) -> Option<Rig> {
    if !ffmpeg() {
        eprintln!("ffmpeg is not installed");
        return None;
    }
    let dir = nori_testdir::TempDir::new("estimated");
    let (a, b, c) = (opus(&dir, "a", A_SECS, 440), opus(&dir, "b", B_SECS, 660), opus(&dir, "c", B_SECS, 880));
    let server = Arc::new(Transcoder {
        files: vec![("a".into(), Arc::new(a)), ("b".into(), Arc::new(b)), ("c".into(), Arc::new(c))],
        extra,
        says,
        ends,
        hold: 48 * 1024,
        cut: Mutex::new(cut.map(|(id, at)| (id.to_string(), at))),
        requests: Mutex::new(Vec::new()),
    });
    // The server's tags round the length, as a library's do.
    let songs = vec![("a".to_string(), A_SECS as i64 * 1000 + 400), ("b".to_string(), B_SECS as i64 * 1000), ("c".to_string(), B_SECS as i64 * 1000)];
    let queue = SharedQueue::default();
    queue.0.lock().set(songs.iter().map(|s| s.0.clone()).collect(), Some(0), false, 0);
    let card = Card::new();
    let events = Arc::new(Mutex::new(Vec::new()));
    let seen = events.clone();
    let clock = Virtual::default();
    let mut app = sim::App::new();
    app.prefs = sim::prefs_off();
    let config = Config { memory_mb: 256, ..Config::default() };
    let engine = Engine::start_on(Songs(server.clone(), songs), app, queue, Box::new(card.clone()), None, config, clock.clone(), move |e| seen.lock().push(e));
    engine.queue_changed();
    Some(Rig { engine, time: Stepper::new(clock, card.pull.clone()), card, events, server, _dir: dir })
}

impl Rig {
    fn errors(&self) -> Vec<Event> {
        self.events.lock().iter().filter(|e| matches!(e, Event::Error { .. } | Event::Bridge | Event::Stopped)).cloned().collect()
    }

    fn heard_song(&self, id: &str) -> bool {
        self.events.lock().iter().any(|e| matches!(e, Event::Song { id: i, .. } if i == id))
    }

    /// Plays `a` from `from_ms` until `b` is heard: the seconds of `a` the card heard.
    fn play_a_to_its_end(&self, from_ms: i64) -> f64 {
        self.play_to_its_end(0, from_ms)
    }

    /// Plays the song at queue `index` from `from_ms` until the next is heard: the seconds of it heard.
    fn play_to_its_end(&self, index: usize, from_ms: i64) -> f64 {
        let (id, next) = (["a", "b"][index], ["b", "c"][index]);
        self.engine.play_at(index, from_ms);
        let limit = Duration::from_secs(A_SECS as u64 + 30);
        assert!(self.time.until(limit, || self.heard_song(next) || !self.errors().is_empty()), "{next} came: {:?}", self.events.lock());
        assert!(self.errors().is_empty(), "{id} played without a failure: {:?} asked {:?}", self.errors(), self.server.asked(id));
        let s = self.engine.status();
        assert!(s.index == Some(index + 1) && s.state == State::Playing, "{next} plays after {id}: {s:?}");
        // What was heard up to the next song's first sound: this one's seconds, and a little of the next.
        self.card.secs()
    }
}

/// The Ogg reader looks for the song's last page (for its length) where the promised length puts it,
/// past the real end; the server says the real length in its 416. The song plays whole, and the 416 is
/// asked for once, not tried again and again until the song counts as failed.
#[test]
fn an_ogg_song_promised_longer_than_it_is_plays_whole_when_the_server_says_the_real_length() {
    let Some(rig) = rig(200_000, true, Ends::Broken, None) else { return };
    let real = rig.server.files[0].1.len() as u64;
    let heard = rig.play_a_to_its_end(0);
    assert!(heard >= A_SECS as f64 - 0.1, "all of a heard: {heard} s");
    let asked = rig.server.asked("a");
    let past = asked.iter().filter(|&&f| f >= real).count();
    assert!((1..=2).contains(&past), "past the end asked for once or twice (the end probe, the body's end): {asked:?}");
    rig.engine.stop();
}

/// The same without the server saying the real length: each look past the end learns only that it is
/// sooner, and the reader looks again until it finds the last page.
#[test]
fn an_ogg_song_promised_longer_than_it_is_plays_whole_when_the_server_does_not_say_the_real_length() {
    let Some(rig) = rig(200_000, false, Ends::Clean, None) else { return };
    let heard = rig.play_a_to_its_end(0);
    assert!(heard >= A_SECS as f64 - 0.1, "all of a heard: {heard} s");
    rig.engine.stop();
}

/// Played from near its end: the seek finds its place with the real length, and the song ends there
/// rather than failing (a seek bar that took the estimate would put the place past the real end).
#[test]
fn a_seek_near_the_end_of_a_song_promised_longer_than_it_is_plays_its_last_seconds() {
    for says in [true, false] {
        let Some(rig) = rig(200_000, says, Ends::Broken, None) else { return };
        let from_ms = A_SECS as i64 * 1000 - 1_500;
        let heard = rig.play_a_to_its_end(from_ms);
        assert!((1.3..=2.0).contains(&heard), "the last second and a half of a, then b (says {says}): {heard} s");
        rig.engine.stop();
    }
}

/// A body that ends cleanly short of the promised length: that is where the song ends. A short song's
/// look for its last page waits for the bytes to come rather than asking past the end, so it is the clean
/// end that tells the real length; the song plays whole into the next.
#[test]
fn a_clean_end_short_of_the_promised_length_is_the_end_of_the_song() {
    let Some(rig) = rig(200_000, false, Ends::Clean, None) else { return };
    let heard = rig.play_to_its_end(1, 0);
    assert!(heard >= B_SECS as f64 - 0.1, "all of b heard: {heard} s");
    let real = rig.server.files[1].1.len() as u64;
    assert!(rig.server.asked("b").iter().all(|&f| f < real), "nothing asked for past the end: {:?}", rig.server.asked("b"));
    rig.engine.stop();
}

/// A network that drops in the middle of the song is not taken for its end: the bytes are asked for
/// again from where they stopped, and the song plays whole.
#[test]
fn a_network_that_drops_mid_song_is_asked_again_and_the_song_plays_whole() {
    // b: short enough that nothing jumps ahead of the break, so it is the break that is asked again.
    let Some(rig) = rig(200_000, true, Ends::Broken, Some(("b", 32_000))) else { return };
    let heard = rig.play_to_its_end(1, 0);
    assert!(heard >= B_SECS as f64 - 0.1, "all of b heard: {heard} s");
    let asked = rig.server.asked("b");
    assert_eq!(asked[..2], [0, 32_000], "asked again where it broke");
    rig.engine.stop();
}
