//! HE-AAC (AAC+, SBR; v2 with PS) for the Rust engine, decoded by the platform's own AAC decoder through
//! the NDK's MediaCodec (`libmediandk`), lent to `nori_player::decode` ([`lend`]) so that a station's
//! treble is heard: symphonia decodes only the core.
//!
//! The NDK's C API is used rather than MediaCodec through JNI: no Java object per packet and no thread of
//! ours. It is driven synchronously on the thread that decodes (the engine's, in its bursts): a packet
//! goes into one of the codec's input buffers and what came out is taken from its output buffers, waiting
//! at most [`WAIT_US`] for it. Nothing is allocated here per packet: the packet is copied into the codec's
//! own buffer, and its output converted straight into the caller's (kept) buffer. MediaCodec's own
//! message thread in this process still answers each call, as it does for ExoPlayer's MediaCodec path.
//!
//! A stream is set up as the demuxer states it (AAC-LC at the core's rate for SBR signalled only inside
//! the stream, as ADTS radio says it): the platform's decoder finds the SBR and PS itself and says the
//! real rate and channels in its output format, which is what the engine then plays at.

/// Lends the platform's HE-AAC decoder to the core's decoder, for the life of the process.
pub(crate) fn lend() {
    #[cfg(target_os = "android")]
    nori_player::decode::lend_platform_aac(ndk::open);
}

/// The AudioSpecificConfig of an AAC-LC stream at `rate` with `channels` (what an ADTS header says, as
/// MediaCodec wants it in `csd-0`); none for a rate AAC has no index for.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn lc_config(rate: u32, channels: usize) -> Option<[u8; 2]> {
    const RATES: [u32; 13] = [96_000, 88_200, 64_000, 48_000, 44_100, 32_000, 24_000, 22_050, 16_000, 12_000, 11_025, 8_000, 7_350];
    let index = RATES.iter().position(|&r| r == rate)? as u16;
    let channels = (channels as u16).clamp(1, 7);
    let asc: u16 = 2 << 11 | index << 7 | channels << 3;
    Some(asc.to_be_bytes())
}

/// Frames of 16-bit or float PCM in `bytes`, appended to `out` as floats.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
pub(crate) fn append_pcm(bytes: &[u8], float: bool, out: &mut Vec<f32>) {
    if float {
        out.extend(bytes.chunks_exact(4).map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]])));
    } else {
        out.extend(bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0));
    }
}

/// How long one call waits for the platform's decoder to hand out what it made of the packet, µs. A
/// software AAC decoder answers well within it; a decoder that holds a packet back is learnt (see
/// `lag`) and not waited on again.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const WAIT_US: i64 = 20_000;

#[cfg(target_os = "android")]
mod ndk {
    use std::ffi::{c_char, c_void, CStr};

    use nori_player::decode::{AacSetup, Fault, PlatformDecoder};

    use super::*;

    #[repr(C)]
    struct AMediaCodec {
        _p: [u8; 0],
    }
    #[repr(C)]
    struct AMediaFormat {
        _p: [u8; 0],
    }
    #[repr(C)]
    #[derive(Default)]
    struct BufferInfo {
        offset: i32,
        size: i32,
        presentation_time_us: i64,
        flags: u32,
    }

    const OK: i32 = 0;
    const TRY_AGAIN_LATER: isize = -1;
    const OUTPUT_FORMAT_CHANGED: isize = -2;
    const OUTPUT_BUFFERS_CHANGED: isize = -3;
    const ENCODING_PCM_FLOAT: i32 = 4;

    #[link(name = "mediandk")]
    unsafe extern "C" {
        fn AMediaCodec_createDecoderByType(mime: *const c_char) -> *mut AMediaCodec;
        fn AMediaCodec_configure(codec: *mut AMediaCodec, format: *const AMediaFormat, surface: *mut c_void, crypto: *mut c_void, flags: u32) -> i32;
        fn AMediaCodec_start(codec: *mut AMediaCodec) -> i32;
        fn AMediaCodec_stop(codec: *mut AMediaCodec) -> i32;
        fn AMediaCodec_delete(codec: *mut AMediaCodec) -> i32;
        fn AMediaCodec_flush(codec: *mut AMediaCodec) -> i32;
        fn AMediaCodec_dequeueInputBuffer(codec: *mut AMediaCodec, timeout_us: i64) -> isize;
        fn AMediaCodec_getInputBuffer(codec: *mut AMediaCodec, idx: usize, out_size: *mut usize) -> *mut u8;
        fn AMediaCodec_queueInputBuffer(codec: *mut AMediaCodec, idx: usize, offset: libc::off_t, size: usize, time_us: u64, flags: u32) -> i32;
        fn AMediaCodec_dequeueOutputBuffer(codec: *mut AMediaCodec, info: *mut BufferInfo, timeout_us: i64) -> isize;
        fn AMediaCodec_getOutputBuffer(codec: *mut AMediaCodec, idx: usize, out_size: *mut usize) -> *mut u8;
        fn AMediaCodec_releaseOutputBuffer(codec: *mut AMediaCodec, idx: usize, render: bool) -> i32;
        fn AMediaCodec_getOutputFormat(codec: *mut AMediaCodec) -> *mut AMediaFormat;
        fn AMediaFormat_new() -> *mut AMediaFormat;
        fn AMediaFormat_delete(format: *mut AMediaFormat) -> i32;
        fn AMediaFormat_setString(format: *mut AMediaFormat, name: *const c_char, value: *const c_char);
        fn AMediaFormat_setInt32(format: *mut AMediaFormat, name: *const c_char, value: i32);
        fn AMediaFormat_setBuffer(format: *mut AMediaFormat, name: *const c_char, data: *const c_void, size: usize);
        fn AMediaFormat_getInt32(format: *mut AMediaFormat, name: *const c_char, out: *mut i32) -> bool;
    }

