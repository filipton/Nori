//! The sample-domain chain (pre-amp, parametric equalizer, crossfeed), called once per audio buffer from the media3
//! AudioProcessor. This is raw JNI on direct ByteBuffers rather than uniffi:
//! it runs on the playback thread every few milliseconds and must not
//! allocate, copy or serialise anything.

use jni::objects::{JByteBuffer, JClass, JFloatArray};
use jni::sys::{jfloat, jint, jlong};
use jni::JNIEnv;
use parking_lot::Mutex;

pub const PEAKING: i32 = 0;
pub const LOW_SHELF: i32 = 1;
pub const HIGH_SHELF: i32 = 2;
const MAX_CHANNELS: usize = 8;

const PCM_16: jint = 2; // C.ENCODING_PCM_16BIT
const PCM_FLOAT: jint = 4; // C.ENCODING_PCM_FLOAT

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub kind: i32,
    pub freq: f64,
    pub gain_db: f64,
    pub q: f64,
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Biquad {
    /// RBJ cookbook filters.
    fn new(rate: f64, band: &Band) -> Self {
        let a = 10f64.powf(band.gain_db / 40.0);
        let w = 2.0 * std::f64::consts::PI * band.freq / rate;
        let (sin, cos) = (w.sin(), w.cos());
        let alpha = sin / (2.0 * band.q.max(0.05));
        let (b0, b1, b2, a0, a1, a2) = match band.kind {
            LOW_SHELF => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) - (a - 1.0) * cos + k),
                    2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                    a * ((a + 1.0) - (a - 1.0) * cos - k),
                    (a + 1.0) + (a - 1.0) * cos + k,
                    -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                    (a + 1.0) + (a - 1.0) * cos - k,
                )
            }
            HIGH_SHELF => {
                let k = 2.0 * a.sqrt() * alpha;
                (
                    a * ((a + 1.0) + (a - 1.0) * cos + k),
                    -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                    a * ((a + 1.0) + (a - 1.0) * cos - k),
                    (a + 1.0) - (a - 1.0) * cos + k,
                    2.0 * ((a - 1.0) - (a + 1.0) * cos),
                    (a + 1.0) - (a - 1.0) * cos - k,
                )
            }
            _ => (1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a),
        };
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0 }
    }
}

/// Headphone crossfeed after Boris Mikhaylov's bs2b: each ear also gets the other channel, low-passed and
/// attenuated, the way a loudspeaker would reach it. Stereo only.
#[derive(Clone, Copy, Default)]
struct Crossfeed {
    a0_lo: f64,
    b1_lo: f64,
    a0_hi: f64,
    a1_hi: f64,
    b1_hi: f64,
    gain: f64,
    lo: [f64; 2],
    hi: [f64; 2],
    last: [f64; 2],
}

impl Crossfeed {
    fn new(rate: f64, level_db: f64, cut_hz: f64) -> Self {
        let gb_lo = level_db * -5.0 / 6.0 - 3.0;
        let gb_hi = level_db / 6.0 - 3.0;
        let g_lo = 10f64.powf(gb_lo / 20.0);
        let g_hi = 1.0 - 10f64.powf(gb_hi / 20.0);
        let cut_hi = cut_hz * 2f64.powf((gb_lo - 20.0 * g_hi.log10()) / 12.0);
        let x_lo = (-2.0 * std::f64::consts::PI * cut_hz / rate).exp();
        let x_hi = (-2.0 * std::f64::consts::PI * cut_hi / rate).exp();
        Crossfeed {
            a0_lo: g_lo * (1.0 - x_lo),
            b1_lo: x_lo,
            a0_hi: 1.0 - g_hi * (1.0 - x_hi),
            a1_hi: -x_hi,
            b1_hi: x_hi,
            gain: 1.0 / (1.0 - g_hi + g_lo),
            ..Default::default()
        }
    }

