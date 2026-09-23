//! The streaming analyser (`nori_core::automix::store::AnalysisStream`) as the playback path feeds it:
//! one call per buffer, the samples read where they lie in a direct buffer.

use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use nori_core::automix::store::AnalysisStream;
use nori_player::decode::Fault;

use crate::decoder::{BAD_PACKET, BROKEN};
use crate::{native, region, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/playback/AutoMixAnalyzer",
    methods: &[
        native!(c"create", c"(IIJ)J", create),
        native!(c"feed", c"(JLjava/nio/ByteBuffer;III)Z", feed),
        native!(c"decode", c"(JJLjava/nio/ByteBuffer;II)I", decode),
        native!(c"frames", c"(J)J", frames),
        native!(c"reset", c"(J)V", reset),
        native!(c"destroy", c"(J)V", destroy),
    ],
};

const PCM_16: jint = 2;
const PCM_FLOAT: jint = 4;

fn stream<'a>(h: jlong) -> Option<&'a AnalysisStream> {
    // SAFETY: Kotlin passes 0 or a handle `create` made, and never one it has destroyed.
    unsafe { AnalysisStream::from_handle(h) }
}

/// `expected_ms` (0 if unknown) sizes the buffers so feeding never reallocates.
extern "system" fn create(rate: jint, channels: jint, expected_ms: jlong) -> jlong {
    AnalysisStream::new(rate.max(1) as u32, channels.max(1) as usize, expected_ms.max(0) as u64).into_handle()
}

extern "system" fn destroy(h: jlong) {
    // SAFETY: `h` came from `create` and Kotlin destroys it once.
    unsafe { AnalysisStream::free_handle(h) }
}

/// Forget everything fed so far (a seek, a new track).
extern "system" fn reset(h: jlong) {
    if let Some(s) = stream(h) {
        s.reset();
    }
}

/// Frames fed since create/reset.
extern "system" fn frames(h: jlong) -> jlong {
    stream(h).map_or(0, |s| s.frames() as jlong)
}

/// Feeds `bytes` bytes of interleaved PCM from `buffer[pos..]` (a direct buffer; read only). False when it cannot.
extern "system" fn feed(env: JNIEnv, _: JClass, h: jlong, buffer: JByteBuffer, pos: jint, bytes: jint, encoding: jint) -> jboolean {
    let Some(s) = stream(h) else { return 0 };
    if bytes <= 0 {
        return 0;
    }
    let Some(data) = region(&env, &buffer, pos, bytes) else { return 0 };
    let (src, n) = (data.as_ptr(), data.len());
    match encoding {
        PCM_16 if src as usize % 2 == 0 => {
            // SAFETY: `data` is `n` readable bytes (`region`), aligned for i16; any bit pattern is a sample.
            s.feed_i16(unsafe { std::slice::from_raw_parts(src as *const i16, n / 2) });
        }
        PCM_FLOAT if src as usize % 4 == 0 => {
            // SAFETY: as above, for f32.
            s.feed_f32(unsafe { std::slice::from_raw_parts(src as *const f32, n / 4) });
        }
        _ => return 0,
    }
    1
}

/// Decodes one packet of a track being measured ahead (`len` bytes at `pos` of the direct buffer `input`,
/// as the platform's extractor split it off) with the core's decoder `d` (`RustDecoderJni.create`) and
/// folds the samples into analyser `h`, without their crossing back. Returns the frames heard; -1 for a
/// packet to skip; -2 for a stream that cannot go on here - broken, or coming out at another rate than
/// the analyser was made for - which the platform's decoder then measures from the start instead.
extern "system" fn decode(env: JNIEnv, _: JClass, h: jlong, d: jlong, input: JByteBuffer, pos: jint, len: jint) -> jint {
    let (Some(s), Some(d)) = (stream(h), crate::decoder::handle(d)) else { return BROKEN };
    let Some(packet) = region(&env, &input, pos, len) else { return BROKEN };
    match s.feed_packet(&mut d.lock(), packet) {
        Ok(frames) => frames as jint,
        Err(Fault::BadPacket) => BAD_PACKET,
        Err(_) => BROKEN,
    }
}
