//! Compressed audio to PCM, one packet at a time: the platform's extractor splits the file (an MP3
//! frame, a FLAC frame, an AAC access unit) and hands each packet in; the samples come out interleaved
//! straight into the caller's buffer. The same decoder on every platform, so every client hears the
//! same samples, and nothing is allocated per packet once the first one is decoded.
//!
//! HE-AAC (AAC+, SBR; v2 with PS) is the exception: symphonia decodes only its core (the lower half of
//! the band), and no decoder of its SBR in Rust is both free to link and fast enough. A platform lends
//! its own ([`lend_platform_aac`]: Android's MediaCodec, crates/android mediacodec.rs), and a stream
//! [`he_aac`] says is HE-AAC is decoded by it, packet by packet on the caller's thread, when the caller
//! asks for the whole ([`Decoder::whole_aac`]). Without one, the core is what plays.

use symphonia::core::audio::{Channels, Position};
use symphonia::core::codecs::audio::{well_known, AudioCodecParameters, AudioDecoder as Inner, AudioDecoderOptions};
use symphonia::core::errors::Error;
use symphonia::core::packet::PacketRef;
use symphonia::core::units::{Duration, Timestamp};

/// What the decoder can take, by the MIME type the platform's extractor names it with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    Mp3,
    Flac,
    /// AAC-LC. HE-AAC (SBR, PS) is not decoded here; the platform's decoder takes it.
    Aac,
    Vorbis,
    Alac,
    /// Mono and stereo Opus (channel mapping family 0); surround Opus is the platform's.
    Opus,
}

impl Codec {
    #[cfg(any(test, feature = "synth"))]
    pub fn from_mime(mime: &str) -> Option<Codec> {
        Some(match mime {
            "audio/mpeg" => Codec::Mp3,
            "audio/flac" => Codec::Flac,
            "audio/mp4a-latm" => Codec::Aac,
            "audio/vorbis" => Codec::Vorbis,
            "audio/alac" => Codec::Alac,
            "audio/opus" => Codec::Opus,
            _ => return None,
        })
    }

    /// Stable numbers for crossing to the platform.
    pub fn id(self) -> i32 {
        self as i32 + 1
    }

    pub fn from_id(id: i32) -> Option<Codec> {
        [Codec::Mp3, Codec::Flac, Codec::Aac, Codec::Vorbis, Codec::Alac, Codec::Opus].get(usize::try_from(id - 1).ok()?).copied()
    }

    /// The most frames one packet decodes to, for sizing the output before the first packet.
    pub fn max_frames(self) -> usize {
        match self {
            Codec::Mp3 => 1152,
            Codec::Aac => 2048,
            Codec::Vorbis => 8192,
            // FLAC's block size is at most 65535 frames; ALAC's frame is 4096 by default.
            Codec::Flac => 65535,
            Codec::Alac => 4096,
            // 120 ms at 48 kHz, the longest an Opus packet can be.
            Codec::Opus => OPUS_MAX_FRAMES,
        }
    }

    fn well_known(self) -> symphonia::core::codecs::audio::AudioCodecId {
        match self {
            Codec::Mp3 => well_known::CODEC_ID_MP3,
            Codec::Flac => well_known::CODEC_ID_FLAC,
            Codec::Aac => well_known::CODEC_ID_AAC,
            Codec::Vorbis => well_known::CODEC_ID_VORBIS,
            Codec::Alac => well_known::CODEC_ID_ALAC,
            Codec::Opus => well_known::CODEC_ID_OPUS,
        }
    }
}

/// Why a packet gave nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fault {
    /// This packet could not be decoded; the next one may be. Skip it.
    BadPacket,
    /// The caller's buffer is too small; the decoded samples are kept, ask for them with [`Decoder::take`].
    NeedRoom(usize),
    /// The stream cannot be decoded any further.
    Broken,
}

/// Whether the output is 16-bit integers or 32-bit floats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sample {
    I16,
    F32,
}

impl Sample {
    pub fn bytes(self) -> usize {
        match self {
            Sample::I16 => 2,
            Sample::F32 => 4,
        }
    }
}

/// What an MP3 decoder holds back: the first 529 samples it makes are its filterbank warming up.
/// media3 (1.11+) counts them into the encoder delay it reads from a LAME/Lavc header and trims them
/// itself, so a stream with that header is decoded raw; one without it, and every stream after a seek
/// (the trimming happens only once), has them dropped here - which is what Android's own MP3 decoder
/// does, so the samples line up with the packets' times either way.
pub const MP3_DECODER_DELAY: usize = 529;

const OPUS_MAX_FRAMES: usize = 5760;
/// Opus always decodes at 48 kHz.
const OPUS_RATE: u32 = 48_000;
/// What Opus plays in again after a seek when the stream does not say (80 ms, RFC 7845).
const OPUS_SEEK_PREROLL: usize = 3840;

