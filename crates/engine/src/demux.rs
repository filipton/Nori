//! A song's container, read packet by packet with symphonia's format readers (MP3, FLAC, Ogg Vorbis
//! and Opus, MP4/AAC and ALAC, WAV), and the packets decoded by `nori_player::decode`, the decoder
//! every platform uses. The encoder's delay and padding the container states are cut off, so songs
//! join sample for sample; a seek lands on the exact sample asked for.
//!
//! Opening a song reads its first bytes (and, for an MP4 with its index at the end, its last ones), so
//! a song still on its way is opened on a thread of its own, and the engine's thread is woken when it
//! is open; it never waits for the network.
//!
//! symphonia's readers hand each packet over in a buffer of their own (`FormatReader::next_packet`
//! returns it boxed, and has no way to read into one kept), so reading allocates once a packet. That
//! happens only while the engine fills the output in a burst; decoding allocates nothing.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::thread::Thread;

use nori_player::decode::{Codec, Decoder, MP3_DECODER_DELAY};
use nori_player::pcm::{Encoding, Format};
use nori_player::pipeline::Reading;
use nori_player::queue::PlaybackError;
use parking_lot::Mutex;
use symphonia::core::codecs::audio::well_known::*;
use symphonia::core::codecs::audio::AudioCodecId;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::{Time, Timestamp};

use crate::source::Loader;

/// What an Opus stream plays in again after a seek, as the decoder drops it (80 ms, RFC 7845).
const OPUS_PRE_ROLL: i64 = 3840;
/// Frames read ahead of a seek into an MP4, for the decoder to warm up on: two AAC frames.
const AAC_WARM_UP: i64 = 2048;

/// Samples as a WAV file stores them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pcm {
    U8,
    S16,
    S24,
    S32,
    F32,
}

impl Pcm {
    fn of(codec: AudioCodecId) -> Option<Pcm> {
        Some(match codec {
            CODEC_ID_PCM_U8 => Pcm::U8,
            CODEC_ID_PCM_S16LE => Pcm::S16,
            CODEC_ID_PCM_S24LE => Pcm::S24,
            CODEC_ID_PCM_S32LE => Pcm::S32,
            CODEC_ID_PCM_F32LE => Pcm::F32,
            _ => return None,
        })
    }

    fn width(self) -> usize {
        match self {
            Pcm::U8 => 1,
            Pcm::S16 => 2,
            Pcm::S24 => 3,
            Pcm::S32 | Pcm::F32 => 4,
        }
    }

    /// One stored sample, written to `out` in `to`: as 16 bits rounded as the decoder rounds, or as a
    /// float.
    fn put(self, b: &[u8], to: Encoding, out: &mut Vec<u8>) {
        let v = match (self, to) {
            (Pcm::U8, Encoding::Pcm16) => return out.extend_from_slice(&(((b[0] as i16) - 128) << 8).to_le_bytes()),
            (Pcm::S16, Encoding::Pcm16) => return out.extend_from_slice(&b[..2]),
            (Pcm::U8, _) => (b[0] as f32 - 128.0) / 128.0,
            (Pcm::S16, _) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
            (Pcm::S24, _) => i32::from_le_bytes([0, b[0], b[1], b[2]]) as f32 / 2_147_483_648.0,
            (Pcm::S32, _) => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            (Pcm::F32, _) => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        };
        put(v, to, out);
    }
}

/// A decoded sample into `out` in `to`: 16 bits rounded as `Decoder::take_i16` rounds, or the float.
fn put(v: f32, to: Encoding, out: &mut Vec<u8>) {
    match to {
        Encoding::Pcm16 => out.extend_from_slice(&((v * 32768.0).round_ties_even().clamp(-32768.0, 32767.0) as i16).to_le_bytes()),
        Encoding::Float => out.extend_from_slice(&v.to_le_bytes()),
    }
}

enum Inner {
    Coded(Decoder),
    Pcm(Pcm),
}

fn codec_of(id: AudioCodecId) -> Option<Codec> {
    Some(match id {
        CODEC_ID_MP3 => Codec::Mp3,
        CODEC_ID_FLAC => Codec::Flac,
        CODEC_ID_AAC => Codec::Aac,
        CODEC_ID_VORBIS => Codec::Vorbis,
        CODEC_ID_ALAC => Codec::Alac,
        CODEC_ID_OPUS => Codec::Opus,
        _ => return None,
    })
}

