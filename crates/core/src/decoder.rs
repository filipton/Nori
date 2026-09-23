//! The decoder as the platform's player reaches it (`nori_player::decode`): a handle per stream, and
//! one JNI call per packet with the packet and the output as direct buffers, read and written where
//! they lie. Primitives in and out; nothing allocated on either side per packet.

use jni::objects::{JByteArray, JByteBuffer, JClass, JString};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use nori_player::decode::{Codec, Decoder, Fault};
use parking_lot::Mutex;

/// Returned for a packet that could not be decoded (skip it) and for a stream that cannot go on.
const BAD_PACKET: jint = -1;
const BROKEN: jint = -2;

fn handle<'a>(h: jlong) -> Option<&'a Mutex<Decoder>> {
    (h != 0).then(|| unsafe { &*(h as *const Mutex<Decoder>) })
}

/// Whether this decoder takes a stream of `mime` (with the RFC 6381 `codecs` string, when known), or
/// leaves it to the platform's own: what it cannot decode (Opus, HE-AAC), and anything the output
/// would rather hand to the audio chip whole - offload is cheaper still than decoding here.
pub fn takes(mime: &str, codecs: Option<&str>, channels: i32, chip_takes_it: bool) -> Option<Codec> {
    let codec = Codec::from_mime(mime)?;
    // Opus: mono and stereo; surround Opus is laid out as several streams, which is the platform's.
    if codec == Codec::Opus && !(1..=2).contains(&channels) {
        return None;
    }
    // AAC: only Low Complexity; SBR and PS (HE-AAC, HE-AACv2) are the platform's.
    if codec == Codec::Aac && !codecs.is_some_and(|c| c.eq_ignore_ascii_case("mp4a.40.2")) {
        return None;
    }
    if chip_takes_it && crate::dsp::offload_wanted() {
        return None;
    }
    Some(codec)
}

/// The codec number for `mime`, or 0 when the platform's decoder should take the stream.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_RustDecoderJni_takes(
    mut env: JNIEnv, _: JClass, mime: JString, codecs: JString, channels: jint, chip_takes_it: jboolean,
) -> jint {
    let Ok(m) = env.get_string(&mime) else { return 0 };
    let m: String = m.into();
    let c: Option<String> = if codecs.is_null() { None } else { env.get_string(&codecs).ok().map(Into::into) };
    takes(&m, c.as_deref(), channels, chip_takes_it != 0).map_or(0, Codec::id)
}

/// A decoder for one stream; 0 when it cannot be made (the platform's decoder then takes over).
/// `delay_known`: the stream states its encoder delay, which the platform trims (see `MP3_DECODER_DELAY`).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_RustDecoderJni_create(
    env: JNIEnv, _: JClass, codec: jint, rate: jint, channels: jint, extra: JByteArray, delay_known: jboolean,
) -> jlong {
    let Some(codec) = Codec::from_id(codec) else { return 0 };
    let extra = if extra.is_null() { None } else { env.convert_byte_array(&extra).ok() };
    match Decoder::new(codec, rate.max(0) as u32, channels.max(1) as usize, extra.as_deref(), delay_known != 0) {
        Ok(d) => Box::into_raw(Box::new(Mutex::new(d))) as jlong,
        Err(e) => {
            crate::alog::info(&format!("decoder: cannot open {codec:?}: {e}"));
            0
        }
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_RustDecoderJni_destroy(_: JNIEnv, _: JClass, h: jlong) {
    if h != 0 {
        drop(unsafe { Box::from_raw(h as *mut Mutex<Decoder>) });
    }
}

/// The next packet does not follow the last (a seek); `at_start`: back at the very beginning.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_RustDecoderJni_reset(_: JNIEnv, _: JClass, h: jlong, at_start: jboolean) {
    if let Some(d) = handle(h) {
        d.lock().reset(at_start != 0);
    }
}

/// The output's channels and rate, known for certain once a packet has been decoded: `channels << 32 | rate`.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_RustDecoderJni_shape(_: JNIEnv, _: JClass, h: jlong) -> jlong {
    handle(h).map_or(0, |d| {
        let d = d.lock();
        ((d.channels() as i64) << 32) | d.rate() as i64
    })
}