/// The engines behind one decoder: symphonia's codecs, Opus, and a decoder the platform lends.
enum Engine {
    Symphonia(Box<dyn Inner>),
    Opus { dec: opus_rs::OpusDecoder, channels: usize, pre_skip: usize, pre_roll: usize, gain: f32 },
    Platform(Box<dyn PlatformDecoder>),
}

/// A decoder a platform lends for what is not decoded whole here (HE-AAC's SBR and PS), driven packet by
/// packet on the caller's thread: no thread of its own is woken for it.
pub trait PlatformDecoder: Send {
    /// Decodes one access unit, appending what came out to `out` (interleaved float; cleared by the
    /// caller, its room kept from packet to packet): the channels and rate of it. What comes out may
    /// lag what goes in by a packet or so, as the platform's decoder holds it.
    fn decode(&mut self, unit: &[u8], out: &mut Vec<f32>) -> Result<(usize, u32), Fault>;
    /// The next unit does not follow the last one (a seek): forget what is held.
    fn reset(&mut self);
}

/// An AAC stream as a platform's decoder is set up for it.
#[derive(Debug, Clone, Copy)]
pub struct AacSetup<'a> {
    /// The rate and channels the stream states (its core's, for HE-AAC signalled only in the stream).
    pub rate: u32,
    pub channels: usize,
    /// Its AudioSpecificConfig, when the container holds one (MP4); an ADTS stream has none.
    pub config: Option<&'a [u8]>,
}

/// What makes a platform's decoder for an HE-AAC stream; none when it cannot.
pub type PlatformAac = fn(&AacSetup) -> Option<Box<dyn PlatformDecoder>>;

static PLATFORM_AAC: std::sync::OnceLock<PlatformAac> = std::sync::OnceLock::new();

/// The platform lends its HE-AAC decoder, once, for the life of the process.
pub fn lend_platform_aac(make: PlatformAac) {
    let _ = PLATFORM_AAC.set(make);
}

/// Whether an AAC stream at `rate` with `config` (its AudioSpecificConfig; none for ADTS) is HE-AAC: its
/// object type says SBR (5) or PS (29), or its config carries the SBR extension (backward-compatible
/// explicit signalling), or it says AAC-LC at 24 kHz or less, which is how HE-AAC signalled only inside
/// the stream announces itself (internet radio's AAC+; `nori_settings::decoder::implicit_sbr`).
pub fn he_aac(config: Option<&[u8]>, rate: u32) -> bool {
    let implicit = rate > 0 && rate <= 24_000;
    let Some(c) = config.filter(|c| c.len() >= 2) else { return implicit };
    let mut bits = Bits { b: c, at: 0 };
    let object = |bits: &mut Bits| -> Option<u32> {
        let o = bits.take(5)?;
        if o == 31 { Some(32 + bits.take(6)?) } else { Some(o) }
    };
    let Some(aot) = object(&mut bits) else { return implicit };
    match aot {
        5 | 29 => true,
        2 => {
            // The rest of the config: the rate (an index, or 24 bits), the channels, then the 3 bits of
            // GASpecificConfig a stream with a channel configuration has; after them may come the SBR
            // extension: sync 0x2b7, object type 5, and whether SBR is present.
            let explicit = (|| {
                if bits.take(4)? == 15 {
                    bits.take(24)?;
                }
                let channels = bits.take(4)?;
                let (_frame_length, core_coder, _extension) = (bits.take(1)?, bits.take(1)?, bits.take(1)?);
                if channels == 0 || core_coder == 1 {
                    return None;
                }
                (bits.take(11)? == 0x2b7 && object(&mut bits)? == 5).then(|| bits.take(1)).flatten()
            })();
            match explicit {
                Some(sbr) => sbr == 1,
                None => implicit,
            }
        }
        _ => false,
    }
}

/// Bits read from the front, most significant first.
struct Bits<'a> {
    b: &'a [u8],
    at: usize,
}

impl Bits<'_> {
    fn take(&mut self, n: usize) -> Option<u32> {
        let mut v = 0u32;
        for _ in 0..n {
            let byte = *self.b.get(self.at / 8)?;
            v = v << 1 | (byte >> (7 - self.at % 8)) as u32 & 1;
            self.at += 1;
        }
        Some(v)
    }
}