/// One song being read: its container, its decoder, and the buffer the last packet decoded into, in
/// the encoding asked for (float for high quality output).
struct Stream {
    reader: Box<dyn FormatReader + 'static>,
    track: u32,
    inner: Inner,
    codec: Option<Codec>,
    /// The container states the encoder's delay (an MP3's LAME header): its trims are the ones to cut.
    delay_known: bool,
    format: Format,
    buf: Vec<u8>,
    /// The next frame to come out, counted from the start of the song; unknown right after a seek
    /// until the first packet says where it is.
    frame: Option<i64>,
    /// After a seek, what comes before this frame is decoded and dropped.
    skip_to: i64,
    at_us: i64,
    duration_us: i64,
    ended: bool,
    /// The first buffer, decoded while opening to learn the stream's true shape.
    primed: bool,
    /// An MP4's encoder delay and where its music ends, in frames of what the decoder makes, as the
    /// file's `iTunSMPB` or edit list says (symphonia's reader reads neither): cut as media3 cuts them.
    mp4: Option<(i64, i64)>,
}

impl Stream {
    /// `whole`: every byte of `source` is here, so the MP4 boxes that hold the gapless numbers can be
    /// read wherever they are without waiting for the network.
    fn open(mut source: Box<dyn MediaSource>, hint: Option<&str>, from_ms: i64, duration_ms: Option<i64>, encoding: Encoding, whole: bool) -> Result<Stream, String> {
        let gapless = if whole { crate::mp4::gapless(&mut source).ok().flatten() } else { None };
        source.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
        let source = past_id3(source).map_err(|e| e.to_string())?;
        let mss = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
        let mut h = Hint::new();
        if let Some(x) = hint {
            if x.contains('/') {
                h.mime_type(x);
            } else {
                h.with_extension(x);
            }
        }
        let reader = symphonia::default::get_probe().probe(&h, mss, FormatOptions::default(), MetadataOptions::default()).map_err(|e| e.to_string())?;
        let track = reader.default_track(TrackType::Audio).ok_or("no audio in it")?;
        let params = track.codec_params.as_ref().and_then(|p| p.audio()).ok_or("no audio in it")?;
        let rate = params.sample_rate.ok_or("no sample rate")?;
        let channels = params.channels.as_ref().map_or(2, |c| c.count()).max(1);
        let codec = codec_of(params.codec);
        let delay_known = track.delay.is_some();
        let inner = match (codec, Pcm::of(params.codec)) {
            (Some(c), _) => Inner::Coded(Decoder::new(c, rate, channels, params.extra_data.as_deref(), c == Codec::Mp3 && delay_known)?),
            (None, Some(p)) => Inner::Pcm(p),
            _ => return Err(format!("{:?} is not decoded here", params.codec)),
        };
        // The gapless numbers are in the track's timescale: taken when it counts in frames, as audio
        // tracks do.
        let per_frame = track.time_base.is_some_and(|t| t.numer.get() == 1 && t.denom.get() == rate);
        let mp4 = gapless.filter(|_| per_frame && codec.is_some()).map(|g| (g.delay as i64, g.frames.map_or(i64::MAX, |f| (g.delay + f) as i64)));
        let frames = match mp4 {
            Some((delay, end)) if end < i64::MAX => Some(end - delay),
            _ => track.num_frames.map(|n| n as i64),
        };
        let duration_us = frames.map(|n| n * 1_000_000 / rate as i64).or(duration_ms.map(|d| d * 1000)).unwrap_or(0);
        let max_frames = codec.map_or(8192, Codec::max_frames);
        let id = track.id;
        let width = encoding.width();
        let mut d = Stream {
            reader,
            track: id,
            inner,
            codec,
            delay_known,
            format: Format { rate, channels, encoding },
            buf: Vec::with_capacity(max_frames * channels * width),
            frame: Some(0),
            skip_to: 0,
            at_us: 0,
            duration_us,
            ended: false,
            primed: false,
            mp4,
        };
        if from_ms > 0 {
            d.seek(from_ms)?;
        }
        // The first packet says for certain what comes out (an AAC stream's real rate, say).
        d.primed = d.next();
        if let Inner::Coded(dec) = &d.inner {
            d.format.rate = dec.rate();
            d.format.channels = dec.channels();
        }
        Ok(d)
    }

    fn seek(&mut self, ms: i64) -> Result<(), String> {
        let to = match self.mp4 {
            // The song's time is past the delay in the track's. AAC's first frame after a reset is
            // only the decoder warming up, so the read starts two frames early and drops them.
            Some((delay, _)) => SeekTo::Timestamp { ts: Timestamp::new((ms * self.format.rate as i64 / 1000 + delay - AAC_WARM_UP).max(0)), track_id: self.track },
            None => SeekTo::Time { time: Time::from_millis(ms), track_id: Some(self.track) },
        };
        let seeked = self.reader.seek(SeekMode::Accurate, to).map_err(|e| e.to_string())?;
        self.skip_to = match self.mp4 {
            Some(_) => ms * self.format.rate as i64 / 1000,
            None => seeked.required_ts.get(),
        };
        self.frame = None;
        if let Inner::Coded(dec) = &mut self.inner {
            dec.reset(false);
        }
        Ok(())
    }