    const MIME: &CStr = c"audio/mp4a-latm";

    /// One stream's MediaCodec, started.
    struct Codec {
        codec: *mut AMediaCodec,
        channels: usize,
        rate: u32,
        float: bool,
        /// Packets queued whose output has not come yet, and how many of those the decoder is known to
        /// hold back (a call does not wait for those).
        pending: usize,
        lag: usize,
        /// A made-up presentation time for each packet, µs: MediaCodec wants them rising.
        time_us: u64,
    }

    // SAFETY: a MediaCodec may be called from any thread, one call at a time; the core's decoder is used
    // by one thread at a time (the engine's, or the thread opening the song before it).
    unsafe impl Send for Codec {}

    pub(super) fn open(setup: &AacSetup) -> Option<Box<dyn PlatformDecoder>> {
        let config: &[u8] = match setup.config {
            Some(c) if !c.is_empty() => c,
            _ => &lc_config(setup.rate, setup.channels)?,
        };
        // SAFETY: plain NDK calls on pointers checked for null; the format is deleted once configured.
        unsafe {
            let codec = AMediaCodec_createDecoderByType(MIME.as_ptr());
            if codec.is_null() {
                return None;
            }
            let format = AMediaFormat_new();
            AMediaFormat_setString(format, c"mime".as_ptr(), MIME.as_ptr());
            AMediaFormat_setInt32(format, c"sample-rate".as_ptr(), setup.rate as i32);
            AMediaFormat_setInt32(format, c"channel-count".as_ptr(), setup.channels as i32);
            AMediaFormat_setInt32(format, c"is-adts".as_ptr(), 0);
            // Float when the decoder gives it (Android 12's), so nothing is rounded to 16 bits on the way.
            AMediaFormat_setInt32(format, c"pcm-encoding".as_ptr(), ENCODING_PCM_FLOAT);
            AMediaFormat_setBuffer(format, c"csd-0".as_ptr(), config.as_ptr().cast(), config.len());
            let configured = AMediaCodec_configure(codec, format, std::ptr::null_mut(), std::ptr::null_mut(), 0);
            AMediaFormat_delete(format);
            if configured != OK || AMediaCodec_start(codec) != OK {
                nori_core::alog::info("decoder: MediaCodec would not take HE-AAC; its core plays");
                AMediaCodec_delete(codec);
                return None;
            }
            let mut c = Codec { codec, channels: setup.channels.max(1), rate: setup.rate, float: false, pending: 0, lag: 0, time_us: 0 };
            c.read_format();
            Some(Box::new(c))
        }
    }

    impl Codec {
        /// The output's rate, channels and encoding, as the decoder says them.
        fn read_format(&mut self) {
            // SAFETY: the format the codec returns is ours to read and delete.
            unsafe {
                let f = AMediaCodec_getOutputFormat(self.codec);
                if f.is_null() {
                    return;
                }
                let (mut rate, mut channels, mut encoding) = (0, 0, 2);
                if AMediaFormat_getInt32(f, c"sample-rate".as_ptr(), &mut rate) && rate > 0 {
                    self.rate = rate as u32;
                }
                if AMediaFormat_getInt32(f, c"channel-count".as_ptr(), &mut channels) && channels > 0 {
                    self.channels = channels as usize;
                }
                AMediaFormat_getInt32(f, c"pcm-encoding".as_ptr(), &mut encoding);
                self.float = encoding == ENCODING_PCM_FLOAT;
                AMediaFormat_delete(f);
            }
            // Once a stream, or when its shape changes: not per packet.
            nori_core::alog::info(&format!("decoder: HE-AAC through MediaCodec, {} Hz, {} channels, {}", self.rate, self.channels, if self.float { "float" } else { "16-bit" }));
        }