/// An Opus stream's setup as media3 hands it over: the OpusHead, then the pre-skip and the seek
/// pre-roll in nanoseconds (8 bytes each, native order). Channels, pre-skip in samples, pre-roll in
/// samples, and the output gain as a factor; none for what this cannot decode (surround).
fn opus_setup(extra: Option<&[u8]>) -> Option<(usize, usize, usize, f32)> {
    let e = extra?;
    if e.len() < 19 || &e[..8] != b"OpusHead" || e[18] != 0 {
        return None;
    }
    let channels = e[9] as usize;
    if !(1..=2).contains(&channels) {
        return None;
    }
    let pre_skip = u16::from_le_bytes([e[10], e[11]]) as usize;
    let gain_q8 = i16::from_le_bytes([e[16], e[17]]);
    let gain = 10f32.powf(gain_q8 as f32 / (20.0 * 256.0));
    let pre_roll = e
        .get(19 + 8..19 + 16)
        .map(|b| i64::from_ne_bytes(b.try_into().unwrap()))
        .filter(|&ns| ns > 0)
        .map_or(OPUS_SEEK_PREROLL, |ns| (ns as u128 * OPUS_RATE as u128 / 1_000_000_000) as usize);
    Some((channels, pre_skip, pre_roll, gain))
}

/// One packet's samples as [`Decoder::decode_lent`] lends them, and the shape they came out in.
pub struct Lent<'a> {
    pub samples: &'a [f32],
    pub channels: usize,
    pub rate: u32,
}

pub struct Decoder {
    inner: Engine,
    codec: Codec,
    channels: usize,
    rate: u32,
    /// The last packet's samples, interleaved, before anything is dropped from their start.
    scratch: Vec<f32>,
    /// Frames of the last packet still to be handed out, and where in `scratch` they start.
    held: usize,
    from: usize,
    /// Frames still to drop from the start of what comes out.
    skip: usize,
    /// The next packet is the first after a reset.
    fresh: bool,
    /// A packet has been decoded: the rate and channels are the stream's own, not what it was opened with.
    decoded: bool,
}

impl Decoder {
    /// A decoder for `codec` at `rate` Hz and `channels` channels. `extra` is the codec's own setup data
    /// as the platform's extractor gives it (FLAC's STREAMINFO, AAC's AudioSpecificConfig, Vorbis's
    /// headers, ALAC's magic cookie). `delay_known`: the stream says its encoder delay (an MP3's LAME
    /// header), which the platform then trims, decoder delay included.
    pub fn new(codec: Codec, rate: u32, channels: usize, extra: Option<&[u8]>, delay_known: bool) -> Result<Decoder, String> {
        if codec == Codec::Opus {
            let (channels, pre_skip, pre_roll, gain) = opus_setup(extra).ok_or("not a mono or stereo Opus stream")?;
            let dec = opus_rs::OpusDecoder::new(OPUS_RATE as i32, channels).map_err(str::to_string)?;
            return Ok(Decoder {
                inner: Engine::Opus { dec, channels, pre_skip, pre_roll, gain },
                codec,
                channels,
                rate: OPUS_RATE,
                scratch: vec![0.0; OPUS_MAX_FRAMES * channels],
                held: 0,
                from: 0,
                skip: pre_skip,
                fresh: false,
                decoded: false,
            });
        }
        let inner = Decoder::symphonia(codec, rate, channels, extra)?;
        let channels = channels.max(1);
        Ok(Decoder {
            inner: Engine::Symphonia(inner),
            codec,
            channels,
            rate,
            scratch: vec![0.0; codec.max_frames() * channels],
            held: 0,
            from: 0,
            skip: if codec == Codec::Mp3 && !delay_known { MP3_DECODER_DELAY } else { 0 },
            fresh: false,
            decoded: false,
        })
    }

    /// symphonia's decoder for `codec` at `rate` and `channels`.
    fn symphonia(codec: Codec, rate: u32, channels: usize, extra: Option<&[u8]>) -> Result<Box<dyn Inner>, String> {
        let mut params = AudioCodecParameters::new();
        params.for_codec(codec.well_known()).with_sample_rate(rate);
        if let Some(p) = Position::from_count(channels as u32) {
            params.with_channels(Channels::Positioned(p));
        }
        if let Some(e) = extra.filter(|e| !e.is_empty()) {
            // media3 hands FLAC over as the stream starts: "fLaC" and the block header before STREAMINFO.
            let e = if codec == Codec::Flac && e.starts_with(b"fLaC") && e.len() >= 8 { &e[8..] } else { e };
            params.with_extra_data(e.into());
        }
        // media3's sink trims the encoder's delay and padding itself; trimming here as well would do it twice.
        let opts = AudioDecoderOptions::default().gapless(false);
        symphonia::default::get_codecs().make_audio_decoder(&params, &opts).map_err(|e| e.to_string())
    }

