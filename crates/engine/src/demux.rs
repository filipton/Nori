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
//!
//! A song can also be read as its packets alone, undecoded ([`Demuxed::load_packets`]), for an output
//! that decodes them itself (audio offload): what they are, the encoder's delay and padding as that
//! output takes them, and each packet with the frames of music it stands for.

use std::io::{self, Read, Seek, SeekFrom};
use std::sync::Arc;
use std::thread::Thread;

use nori_player::automix::resample::Resampler;
use nori_player::automix::PCM_FLOAT;
use nori_player::decode::{Codec, Decoder, MP3_DECODER_DELAY};
use nori_player::pcm::{Encoding, Format};
use nori_player::pipeline::Reading;
use nori_player::queue::PlaybackError;
use parking_lot::Mutex;
use symphonia::core::codecs::audio::well_known::*;
use symphonia::core::codecs::audio::AudioCodecId;
use symphonia::core::errors::Error as SymphoniaError;
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
    /// Read as packets, not decoded.
    Raw,
}

/// A compression an output may decode itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Coding {
    Mp3,
    /// AAC Low Complexity.
    Aac,
    Opus,
}

impl Coding {
    /// Its name, as a report says it.
    pub fn name(self) -> &'static str {
        match self {
            Coding::Mp3 => "MP3",
            Coding::Aac => "AAC-LC",
            Coding::Opus => "Opus",
        }
    }
}

/// A compressed stream as an output that decodes it is asked about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coded {
    pub coding: Coding,
    pub rate: u32,
    pub channels: usize,
}

/// A song read as packets: what they are, and how the output that decodes them cuts the encoder's delay
/// and padding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodedSong {
    pub coded: Coded,
    /// Bits a second, as the song's bytes and length say (0 unknown): what sizes a track for it.
    pub bitrate: u32,
    /// Frames the decoder makes that are cut from the start and from the end, as media3 hands them to an
    /// offloaded AudioTrack: an MP3's LAME numbers, an MP4's edit list. An Opus stream's pre-skip is in
    /// its header, which the output reads.
    pub delay: u32,
    pub padding: u32,
    /// The codec's own header: an Opus stream's `OpusHead`.
    pub setup: Option<Box<[u8]>>,
    /// Where the first packet read starts in the song, frames: after a seek, the packet it landed in.
    pub from_frame: i64,
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
    /// Bits per sample as the file stores them (0 unknown or not stored that way).
    bits: u32,
    /// What the song's compression is called, for a report of why it plays where it does.
    compression: &'static str,
    /// Read as packets: what they are, and the frames of music the last one stands for.
    coded: Option<CodedSong>,
    packet_frames: u64,
    first_packet: bool,
    /// The format has been told (from the first packet): anything decoded in another shape from here on
    /// is converted to it.
    settled: bool,
    /// Decoded audio in another shape than the one told, converted to it: a live stream whose station
    /// changed its rate or channels (the next song of a chained Ogg stream).
    reshape: Option<Reshape>,
}

/// Audio of one shape made into another, as floats.
struct Reshape {
    from: (u32, usize),
    resampler: Resampler,
    input: Vec<u8>,
    output: Vec<u8>,
}

/// `samples` (interleaved float, `from` rate and channels) into `out` in `to`'s rate, channels and
/// encoding, through `reshape` (made for `from` if it is not): the frames made.
fn reshaped(reshape: &mut Option<Reshape>, from: (u32, usize), to: Format, samples: &[f32], out: &mut Vec<u8>) -> usize {
    if reshape.as_ref().is_none_or(|r| r.from != from) {
        let Some(resampler) = Resampler::new(from.0 as i32, from.1 as i32, to.rate as i32, to.channels as i32) else { return 0 };
        *reshape = Some(Reshape { from, resampler, input: Vec::new(), output: Vec::new() });
    }
    let r = reshape.as_mut().expect("made above");
    r.input.clear();
    r.input.extend(samples.iter().flat_map(|v| v.to_le_bytes()));
    let frames = samples.len() / from.1.max(1);
    r.output.resize((frames * to.rate as usize / from.0.max(1) as usize + 4) * to.channels * 4, 0);
    let Some((_, made)) = r.resampler.process(&r.input, PCM_FLOAT, &mut r.output, PCM_FLOAT) else { return 0 };
    for b in r.output[..made].chunks_exact(4) {
        put(f32::from_le_bytes([b[0], b[1], b[2], b[3]]), to.encoding, out);
    }
    made / 4 / to.channels
}

