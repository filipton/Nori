//! Internet radio through the whole engine: a station's bytes (MP3, AAC, HE-AAC, Vorbis, Opus, at the
//! rates and channel counts stations send), with the ICY announcements between them, and what reaches
//! the sound card measured: a tone's pitch from its zero crossings, a real station's capture against
//! what ffmpeg decodes from the same bytes. A station that plays at the wrong pitch or speed (a rate or a
//! channel count taken wrong between the decoder and the card: 48 kHz played as 44.1, mono as stereo)
//! fails here, and so does one that goes quiet when its format changes mid-stream.
//!
//! The tones are made by ffmpeg on the machine running the tests; without it those tests say so and
//! pass. The HE-AAC capture (six seconds of SomaFM's 32 kbps AAC+ stream) is in `testdata`.

mod common;

use std::io::Read;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use common::card::{Card, Pull};
use common::{Stepper, Virtual};
use nori_engine::{Body, ByteSource, Config, Engine, Library, Located, OutputFormat, SharedQueue, Source};
use nori_player::sim;
use nori_player::transitions::WindowSong;

fn ffmpeg() -> bool {
    Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success())
}

fn dir() -> PathBuf {
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
    let d = std::env::temp_dir().join(format!("nori-radio-{}-{nanos}", std::process::id()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// A tone of `hz` for `secs` at `rate` and `channels`, encoded by ffmpeg with `codec` into a stream of
/// container `format`, as a station sends it (no Xing or LAME header, no end to wait for).
fn tone(hz: u32, secs: f64, rate: u32, channels: u32, codec: &[&str], format: &str) -> Vec<u8> {
    let out = dir().join(format!("tone.{format}"));
    let ok = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-y", "-f", "lavfi", "-i", &format!("sine=frequency={hz}:sample_rate={rate}:duration={secs}")])
        .args(["-ac", &channels.to_string()])
        .args(codec)
        .args(["-f", format])
        .arg(&out)
        .status()
        .is_ok_and(|s| s.success());
    assert!(ok, "ffmpeg made a {format} tone");
    std::fs::read(out).unwrap()
}

fn mp3(hz: u32, secs: f64, rate: u32, channels: u32) -> Vec<u8> {
    tone(hz, secs, rate, channels, &["-c:a", "libmp3lame", "-b:a", "96k", "-write_xing", "0"], "mp3")
}

// ---- the station ----

/// Bytes of music between two announcements, as many stations send.
const EVERY: usize = 16_000;

/// A station sending `bytes` once, an announcement every [`EVERY`] bytes of music.
struct Station(Arc<Vec<u8>>);

struct Live {
    bytes: Arc<Vec<u8>>,
    at: usize,
    since: usize,
    meta: Vec<u8>,
}

impl Read for Live {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if !self.meta.is_empty() {
            let n = buf.len().min(self.meta.len());
            buf[..n].copy_from_slice(&self.meta[..n]);
            self.meta.drain(..n);
            return Ok(n);
        }
        if self.since == EVERY {
            self.since = 0;
            let text = b"StreamTitle='Artist - Song';";
            let blocks = text.len().div_ceil(16);
            self.meta = vec![blocks as u8];
            self.meta.extend_from_slice(text);
            self.meta.resize(1 + blocks * 16, 0);
            return self.read(buf);
        }
        let n = buf.len().min(EVERY - self.since).min(4096).min(self.bytes.len() - self.at);
        buf[..n].copy_from_slice(&self.bytes[self.at..self.at + n]);
        self.at += n;
        self.since += n;
        Ok(n)
    }
}

impl ByteSource for Station {
    fn open(&self, _: &str, _: u64) -> Result<Body, String> {
        Err("a live stream is opened live".into())
    }

    fn open_live(&self, _: &str) -> Result<(Body, Option<usize>), String> {
        Ok((Body { start: 0, len: None, reader: Box::new(Live { bytes: self.0.clone(), at: 0, since: 0, meta: Vec::new() }) }, Some(EVERY)))
    }
}

/// The stations queued, by id.
struct Radio(Vec<(String, Arc<Vec<u8>>)>);

impl Library for Radio {
    fn locate(&mut self, id: &str) -> Result<Located, String> {
        let bytes = self.0.iter().find(|(i, _)| i == id).map(|(_, b)| b.clone()).ok_or("no such station")?;
        // Android hands no hint for a station: the bytes say what they are.
        Ok(Located { source: Source::Live { url: id.into(), bytes: Arc::new(Station(bytes)) }, hint: None, duration_ms: None })
    }

    fn about(&self, id: &str) -> WindowSong {
        WindowSong { id: id.into(), title: id.into(), radio: true, ..Default::default() }
    }
}

fn app() -> sim::App {
    let mut a = sim::App::new();
    a.prefs = sim::prefs_off();
    a
}

struct Rig {
    engine: Engine,
    time: Stepper<Pull>,
    card: Card,
}

impl Rig {
    /// The stations queued in this order, the first one playing.
    fn new(stations: Vec<(&str, Vec<u8>)>) -> Rig {
        let queue = SharedQueue::default();
        queue.0.lock().set(stations.iter().map(|s| s.0.to_string()).collect(), Some(0), false, 0);
        let card = Card::new();
        let clock = Virtual::default();
        let radio = Radio(stations.into_iter().map(|(id, b)| (id.to_string(), Arc::new(b))).collect());
        let engine = Engine::start_on(radio, app(), queue, Box::new(card.clone()), None, Config::default(), clock.clone(), |_| {});
        engine.queue_changed();
        engine.play_at(0, 0);
        Rig { engine, time: Stepper::new(clock, card.pull.clone()), card }
    }

    /// Plays on until `secs` more have reached the card since it was last opened: what it was opened
    /// at, and all it heard since (interleaved float).
    fn hear(&self, secs: f64) -> (OutputFormat, Vec<f32>) {
        let played = self.time.until(Duration::from_secs(600), || self.card.secs() >= secs);
        assert!(played, "{secs} s reached the card: {} s did", self.card.secs());
        (self.card.format().expect("opened"), self.card.heard.lock().clone())
    }
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.engine.stop();
    }
}

/// A station's `bytes` played from its start until `secs` reached the card.
fn play(bytes: Vec<u8>, secs: f64) -> (OutputFormat, Vec<f32>) {
    Rig::new(vec![("radio:1", bytes)]).hear(secs)
}

// ---- measuring ----

fn mono(x: &[f32], channels: usize) -> Vec<f32> {
    x.chunks_exact(channels).map(|f| f.iter().sum::<f32>() / channels as f32).collect()
}

/// A tone's frequency in `x` at `rate`, from its zero crossings.
fn hz(x: &[f32], rate: u32) -> f64 {
    let crossings = x.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
    crossings as f64 / 2.0 / (x.len() as f64 / rate as f64)
}

/// The pitch of each whole second heard (mono), from `from_s` on.
fn pitches(heard: &[f32], f: OutputFormat, from_s: usize) -> Vec<f64> {
    let m = mono(heard, f.channels);
    let r = f.rate as usize;
    (from_s..m.len() / r).map(|s| hz(&m[s * r..(s + 1) * r], f.rate)).collect()
}

fn assert_pitch(what: &str, heard: &[f32], f: OutputFormat, want: f64) {
    let p = pitches(heard, f, 1);
    assert!(!p.is_empty(), "{what}: nothing heard past its first second");
    for (s, hz) in p.iter().enumerate() {
        assert!((hz - want).abs() < want * 0.005, "{what}: {hz:.1} Hz in second {}, not {want} (heard {:?})", s + 1, p);
    }
}

/// ffmpeg's decode of `bytes` at `rate` and `channels`, interleaved float.
fn reference(bytes: &[u8], rate: u32, channels: usize) -> Vec<f32> {
    let input = dir().join("in.bin");
    std::fs::write(&input, bytes).unwrap();
    let out = Command::new("ffmpeg")
        .args(["-hide_banner", "-loglevel", "error", "-i"])
        .arg(&input)
        .args(["-f", "f32le", "-ar", &rate.to_string(), "-ac", &channels.to_string(), "-"])
        .output()
        .unwrap();
    assert!(out.status.success(), "ffmpeg decodes it");
    out.stdout.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])).collect()
}