        /// Takes what the decoder has made into `out`, waiting up to `wait_us` for the first of it:
        /// whether anything came.
        fn take(&mut self, wait_us: i64, out: &mut Vec<f32>) -> Result<bool, Fault> {
            let mut info = BufferInfo::default();
            let mut wait = wait_us;
            let mut got = false;
            // Bounded: a format change or two, and the buffers the decoder has ready.
            for _ in 0..16 {
                // SAFETY: `info` is a live BufferInfo; the index is the codec's own, released below.
                let idx = unsafe { AMediaCodec_dequeueOutputBuffer(self.codec, &mut info, wait) };
                match idx {
                    OUTPUT_FORMAT_CHANGED => self.read_format(),
                    OUTPUT_BUFFERS_CHANGED => {}
                    TRY_AGAIN_LATER => return Ok(got),
                    i if i >= 0 => {
                        let mut cap = 0usize;
                        // SAFETY: the buffer is the codec's, `cap` bytes long, read before it is released.
                        unsafe {
                            let p = AMediaCodec_getOutputBuffer(self.codec, i as usize, &mut cap);
                            let (from, len) = (info.offset.max(0) as usize, info.size.max(0) as usize);
                            if !p.is_null() && from + len <= cap {
                                append_pcm(std::slice::from_raw_parts(p.add(from), len), self.float, out);
                            }
                            AMediaCodec_releaseOutputBuffer(self.codec, i as usize, false);
                        }
                        self.pending = self.pending.saturating_sub(1);
                        got = true;
                        // More may be ready: taken without waiting.
                        wait = 0;
                    }
                    _ => return Err(Fault::Broken),
                }
            }
            Ok(got)
        }
    }

    impl PlatformDecoder for Codec {
        fn decode(&mut self, unit: &[u8], out: &mut Vec<f32>) -> Result<(usize, u32), Fault> {
            // An input buffer: when all are taken, what the decoder made is taken until one frees.
            let mut idx = TRY_AGAIN_LATER;
            for _ in 0..4 {
                // SAFETY: a plain call on the started codec.
                idx = unsafe { AMediaCodec_dequeueInputBuffer(self.codec, 0) };
                if idx >= 0 {
                    break;
                }
                self.take(WAIT_US, out)?;
            }
            if idx < 0 {
                return Err(Fault::BadPacket);
            }
            let mut cap = 0usize;
            // SAFETY: the input buffer is the codec's, `cap` bytes long, written within it and queued.
            unsafe {
                let p = AMediaCodec_getInputBuffer(self.codec, idx as usize, &mut cap);
                let n = unit.len().min(cap);
                if !p.is_null() {
                    std::ptr::copy_nonoverlapping(unit.as_ptr(), p, n);
                }
                if AMediaCodec_queueInputBuffer(self.codec, idx as usize, 0, n, self.time_us, 0) != OK {
                    return Err(Fault::Broken);
                }
            }
            self.time_us += 1;
            self.pending += 1;
            // What it made of this packet, waited for unless the decoder is known to hold packets back.
            let wait = if self.pending > self.lag { WAIT_US } else { 0 };
            if !self.take(wait, out)? && wait > 0 {
                // Nothing within the wait: it holds this many back, and is not waited on for them again.
                self.lag = self.pending;
            }
            Ok((self.channels, self.rate))
        }

        fn reset(&mut self) {
            // SAFETY: a plain call on the started codec; the queued packets are dropped with it.
            unsafe { AMediaCodec_flush(self.codec) };
            self.pending = 0;
        }
    }

    impl Drop for Codec {
        fn drop(&mut self) {
            // SAFETY: the codec is ours and not used after this.
            unsafe {
                AMediaCodec_stop(self.codec);
                AMediaCodec_delete(self.codec);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_adts_stream_is_described_as_the_config_it_would_have() {
        assert_eq!(lc_config(22_050, 2), Some([0x13, 0x90]), "AAC-LC, 22.05 kHz, stereo");
        assert_eq!(lc_config(44_100, 2), Some([0x12, 0x10]));
        assert_eq!(lc_config(24_000, 1), Some([0x13, 0x08]));
        assert_eq!(lc_config(44_000, 2), None);
    }

    #[test]
    fn pcm_comes_out_as_floats() {
        let mut out = Vec::new();
        append_pcm(&[0x00, 0x40, 0x00, 0xc0], false, &mut out);
        append_pcm(&0.25f32.to_le_bytes(), true, &mut out);
        assert_eq!(out, [0.5, -0.5, 0.25]);
    }
}