    /// What the decoder drops of the first packet after a reset: an MP3's filterbank warming up, an
    /// Opus stream's pre-roll.
    fn dropped_after_reset(&self) -> i64 {
        match self.codec {
            Some(Codec::Mp3) => MP3_DECODER_DELAY as i64,
            Some(Codec::Opus) => OPUS_PRE_ROLL,
            _ => 0,
        }
    }

    /// Where a packet stamped `pts` starts in the song, as its samples come out of the decoder. An MP3
    /// without a LAME header is stamped from its first decoded sample, which the decoder drops.
    fn song_frame(&self, pts: i64) -> i64 {
        let origin = if self.codec == Some(Codec::Mp3) && !self.delay_known { MP3_DECODER_DELAY as i64 } else { 0 };
        pts - origin + self.dropped_after_reset()
    }

    /// Decodes the next packet with anything in it into `buf`; false at the end of the song.
    fn next(&mut self) -> bool {
        let (ch, enc) = (self.format.channels, self.format.encoding);
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(p)) => p,
                // The end, or a stream that cannot be read any further: either way the song is over.
                Ok(None) | Err(_) => {
                    self.ended = true;
                    return false;
                }
            };
            if packet.track_id != self.track {
                continue;
            }
            let mut at = match self.frame {
                Some(f) => f,
                None if matches!(self.inner, Inner::Pcm(_)) => packet.pts.get(),
                None => self.song_frame(packet.pts.get()),
            };
            // The samples as the decoder lends them (or as the file stores them), where they lie.
            let (samples, pcm, ch): (&[f32], Option<(Pcm, usize)>, usize) = match &mut self.inner {
                Inner::Coded(dec) => match dec.decode_lent(&packet.data) {
                    Ok(lent) => (lent.samples, None, lent.channels.max(1)),
                    Err(nori_player::decode::Fault::Broken) => {
                        self.ended = true;
                        return false;
                    }
                    Err(_) => continue,
                },
                Inner::Pcm(p) => (&[], Some((*p, p.width())), ch),
            };
            let n = match pcm {
                Some((_, w)) => packet.data.len() / (w * ch),
                None => samples.len() / ch,
            };
            if n == 0 {
                continue;
            }
            // The encoder's delay and padding, as the container states them. An Opus decoder drops its
            // own pre-skip.
            let (mut trim_start, mut trim_end) = if self.codec == Some(Codec::Opus) || self.frame.is_none() { (0, packet.trim_end.get() as usize) } else { (packet.trim_start.get() as usize, packet.trim_end.get() as usize) };
            if let Some((delay, end)) = self.mp4 {
                // An MP4 packet's stamp is where it starts in what the decoder makes, delay included.
                let raw = packet.pts.get();
                trim_start = (delay - raw).clamp(0, n as i64) as usize;
                trim_end = (raw + n as i64 - end).clamp(0, n as i64) as usize;
                at = raw + trim_start as i64 - delay;
            }
            let (from, to) = (trim_start.min(n), n.saturating_sub(trim_end).max(trim_start.min(n)));
            let mut from = from;
            // A seek: what comes before the place asked for is decoded and dropped.
            let (mut first, end) = (at, at + (to - from) as i64);
            self.frame = Some(end);
            if end <= self.skip_to {
                continue;
            }
            if first < self.skip_to {
                from += (self.skip_to - first) as usize;
                first = self.skip_to;
            }
            if from >= to {
                continue;
            }
            self.buf.clear();
            match pcm {
                Some((p, w)) => {
                    for b in packet.data[from * ch * w..to * ch * w].chunks_exact(w) {
                        p.put(b, enc, &mut self.buf);
                    }
                }
                None => {
                    for &v in &samples[from * ch..to * ch] {
                        put(v, enc, &mut self.buf);
                    }
                }
            }
            self.at_us = first * 1_000_000 / self.format.rate as i64;
            return true;
        }
    }
}

impl Stream {
    fn duration_us(&self) -> i64 {
        match self.frame {
            // Read to its end: exactly as long as what came out.
            Some(f) if self.ended => f * 1_000_000 / self.format.rate as i64,
            _ => self.duration_us,
        }
    }