    /// A decoder for an AAC stream that plays it whole: the platform's (see [`lend_platform_aac`]) when
    /// [`he_aac`] says it is HE-AAC and a platform lent one, which then says the real rate and channels
    /// once the first packets are decoded; this crate's (the core alone, for HE-AAC) otherwise.
    pub fn whole_aac(rate: u32, channels: usize, config: Option<&[u8]>) -> Result<Decoder, String> {
        let platform = PLATFORM_AAC.get().filter(|_| he_aac(config, rate)).and_then(|make| make(&AacSetup { rate, channels, config }));
        match platform {
            Some(dec) => Ok(Decoder {
                inner: Engine::Platform(dec),
                codec: Codec::Aac,
                channels: channels.max(1),
                rate,
                // Two packets of HE-AAC at twice the core's rate, in stereo (PS makes stereo of mono).
                scratch: Vec::with_capacity(2 * 2048 * 2.max(channels)),
                held: 0,
                from: 0,
                skip: 0,
                fresh: false,
                decoded: false,
            }),
            None => Decoder::new(Codec::Aac, rate, channels, config, false),
        }
    }

    /// Whether the platform's decoder decodes this stream.
    #[cfg(any(test, feature = "synth"))]
    pub fn on_platform(&self) -> bool {
        matches!(self.inner, Engine::Platform(_))
    }

    pub fn codec(&self) -> Codec {
        self.codec
    }

    /// The channels and rate of what comes out: known for certain after the first packet.
    pub fn channels(&self) -> usize {
        self.channels
    }
    pub fn rate(&self) -> u32 {
        self.rate
    }

    /// Decodes `packet` into `out` (interleaved), returning the frames written. A packet that does not
    /// fit is kept: [`Fault::NeedRoom`] says how many samples it needs, and `take_*` hands it over.
    #[cfg(any(test, feature = "synth"))]
    pub fn decode_i16(&mut self, packet: &[u8], out: &mut [i16]) -> Result<usize, Fault> {
        self.decode(packet)?;
        self.take_i16(out)
    }

    #[cfg(any(test, feature = "synth"))]
    pub fn decode_f32(&mut self, packet: &[u8], out: &mut [f32]) -> Result<usize, Fault> {
        self.decode(packet)?;
        self.take_f32(out)
    }

    /// Decodes `packet` and lends its samples where they lie, interleaved: for a reader in the same
    /// process (the analyser measuring a track ahead) that would only copy them on. Nothing is kept
    /// for `take_*`.
    pub fn decode_lent(&mut self, packet: &[u8]) -> Result<Lent<'_>, Fault> {
        self.decode(packet)?;
        let n = std::mem::take(&mut self.held) * self.channels;
        let from = self.from * self.channels;
        Ok(Lent { samples: &self.scratch[from..from + n], channels: self.channels, rate: self.rate })
    }

    fn decode(&mut self, packet: &[u8]) -> Result<(), Fault> {
        self.held = 0;
        let mut fresh = std::mem::take(&mut self.fresh);
        let frames = match &mut self.inner {
            Engine::Opus { dec, channels, gain, .. } => {
                let n = dec.decode(packet, OPUS_MAX_FRAMES, &mut self.scratch).map_err(|_| Fault::BadPacket)?;
                if *gain != 1.0 {
                    self.scratch[..n * *channels].iter_mut().for_each(|s| *s *= *gain);
                }
                n
            }
            Engine::Platform(dec) => {
                self.scratch.clear();
                let (channels, rate) = dec.decode(packet, &mut self.scratch)?;
                (self.channels, self.rate) = (channels.max(1), rate);
                self.decoded = true;
                self.scratch.len() / self.channels
            }
            Engine::Symphonia(inner) => {
                // An MP3 stream whose rate or channels changed (a station's next song): symphonia's decoder
                // refuses every frame of another shape than its first, so one for the new shape takes
                // over. What the old one still held (its filterbank's last 529 samples) is not heard; the
                // new one's first frame, whose audio may begin in frames it never saw, is silence.
                if self.codec == Codec::Mp3 && self.decoded {
                    if let Some((rate, channels)) = mp3_shape(packet).filter(|&s| s != (self.rate, self.channels)) {
                        *inner = Decoder::symphonia(Codec::Mp3, rate, channels, None).map_err(|_| Fault::Broken)?;
                        (self.rate, self.channels) = (rate, channels);
                        fresh = true;
                    }
                }
                let p = PacketRef::new(0, Timestamp::new(0), Duration::new(0), packet);
                match inner.decode_ref(&p) {
                    Ok(buf) => {
                        let spec = buf.spec();
                        self.channels = spec.channels().count().max(1);
                        self.rate = spec.rate();
                        let n = buf.frames();
                        if self.scratch.len() < n * self.channels {
                            // A stream larger than its codec's usual packet: grown once, then kept.
                            self.scratch.resize(n * self.channels, 0.0);
                        }
                        buf.copy_to_slice_interleaved::<f32, _>(&mut self.scratch[..n * self.channels]);
                        self.decoded = true;
                        n
                    }
                    Err(Error::DecodeError(_)) | Err(Error::IoError(_)) => return Err(Fault::BadPacket),
                    Err(Error::ResetRequired) => {
                        inner.reset();
                        return Err(Fault::BadPacket);
                    }
                    Err(_) => return Err(Fault::Broken),
                }
            }
        };
        // The first MP3 frame after a seek whose audio begins in frames before it (the bit reservoir)
        // cannot be decoded whole: silence, as Android's decoder gives, rather than half a frame.
        if fresh && self.codec == Codec::Mp3 && mp3_main_data_begin(packet) != 0 {
            self.scratch[..frames * self.channels].fill(0.0);
        }
        let drop = self.skip.min(frames);
        self.skip -= drop;
        self.from = drop;
        self.held = frames - drop;
        Ok(())
    }

    /// The last packet's samples, into `out`: rounded to 16 bits the way every reference decoder does
    /// (symphonia's own conversion truncates towards zero, half a step low on average).
    pub fn take_i16(&mut self, out: &mut [i16]) -> Result<usize, Fault> {
        let n = self.held * self.channels;
        if n > out.len() {
            return Err(Fault::NeedRoom(n));
        }
        let src = &self.scratch[self.from * self.channels..self.from * self.channels + n];
        for (o, s) in out[..n].iter_mut().zip(src) {
            *o = (s * 32768.0).round_ties_even().clamp(-32768.0, 32767.0) as i16;
        }
        Ok(std::mem::take(&mut self.held))
    }

    pub fn take_f32(&mut self, out: &mut [f32]) -> Result<usize, Fault> {
        let n = self.held * self.channels;
        if n > out.len() {
            return Err(Fault::NeedRoom(n));
        }
        out[..n].copy_from_slice(&self.scratch[self.from * self.channels..self.from * self.channels + n]);
        Ok(std::mem::take(&mut self.held))
    }

    /// The next packet does not follow the last one (a seek; `at_start` when back at the very
    /// beginning): forget what came before. An MP3's filterbank warms up again, and the platform does
    /// not trim a second time; Opus plays its pre-skip again at the start, and its pre-roll anywhere else.
    pub fn reset(&mut self, at_start: bool) {
        self.held = 0;
        self.fresh = true;
        match &mut self.inner {
            Engine::Symphonia(inner) => {
                inner.reset();
                if self.codec == Codec::Mp3 {
                    self.skip = MP3_DECODER_DELAY;
                }
            }
            Engine::Platform(dec) => dec.reset(),
            Engine::Opus { dec, channels, pre_skip, pre_roll, .. } => {
                // No reset of its own: a new one, once per seek.
                if let Ok(d) = opus_rs::OpusDecoder::new(OPUS_RATE as i32, *channels) {
                    *dec = d;
                }
                self.skip = if at_start { *pre_skip } else { *pre_roll };
            }
        }
    }
}