/// How alike `ours` and `theirs` are at the best lag within `max_lag` frames (a decoder's delay): the
/// normalised correlation of a stretch of a third of a second, 1 for the same waveform. Music at
/// another pitch or speed does not line up with itself anywhere and scores far lower.
fn likeness(ours: &[f32], theirs: &[f32], max_lag: usize) -> f64 {
    let len = 8_192;
    let from = max_lag + 8_192;
    assert!(ours.len() > from + len && theirs.len() > from + len + max_lag, "{} and {} frames", ours.len(), theirs.len());
    let a = &ours[from..from + len];
    let ea: f64 = a.iter().map(|v| (*v as f64).powi(2)).sum();
    let mut best: f64 = 0.0;
    for lag in -(max_lag as i64)..=max_lag as i64 {
        let b = &theirs[(from as i64 + lag) as usize..(from as i64 + lag) as usize + len];
        let eb: f64 = b.iter().map(|v| (*v as f64).powi(2)).sum();
        let dot: f64 = a.iter().zip(b).map(|(x, y)| *x as f64 * *y as f64).sum();
        best = best.max(dot / (ea * eb).sqrt().max(1e-12));
    }
    best
}

// ---- the tests ----

#[test]
fn a_station_plays_at_its_own_pitch_whatever_its_rate_and_channels() {
    if !ffmpeg() {
        eprintln!("no ffmpeg: skipped");
        return;
    }
    let aac = ["-c:a", "aac", "-b:a", "64k"];
    let vorbis = ["-c:a", "libvorbis", "-q:a", "3"];
    let opus = ["-c:a", "libopus", "-b:a", "48k"];
    let stations: Vec<(&str, Vec<u8>, u32, usize)> = vec![
        ("MP3 44.1 kHz stereo", mp3(1000, 4.0, 44_100, 2), 44_100, 2),
        ("MP3 48 kHz stereo", mp3(1000, 4.0, 48_000, 2), 48_000, 2),
        ("MP3 32 kHz stereo", mp3(1000, 4.0, 32_000, 2), 32_000, 2),
        ("MP3 22.05 kHz mono", mp3(1000, 4.0, 22_050, 1), 22_050, 1),
        ("MP3 24 kHz mono", mp3(1000, 4.0, 24_000, 1), 24_000, 1),
        ("AAC 48 kHz stereo", tone(1000, 4.0, 48_000, 2, &aac, "adts"), 48_000, 2),
        ("AAC 22.05 kHz mono", tone(1000, 4.0, 22_050, 1, &aac, "adts"), 22_050, 1),
        ("Vorbis 22.05 kHz mono", tone(1000, 4.0, 22_050, 1, &vorbis, "ogg"), 22_050, 1),
        ("Opus mono", tone(1000, 4.0, 48_000, 1, &opus, "ogg"), 48_000, 1),
    ];
    for (what, bytes, rate, channels) in stations {
        let (f, heard) = play(bytes, 3.0);
        assert_eq!((f.rate, f.channels), (rate, channels), "{what}: the card is opened at the station's own format");
        assert_pitch(what, &heard, f, 1000.0);
    }
}

