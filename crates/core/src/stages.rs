//! The chain stages that change how long the audio is - speed/pitch and silence skipping - as Kotlin
//! reaches them. The work is `nori_player::speed` and `nori_player::silence`; this only moves bytes and
//! handles across. What a stage produces stays in its own memory and Kotlin hands that memory on as it
//! is, through one direct buffer made over it: one call per buffer, no copy, and no allocation on either
//! side unless a buffer arrives bigger than any before it.

use jni::objects::{JByteBuffer, JClass, JObject};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use nori_player::pcm::Encoding;
use nori_player::silence::SilenceSkipper;
use nori_player::speed::SpeedPitch;
use parking_lot::Mutex;

const PCM_FLOAT: jint = 4; // C.ENCODING_PCM_FLOAT

enum Kind {
    Speed(SpeedPitch),
    Silence(SilenceSkipper),
}

struct Stage {
    kind: Kind,
    out: Vec<u8>,
    /// Where the Java view over `out` points: when `out` has to move, Kotlin is told to make a new one.
    viewed: (usize, usize),
}

/// Room made up front, so the output does not have to move for any ordinary buffer.
const OUT_RESERVE: usize = 256 * 1024;
/// Set in a returned length when the output moved and the view over it must be made again.
const MOVED: jint = 1 << 30;

fn stage<'a>(handle: jlong) -> Option<&'a Mutex<Stage>> {
    (handle != 0).then(|| unsafe { &*(handle as *const Mutex<Stage>) })
}

fn boxed(kind: Kind) -> jlong {
    Box::into_raw(Box::new(Mutex::new(Stage { kind, out: Vec::with_capacity(OUT_RESERVE), viewed: (0, 0) }))) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_speed(_: JNIEnv, _: JClass, rate: jint, channels: jint, encoding: jint, speed: f32, pitch: f32) -> jlong {
    let enc = if encoding == PCM_FLOAT { Encoding::Float } else { Encoding::Pcm16 };
    let mut s = SpeedPitch::new(rate.max(1) as u32, channels.max(1) as usize, enc);
    s.set(speed, pitch);
    s.flush();
    boxed(Kind::Speed(s))
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_silence(_: JNIEnv, _: JClass, rate: jint, channels: jint) -> jlong {
    boxed(Kind::Silence(SilenceSkipper::new(rate.max(1) as u32, channels.max(1) as usize)))
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_destroy(_: JNIEnv, _: JClass, handle: jlong) {
    if handle != 0 {
        drop(unsafe { Box::from_raw(handle as *mut Mutex<Stage>) });
    }
}

/// A new stream: what is held inside is dropped.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_flush(_: JNIEnv, _: JClass, handle: jlong) {
    if let Some(s) = stage(handle) {
        let mut s = s.lock();
        s.out.clear();
        match &mut s.kind {
            Kind::Speed(p) => p.flush(),
            Kind::Silence(k) => k.flush(),
        }
    }
}

fn answer(s: &Stage) -> jint {
    let moved = (s.out.as_ptr() as usize, s.out.capacity()) != s.viewed;
    s.out.len() as jint | if moved { MOVED } else { 0 }
}

/// Takes `bytes` bytes from the direct buffer `input` at `pos`. What was produced before is dropped
/// first unless `keep` (Kotlin has not handed it on yet). Returns the output's length, with [`MOVED`]
/// set when the view over it must be made again (see `output`); -1 when the input cannot be reached.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_process(env: JNIEnv, _: JClass, handle: jlong, input: JByteBuffer, pos: jint, bytes: jint, keep: jboolean) -> jint {
    let Some(s) = stage(handle) else { return -1 };
    let Ok(src) = env.get_direct_buffer_address(&input) else { return -1 };
    if src.is_null() || bytes < 0 {
        return -1;
    }
    let data = unsafe { std::slice::from_raw_parts(src.add(pos as usize), bytes as usize) };
    let mut s = s.lock();
    let Stage { kind, out, .. } = &mut *s;
    if keep == 0 {
        out.clear();
    }
    match kind {
        Kind::Speed(p) => p.process(data, out),
        Kind::Silence(k) => k.process(data, out),
    }
    answer(&s)
}

/// The input has ended: whatever is still inside comes out. Returns as `process` does.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_end(_: JNIEnv, _: JClass, handle: jlong, keep: jboolean) -> jint {
    let Some(s) = stage(handle) else { return 0 };
    let mut s = s.lock();
    let Stage { kind, out, .. } = &mut *s;
    if keep == 0 {
        out.clear();
    }
    match kind {
        Kind::Speed(p) => p.end_of_stream(out),
        Kind::Silence(k) => k.end_of_stream(out),
    }
    answer(&s)
}

/// A direct buffer over the stage's output memory (all of it; Kotlin sets the limit to the length).
/// Valid until a returned length carries [`MOVED`], or the stage is destroyed.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_output<'e>(mut env: JNIEnv<'e>, _: JClass, handle: jlong) -> JObject<'e> {
    let Some(s) = stage(handle) else { return JObject::null() };
    let mut s = s.lock();
    let (p, cap) = (s.out.as_mut_ptr(), s.out.capacity());
    s.viewed = (p as usize, cap);
    unsafe { env.new_direct_byte_buffer(p, cap) }.map(JObject::from).unwrap_or(JObject::null())
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_mediaDurationUs(_: JNIEnv, _: JClass, handle: jlong, playout_us: jlong) -> jlong {
    match stage(handle).map(|s| s.lock()) {
        Some(s) => match &s.kind {
            Kind::Speed(p) => p.media_duration_us(playout_us),
            Kind::Silence(_) => playout_us,
        },
        None => playout_us,
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_playoutDurationUs(_: JNIEnv, _: JClass, handle: jlong, media_us: jlong) -> jlong {
    match stage(handle).map(|s| s.lock()) {
        Some(s) => match &s.kind {
            Kind::Speed(p) => p.playout_duration_us(media_us),
            Kind::Silence(_) => media_us,
        },
        None => media_us,
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Stages_skippedFrames(_: JNIEnv, _: JClass, handle: jlong) -> jlong {
    match stage(handle).map(|s| s.lock()) {
        Some(s) => match &s.kind {
            Kind::Silence(k) => k.skipped_frames() as jlong,
            Kind::Speed(_) => 0,
        },
        None => 0,
    }
}
