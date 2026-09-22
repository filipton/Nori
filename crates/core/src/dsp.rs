//! The sound chain as Kotlin reaches it: raw JNI on direct ByteBuffers, called once per audio buffer
//! from the media3 AudioProcessor. The chain itself is `nori_player::dsp`; this only moves bytes and
//! handles across. It must not allocate, copy or serialise anything on the process path.

use std::sync::atomic::{AtomicU32, Ordering};

use jni::objects::{JByteBuffer, JClass, JFloatArray, JIntArray};
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::JNIEnv;
use parking_lot::Mutex;

pub use nori_player::dsp::*;

const PCM_16: jint = 2; // C.ENCODING_PCM_16BIT
const PCM_FLOAT: jint = 4; // C.ENCODING_PCM_FLOAT

/// The meter lives outside the lock so the UI can poll it without ever waiting on the playback thread.
struct Handle {
    eq: Mutex<Equalizer>,
    reduction_db: AtomicU32,
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Dsp_create(_: JNIEnv, _: JClass, rate: jint, channels: jint) -> jlong {
    let eq = Mutex::new(Equalizer::new(rate.max(1) as u32, channels.max(1) as usize));
    Box::into_raw(Box::new(Handle { eq, reduction_db: AtomicU32::new(0) })) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Dsp_destroy(_: JNIEnv, _: JClass, handle: jlong) {
    if handle != 0 {
        drop(unsafe { Box::from_raw(handle as *mut Handle) });
    }
}

/// `bands` is flat: kind, frequency, gain dB, Q (slope S for the slope shelves), channel for each band.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Dsp_configure(env: JNIEnv, _: JClass, handle: jlong, bands: JFloatArray, preamp_db: jfloat, crossfeed_db: jfloat) {
    if handle == 0 {
        return;
    }
    let n = env.get_array_length(&bands).unwrap_or(0).clamp(0, 5 * 64) as usize;
    let mut flat = vec![0f32; n];
    if env.get_float_array_region(&bands, 0, &mut flat).is_err() {
        return;
    }
    let bands: Vec<Band> =
        flat.chunks_exact(5).map(|b| Band { kind: b[0] as i32, freq: b[1] as f64, gain_db: b[2] as f64, q: b[3] as f64, channel: b[4] as i32 }).collect();
    unsafe { &*(handle as *const Handle) }.eq.lock().configure(&bands, preamp_db as f64, crossfeed_db as f64);
}

/// `balance` is -1 (hard left) to 1 (hard right); `lookahead_ms` at or below 0 turns the limiter off.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Dsp_configureOutput(
    _: JNIEnv, _: JClass, handle: jlong, balance: jfloat, mono: jboolean, threshold_db: jfloat, release_ms: jfloat, lookahead_ms: jfloat,
) {
    if handle != 0 {
        unsafe { &*(handle as *const Handle) }.eq.lock().configure_output(balance as f64, mono != 0, threshold_db as f64, release_ms as f64, lookahead_ms as f64);
    }
}

/// The limiter meter: peak gain reduction in dB in the last buffer, 0 when it is off or idle. Lock-free, poll freely.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Dsp_gainReductionDb(_: JNIEnv, _: JClass, handle: jlong) -> jfloat {
    if handle == 0 {
        return 0.0;
    }
    f32::from_bits(unsafe { &*(handle as *const Handle) }.reduction_db.load(Ordering::Relaxed))
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Dsp_reset(_: JNIEnv, _: JClass, handle: jlong) {
    if handle != 0 {
        unsafe { &*(handle as *const Handle) }.eq.lock().reset();
    }
}

/// Filters `bytes` bytes from `input[in_pos..]` into `output[out_pos..]`; both are direct buffers.
/// Returns false when the buffers cannot be reached, so the caller copies instead.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Dsp_process(
    env: JNIEnv, _: JClass, handle: jlong, input: JByteBuffer, in_pos: jint, output: JByteBuffer, out_pos: jint, bytes: jint, encoding: jint,
) -> bool {
    let (Ok(src), Ok(dst)) = (env.get_direct_buffer_address(&input), env.get_direct_buffer_address(&output)) else { return false };
    if handle == 0 || src.is_null() || dst.is_null() || bytes <= 0 {
        return false;
    }
    let h = unsafe { &*(handle as *const Handle) };
    let mut eq = h.eq.lock();
    let (src, dst, bytes) = unsafe { (src.add(in_pos as usize), dst.add(out_pos as usize), bytes as usize) };
    // Android direct buffers are 8-byte aligned and positions are whole frames; stay safe anyway.
    match encoding {
        PCM_16 if src as usize % 2 == 0 && dst as usize % 2 == 0 => unsafe {
            eq.process_i16(std::slice::from_raw_parts(src as *const i16, bytes / 2), std::slice::from_raw_parts_mut(dst as *mut i16, bytes / 2));
        },
        PCM_FLOAT if src as usize % 4 == 0 && dst as usize % 4 == 0 => unsafe {
            eq.process_f32(std::slice::from_raw_parts(src as *const f32, bytes / 4), std::slice::from_raw_parts_mut(dst as *mut f32, bytes / 4));
        },
        _ => return false,
    }
    h.reduction_db.store(eq.gain_reduction_db().to_bits(), Ordering::Relaxed);
    true
}