    #[inline]
    fn frame(&mut self, l: f64, r: f64) -> (f64, f64) {
        let x = [l, r];
        for c in 0..2 {
            self.lo[c] = self.a0_lo * x[c] + self.b1_lo * self.lo[c];
            self.hi[c] = self.a0_hi * x[c] + self.a1_hi * self.last[c] + self.b1_hi * self.hi[c];
            self.last[c] = x[c];
        }
        ((self.hi[0] + self.lo[1]) * self.gain, (self.hi[1] + self.lo[0]) * self.gain)
    }
}

/// The whole sample-domain chain: pre-amp, parametric equalizer, crossfeed.
pub struct Equalizer {
    rate: f64,
    channels: usize,
    /// Only the bands that do something, so a flat band costs nothing.
    filters: Vec<Biquad>,
    /// Transposed direct form II state, per filter per channel.
    state: Vec<[[f64; 2]; MAX_CHANNELS]>,
    preamp: f64,
    crossfeed: Option<Crossfeed>,
}

impl Equalizer {
    pub fn new(rate: u32, channels: usize) -> Self {
        Equalizer { rate: rate as f64, channels: channels.clamp(1, MAX_CHANNELS), filters: Vec::new(), state: Vec::new(), preamp: 1.0, crossfeed: None }
    }

    /// `crossfeed_db` 0 turns crossfeed off; typical values are 3 to 6.
    pub fn configure(&mut self, bands: &[Band], preamp_db: f64, crossfeed_db: f64) {
        self.filters.clear();
        for b in bands {
            if b.gain_db.abs() >= 0.05 && b.freq > 0.0 && b.freq < self.rate / 2.0 {
                self.filters.push(Biquad::new(self.rate, &Band { gain_db: b.gain_db.clamp(-24.0, 24.0), ..*b }));
            }
        }
        self.state.resize(self.filters.len(), [[0.0; 2]; MAX_CHANNELS]);
        self.preamp = 10f64.powf(preamp_db.clamp(-30.0, 12.0) / 20.0);
        self.crossfeed = (crossfeed_db > 0.0 && self.channels == 2).then(|| Crossfeed::new(self.rate, crossfeed_db.clamp(1.0, 15.0), 700.0));
    }

