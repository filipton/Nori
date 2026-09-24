//! The decoder as the platform's player reaches it (`nori_player::decode`): a handle per stream, and
//! one JNI call per packet with the packet and the output as direct buffers, read and written where
//! they lie. Primitives in and out; nothing allocated on either side per packet.

use jni::objects::{JByteArray, JByteBuffer, JClass, JString};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use nori_player::decode::{Codec, Decoder, Fault};
use parking_lot::Mutex;

use crate::{native, region, string, with_str, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/playback/RustDecoderJni",
    methods: &[
        native!(c"takes", c"(Ljava/lang/String;Ljava/lang/String;IZ)I", takes),
        native!(c"takesExtracted", c"(Ljava/lang/String;Ljava/lang/String;I)I", takes_extracted),
        native!(c"create", c"(III[BZ)J", create),
        native!(c"destroy", c"(J)V", destroy),
        native!(c"reset", c"(JZ)V", reset),
        native!(c"shape", c"(J)J", shape),
        native!(c"maxBytes", c"(IIZ)I", max_bytes),
        native!(c"decode", c"(JLjava/nio/ByteBuffer;IILjava/nio/ByteBuffer;IIZ)I", decode),
        native!(c"take", c"(JLjava/nio/ByteBuffer;IIZ)I", take),
    ],
};

/// Returned for a packet that could not be decoded (skip it) and for a stream that cannot go on.
pub(crate) const BAD_PACKET: jint = -1;
pub(crate) const BROKEN: jint = -2;

pub(crate) fn handle<'a>(h: jlong) -> Option<&'a Mutex<Decoder>> {
    // SAFETY: a non-zero `h` is a pointer `create` made, and Kotlin never passes one on after `destroy`.
    (h != 0).then(|| unsafe { &*(h as *const Mutex<Decoder>) })
}

/// [`nori_core::decoder::takes_extracted`] as a codec number, 0 for the platform's decoder.
extern "system" fn takes_extracted(env: JNIEnv, _: JClass, mime: JString, codecs: JString, channels: jint) -> jint {
    let c = string(&env, &codecs);
    with_str(&env, &mime, |m| nori_core::decoder::takes_extracted(m, c.as_deref(), channels).map_or(0, Codec::id)).unwrap_or(0)
}

/// The codec number for `mime`, or 0 when the platform's decoder should take the stream.
extern "system" fn takes(env: JNIEnv, _: JClass, mime: JString, codecs: JString, channels: jint, chip_takes_it: jboolean) -> jint {
    let c = string(&env, &codecs);
    with_str(&env, &mime, |m| nori_core::decoder::takes(m, c.as_deref(), channels, chip_takes_it != 0).map_or(0, Codec::id)).unwrap_or(0)
}

/// A decoder for one stream; 0 when it cannot be made (the platform's decoder then takes over).
/// `delay_known`: the stream states its encoder delay, which the platform trims (see `MP3_DECODER_DELAY`).
extern "system" fn create(env: JNIEnv, _: JClass, codec: jint, rate: jint, channels: jint, extra: JByteArray, delay_known: jboolean) -> jlong {
    let Some(codec) = Codec::from_id(codec) else { return 0 };
    let extra = if extra.is_null() { None } else { env.convert_byte_array(&extra).ok() };
    match Decoder::new(codec, rate.max(0) as u32, channels.max(1) as usize, extra.as_deref(), delay_known != 0) {
        Ok(d) => Box::into_raw(Box::new(Mutex::new(d))) as jlong,
        Err(e) => {
            nori_core::alog::info(&format!("decoder: cannot open {codec:?}: {e}"));
            0
        }
    }
}

extern "system" fn destroy(h: jlong) {
    if h != 0 {
        // SAFETY: `h` came from `create` and Kotlin destroys it once.
        drop(unsafe { Box::from_raw(h as *mut Mutex<Decoder>) });
    }
}

/// The next packet does not follow the last (a seek); `at_start`: back at the very beginning.
extern "system" fn reset(h: jlong, at_start: jboolean) {
    if let Some(d) = handle(h) {
        d.lock().reset(at_start != 0);
    }
}

/// The output's channels and rate, known for certain once a packet has been decoded: `channels << 32 | rate`.
extern "system" fn shape(h: jlong) -> jlong {
    handle(h).map_or(0, |d| {
        let d = d.lock();
        ((d.channels() as i64) << 32) | d.rate() as i64
    })
}

/// The most bytes one packet of `codec` decodes to, for sizing the output buffer once.
extern "system" fn max_bytes(codec: jint, channels: jint, float: jboolean) -> jint {
    let Some(c) = Codec::from_id(codec) else { return 0 };
    (c.max_frames() * channels.max(1) as usize * if float != 0 { 4 } else { 2 }) as jint
}

/// Writes decoded samples into `out`: 16-bit integers, or 32-bit floats when `float`.
fn write(d: &mut Decoder, out: &mut [u8], float: bool, packet: Option<&[u8]>) -> jint {
    let r = if float {
        if out.as_ptr() as usize % 4 != 0 {
            return BROKEN;
        }
        // SAFETY: `out` is aligned for f32 (checked above) and holds `len / 4` whole floats; any bit
        // pattern is a valid f32.
        let s = unsafe { std::slice::from_raw_parts_mut(out.as_mut_ptr() as *mut f32, out.len() / 4) };
        match packet {
            Some(p) => d.decode_f32(p, s),
            None => d.take_f32(s),
        }
    } else {
        if out.as_ptr() as usize % 2 != 0 {
            return BROKEN;
        }
        // SAFETY: as above, for i16.
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
#[allow(clippy::too_many_arguments)]
extern "system" fn decode(
    env: JNIEnv, _: JClass, h: jlong, input: JByteBuffer, in_pos: jint, in_len: jint, output: JByteBuffer, out_pos: jint, out_cap: jint, float: jboolean,
) -> jint {
    let Some(d) = handle(h) else { return BROKEN };
    let (Some(packet), Some(out)) = (region(&env, &input, in_pos, in_len), region(&env, &output, out_pos, out_cap)) else { return BROKEN };
    write(&mut d.lock(), out, float != 0, Some(packet))
}

/// The last packet's samples, after the output was grown for them.
extern "system" fn take(env: JNIEnv, _: JClass, h: jlong, output: JByteBuffer, out_pos: jint, out_cap: jint, float: jboolean) -> jint {
    let Some(d) = handle(h) else { return BROKEN };
    let Some(out) = region(&env, &output, out_pos, out_cap) else { return BROKEN };
    write(&mut d.lock(), out, float != 0, None)
}