#[test]
fn a_station_joined_in_the_middle_of_a_frame_plays_at_its_own_pitch() {
    if !ffmpeg() {
        eprintln!("no ffmpeg: skipped");
        return;
    }
    // A station's first bytes are wherever its server's buffer began, seldom a frame's start.
    let bytes = mp3(1000, 4.0, 48_000, 2);
    let (f, heard) = play(bytes[1001..].to_vec(), 3.0);
    assert_eq!(f.rate, 48_000);
    assert_pitch("joined mid-frame", &heard, f, 1000.0);
}

#[test]
fn a_chained_ogg_station_plays_on_into_its_next_song_at_its_own_pitch() {
    if !ffmpeg() {
        eprintln!("no ffmpeg: skipped");
        return;
    }
    // An Ogg station starts a new logical stream, headers and all, with every song, and the next may be
    // of another rate and channel count. Reading stopped at the first song's end.
    let vorbis = ["-c:a", "libvorbis", "-q:a", "3"];
    let mut bytes = tone(1000, 3.0, 44_100, 2, &vorbis, "ogg");
    bytes.extend_from_slice(&tone(1000, 4.0, 22_050, 1, &vorbis, "ogg"));
    let (f, heard) = play(bytes, 6.0);
    assert_eq!((f.rate, f.channels), (44_100, 2));
    let p = pitches(&heard, f, 0);
    assert!(p.len() >= 6, "it plays on into the next song: {p:?}");
    for (s, hz) in p.iter().enumerate().filter(|(s, _)| *s != 0 && *s != 3) {
        assert!((hz - 1000.0).abs() < 5.0, "{hz:.1} Hz in second {s}: {p:?}");
    }
}

#[test]
fn the_next_station_at_another_rate_plays_at_its_own_pitch() {
    if !ffmpeg() {
        eprintln!("no ffmpeg: skipped");
        return;
    }
    let rig = Rig::new(vec![("radio:1", mp3(1000, 6.0, 44_100, 2)), ("radio:2", mp3(1000, 6.0, 48_000, 1))]);
    let (f, heard) = rig.hear(2.0);
    assert_pitch("the first station", &heard, f, 1000.0);
    rig.engine.play_at(1, 0);
    rig.card.heard.lock().clear();
    let (f, heard) = rig.hear(3.0);
    assert_pitch("the second station", &heard, f, 1000.0);
}

#[test]
fn an_he_aac_station_plays_at_its_own_pitch() {
    // SomaFM's Groove Salad at 32 kbps: ADTS saying AAC-LC at 22.05 kHz, stereo, SBR inside. symphonia
    // has no SBR, so what plays is the core at the rate the stream states: the right pitch, a quarter of
    // the band (the platform's own decoder plays the whole on Android's ExoPlayer path).
    let bytes = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/he-aac-32k.aac")).unwrap();
    let (f, heard) = play(bytes.clone(), 4.0);
    assert_eq!((f.rate, f.channels), (22_050, 2), "opened at the core's rate, which is what comes out");
    if !ffmpeg() {
        eprintln!("no ffmpeg: the pitch is not compared");
        return;
    }
    // ffmpeg decodes the SBR too, at 44.1 kHz; brought down to the core's rate the two are one waveform.
    let theirs = mono(&reference(&bytes, f.rate, f.channels), f.channels);
    let c = likeness(&mono(&heard, f.channels), &theirs, 1024);
    assert!(c > 0.95, "the same music at the same pitch: likeness {c:.3}");
}