impl Stream {
    /// `whole`: every byte of `source` is here, so the MP4 boxes that hold the gapless numbers can be
    /// read wherever they are without waiting for the network. `packets`: read undecoded, for an output
    /// that decodes them itself.
    fn open(mut source: Box<dyn MediaSource>, hint: Option<&str>, from_ms: i64, duration_ms: Option<i64>, encoding: Encoding, whole: bool, packets: bool) -> Result<Stream, String> {
        let gapless = if whole { crate::mp4::gapless(&mut source).ok().flatten() } else { None };
        let byte_len = source.byte_len();
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
            _ if packets => Inner::Raw,
            (Some(c), _) => Inner::Coded(decoder(c, rate, channels, params.extra_data.as_deref(), c == Codec::Mp3 && delay_known)?),
            (None, Some(p)) => Inner::Pcm(p),
            _ => return Err(format!("{:?} is not decoded here", params.codec)),
        };
        let (track_delay, track_padding) = (track.delay, track.padding);
        let setup = params.extra_data.clone();
        let bits = params.bits_per_sample.or(params.bits_per_coded_sample).unwrap_or(0);
        // The gapless numbers are in the track's timescale: taken when it counts in frames, as audio
        // tracks do.
        let per_frame = track.time_base.is_some_and(|t| t.numer.get() == 1 && t.denom.get() == rate);
        let mp4 = gapless.filter(|_| per_frame && codec.is_some()).map(|g| (g.delay as i64, g.frames.map_or(i64::MAX, |f| (g.delay + f) as i64)));
        let mp4_total = gapless.map_or(0, |g| g.total as i64);
        let frames = match mp4 {
            Some((delay, end)) if end < i64::MAX => Some(end - delay),
            _ => track.num_frames.map(|n| n as i64),
        };
        let duration_us = frames.map(|n| n * 1_000_000 / rate as i64).or(duration_ms.map(|d| d * 1000)).unwrap_or(0);
        let coded = packets.then(|| coding(codec, setup.as_deref(), rate)).flatten().map(|coding| {
            let bitrate = match byte_len {
                Some(b) if duration_us > 0 => (b as i128 * 8_000_000 / duration_us as i128).min(u32::MAX as i128) as u32,
                _ => 0,
            };
            let (delay, padding) = match (coding, mp4) {
                // An MP4's numbers are in what the decoder makes, which is how media3 hands them over.
                (_, Some((delay, end))) => (delay as u32, if end < i64::MAX { (mp4_total - end).max(0) as u32 } else { 0 }),
                // symphonia counts the MP3 decoder's own 529 frames into the delay and out of the padding;
                // media3 hands the LAME tag's numbers over as they are.
                (Coding::Mp3, None) => match track_delay {
                    Some(d) => (d.saturating_sub(MP3_DECODER_DELAY as u32), track_padding.unwrap_or(0) + MP3_DECODER_DELAY as u32),
                    None => (0, 0),
                },
                // The pre-skip is in the stream's header, which the output reads itself.
                (Coding::Opus, None) => (0, track_padding.unwrap_or(0)),
                (Coding::Aac, None) => (track_delay.unwrap_or(0), track_padding.unwrap_or(0)),
            };
            CodedSong { coded: Coded { coding, rate, channels }, bitrate, delay, padding, setup: setup.clone(), from_frame: 0 }
        });
        let compression = match (codec, Pcm::of(params.codec)) {
            (Some(Codec::Aac), _) => match coding(codec, setup.as_deref(), rate) {
                Some(_) => "AAC-LC",
                None => "HE-AAC",
            },
            (Some(Codec::Mp3), _) => "MP3",
            (Some(Codec::Flac), _) => "FLAC",
            (Some(Codec::Vorbis), _) => "Vorbis",
            (Some(Codec::Alac), _) => "ALAC",
            (Some(Codec::Opus), _) => "Opus",
            (None, Some(_)) => "PCM",
            (None, None) => "an unknown compression",
        };
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
            bits,
            compression,
            coded,
            packet_frames: 0,
            first_packet: true,
            settled: false,
            reshape: None,
        };
        if from_ms > 0 {
            d.seek(from_ms)?;
        }
        if packets {
            // The first packet says where the reading starts after a seek.
            d.primed = d.next_packet();
            return Ok(d);
        }
        // The first packet says for certain what comes out (an AAC stream's real rate, say).
        d.primed = d.next();
        if let Inner::Coded(dec) = &d.inner {
            d.format.rate = dec.rate();
            d.format.channels = dec.channels();
        }
        d.settled = true;
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

    /// The next packet of the song as it is into `buf`, and the frames of music it stands for; false at
    /// the end of the song.
    fn next_packet(&mut self) -> bool {
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(p)) => p,
                Ok(None) | Err(_) => {
                    self.ended = true;
                    return false;
                }
            };
            if packet.track_id != self.track || packet.data.is_empty() {
                continue;
            }
            let (pts, dur) = (packet.pts.get(), packet.dur.get() as i64);
            // What of it is heard: an MP4's stamps and lengths count its delay and padding in; elsewhere
            // the packet's trims say it (symphonia 0.6.1 leaves them inside its length, as `dur` builds it).
            let (start, frames) = match self.mp4 {
                Some((delay, end)) => {
                    let (from, to) = (pts.max(delay), (pts + dur).min(end));
                    (from - delay, (to - from).max(0))
                }
                None => {
                    let (trim_start, trim_end) = (packet.trim_start.get() as i64, packet.trim_end.get() as i64);
                    (pts + trim_start, (dur - trim_start - trim_end).max(0))
                }
            };
            self.buf.clear();
            self.buf.extend_from_slice(&packet.data);
            self.packet_frames = frames as u64;
            if std::mem::take(&mut self.first_packet) {
                if let Some(c) = self.coded.as_mut() {
                    c.from_frame = start.max(0);
                }
            }
            self.frame = Some(start + frames);
            self.at_us = start.max(0) * 1_000_000 / self.format.rate as i64;
            return true;
        }
    }

    /// Decodes the next packet with anything in it into `buf`; false at the end of the song.
    fn next(&mut self) -> bool {
        let (ch, enc) = (self.format.channels, self.format.encoding);
        loop {
            let packet = match self.reader.next_packet() {
                Ok(Some(p)) => p,
                // The next stream of a chained Ogg one (a station's next song): read on in it.
                Err(SymphoniaError::ResetRequired) if self.chain_on() => continue,
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
            let (samples, pcm, ch, rate): (&[f32], Option<(Pcm, usize)>, usize, u32) = match &mut self.inner {
                Inner::Coded(dec) => match dec.decode_lent(&packet.data) {
                    Ok(lent) => (lent.samples, None, lent.channels.max(1), lent.rate),
                    Err(nori_player::decode::Fault::Broken) => {
                        self.ended = true;
                        return false;
                    }
                    Err(_) => continue,
                },
                Inner::Pcm(p) => (&[], Some((*p, p.width())), ch, self.format.rate),
                Inner::Raw => return false,
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
                // Decoded in another shape than the one told (the station's next song, of another rate or
                // channel count): converted to it, which the output below goes on playing at. Handed on
                // as it is, it would play at the wrong speed and pitch.
                None if self.settled && (rate, ch) != (self.format.rate, self.format.channels) => {
                    let made = reshaped(&mut self.reshape, (rate, ch), self.format, &samples[from * ch..to * ch], &mut self.buf);
                    self.frame = Some(first + made as i64);
                    if made == 0 {
                        continue;
                    }
                }
                None => {
                    self.reshape = None;
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
    /// A chained Ogg stream began its next logical stream (an Ogg station's next song, with headers of
    /// its own): its track and a decoder for it, read on at the format told. False when it cannot be.
    fn chain_on(&mut self) -> bool {
        let Some(track) = self.reader.default_track(TrackType::Audio) else { return false };
        let Some(params) = track.codec_params.as_ref().and_then(|p| p.audio()) else { return false };
        let (Some(codec), Some(rate)) = (codec_of(params.codec), params.sample_rate) else { return false };
        let channels = params.channels.as_ref().map_or(2, |c| c.count()).max(1);
        let id = track.id;
        let Ok(dec) = decoder(codec, rate, channels, params.extra_data.as_deref(), false) else { return false };
        if !matches!(self.inner, Inner::Coded(_)) {
            return false;
        }
        self.inner = Inner::Coded(dec);
        self.codec = Some(codec);
        self.track = id;
        true
    }
}

/// A decoder for a song read here.
fn decoder(codec: Codec, rate: u32, channels: usize, extra: Option<&[u8]>, delay_known: bool) -> Result<Decoder, String> {
    Decoder::new(codec, rate, channels, extra, delay_known)
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

/// Decodes the song in `source`, every byte of it on the disk, from its start to its end, as it plays
/// (delay and padding cut), handing each buffer over as float samples with the rate and channels: for
/// measuring a song ahead of time. `each` answers whether to go on. False when it was not read to its
/// end, or was stopped.
pub(crate) fn decode_whole(source: Box<dyn MediaSource>, hint: Option<&str>, mut each: impl FnMut(u32, usize, &[f32]) -> bool) -> Result<bool, String> {
    let mut s = Stream::open(source, hint, 0, None, Encoding::Float, true, false)?;
    let mut floats: Vec<f32> = Vec::new();
    while s.fill() {
        floats.clear();
        floats.extend(s.buffer().chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
        if !each(s.format.rate, s.format.channels, &floats) {
            return Ok(false);
        }
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

/// Which of the compressions an output may decode itself `codec` is, by its setup: AAC only as Low
/// Complexity (object type 2), which is all `AudioFormat.ENCODING_AAC_LC` promises - and not at 24 kHz or
/// less, where "AAC-LC" is how HE-AAC whose SBR is signalled only inside the stream announces itself
/// (`nori_settings::decoder::implicit_sbr`): an output set up for what it says would play it at half its rate.
fn coding(codec: Option<Codec>, setup: Option<&[u8]>, rate: u32) -> Option<Coding> {
    match codec? {
        Codec::Mp3 => Some(Coding::Mp3),
        Codec::Aac if rate > 24_000 && setup.and_then(|s| s.first()).is_some_and(|b| b >> 3 == 2) => Some(Coding::Aac),
        Codec::Opus => Some(Coding::Opus),
        _ => None,
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
        let s = Stream::open(source, hint, from_ms, duration_ms, encoding, true, false)?;
        Ok(Demuxed { state: State::Open(Box::new(s)), loader: None })
    }

    /// [`Demuxed::open`], read as packets and not decoded ([`Demuxed::packet`]).
    pub fn open_packets(source: Box<dyn MediaSource>, hint: Option<&str>, from_ms: i64, duration_ms: Option<i64>) -> Result<Demuxed, String> {
        let s = Stream::open(source, hint, from_ms, duration_ms, Encoding::Pcm16, true, true)?;
        Ok(Demuxed { state: State::Open(Box::new(s)), loader: None })
    }

    /// [`Demuxed::load`], read as packets and not decoded ([`Demuxed::packet`]).
    pub fn load_packets(loader: Arc<Loader>, engine: Thread, hint: Option<&str>, from_ms: i64, duration_ms: Option<i64>) -> Demuxed {
        Demuxed::start(loader, engine, hint, from_ms, duration_ms, Encoding::Pcm16, true)
    }

    /// What the packets are and how the output cuts them, once the song is open; none when it is read
    /// decoded, or is in a compression no output decodes itself.
    pub fn coded(&self) -> Option<&CodedSong> {
        self.stream().and_then(|s| s.coded.as_ref())
    }

    /// What the song's compression is called (MP3, FLAC, HE-AAC, ...), once it is open.
    pub fn compression(&self) -> Option<&'static str> {
        self.stream().map(|s| s.compression)
    }

    /// The next packet into [`Reading::buffer`], with the frames of music it stands for
    /// ([`Demuxed::packet_frames`]); false at the end of the song. Asked only once it is ready.
    pub fn packet(&mut self) -> bool {
        match &mut self.state {
            State::Open(s) if s.primed => {
                s.primed = false;
                true
            }
            State::Open(s) if !s.ended => s.next_packet(),
            _ => false,
        }
    }

    /// The frames of music the last packet stands for.
    pub fn packet_frames(&self) -> u64 {
        self.stream().map_or(0, |s| s.packet_frames)
    }

    /// The song's bytes, while a loader holds them: a live stream's announcements are read there.
    pub fn loader(&self) -> Option<&Arc<Loader>> {
        self.loader.as_ref().map(|(l, _)| l)
    }

    /// The song `loader` is fetching, read from `from_ms`. Unless all of it is here already it is opened
    /// on a thread of its own, since opening reads bytes that may still be on their way; `engine` is
    /// woken when it is open, and again whenever it waited for bytes.
    pub fn load(loader: Arc<Loader>, engine: Thread, hint: Option<&str>, from_ms: i64, duration_ms: Option<i64>, encoding: Encoding) -> Demuxed {
        Demuxed::start(loader, engine, hint, from_ms, duration_ms, encoding, false)
    }

    fn start(loader: Arc<Loader>, engine: Thread, hint: Option<&str>, from_ms: i64, duration_ms: Option<i64>, encoding: Encoding, packets: bool) -> Demuxed {
        if loader.complete() {
            let state = match Stream::open(Box::new(loader.reader()), hint, from_ms, duration_ms, encoding, true, packets) {
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
            let opened = Stream::open(Box::new(l.reader()), hint.as_deref(), from_ms, duration_ms, encoding, whole, packets);
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

    fn bits(&self) -> u32 {
        self.stream().map_or(0, |s| s.bits)
    }
}
