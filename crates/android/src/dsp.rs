//! The sound chain as Kotlin reaches it: raw JNI on direct ByteBuffers, called once per audio buffer
//! from the media3 AudioProcessor. The chain itself is `nori_core::dsp::SoundChain`; this only moves bytes
//! and handles across. It must not allocate, copy or serialise anything on the process path.

use jni::objects::{JByteBuffer, JClass, JFloatArray, JIntArray};
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::JNIEnv;
use std::sync::atomic::{AtomicU32, Ordering};
use nori_core::dsp::SoundChain;

use crate::{native, region_ptr, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/playback/Dsp",
    methods: &[
        native!(c"create", c"(II)J", create),
        native!(c"destroy", c"(J)V", destroy),
        native!(c"meter", c"()F", meter),
        native!(c"delayFrames", c"(J)I", delay_frames),
        native!(c"reset", c"(J)V", reset),
        native!(c"process", c"(JLjava/nio/ByteBuffer;ILjava/nio/ByteBuffer;III)Z", process),
        native!(c"effectivePreampDb", c"(ZFZ[I[F)F", effective_preamp_db),
        native!(c"soundOn", c"(ZFFZZ)Z", sound_on),
    ],
};

const PCM_16: jint = 2; // C.ENCODING_PCM_16BIT
const PCM_FLOAT: jint = 4; // C.ENCODING_PCM_FLOAT

fn chain<'a>(handle: jlong) -> Option<&'a SoundChain> {
    // SAFETY: a non-zero handle is a pointer `create` made, and Kotlin never passes one on after `destroy`.
    (handle != 0).then(|| unsafe { &*(handle as *const SoundChain) })
}

extern "system" fn create(rate: jint, channels: jint) -> jlong {
    Box::into_raw(Box::new(SoundChain::new(rate.max(1) as u32, channels.max(1) as usize))) as jlong
}

extern "system" fn destroy(handle: jlong) {
    METER.store(0, Ordering::Relaxed);
    if handle != 0 {
        // SAFETY: the handle came from `create` and Kotlin destroys it once.
        drop(unsafe { Box::from_raw(handle as *mut SoundChain) });
    }
}

/// The limiter meter, kept outside any chain: the screen polls it from the UI thread while the playback
/// thread replaces its chain on a seek, and a handle read on one thread and freed on the other was a
/// use-after-free. The chain that processed last writes it; a reset clears it.
static METER: AtomicU32 = AtomicU32::new(0);

/// Peak gain reduction in dB over the last buffer processed, 0 when the limiter is off or idle. Needs no
/// handle, so it is safe from any thread at any time.
extern "system" fn meter() -> jfloat {
    f32::from_bits(METER.load(Ordering::Relaxed))
}

/// How many frames the chain holds back; the stage drains that much at the end of a stream.
extern "system" fn delay_frames(handle: jlong) -> jint {
    chain(handle).map_or(0, |c| c.delay_frames() as jint)
}

extern "system" fn reset(handle: jlong) {
    if let Some(c) = chain(handle) {
        c.reset();
    }
    METER.store(0, Ordering::Relaxed);
}

/// Filters `bytes` bytes from `input[in_pos..]` into `output[out_pos..]`; both are direct buffers.
/// Returns false when the buffers cannot be reached, so the caller copies instead.
#[allow(clippy::too_many_arguments)]
extern "system" fn process(
    env: JNIEnv, _: JClass, handle: jlong, input: JByteBuffer, in_pos: jint, output: JByteBuffer, out_pos: jint, bytes: jint, encoding: jint,
) -> jboolean {
    let Some(c) = chain(handle) else { return 0 };
    if bytes <= 0 {
        return 0;
    }
    let (Some(src), Some(dst)) = (region_ptr(&env, &input, in_pos, bytes), region_ptr(&env, &output, out_pos, bytes)) else { return 0 };
    let n = bytes as usize;
    // The two views must not overlap: one is read while the other is written.
    if (src as usize) < (dst as usize) + n && (dst as usize) < (src as usize) + n {
        return 0;
    }
    // Android direct buffers are 8-byte aligned and positions are whole frames; stay safe anyway.
    match encoding {
        // SAFETY: both ranges lie inside their buffers (`region`), do not overlap (checked above) and are
        // aligned for i16; any bit pattern is a valid sample.
        PCM_16 if src as usize % 2 == 0 && dst as usize % 2 == 0 => unsafe {
            c.process_i16(std::slice::from_raw_parts(src as *const i16, n / 2), std::slice::from_raw_parts_mut(dst as *mut i16, n / 2));
        },
        // SAFETY: as above, for f32.
        PCM_FLOAT if src as usize % 4 == 0 && dst as usize % 4 == 0 => unsafe {
            c.process_f32(std::slice::from_raw_parts(src as *const f32, n / 4), std::slice::from_raw_parts_mut(dst as *mut f32, n / 4));
        },
        _ => return 0,
    }
    METER.store(c.gain_reduction_db().to_bits(), Ordering::Relaxed);
    1
}

/// The pre-amp in effect (`nori_core::settings::effective_preamp_db`), worked out once per settings but
/// for each step of a band's drag on the equalizer screen: the bands' kinds and gains come in as two
/// primitive arrays.
extern "system" fn effective_preamp_db(
    env: JNIEnv, _: JClass, eq_enabled: jboolean, eq_preamp_db: jfloat, automatic: jboolean, kinds: JIntArray, gains: JFloatArray,
) -> jfloat {
    let n = env.get_array_length(&kinds).unwrap_or(0).max(0) as usize;
    let (mut k, mut g) = (vec![0i32; n], vec![0f32; n]);
    if env.get_int_array_region(&kinds, 0, &mut k).is_err() || env.get_float_array_region(&gains, 0, &mut g).is_err() {
        return 0.0;
    }
    nori_core::settings::effective_preamp_db(eq_enabled != 0, (automatic == 0).then_some(eq_preamp_db), k.into_iter().zip(g))
}

/// Whether the sound chain has anything to do (see `nori_player::sound::sound_on`). Read once per
/// settings change, possibly while a screen is drawn, so primitives in and out.
extern "system" fn sound_on(eq_enabled: jboolean, crossfeed_db: jfloat, balance: jfloat, mono: jboolean, limiter: jboolean) -> jboolean {
    nori_player::sound::sound_on(eq_enabled != 0, crossfeed_db, balance, mono != 0, limiter != 0) as jboolean
}
