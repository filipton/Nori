//! Ten-band equalizer, called once per audio buffer from the media3
//! AudioProcessor. This is raw JNI on direct ByteBuffers rather than uniffi:
//! it runs on the playback thread every few milliseconds and must not
//! allocate, copy or serialise anything.

use jni::objects::{JByteBuffer, JClass, JFloatArray};
use jni::sys::{jint, jlong};
use jni::JNIEnv;
use parking_lot::Mutex;

pub const BANDS: usize = 10;
pub const FREQUENCIES: [f64; BANDS] = [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0];
const Q: f64 = 1.41;
const MAX_CHANNELS: usize = 8;

const PCM_16: jint = 2; // C.ENCODING_PCM_16BIT
const PCM_FLOAT: jint = 4; // C.ENCODING_PCM_FLOAT

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    /// RBJ peaking filter.
    fn peaking(rate: f64, freq: f64, gain_db: f64) -> Self {
        let a = 10f64.powf(gain_db / 40.0);
        let w = 2.0 * std::f64::consts::PI * freq / rate;
        let alpha = w.sin() / (2.0 * Q);
        let a0 = 1.0 + alpha / a;
        Biquad { b0: (1.0 + alpha * a) / a0, b1: -2.0 * w.cos() / a0, b2: (1.0 - alpha * a) / a0, a1: -2.0 * w.cos() / a0, a2: (1.0 - alpha / a) / a0 }
    }
}

pub struct Equalizer {
    rate: f64,
    channels: usize,
    /// Only the bands that do something, so a flat band costs nothing.
    filters: Vec<Biquad>,
    /// Transposed direct form II state, per filter per channel.
    state: Vec<[[f64; 2]; MAX_CHANNELS]>,
    /// Pulls the signal down by the largest boost so the curve cannot clip.
    preamp: f64,
}

impl Equalizer {
    pub fn new(rate: u32, channels: usize) -> Self {
        Equalizer { rate: rate as f64, channels: channels.clamp(1, MAX_CHANNELS), filters: Vec::new(), state: Vec::new(), preamp: 1.0 }
    }

    pub fn set_gains(&mut self, gains: &[f32]) {
        self.filters.clear();
        let mut boost = 0f64;
        for (i, &g) in gains.iter().take(BANDS).enumerate() {
            let g = (g as f64).clamp(-15.0, 15.0);
            if g.abs() >= 0.05 && FREQUENCIES[i] < self.rate / 2.0 {
                self.filters.push(Biquad::peaking(self.rate, FREQUENCIES[i], g));
                boost = boost.max(g);
            }
        }
        self.state.resize(self.filters.len(), [[0.0; 2]; MAX_CHANNELS]);
        self.preamp = 10f64.powf(-boost / 20.0);
    }

    #[inline]
    fn sample(&mut self, ch: usize, x: f64) -> f64 {
        let mut x = x * self.preamp;
        for (f, st) in self.filters.iter().zip(self.state.iter_mut()) {
            let s = &mut st[ch];
            let y = f.b0 * x + s[0];
            s[0] = f.b1 * x - f.a1 * y + s[1];
            s[1] = f.b2 * x - f.a2 * y;
            x = y;
        }
        x
    }

    pub fn process_i16(&mut self, input: &[i16], output: &mut [i16]) {
        let n = self.channels;
        for (i, (x, y)) in input.iter().zip(output.iter_mut()).enumerate() {
            *y = self.sample(i % n, *x as f64).round().clamp(-32768.0, 32767.0) as i16;
        }
    }

    pub fn process_f32(&mut self, input: &[f32], output: &mut [f32]) {
        let n = self.channels;
        for (i, (x, y)) in input.iter().zip(output.iter_mut()).enumerate() {
            *y = self.sample(i % n, *x as f64) as f32;
        }
    }

    pub fn reset(&mut self) {
        self.state.iter_mut().for_each(|s| *s = [[0.0; 2]; MAX_CHANNELS]);
    }
}

type Handle = Mutex<Equalizer>;

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_create(_: JNIEnv, _: JClass, rate: jint, channels: jint) -> jlong {
    Box::into_raw(Box::new(Mutex::new(Equalizer::new(rate.max(1) as u32, channels.max(1) as usize)))) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_destroy(_: JNIEnv, _: JClass, handle: jlong) {
    if handle != 0 {
        drop(unsafe { Box::from_raw(handle as *mut Handle) });
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_setGains(env: JNIEnv, _: JClass, handle: jlong, gains: JFloatArray) {
    let mut g = [0f32; BANDS];
    let n = env.get_array_length(&gains).unwrap_or(0).clamp(0, BANDS as i32) as usize;
    if handle != 0 && env.get_float_array_region(&gains, 0, &mut g[..n]).is_ok() {
        unsafe { &*(handle as *const Handle) }.lock().set_gains(&g[..n]);
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_reset(_: JNIEnv, _: JClass, handle: jlong) {
    if handle != 0 {
        unsafe { &*(handle as *const Handle) }.lock().reset();
    }
}

/// Filters `bytes` bytes from `input[in_pos..]` into `output[out_pos..]`; both are direct buffers.
/// Returns false when the buffers cannot be reached, so the caller copies instead.
#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_process(
    env: JNIEnv, _: JClass, handle: jlong, input: JByteBuffer, in_pos: jint, output: JByteBuffer, out_pos: jint, bytes: jint, encoding: jint,
) -> bool {
    let (Ok(src), Ok(dst)) = (env.get_direct_buffer_address(&input), env.get_direct_buffer_address(&output)) else { return false };
    if handle == 0 || src.is_null() || dst.is_null() || bytes <= 0 {
        return false;
    }
    let mut eq = unsafe { &*(handle as *const Handle) }.lock();
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
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len() as f64).sqrt()
    }

    fn tone(freq: f64) -> Vec<f32> {
        (0..48000).map(|i| (0.25 * (2.0 * std::f64::consts::PI * freq * i as f64 / 48000.0).sin()) as f32).collect()
    }

    #[test]
    fn flat_is_identity_and_cut_attenuates_its_band() {
        let mut eq = Equalizer::new(48000, 1);
        let x = tone(1000.0);
        let mut y = vec![0f32; x.len()];
        eq.set_gains(&[0.0; BANDS]);
        eq.process_f32(&x, &mut y);
        assert_eq!(x, y);

        let mut g = [0f32; BANDS];
        g[5] = -12.0;
        eq.set_gains(&g);
        eq.process_f32(&x, &mut y);
        let db = 20.0 * (rms(&y[4800..]) / rms(&x[4800..])).log10();
        assert!((db + 12.0).abs() < 0.5, "1 kHz moved by {db} dB");

        let far = tone(8000.0);
        eq.reset();
        eq.process_f32(&far, &mut y);
        let db = 20.0 * (rms(&y[4800..]) / rms(&far[4800..])).log10();
        assert!(db.abs() < 0.5, "8 kHz moved by {db} dB");
    }
}