    fn fill(&mut self) -> bool {
        if std::mem::take(&mut self.primed) {
            return true;
        }
        if !self.ended && self.next() {
            return true;
        }
        self.buf.clear();
        false
    }

    fn buffer(&self) -> &[u8] {
        // The first buffer is only handed out by the first fill.
        if self.primed {
            &[]
        } else {
            &self.buf
        }
    }

    fn at_us(&self) -> i64 {
        self.at_us
    }
}

/// Decodes the song in the file at `path` from its start to its end, as it plays (delay and padding
/// cut), handing each buffer over as float samples with the rate and channels: for measuring a song
/// ahead of time. False when it could not be read to its end.
pub(crate) fn decode_whole(path: &std::path::Path, hint: Option<&str>, mut each: impl FnMut(u32, usize, &[f32])) -> Result<bool, String> {
    let file = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut s = Stream::open(Box::new(file), hint, 0, None, Encoding::Float, true)?;
    let mut floats: Vec<f32> = Vec::new();
    while s.fill() {
        floats.clear();
        floats.extend(s.buffer().chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        each(s.format.rate, s.format.channels, &floats);
    }
    Ok(s.ended)
}

/// The source from the first byte after its ID3v2 tags, which is where the music starts.
///
/// symphonia's probe does not know the tag (its reader is a feature left out: it would keep the cover
/// picture in memory for as long as the song plays), so it would look for the music inside it, and a
/// cover of a megabyte or two holds bytes that look like an MPEG frame header often enough: a song
/// opened as MPEG layer I, which nothing decodes. The tag is read through and dropped rather than
/// seeked over, so that a song still coming is fetched in one piece.
fn past_id3(mut source: Box<dyn MediaSource>) -> io::Result<Box<dyn MediaSource>> {
    let mut start = 0u64;
    loop {
        let mut header = [0u8; 10];
        let whole = read_up_to(&mut source, &mut header)? == header.len();
        let size = header[6..].iter().try_fold(0u64, |n, &b| (b < 0x80).then_some(n << 7 | b as u64));
        match size {
            Some(size) if whole && header.starts_with(b"ID3") && header[3] < 0xff && header[4] < 0xff => {
                // A footer, when the flags say there is one, is ten bytes more.
                let length = size + if header[5] & 0x10 != 0 { 10 } else { 0 };
                if io::copy(&mut (&mut source).take(length), &mut io::sink())? < length {
                    return Err(io::Error::new(io::ErrorKind::UnexpectedEof, "the song ends inside its tag"));
                }
                start += 10 + length;
            }
            _ => break,
        }
    }
    source.seek(SeekFrom::Start(start))?;
    Ok(if start == 0 { source } else { Box::new(After { inner: source, start }) })
}

/// Reads until `buf` is full or the source ends: the bytes read.
fn read_up_to(source: &mut dyn Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match source.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
}

/// A source seen from byte `start` on, as if that were its first.
struct After {
    inner: Box<dyn MediaSource>,
    start: u64,
}

impl Read for After {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.inner.read(buf)
    }
}

impl Seek for After {
    fn seek(&mut self, to: SeekFrom) -> io::Result<u64> {
        let to = match to {
            SeekFrom::Start(p) => SeekFrom::Start(self.start + p),
            other => other,
        };
        let at = self.inner.seek(to)?;
        at.checked_sub(self.start).ok_or_else(|| io::Error::other("seek before the start"))
    }
}

impl MediaSource for After {
    fn is_seekable(&self) -> bool {
        self.inner.is_seekable()
    }

    fn byte_len(&self) -> Option<u64> {
        self.inner.byte_len().map(|l| l.saturating_sub(self.start))
    }
}

/// Whether a song's hint names an MP4 container.
fn mp4_like(hint: &str) -> bool {
    matches!(hint.to_ascii_lowercase().as_str(), "m4a" | "m4b" | "mp4" | "aac" | "alac" | "audio/mp4" | "audio/x-m4a" | "audio/aac")
}

/// A song opened for the engine: open, being opened on a thread of its own, or not to be opened.
pub struct Demuxed {
    state: State,
    /// Bytes still on their way, and who to wake when they come.
    loader: Option<(Arc<Loader>, Thread)>,
}

enum State {
    Opening(Arc<Opening>),
    Open(Box<Stream>),
    Failed(PlaybackError, String),
}

/// A song being opened elsewhere: what came of it, and who to wake when it does.
#[derive(Default)]
struct Opening {
    done: Mutex<(Option<Result<Stream, String>>, Option<Thread>)>,
}

