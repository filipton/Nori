//! The chain stages that change how long the audio is - speed/pitch and silence skipping - as Kotlin
//! reaches them. The work is `nori_player::speed` and `nori_player::silence`; this only moves bytes and
//! handles across. What a stage produces stays in its own memory and Kotlin hands that memory on as it
//! is, through one direct buffer made over it: one call per buffer, no copy, and no allocation on either
//! side unless a buffer arrives bigger than any before it.

use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jboolean, jint, jlong, jobject};
use jni::JNIEnv;
use nori_player::pcm::Encoding;
use nori_player::silence::SilenceSkipper;
use nori_player::speed::{nominal_media_us, nominal_playout_us, speed_active, SpeedPitch};
use parking_lot::Mutex;

use crate::{native, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/playback/Stages",
    methods: &[
        native!(c"speed", c"(IIIFF)J", speed),
        native!(c"silence", c"(II)J", silence),
        native!(c"destroy", c"(J)V", destroy),
        native!(c"flush", c"(J)V", flush),
        native!(c"process", c"(JLjava/nio/ByteBuffer;IIZ)I", process),
        native!(c"end", c"(JZ)I", end),
        native!(c"output", c"(J)Ljava/nio/ByteBuffer;", output),
        native!(c"mediaDurationUs", c"(JFJ)J", media_duration_us),
        native!(c"playoutDurationUs", c"(JFJ)J", playout_duration_us),
        native!(c"speedActive", c"(FF)Z", speed_active_door),
        native!(c"fadeStep", c"(FFJJI)J", fade_step),
        native!(c"skippedFrames", c"(J)J", skipped_frames),
    ],
};

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
    // SAFETY: a non-zero handle is a pointer `boxed` made, and Kotlin never passes one on after `destroy`.
    (handle != 0).then(|| unsafe { &*(handle as *const Mutex<Stage>) })
}

fn boxed(kind: Kind) -> jlong {
    Box::into_raw(Box::new(Mutex::new(Stage { kind, out: Vec::with_capacity(OUT_RESERVE), viewed: (0, 0) }))) as jlong
}

extern "system" fn speed(rate: jint, channels: jint, encoding: jint, speed: f32, pitch: f32) -> jlong {
    let enc = if encoding == PCM_FLOAT { Encoding::Float } else { Encoding::Pcm16 };
    let mut s = SpeedPitch::new(rate.max(1) as u32, channels.max(1) as usize, enc);
    s.set(speed, pitch);
    s.flush();
    boxed(Kind::Speed(s))
}

extern "system" fn silence(rate: jint, channels: jint) -> jlong {
    boxed(Kind::Silence(SilenceSkipper::new(rate.max(1) as u32, channels.max(1) as usize)))
}

extern "system" fn destroy(handle: jlong) {
    if handle != 0 {
        // SAFETY: the handle came from `boxed` and Kotlin destroys it once.
        drop(unsafe { Box::from_raw(handle as *mut Mutex<Stage>) });
    }
}

/// A new stream: what is held inside is dropped.
extern "system" fn flush(handle: jlong) {
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
extern "system" fn process(env: JNIEnv, _: JClass, handle: jlong, input: JByteBuffer, pos: jint, bytes: jint, keep: jboolean) -> jint {
    let Some(s) = stage(handle) else { return -1 };
    let Some(data) = crate::region(&env, &input, pos, bytes) else { return -1 };
    let data: &[u8] = data;
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
extern "system" fn end(handle: jlong, keep: jboolean) -> jint {
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
extern "system" fn output(mut env: JNIEnv, _: JClass, handle: jlong) -> jobject {
    let Some(s) = stage(handle) else { return std::ptr::null_mut() };
    let mut s = s.lock();
    let (p, cap) = (s.out.as_mut_ptr(), s.out.capacity());
    s.viewed = (p as usize, cap);
    // SAFETY: `out` keeps its `cap` bytes where they are until it grows, which the next `process` or `end`
    // reports with [`MOVED`] so Kotlin makes a new view; the stage outlives every view (it is destroyed last).
    unsafe { env.new_direct_byte_buffer(p, cap) }.map_or(std::ptr::null_mut(), |b| b.into_raw())
}

/// The media time `playout_us` of output stands for: the stage's own books, or, with no stage made yet
/// (`handle` 0), the nominal `speed`. Asked whenever the player works out its position.
extern "system" fn media_duration_us(handle: jlong, speed: f32, playout_us: jlong) -> jlong {
    match stage(handle).map(|s| s.lock()) {
        Some(s) => match &s.kind {
            Kind::Speed(p) => p.media_duration_us(playout_us),
            Kind::Silence(_) => playout_us,
        },
        None => nominal_media_us(speed, playout_us),
    }
}

/// How long `media_us` of the song plays for; see [`media_duration_us`].
extern "system" fn playout_duration_us(handle: jlong, speed: f32, media_us: jlong) -> jlong {
    match stage(handle).map(|s| s.lock()) {
        Some(s) => match &s.kind {
            Kind::Speed(p) => p.playout_duration_us(media_us),
            Kind::Silence(_) => media_us,
        },
        None => nominal_playout_us(speed, media_us),
    }
}

/// Whether speed and pitch change the sound at all, so the stage joins the chain (`nori_player::speed`).
extern "system" fn speed_active_door(speed: f32, pitch: f32) -> jboolean {
    speed_active(speed, pitch) as jboolean
}

/// One tick of a volume fade (`nori_player::transport::fade_step`): the volume's bits in the low 32, and
/// bit 32 set when the fade is over. Asked every 16 ms while a fade runs, so primitives only.
extern "system" fn fade_step(from: f32, to: f32, start_ms: jlong, now_ms: jlong, ms: jint) -> jlong {
    let (v, done) = nori_player::transport::fade_step(from, to, start_ms, now_ms, ms);
    (v.to_bits() as jlong) | ((done as jlong) << 32)
}

extern "system" fn skipped_frames(handle: jlong) -> jlong {
    match stage(handle).map(|s| s.lock()) {
        Some(s) => match &s.kind {
            Kind::Silence(k) => k.skipped_frames() as jlong,
            Kind::Speed(_) => 0,
        },
        None => 0,
    }
}