/// The most bytes one packet of `codec` decodes to, for sizing the output buffer once.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_RustDecoderJni_maxBytes(_: JNIEnv, _: JClass, codec: jint, channels: jint, float: jboolean) -> jint {
    let Some(c) = Codec::from_id(codec) else { return 0 };
    (c.max_frames() * channels.max(1) as usize * if float != 0 { 4 } else { 2 }) as jint
}

/// Reads a direct buffer's memory: `len` bytes at `pos`, checked against its capacity.
fn region<'a>(env: &JNIEnv, buf: &JByteBuffer, pos: jint, len: jint) -> Option<&'a mut [u8]> {
    let addr = env.get_direct_buffer_address(buf).ok()?;
    let cap = env.get_direct_buffer_capacity(buf).ok()?;
    let (pos, len) = (usize::try_from(pos).ok()?, usize::try_from(len).ok()?);
    if addr.is_null() || pos.checked_add(len)? > cap {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts_mut(addr.add(pos), len) })
}

/// Writes decoded samples into `out`: 16-bit integers, or 32-bit floats when `float`.
fn write(d: &mut Decoder, out: &mut [u8], float: bool, packet: Option<&[u8]>) -> jint {
    let r = if float {
        if out.as_ptr() as usize % 4 != 0 {
            return BROKEN;
        }
        let s = unsafe { std::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut f32, out.len() / 4) };
        match packet {
            Some(p) => d.decode_f32(p, s),
            None => d.take_f32(s),
        }
    } else {
        if out.as_ptr() as usize % 2 != 0 {
            return BROKEN;
        }
        let s = unsafe { std::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut i16, out.len() / 2) };
        match packet {
            Some(p) => d.decode_i16(p, s),
            None => d.take_i16(s),
        }
    };
    let bytes = if float { 4 } else { 2 };
    match r {
        Ok(frames) => (frames * d.channels() * bytes) as jint,
        Err(Fault::BadPacket) => BAD_PACKET,
        Err(Fault::Broken) => BROKEN,
        // Room for this many more bytes is needed; the samples are kept for `take`.
        Err(Fault::NeedRoom(samples)) => -((samples * bytes) as jint) - 2,
    }
}

/// Decodes `in_len` bytes at `in_pos` of `input` into `output` (from `out_pos`, at most `out_cap`
/// bytes). Returns the bytes written; [`BAD_PACKET`] for a packet to skip; [`BROKEN`] for a stream that
/// cannot go on; below that, `-(bytes needed) - 2`: grow the output and call `take`.
#[no_mangle]
#[allow(clippy::too_many_arguments)]
pub extern "system" fn Java_dev_nori_music_playback_RustDecoderJni_decode(
    env: JNIEnv, _: JClass, h: jlong, input: JByteBuffer, in_pos: jint, in_len: jint, output: JByteBuffer, out_pos: jint, out_cap: jint,
    float: jboolean,
) -> jint {
    let Some(d) = handle(h) else { return BROKEN };
    let (Some(packet), Some(out)) = (region(&env, &input, in_pos, in_len), region(&env, &output, out_pos, out_cap)) else { return BROKEN };
    write(&mut d.lock(), out, float != 0, Some(packet))
}

/// The last packet's samples, after the output was grown for them.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_RustDecoderJni_take(
    env: JNIEnv, _: JClass, h: jlong, output: JByteBuffer, out_pos: jint, out_cap: jint, float: jboolean,
) -> jint {
    let Some(d) = handle(h) else { return BROKEN };
    let Some(out) = region(&env, &output, out_pos, out_cap) else { return BROKEN };
    write(&mut d.lock(), out, float != 0, None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_it_decodes_and_not_what_the_chip_would_take() {
        assert_eq!(takes("audio/mpeg", None, 2, false), Some(Codec::Mp3));
        assert_eq!(takes("audio/opus", None, 2, false), Some(Codec::Opus));
        assert_eq!(takes("audio/opus", None, 6, false), None, "surround Opus is the platform's");
        assert_eq!(takes("audio/ac3", None, 2, false), None);
        assert_eq!(takes("audio/mp4a-latm", Some("mp4a.40.2"), 2, false), Some(Codec::Aac));
        assert_eq!(takes("audio/mp4a-latm", Some("mp4a.40.5"), 2, false), None, "HE-AAC is the platform's");
        assert_eq!(takes("audio/mp4a-latm", None, 2, false), None, "an AAC of unknown profile too");
    }
}