/// The built-in curves, as data, so the UI (and the settings store) never holds a frequency of its own.
#[uniffi::export]
pub fn eq_presets() -> Vec<crate::NamedPreset> {
    nori_player::dsp::eq_presets()
}

/// Which parts of the chain may run, from the settings and the output; see `nori_player::policy`.
#[uniffi::export]
pub fn audio_policy(prefs: crate::AudioPrefs, output: crate::OutputState) -> crate::AudioPolicy {
    nori_player::policy::audio_policy(&prefs, &output)
}

/// The volume a track plays at under ReplayGain, 0..1; see `nori_player::policy::replay_gain`.
#[uniffi::export]
pub fn replay_gain_volume(
    mode: crate::GainMode, tags: Option<crate::GainTags>, in_album_run: bool, preamp_db: f32, untagged_db: f32, radio: bool, bit_perfect: bool,
) -> f32 {
    nori_player::policy::replay_gain(mode, tags.as_ref(), in_album_run, preamp_db, untagged_db, radio, bit_perfect)
}

/// Where a volume fade from `from` to `to` stands at `t` (0..1) of its length. Asked on every tick of
/// a fade, so it is plain JNI: primitives in and out, nothing boxed.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Dsp_fadeVolume(_: JNIEnv, _: JClass, from: jfloat, to: jfloat, t: jfloat) -> jfloat {
    nori_player::policy::fade(from, to, t)
}

/// The dip around a switch made while music plays; `None`: switch at once. See `nori_player::transport`.
#[uniffi::export]
pub fn switch_dip(fade_ms: i32, switch: crate::Switch, playing: bool) -> Option<crate::Dip> {
    nori_player::transport::switch_dip(fade_ms, switch, playing)
}

/// Pressing play: fade in over this long; `None` to start at full volume.
#[uniffi::export]
pub fn play_fade(fade_ms: i32, playing: bool) -> Option<i32> {
    nori_player::transport::play_fade(fade_ms, playing)
}

/// Pressing pause: fade out over this long first; `None` to pause at once.
#[uniffi::export]
pub fn pause_fade(fade_ms: i32, playing: bool) -> Option<i32> {
    nori_player::transport::pause_fade(fade_ms, playing)
}

/// Whether a settings change must rebuild the output, and when.
#[uniffi::export]
pub fn sink_rebuild(change: crate::ChainChange) -> crate::Rebuild {
    nori_player::transport::rebuild(change)
}

/// What an output device gets as music moves to it; see `nori_player::device`.
#[uniffi::export]
pub fn device_arrival(arrival: crate::Arrival) -> crate::ArrivalPlan {
    nori_player::device::on_arrival(arrival)
}

/// How deep the output buffer is made so it can be fed in bursts; see `nori_player::burst`.
#[uniffi::export]
pub fn burst_buffer_us() -> i64 {
    nori_player::burst::BUFFER_US
}

/// Which of a DAC's modes plays a song untouched, or why none can; see `nori_player::dac`.
#[uniffi::export]
pub fn dac_choice(enabled: bool, modes: Vec<crate::DacMode>, playing: crate::DacMode) -> crate::DacChoice {
    nori_player::dac::choose(enabled, &modes, playing)
}

/// The automatic pre-amp for bands given as parallel arrays of kinds and gains; see
/// `nori_player::dsp::auto_preamp_db`.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_Dsp_autoPreampDb(env: JNIEnv, _: JClass, kinds: JIntArray, gains: JFloatArray) -> jfloat {
    let n = env.get_array_length(&kinds).unwrap_or(0).clamp(0, 64) as usize;
    let (mut k, mut g) = ([0i32; 64], [0f32; 64]);
    if env.get_int_array_region(&kinds, 0, &mut k[..n]).is_err() || env.get_float_array_region(&gains, 0, &mut g[..n]).is_err() {
        return 0.0;
    }
    nori_player::dsp::auto_preamp_db(k[..n].iter().copied().zip(g[..n].iter().copied()))
}
