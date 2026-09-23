//! A song's container, read packet by packet with symphonia's format readers (MP3, FLAC, Ogg Vorbis
//! and Opus, MP4/AAC and ALAC, WAV), and the packets decoded by `nori_player::decode`, the decoder
//! every platform uses. The encoder's delay and padding the container states are cut off, so songs
//! join sample for sample; a seek lands on the exact sample asked for.
//!
//! symphonia's readers hand each packet over in a buffer of its own, so reading allocates once a
//! packet. That happens only while the engine fills the output in a burst; decoding allocates nothing.

use std::sync::Arc;
use std::thread::Thread;

use nori_player::decode::{Codec, Decoder, MP3_DECODER_DELAY};
use nori_player::pcm::{Encoding, Format};
use nori_player::pipeline::Reading;
use symphonia::core::codecs::audio::well_known::*;
use symphonia::core::codecs::audio::AudioCodecId;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, FormatReader, SeekMode, SeekTo, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;
use symphonia::core::units::Time;

use crate::source::Loader;

/// What an Opus stream plays in again after a seek, as the decoder drops it (80 ms, RFC 7845).
const OPUS_PRE_ROLL: i64 = 3840;

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

    /// One stored sample as 16 bits, rounded as the decoder rounds.
    fn sample(self, b: &[u8]) -> i16 {
        let v = match self {
            Pcm::U8 => return ((b[0] as i16) - 128) << 8,
            Pcm::S16 => return i16::from_le_bytes([b[0], b[1]]),
            Pcm::S24 => i32::from_le_bytes([0, b[0], b[1], b[2]]) as f32 / 2_147_483_648.0,
            Pcm::S32 => i32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32 / 2_147_483_648.0,
            Pcm::F32 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        };
        (v * 32768.0).round_ties_even().clamp(-32768.0, 32767.0) as i16
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

/// One song being read: its container, its decoder, and the buffer the last packet decoded into.
pub struct Demuxed {
    reader: Box<dyn FormatReader + 'static>,
    track: u32,
    inner: Inner,
    codec: Option<Codec>,
    /// The container states the encoder's delay (an MP3's LAME header): its trims are the ones to cut.
    delay_known: bool,
    format: Format,
    out: Vec<i16>,
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
    /// Bytes still on their way, and who to wake when they come.
    loader: Option<(Arc<Loader>, Thread)>,
}

impl Demuxed {
    /// Opens `source` (with a file extension or MIME type as a hint) and reads from `from_ms`.
    /// `duration_ms` is the song's tagged length, for a container that does not say its own.
    pub fn open(
        source: Box<dyn MediaSource>, hint: Option<&str>, from_ms: i64, duration_ms: Option<i64>, loader: Option<(Arc<Loader>, Thread)>,
    ) -> Result<Demuxed, String> {
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
        let frames = track.num_frames.map(|n| n as i64);
        let duration_us = frames.map(|n| n * 1_000_000 / rate as i64).or(duration_ms.map(|d| d * 1000)).unwrap_or(0);
        let max_frames = codec.map_or(8192, Codec::max_frames);
        let id = track.id;
        let mut d = Demuxed {
            reader,
            track: id,
            inner,
            codec,
            delay_known,
            format: Format { rate, channels, encoding: Encoding::Pcm16 },
            out: vec![0; max_frames * channels],
            buf: Vec::new(),
            frame: Some(0),
            skip_to: 0,
            at_us: 0,
            duration_us,
            ended: false,
            primed: false,
            loader,
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
        let to = SeekTo::Time { time: Time::from_millis(ms), track_id: Some(self.track) };
        let seeked = self.reader.seek(SeekMode::Accurate, to).map_err(|e| e.to_string())?;
        self.skip_to = seeked.required_ts.get();
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
        let ch = self.format.channels;
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
            let n = match &mut self.inner {
                Inner::Coded(dec) => match dec.decode_i16(&packet.data, &mut self.out) {
                    Ok(n) => n,
                    Err(nori_player::decode::Fault::NeedRoom(need)) => {
                        // A packet larger than the codec's usual: grown once, then kept.
                        self.out.resize(need, 0);
                        match dec.take_i16(&mut self.out) {
                            Ok(n) => n,
                            Err(_) => continue,
                        }
                    }
                    Err(nori_player::decode::Fault::BadPacket) => continue,
                    Err(nori_player::decode::Fault::Broken) => {
                        self.ended = true;
                        return false;
                    }
                },
                Inner::Pcm(p) => {
                    let w = p.width();
                    let n = packet.data.len() / (w * ch);
                    if self.out.len() < n * ch {
                        self.out.resize(n * ch, 0);
                    }
                    for (o, b) in self.out.iter_mut().zip(packet.data.chunks_exact(w)) {
                        *o = p.sample(b);
                    }
                    n
                }
            };
            if n == 0 {
                continue;
            }
            let at = match self.frame {
                Some(f) => f,
                None if matches!(self.inner, Inner::Pcm(_)) => packet.pts.get(),
                None => self.song_frame(packet.pts.get()),
            };
            // The encoder's delay and padding, as the container states them. An Opus decoder drops its
            // own pre-skip.
            let trim_start = if self.codec == Some(Codec::Opus) || self.frame.is_none() { 0 } else { packet.trim_start.get() as usize };
            let trim_end = packet.trim_end.get() as usize;
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
            self.buf.extend(self.out[from * ch..to * ch].iter().flat_map(|v| v.to_le_bytes()));
            self.at_us = first * 1_000_000 / self.format.rate as i64;
            return true;
        }
    }
}

impl Reading for Demuxed {
    fn format(&self) -> Format {
        self.format
    }

    fn duration_us(&self) -> i64 {
        match self.frame {
            // Read to its end: exactly as long as what came out.
            Some(f) if self.ended => f * 1_000_000 / self.format.rate as i64,
            _ => self.duration_us,
        }
    }

    fn ready(&self) -> bool {
        self.primed || self.loader.as_ref().is_none_or(|(l, engine)| l.ready_or_wake(engine))
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