    /// True when the chain would not change a single sample.
    pub fn is_identity(&self) -> bool {
        self.filters.is_empty() && self.crossfeed.is_none() && (self.preamp - 1.0).abs() < 1e-6
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

    /// One generic loop; `load` and `store` are the only things that differ between sample formats.
    #[inline]
    fn run<T: Copy>(&mut self, input: &[T], output: &mut [T], load: impl Fn(T) -> f64, store: impl Fn(f64) -> T) {
        let n = self.channels;
        if self.is_identity() {
            output[..input.len()].copy_from_slice(input);
        } else if n == 2 && self.crossfeed.is_some() {
            for (x, y) in input.chunks_exact(2).zip(output.chunks_exact_mut(2)) {
                let (l, r) = (self.sample(0, load(x[0])), self.sample(1, load(x[1])));
                let (l, r) = self.crossfeed.as_mut().unwrap().frame(l, r);
                y[0] = store(l);
                y[1] = store(r);
            }
        } else {
            for (i, (x, y)) in input.iter().zip(output.iter_mut()).enumerate() {
                *y = store(self.sample(i % n, load(*x)));
            }
        }
    }

    pub fn process_i16(&mut self, input: &[i16], output: &mut [i16]) {
        self.run(input, output, |x| x as f64, |y| y.round().clamp(-32768.0, 32767.0) as i16);
    }

    pub fn process_f32(&mut self, input: &[f32], output: &mut [f32]) {
        self.run(input, output, |x| x as f64, |y| y as f32);
    }

    pub fn reset(&mut self) {
        self.state.iter_mut().for_each(|s| *s = [[0.0; 2]; MAX_CHANNELS]);
        if let Some(c) = self.crossfeed.as_mut() {
            (c.lo, c.hi, c.last) = ([0.0; 2], [0.0; 2], [0.0; 2]);
        }
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

/// `bands` is flat: kind, frequency, gain dB, Q for each band.
#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_configure(env: JNIEnv, _: JClass, handle: jlong, bands: JFloatArray, preamp_db: jfloat, crossfeed_db: jfloat) {
    if handle == 0 {
        return;
    }
    let n = env.get_array_length(&bands).unwrap_or(0).clamp(0, 4 * 64) as usize;
    let mut flat = vec![0f32; n];
    if env.get_float_array_region(&bands, 0, &mut flat).is_err() {
        return;
    }
    let bands: Vec<Band> = flat.chunks_exact(4).map(|b| Band { kind: b[0] as i32, freq: b[1] as f64, gain_db: b[2] as f64, q: b[3] as f64 }).collect();
    unsafe { &*(handle as *const Handle) }.lock().configure(&bands, preamp_db as f64, crossfeed_db as f64);
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

    fn gain_at(eq: &mut Equalizer, freq: f64) -> f64 {
        let x = tone(freq);
        let mut y = vec![0f32; x.len()];
        eq.reset();
        eq.process_f32(&x, &mut y);
        20.0 * (rms(&y[9600..]) / rms(&x[9600..])).log10()
    }

    #[test]
    fn flat_is_identity_and_a_band_moves_only_its_neighbourhood() {
        let mut eq = Equalizer::new(48000, 1);
        eq.configure(&[], 0.0, 0.0);
        assert!(eq.is_identity());
        let x = tone(1000.0);
        let mut y = vec![0f32; x.len()];
        eq.process_f32(&x, &mut y);
        assert_eq!(x, y);

        eq.configure(&[Band { kind: PEAKING, freq: 1000.0, gain_db: -12.0, q: 1.41 }], 0.0, 0.0);
        assert!((gain_at(&mut eq, 1000.0) + 12.0).abs() < 0.5);
        assert!(gain_at(&mut eq, 8000.0).abs() < 0.5);
    }

    #[test]
    fn shelves_and_preamp() {
        let mut eq = Equalizer::new(48000, 1);
        eq.configure(&[Band { kind: LOW_SHELF, freq: 200.0, gain_db: 6.0, q: 0.71 }], -3.0, 0.0);
        assert!((gain_at(&mut eq, 40.0) - 3.0).abs() < 0.5, "low shelf + preamp at 40 Hz");
        assert!((gain_at(&mut eq, 5000.0) + 3.0).abs() < 0.5, "only the preamp at 5 kHz");
        eq.configure(&[Band { kind: HIGH_SHELF, freq: 4000.0, gain_db: -6.0, q: 0.71 }], 0.0, 0.0);
        assert!((gain_at(&mut eq, 16000.0) + 6.0).abs() < 0.6);
        assert!(gain_at(&mut eq, 200.0).abs() < 0.5);
    }

    #[test]
    fn crossfeed_leaks_bass_to_the_other_ear_and_keeps_mono_level() {
        let mut eq = Equalizer::new(48000, 2);
        eq.configure(&[], 0.0, 4.5);
        let left_only: Vec<f32> = tone(150.0).iter().flat_map(|s| [*s, 0.0]).collect();
        let mut y = vec![0f32; left_only.len()];
        eq.process_f32(&left_only, &mut y);
        let l: Vec<f32> = y.iter().step_by(2).copied().collect();
        let r: Vec<f32> = y.iter().skip(1).step_by(2).copied().collect();
        let leak = 20.0 * (rms(&r[9600..]) / rms(&l[9600..])).log10();
        assert!(leak < -2.0 && leak > -12.0, "right ear is {leak} dB below left");

        let mono: Vec<f32> = tone(150.0).iter().flat_map(|s| [*s, *s]).collect();
        eq.reset();
        eq.process_f32(&mono, &mut y);
        let db = 20.0 * (rms(&y[19200..]) / rms(&mono[19200..])).log10();
        assert!(db.abs() < 1.0, "mono level moved by {db} dB");
    }
}