impl Demuxed {
    /// Opens `source` (with a file extension or MIME type as a hint) and reads from `from_ms`, decoding
    /// to `encoding`. `duration_ms` is the song's tagged length, for a container that does not say its
    /// own. For bytes that are all here (a file): nothing is waited for.
    pub fn open(source: Box<dyn MediaSource>, hint: Option<&str>, from_ms: i64, duration_ms: Option<i64>, encoding: Encoding) -> Result<Demuxed, String> {
        let s = Stream::open(source, hint, from_ms, duration_ms, encoding, true)?;
        Ok(Demuxed { state: State::Open(Box::new(s)), loader: None })
    }

    /// The song `loader` is fetching, read from `from_ms`. Unless all of it is here already it is opened
    /// on a thread of its own, since opening reads bytes that may still be on their way; `engine` is
    /// woken when it is open, and again whenever it waited for bytes.
    pub fn load(loader: Arc<Loader>, engine: Thread, hint: Option<&str>, from_ms: i64, duration_ms: Option<i64>, encoding: Encoding) -> Demuxed {
        if loader.complete() {
            let state = match Stream::open(Box::new(loader.reader()), hint, from_ms, duration_ms, encoding, true) {
                Ok(s) => State::Open(Box::new(s)),
                Err(why) => State::Failed(PlaybackError::Other, why),
            };
            return Demuxed { state, loader: Some((loader, engine)) };
        }
        let opening = Arc::new(Opening::default());
        let (o, l, hint) = (opening.clone(), loader.clone(), hint.map(str::to_string));
        let spawned = std::thread::Builder::new().name("nori-open".into()).spawn(move || {
            // An MP4's gapless numbers may sit at its very end: it is read once all of it is here (a
            // song that fits one burst, as most do), rather than fetched from the end and again.
            let whole = hint.as_deref().is_some_and(mp4_like) && l.wait_whole();
            let opened = Stream::open(Box::new(l.reader()), hint.as_deref(), from_ms, duration_ms, encoding, whole);
            let mut done = o.done.lock();
            done.0 = Some(opened);
            if let Some(t) = done.1.take() {
                t.unpark();
            }
        });
        let state = match spawned {
            Ok(_) => State::Opening(opening),
            Err(e) => State::Failed(PlaybackError::Other, e.to_string()),
        };
        Demuxed { state, loader: Some((loader, engine)) }
    }

    fn stream(&self) -> Option<&Stream> {
        match &self.state {
            State::Open(s) => Some(s),
            _ => None,
        }
    }

    /// Why the song failed: the connection's own failure when there was one, which says more than
    /// what the container reader made of the bytes running out.
    fn failure(&self, why: String) -> (PlaybackError, String) {
        match self.loader.as_ref().and_then(|(l, _)| l.error()) {
            Some(e) => (PlaybackError::Network, e),
            None => (PlaybackError::Other, why),
        }
    }
}

impl Reading for Demuxed {
    fn format(&self) -> Format {
        self.stream().map_or(Format { rate: 44_100, channels: 2, encoding: Encoding::Pcm16 }, |s| s.format)
    }

    fn duration_us(&self) -> i64 {
        self.stream().map_or(0, Stream::duration_us)
    }

    fn ready(&mut self) -> bool {
        if let State::Opening(o) = &self.state {
            let mut done = o.done.lock();
            let Some(opened) = done.0.take() else {
                done.1 = self.loader.as_ref().map(|(_, engine)| engine.clone());
                return false;
            };
            drop(done);
            self.state = match opened {
                Ok(s) => State::Open(Box::new(s)),
                Err(why) => {
                    let (kind, why) = self.failure(why);
                    State::Failed(kind, why)
                }
            };
        }
        match &self.state {
            State::Open(s) => s.primed || s.ended || self.loader.as_ref().is_none_or(|(l, engine)| l.ready_or_wake(engine)),
            _ => true,
        }
    }

    fn error(&self) -> Option<(PlaybackError, String)> {
        match &self.state {
            State::Failed(kind, why) => Some((*kind, why.clone())),
            // Read to an early end because the bytes stopped coming.
            State::Open(s) if s.ended => self.loader.as_ref().and_then(|(l, _)| l.error()).map(|e| (PlaybackError::Network, e)),
            _ => None,
        }
    }

    fn fill(&mut self) -> bool {
        match &mut self.state {
            State::Open(s) => s.fill(),
            _ => false,
        }
    }

    fn buffer(&self) -> &[u8] {
        self.stream().map_or(&[], Stream::buffer)
    }

    fn at_us(&self) -> i64 {
        self.stream().map_or(0, Stream::at_us)
    }
}