/// The rate and channels an MPEG audio frame (layer I, II or III, MPEG-1, 2 or 2.5) says it holds, from its
/// header; none for bytes that are not one.
pub fn mp3_shape(frame: &[u8]) -> Option<(u32, usize)> {
    let h = u32::from_be_bytes(frame.get(..4)?.try_into().ok()?);
    let (version, layer, bitrate, rate) = ((h >> 19) & 3, (h >> 17) & 3, (h >> 12) & 0xf, (h >> 10) & 3);
    if h >> 21 != 0x7ff || version == 1 || layer == 0 || bitrate == 0xf || rate == 3 {
        return None;
    }
    let base = [44_100, 48_000, 32_000][rate as usize];
    let rate = match version {
        3 => base,
        2 => base / 2,
        _ => base / 4,
    };
    Some((rate, if (h >> 6) & 3 == 3 { 1 } else { 2 }))
}

/// Where an MP3 frame's audio data starts, counted back into the frames before it (0: in this one).
fn mp3_main_data_begin(frame: &[u8]) -> u16 {
    if frame.len() < 7 {
        return 0;
    }
    let mpeg1 = frame[1] & 0x08 != 0;
    let crc = frame[1] & 0x01 == 0;
    let side = if crc { 6 } else { 4 };
    if frame.len() < side + 2 {
        return 0;
    }
    let bits = u16::from_be_bytes([frame[side], frame[side + 1]]);
    // MPEG-1: 9 bits; MPEG-2 and 2.5: 8.
    if mpeg1 { bits >> 7 } else { bits >> 8 }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::{mp3_frames, ogg_opus};

    #[test]
    fn codecs_cross_by_number_and_are_named_by_mime() {
        for c in [Codec::Mp3, Codec::Flac, Codec::Aac, Codec::Vorbis, Codec::Alac, Codec::Opus] {
            assert_eq!(Codec::from_id(c.id()), Some(c));
        }
        assert_eq!(Codec::from_mime("audio/mpeg"), Some(Codec::Mp3));
        assert_eq!(Codec::from_mime("audio/ac3"), None, "the platform's decoder takes what this cannot");
        assert_eq!(Codec::from_id(0), None);
    }

    #[test]
    fn an_mp3_decodes_to_the_tone_it_was_made_from() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/tone440.mp3")).unwrap();
        let frames = mp3_frames(&file);
        assert!(frames.len() > 30);
        let mut d = Decoder::new(Codec::Mp3, 44_100, 2, None, true).unwrap();
        let mut out = vec![0i16; Codec::Mp3.max_frames() * 2];
        let mut pcm: Vec<i16> = Vec::new();
        for f in &frames {
            let n = d.decode_i16(f, &mut out).unwrap();
            pcm.extend_from_slice(&out[..n * 2]);
        }
        assert_eq!((d.channels(), d.rate()), (2, 44_100));
        // Well past the start (the encoder's delay is silence), the left channel is the 440 Hz tone.
        let left: Vec<f64> = pcm.chunks(2).skip(4410).take(22_050).map(|c| c[0] as f64).collect();
        let rms = (left.iter().map(|v| v * v).sum::<f64>() / left.len() as f64).sqrt();
        // ffmpeg's decoder gives 8060.1 over the same stretch (the encoder took the tone down a little).
        assert!((rms - 8060.1).abs() < 5.0, "rms {rms}");
        let crossings = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        // Half a second of 440 Hz crosses zero 440 times.
        assert!((438..=442).contains(&crossings), "{crossings} crossings");
    }

    #[test]
    fn a_seek_resets_and_decoding_carries_on() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/tone440.mp3")).unwrap();
        let frames = mp3_frames(&file);
        let mut d = Decoder::new(Codec::Mp3, 44_100, 2, None, true).unwrap();
        let mut out = vec![0i16; 1152 * 2];
        for f in &frames[..10] {
            d.decode_i16(f, &mut out).unwrap();
        }
        d.reset(false);
        let n: usize = frames[20..].iter().map(|f| d.decode_i16(f, &mut out).unwrap_or(0)).sum();
        assert!(n > 1152 * (frames.len() - 22));
        // Too small a buffer keeps the packet for a second ask.
        let mut small = [0i16; 16];
        assert_eq!(d.decode_i16(frames[5], &mut small), Err(Fault::NeedRoom(2304)));
        assert_eq!(d.take_i16(&mut out), Ok(1152));
    }

    #[test]
    fn the_mp3_decoder_delay_is_dropped_only_where_nobody_else_trims_it() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/tone440.mp3")).unwrap();
        let frames = mp3_frames(&file);
        let mut out = vec![0i16; 1152 * 2];
        // With a LAME header the platform trims the delay: raw frames.
        let mut known = Decoder::new(Codec::Mp3, 44_100, 2, None, true).unwrap();
        assert_eq!(known.decode_i16(frames[0], &mut out), Ok(1152));
        // Without one, the decoder's own 529 go here.
        let mut unknown = Decoder::new(Codec::Mp3, 44_100, 2, None, false).unwrap();
        assert_eq!(unknown.decode_i16(frames[0], &mut out), Ok(1152 - MP3_DECODER_DELAY));
        assert_eq!(unknown.decode_i16(frames[1], &mut out), Ok(1152));
        // After a seek, always: the platform trims only once.
        known.decode_i16(frames[1], &mut out).unwrap();
        known.reset(false);
        assert_eq!(known.decode_i16(frames[20], &mut out), Ok(1152 - MP3_DECODER_DELAY));
        assert_eq!(known.decode_i16(frames[21], &mut out), Ok(1152));
    }

    #[test]
    fn lent_samples_are_the_ones_taken_otherwise() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/tone440.mp3")).unwrap();
        let frames = mp3_frames(&file);
        let mut taken = Decoder::new(Codec::Mp3, 44_100, 2, None, false).unwrap();
        let mut lent = Decoder::new(Codec::Mp3, 44_100, 2, None, false).unwrap();
        let mut out = vec![0f32; 1152 * 2];
        for f in &frames[..4] {
            let n = taken.decode_f32(f, &mut out).unwrap();
            let l = lent.decode_lent(f).unwrap();
            assert_eq!((l.samples, l.channels, l.rate), (&out[..n * 2], 2, 44_100), "the decoder delay dropped the same way");
        }
        assert_eq!(lent.take_f32(&mut out), Ok(0), "nothing kept after a loan");
    }

    #[test]
    fn a_frame_whose_audio_began_before_the_seek_is_silence() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/tone440.mp3")).unwrap();
        let frames = mp3_frames(&file);
        let reservoir = frames.iter().position(|f| mp3_main_data_begin(f) != 0).expect("a frame using the bit reservoir");
        let mut d = Decoder::new(Codec::Mp3, 44_100, 2, None, true).unwrap();
        let mut out = vec![1i16; 1152 * 2];
        d.reset(false);
        let n = d.decode_i16(frames[reservoir], &mut out).unwrap();
        assert!(out[..n * 2].iter().all(|&s| s == 0));
    }

    #[test]
    fn a_frame_header_says_its_rate_and_channels() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/tone440.mp3")).unwrap();
        assert_eq!(mp3_shape(mp3_frames(&file)[3]), Some((44_100, 2)));
        // MPEG-2 at 22.05 kHz, mono; MPEG-2.5 at 8 kHz, joint stereo; MPEG-1 at 48 kHz.
        assert_eq!(mp3_shape(&[0xff, 0xf3, 0x00, 0xc0]), Some((22_050, 1)));
        assert_eq!(mp3_shape(&[0xff, 0xe3, 0x08, 0x40]), Some((8_000, 2)));
        assert_eq!(mp3_shape(&[0xff, 0xfb, 0x94, 0x00]), Some((48_000, 2)));
        // Not a header: no sync, the reserved version, the reserved rate, too short.
        assert_eq!(mp3_shape(&[0x00, 0xfb, 0x90, 0x00]), None);
        assert_eq!(mp3_shape(&[0xff, 0xeb, 0x90, 0x00]), None);
        assert_eq!(mp3_shape(&[0xff, 0xfb, 0x9c, 0x00]), None);
        assert_eq!(mp3_shape(&[0xff, 0xfb]), None);
    }

    #[test]
    fn an_mp3_decoder_follows_the_stream_to_another_rate_and_channel_count() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/tone440.mp3")).unwrap();
        let frames = mp3_frames(&file);
        let mut d = Decoder::new(Codec::Mp3, 44_100, 2, None, true).unwrap();
        let mut out = vec![0f32; 1152 * 2];
        d.decode_f32(frames[0], &mut out).unwrap();
        // A 22.05 kHz mono MPEG-2 frame of silence: header, side info, no main data.
        let mut mpeg2 = vec![0u8; 64];
        mpeg2[..4].copy_from_slice(&[0xff, 0xf3, 0x10, 0xc0]);
        mpeg2.truncate(mp3_frame_len(&mpeg2));
        assert_eq!(d.decode_f32(&mpeg2, &mut out), Ok(576), "a frame of the new shape decodes");
        assert_eq!((d.rate(), d.channels()), (22_050, 1));
        assert!(d.decode_f32(frames[5], &mut out).is_ok(), "and back");
        assert_eq!((d.rate(), d.channels()), (44_100, 2));
    }

    /// An MPEG-2 layer III frame's length from its header (8 kbps steps per the table's index).
    fn mp3_frame_len(h: &[u8]) -> usize {
        let kbps = [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160][(h[2] >> 4) as usize];
        72 * kbps * 1000 / mp3_shape(h).unwrap().0 as usize
    }

    #[test]
    fn he_aac_is_known_by_its_config_or_by_a_core_rate() {
        // AudioSpecificConfigs: object type, rate index, channels, GASpecificConfig, extension.
        let lc_44 = [0x12, 0x10]; // AAC-LC, 44.1 kHz, stereo
        let lc_22 = [0x13, 0x90]; // AAC-LC, 22.05 kHz, stereo
        let sbr = [0x2b, 0x92, 0x08, 0x00]; // object type 5 (SBR), 22.05 kHz core, stereo, 44.1 kHz out
        let ps = [0xeb, 0x09, 0x88, 0x00]; // object type 29 (PS), 24 kHz core, mono
        let explicit = [0x13, 0x90, 0x56, 0xe5, 0x98]; // AAC-LC 22.05 kHz stereo, sync 0x2b7, SBR 5, present, 44.1 kHz
        let explicit_off = [0x11, 0x90, 0x56, 0xe5, 0x00]; // AAC-LC 48 kHz, sync 0x2b7, SBR 5, not present
        assert!(!he_aac(Some(&lc_44), 44_100), "AAC-LC at a full rate");
        assert!(he_aac(Some(&lc_22), 22_050), "AAC-LC at a core rate: SBR signalled in the stream, as radio sends it");
        assert!(he_aac(Some(&sbr), 22_050) && he_aac(Some(&sbr), 44_100));
        assert!(he_aac(Some(&ps), 24_000));
        assert!(he_aac(Some(&explicit), 22_050), "the SBR extension after an AAC-LC config");
        assert!(!he_aac(Some(&explicit_off), 48_000), "the extension saying there is no SBR");
        assert!(he_aac(None, 22_050) && !he_aac(None, 48_000), "ADTS: by its rate");
        assert!(!he_aac(None, 0), "unknown: as it says");
        assert!(!he_aac(Some(&[0x0a, 0x10]), 22_050), "AAC Main, whatever its rate");
    }

    /// A platform decoder that says what it was handed: each unit's first byte, as a frame of 2048 at
    /// twice the rate, in stereo.
    struct Echo;

    impl PlatformDecoder for Echo {
        fn decode(&mut self, unit: &[u8], out: &mut Vec<f32>) -> Result<(usize, u32), Fault> {
            out.extend(std::iter::repeat_n(unit[0] as f32, 2048 * 2));
            Ok((2, 44_100))
        }
        fn reset(&mut self) {}
    }

    #[test]
    fn he_aac_goes_to_the_platforms_decoder_and_aac_lc_stays_here() {
        lend_platform_aac(|_| Some(Box::new(Echo)));
        let mut d = Decoder::whole_aac(22_050, 2, None).unwrap();
        assert!(d.on_platform(), "an ADTS stream at a core rate");
        let l = d.decode_lent(&[7, 1, 2]).unwrap();
        assert_eq!((l.samples.len(), l.channels, l.rate), (4096, 2, 44_100));
        assert!(l.samples.iter().all(|&s| s == 7.0));
        assert_eq!((d.rate(), d.channels()), (44_100, 2), "the platform's shape is the stream's");
        assert!(!Decoder::whole_aac(44_100, 2, Some(&[0x12, 0x10])).unwrap().on_platform(), "AAC-LC is decoded here");
        assert!(!Decoder::new(Codec::Aac, 22_050, 2, None, false).unwrap().on_platform(), "the core alone, when asked for");
    }

    #[test]
    fn samples_are_rounded_not_truncated() {
        let mut d = Decoder::new(Codec::Mp3, 44_100, 2, None, true).unwrap();
        d.scratch[..4].copy_from_slice(&[0.5 / 32768.0, -1.6 / 32768.0, 1.0, -1.0]);
        d.held = 2;
        d.from = 0;
        let mut out = [0i16; 4];
        d.take_i16(&mut out).unwrap();
        assert_eq!(out, [0, -2, 32767, -32768], "ties to even, rounded, clamped");
    }

    #[test]
    fn flac_setup_is_taken_as_media3_hands_it_over() {
        // "fLaC", a STREAMINFO block header, then a 34-byte STREAMINFO for 44.1 kHz stereo 16-bit.
        let mut e = b"fLaC\x80\x00\x00\x22".to_vec();
        e.extend_from_slice(&[0x10, 0x00, 0x10, 0x00, 0, 0, 0, 0, 0, 0, 0x0A, 0xC4, 0x42, 0xF0, 0, 0, 0, 0]);
        e.extend_from_slice(&[0u8; 16]);
        assert!(Decoder::new(Codec::Flac, 44_100, 2, Some(&e), false).is_ok());
    }

    #[test]
    fn an_opus_stream_decodes_to_its_tone_without_the_pre_skip() {
        let file = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/testdata/tone440.opus")).unwrap();
        let (setup, packets) = ogg_opus(&file);
        let mut d = Decoder::new(Codec::Opus, 48_000, 2, Some(&setup), false).unwrap();
        let mut out = vec![0i16; Codec::Opus.max_frames() * 2];
        let mut pcm: Vec<i16> = Vec::new();
        for p in &packets {
            let n = d.decode_i16(p, &mut out).unwrap();
            pcm.extend_from_slice(&out[..n * 2]);
        }
        assert_eq!((d.channels(), d.rate()), (2, 48_000));
        // The pre-skip is gone: what is left is the second of tone, plus at most the last packet's padding.
        assert!(pcm.len() / 2 >= 48_000 && pcm.len() / 2 < 48_000 + 960, "{} frames", pcm.len() / 2);
        let left: Vec<f64> = pcm.chunks(2).skip(4800).take(24_000).map(|c| c[0] as f64).collect();
        let rms = (left.iter().map(|v| v * v).sum::<f64>() / left.len() as f64).sqrt();
        // ffmpeg's libopus decode gives 8463.3 over the same stretch.
        assert!((rms - 8463.3).abs() < 30.0, "rms {rms}");
        let crossings = left.windows(2).filter(|w| (w[0] < 0.0) != (w[1] < 0.0)).count();
        assert!((438..=442).contains(&crossings), "{crossings} crossings");
        // A seek away from the start plays the pre-roll in again rather than the pre-skip.
        d.reset(false);
        assert_eq!(d.decode_i16(&packets[20], &mut out), Ok(0), "the first 20 ms go to the 80 ms pre-roll");
    }

    #[test]
    fn surround_opus_is_the_platforms() {
        let mut head = b"OpusHead\x01\x06\x38\x01\x80\xbb\x00\x00\x00\x00\x01".to_vec();
        head.extend_from_slice(&[4, 2, 0, 4, 1, 2, 3, 5]);
        assert!(Decoder::new(Codec::Opus, 48_000, 6, Some(&head), false).is_err());
    }
}
