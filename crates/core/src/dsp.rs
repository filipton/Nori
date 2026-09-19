//! The sample-domain chain (pre-amp, parametric equalizer, mono, crossfeed, balance, limiter), called once per audio
//! buffer from the media3 AudioProcessor. This is raw JNI on direct ByteBuffers rather than uniffi:
//! it runs on the playback thread every few milliseconds and must not
//! allocate, copy or serialise anything.
//!
//! Chain order, and why: pre-amp and the equalizer come first because everything after them is a mix or a level
//! decision that wants to see the tone the user actually chose. Mono collapses the stage before the crossfeed, so the
//! crossfeed models one loudspeaker pair rather than two already-mixed ears. Balance sits after the crossfeed,
//! otherwise the crossfeed would leak the louder side back into the quieter ear and undo half of it. The limiter is
//! last, so it sees every boost (pre-amp, EQ, ReplayGain that the player applied upstream, the +3 dB that centred
//! material gains from the mono sum) and is the only stage that can decide what leaves the chain.

use std::sync::atomic::{AtomicU32, Ordering};

use jni::objects::{JByteBuffer, JClass, JFloatArray};
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::JNIEnv;
use parking_lot::Mutex;

use crate::{EqBand, EqKind, NamedPreset};

pub const PEAKING: i32 = 0;
pub const LOW_SHELF: i32 = 1;
pub const HIGH_SHELF: i32 = 2;
pub const LOW_PASS: i32 = 3;
pub const HIGH_PASS: i32 = 4;
pub const BAND_PASS: i32 = 5;
pub const NOTCH: i32 = 6;
pub const ALL_PASS: i32 = 7;
/// Shelves whose `q` is the RBJ slope S (1 is the steepest slope that does not ripple) instead of a Q.
pub const LOW_SHELF_SLOPE: i32 = 8;
pub const HIGH_SHELF_SLOPE: i32 = 9;

pub const CH_BOTH: i32 = 0;
pub const CH_LEFT: i32 = 1;
pub const CH_RIGHT: i32 = 2;

const MAX_CHANNELS: usize = 8;
/// A balance of ±1 mutes one side; in between the quiet side is trimmed linearly in dB, which is what a slider feels like.
const BALANCE_RANGE_DB: f64 = 24.0;
/// Sum and then -3.01 dB: uncorrelated material keeps its level. Centred material gains 3 dB, which the limiter catches.
const MONO_SUM: f64 = std::f64::consts::FRAC_1_SQRT_2;
/// Width of the limiter's soft knee, centred on the threshold. Below `threshold - KNEE_DB / 2` the limiter is bit-exact.
const KNEE_DB: f64 = 4.0;

const PCM_16: jint = 2; // C.ENCODING_PCM_16BIT
const PCM_FLOAT: jint = 4; // C.ENCODING_PCM_FLOAT

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    pub kind: i32,
    pub freq: f64,
    pub gain_db: f64,
    /// Q, or the slope S for the `*_SHELF_SLOPE` kinds.
    pub q: f64,
    /// `CH_BOTH`, `CH_LEFT` or `CH_RIGHT`.
    pub channel: i32,
}

/// Anything but NaN and the infinities, which would otherwise poison the filter state for good.
#[inline]
fn finite(v: f64, fallback: f64) -> f64 {
    if v.is_finite() {
        v
    } else {
        fallback
    }
}

#[derive(Clone, Copy, Default)]
struct Biquad {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
    /// Bit per channel this filter runs on, so per-side bands cost a test rather than a second filter list.
    chans: u8,
}

impl Biquad {
    /// RBJ cookbook filters.
    fn new(rate: f64, band: &Band) -> Self {
        let a = 10f64.powf(band.gain_db / 40.0);
        let w = 2.0 * std::f64::consts::PI * band.freq / rate;
        let (sin, cos) = (w.sin(), w.cos());
        let q = band.q.clamp(0.05, 40.0);
        let alpha = sin / (2.0 * q);
        // The shelf slope form: S = 1 is as steep as a shelf gets without a peak at the corner.
        let slope_alpha = || {
            let s = band.q.clamp(0.05, 1.0);
            sin / 2.0 * ((a + 1.0 / a) * (1.0 / s - 1.0) + 2.0).max(0.0).sqrt()
        };
        let low_shelf = |k: f64| {
            (
                a * ((a + 1.0) - (a - 1.0) * cos + k),
                2.0 * a * ((a - 1.0) - (a + 1.0) * cos),
                a * ((a + 1.0) - (a - 1.0) * cos - k),
                (a + 1.0) + (a - 1.0) * cos + k,
                -2.0 * ((a - 1.0) + (a + 1.0) * cos),
                (a + 1.0) + (a - 1.0) * cos - k,
            )
        };
        let high_shelf = |k: f64| {
            (
                a * ((a + 1.0) + (a - 1.0) * cos + k),
                -2.0 * a * ((a - 1.0) + (a + 1.0) * cos),
                a * ((a + 1.0) + (a - 1.0) * cos - k),
                (a + 1.0) - (a - 1.0) * cos + k,
                2.0 * ((a - 1.0) - (a + 1.0) * cos),
                (a + 1.0) - (a - 1.0) * cos - k,
            )
        };
        let (b0, b1, b2, a0, a1, a2) = match band.kind {
            LOW_SHELF => low_shelf(2.0 * a.sqrt() * alpha),
            HIGH_SHELF => high_shelf(2.0 * a.sqrt() * alpha),
            LOW_SHELF_SLOPE => low_shelf(2.0 * a.sqrt() * slope_alpha()),
            HIGH_SHELF_SLOPE => high_shelf(2.0 * a.sqrt() * slope_alpha()),
            LOW_PASS => ((1.0 - cos) / 2.0, 1.0 - cos, (1.0 - cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            HIGH_PASS => ((1.0 + cos) / 2.0, -(1.0 + cos), (1.0 + cos) / 2.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            // Constant 0 dB peak gain, so Q only sets the width and never the level.
            BAND_PASS => (alpha, 0.0, -alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            NOTCH => (1.0, -2.0 * cos, 1.0, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            ALL_PASS => (1.0 - alpha, -2.0 * cos, 1.0 + alpha, 1.0 + alpha, -2.0 * cos, 1.0 - alpha),
            _ => (1.0 + alpha * a, -2.0 * cos, 1.0 - alpha * a, 1.0 + alpha / a, -2.0 * cos, 1.0 - alpha / a),
        };
        let chans = match band.channel {
            CH_LEFT => 0b0000_0001,
            CH_RIGHT => 0b0000_0010,
            _ => u8::MAX,
        };
        Biquad { b0: b0 / a0, b1: b1 / a0, b2: b2 / a0, a1: a1 / a0, a2: a2 / a0, chans }
    }
}

/// True for the kinds whose `gain_db` means something; the others are shapes and stay in the chain at any gain.
fn uses_gain(kind: i32) -> bool {
    matches!(kind, PEAKING | LOW_SHELF | HIGH_SHELF | LOW_SHELF_SLOPE | HIGH_SHELF_SLOPE)
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

/// Look-ahead peak limiter. The gain follower reads the frame going in while the output reads the delay line, so the
/// gain has the whole look-ahead window to arrive before the peak does. Below the knee the gain is *exactly* 1.0 and
/// the samples come back out of the delay line untouched, which is what makes turning the limiter on free: a boost
/// that never reaches the threshold costs a few ms of delay and nothing else.
struct Limiter {
    /// `frames * channels`, a ring; allocated here, never in the per-buffer path.
    delay: Vec<f64>,
    pos: usize,
    frames: usize,
    channels: usize,
    thresh_lin: f64,
    thresh_db: f64,
    knee_start: f64,
    knee_end: f64,
    attack: f64,
    release: f64,
    decay: f64,
    /// Peak hold, so the envelope cannot start releasing while the peak it describes is still inside the delay line.
    env: f64,
    hold: usize,
    gain: f64,
    /// Smallest gain in the buffer just processed; the meter the UI polls.
    meter: f64,
}

impl Limiter {
    fn frames_for(rate: f64, lookahead_ms: f64) -> usize {
        ((finite(lookahead_ms, 5.0).clamp(0.5, 20.0) / 1000.0 * rate) as usize).max(1)
    }

    fn new(rate: f64, channels: usize, frames: usize) -> Self {
        let mut l = Limiter {
            delay: vec![0.0; frames * channels],
            pos: 0,
            frames,
            channels,
            thresh_lin: 1.0,
            thresh_db: 0.0,
            knee_start: 1.0,
            knee_end: 1.0,
            attack: 1.0,
            release: 1.0,
            decay: 1.0,
            env: 0.0,
            hold: 0,
            gain: 1.0,
            meter: 1.0,
        };
        l.tune(rate, 0.0, 100.0);
        l
    }

    /// Coefficients only, so moving a slider re-tunes the curve without dropping the delay line or the current gain.
    fn tune(&mut self, rate: f64, thresh_db: f64, release_ms: f64) {
        let thresh_db = finite(thresh_db, -1.0).clamp(-40.0, 0.0);
        let per_sample = (-1.0 / (finite(release_ms, 100.0).clamp(5.0, 2000.0) / 1000.0 * rate)).exp();
        self.thresh_db = thresh_db;
        self.thresh_lin = 10f64.powf(thresh_db / 20.0);
        self.knee_start = 10f64.powf((thresh_db - KNEE_DB / 2.0) / 20.0);
        self.knee_end = 10f64.powf((thresh_db + KNEE_DB / 2.0) / 20.0);
        // Converge to within 0.1 % of the target inside the look-ahead window, so peaks arrive at the right gain.
        self.attack = 1.0 - 0.001f64.powf(1.0 / self.frames as f64);
        self.release = 1.0 - per_sample;
        self.decay = per_sample;
    }

    /// The static curve: unity below the knee, a quadratic through it, a hard ceiling above it.
    #[inline]
    fn curve(&self, env: f64) -> f64 {
        if env <= self.knee_start {
            1.0
        } else if env >= self.knee_end {
            self.thresh_lin / env // = 10^(-(env_db - thresh_db)/20), without the logarithms
        } else {
            let over = 20.0 * env.log10() - self.thresh_db + KNEE_DB / 2.0;
            10f64.powf(-(over * over / (2.0 * KNEE_DB)) / 20.0)
        }
    }

    #[inline]
    fn frame(&mut self, x: &mut [f64]) {
        let mut peak = 0.0f64;
        for v in x.iter() {
            peak = peak.max(v.abs());
        }
        if peak >= self.env {
            (self.env, self.hold) = (peak, self.frames);
        } else if self.hold > 0 {
            self.hold -= 1;
        } else {
            self.env *= self.decay;
        }
        let want = self.curve(self.env);
        self.gain += (want - self.gain) * if want < self.gain { self.attack } else { self.release };
        if self.gain > 1.0 - 1e-7 {
            self.gain = 1.0; // snap, so the chain goes back to bit-exact once it has released
        }
        let slot = self.pos * self.channels;
        for (c, v) in x.iter_mut().enumerate() {
            let out = self.delay[slot + c];
            self.delay[slot + c] = *v;
            *v = out * self.gain;
        }
        self.pos = if self.pos + 1 == self.frames { 0 } else { self.pos + 1 };
        self.meter = self.meter.min(self.gain);
    }

    fn reset(&mut self) {
        self.delay.fill(0.0);
        (self.pos, self.env, self.hold, self.gain, self.meter) = (0, 0.0, 0, 1.0, 1.0);
    }
}

/// The whole sample-domain chain: pre-amp, parametric equalizer, mono, crossfeed, balance, limiter.
pub struct Equalizer {
    rate: f64,
    channels: usize,
    /// Only the bands that do something, so a flat band costs nothing.
    filters: Vec<Biquad>,
    /// Transposed direct form II state, per filter per channel.
    state: Vec<[[f64; 2]; MAX_CHANNELS]>,
    preamp: f64,
    crossfeed: Option<Crossfeed>,
    mono: bool,
    balance: (f64, f64),
    limiter: Option<Limiter>,
}

/// Left and right gain for a balance in -1 (hard left) to 1 (hard right).
fn balance_gains(balance: f64) -> (f64, f64) {
    let b = finite(balance, 0.0).clamp(-1.0, 1.0);
    let att = if b.abs() >= 1.0 { 0.0 } else { 10f64.powf(-b.abs() * BALANCE_RANGE_DB / 20.0) };
    if b >= 0.0 {
        (att, 1.0)
    } else {
        (1.0, att)
    }
}

impl Equalizer {
    pub fn new(rate: u32, channels: usize) -> Self {
        Equalizer {
            rate: rate as f64,
            channels: channels.clamp(1, MAX_CHANNELS),
            filters: Vec::new(),
            state: Vec::new(),
            preamp: 1.0,
            crossfeed: None,
            mono: false,
            balance: (1.0, 1.0),
            limiter: None,
        }
    }

    /// `crossfeed_db` 0 turns crossfeed off; typical values are 3 to 6.
    pub fn configure(&mut self, bands: &[Band], preamp_db: f64, crossfeed_db: f64) {
        self.filters.clear();
        for b in bands {
            // A side band needs a side: on a mono stream there is nothing to route, so only `CH_BOTH` survives.
            let routable = b.channel == CH_BOTH || (self.channels >= 2 && (b.channel == CH_LEFT || b.channel == CH_RIGHT));
            let shaped = !uses_gain(b.kind) || b.gain_db.abs() >= 0.05;
            if routable && shaped && b.freq > 0.0 && b.freq < self.rate / 2.0 && matches!(b.kind, PEAKING..=HIGH_SHELF_SLOPE) {
                self.filters.push(Biquad::new(self.rate, &Band { gain_db: b.gain_db.clamp(-24.0, 24.0), ..*b }));
            }
        }
        self.state.resize(self.filters.len(), [[0.0; 2]; MAX_CHANNELS]);
        self.preamp = 10f64.powf(finite(preamp_db, 0.0).clamp(-30.0, 12.0) / 20.0);
        self.crossfeed = (crossfeed_db > 0.0 && self.channels == 2).then(|| Crossfeed::new(self.rate, crossfeed_db.clamp(1.0, 15.0), 700.0));
    }

    /// The output stage. `balance` is -1 (left) to 1 (right); mono and balance are stereo ideas and are ignored
    /// otherwise. `lookahead_ms` at or below 0 turns the limiter off, which is also the only way to get its delay back.
    pub fn configure_output(&mut self, balance: f64, mono: bool, threshold_db: f64, release_ms: f64, lookahead_ms: f64) {
        self.mono = mono && self.channels == 2;
        self.balance = if self.channels == 2 { balance_gains(balance) } else { (1.0, 1.0) };
        if lookahead_ms <= 0.0 {
            self.limiter = None;
            return;
        }
        let frames = Limiter::frames_for(self.rate, lookahead_ms);
        let mut l = self.limiter.take().filter(|l| l.frames == frames).unwrap_or_else(|| Limiter::new(self.rate, self.channels, frames));
        l.tune(self.rate, threshold_db, release_ms);
        self.limiter = Some(l);
    }

    /// True when the chain would not change a single sample.
    pub fn is_identity(&self) -> bool {
        self.filters.is_empty()
            && self.crossfeed.is_none()
            && self.limiter.is_none()
            && !self.mono
            && self.balance == (1.0, 1.0)
            && (self.preamp - 1.0).abs() < 1e-6
    }

    /// Peak gain reduction in the buffer just processed, for the UI meter. Zero when the limiter is off or idle.
    pub fn gain_reduction_db(&self) -> f32 {
        self.limiter.as_ref().map_or(0.0, |l| (-20.0 * l.meter.log10()) as f32)
    }

    #[inline]
    fn sample(&mut self, ch: usize, x: f64) -> f64 {
        let mut x = x * self.preamp;
        let bit = 1u8 << ch;
        for (f, st) in self.filters.iter().zip(self.state.iter_mut()) {
            if f.chans & bit == 0 {
                continue;
            }
            let s = &mut st[ch];
            let y = f.b0 * x + s[0];
            s[0] = f.b1 * x - f.a1 * y + s[1];
            s[1] = f.b2 * x - f.a2 * y;
            x = y;
        }
        x
    }

    /// Everything after the equalizer, on one frame: mono, crossfeed, balance, limiter.
    #[inline]
    fn output_stage(&mut self, f: &mut [f64]) {
        if self.channels == 2 {
            if self.mono {
                let m = (f[0] + f[1]) * MONO_SUM;
                (f[0], f[1]) = (m, m);
            }
            if let Some(cf) = self.crossfeed.as_mut() {
                (f[0], f[1]) = cf.frame(f[0], f[1]);
            }
            f[0] *= self.balance.0;
            f[1] *= self.balance.1;
        }
        if let Some(l) = self.limiter.as_mut() {
            l.frame(f);
        }
    }

    /// One generic loop; `load` and `store` are the only things that differ between sample formats.
    #[inline]
    fn run<T: Copy>(&mut self, input: &[T], output: &mut [T], load: impl Fn(T) -> f64, store: impl Fn(f64) -> T) {
        let len = input.len().min(output.len());
        let (input, output) = (&input[..len], &mut output[..len]);
        if self.is_identity() {
            output.copy_from_slice(input);
            return;
        }
        let n = self.channels;
        if let Some(l) = self.limiter.as_mut() {
            l.meter = 1.0;
        }
        let mut frame = [0f64; MAX_CHANNELS];
        for (x, y) in input.chunks_exact(n).zip(output.chunks_exact_mut(n)) {
            for (c, v) in x.iter().enumerate() {
                frame[c] = self.sample(c, load(*v));
            }
            self.output_stage(&mut frame[..n]);
            for (c, v) in y.iter_mut().enumerate() {
                *v = store(frame[c]);
            }
        }
        // media3 hands over whole frames; a ragged tail would still have to come out somewhere.
        let tail = len - len % n;
        output[tail..].copy_from_slice(&input[tail..]);
    }

    /// 16-bit samples are brought to the same scale as float ones, where 1.0 is full scale, before anything touches
    /// them. The filters would not notice either way - they are linear - but the limiter compares against a ceiling
    /// in full-scale units, and fed raw integers it took every sample for thirty thousand times too loud and turned
    /// the music down by some 91 dB: silence, on the default 16-bit path, whenever the limiter was on. Dividing and
    /// multiplying by a power of two is exact in f64, so a chain that changes nothing still changes nothing.
    pub fn process_i16(&mut self, input: &[i16], output: &mut [i16]) {
        self.run(input, output, |x| x as f64 / I16_SCALE, |y| (y * I16_SCALE).round().clamp(-32768.0, 32767.0) as i16);
    }

    pub fn process_f32(&mut self, input: &[f32], output: &mut [f32]) {
        self.run(input, output, |x| x as f64, |y| y as f32);
    }

    pub fn reset(&mut self) {
        self.state.iter_mut().for_each(|s| *s = [[0.0; 2]; MAX_CHANNELS]);
        if let Some(c) = self.crossfeed.as_mut() {
            (c.lo, c.hi, c.last) = ([0.0; 2], [0.0; 2], [0.0; 2]);
        }
        if let Some(l) = self.limiter.as_mut() {
            l.reset();
        }
    }
}

/// Full scale for 16-bit samples: `i16::MIN` maps to exactly -1.0.
const I16_SCALE: f64 = 32768.0;

/// The meter lives outside the lock so the UI can poll it without ever waiting on the playback thread.
struct Handle {
    eq: Mutex<Equalizer>,
    reduction_db: AtomicU32,
}

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_create(_: JNIEnv, _: JClass, rate: jint, channels: jint) -> jlong {
    let eq = Mutex::new(Equalizer::new(rate.max(1) as u32, channels.max(1) as usize));
    Box::into_raw(Box::new(Handle { eq, reduction_db: AtomicU32::new(0) })) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_destroy(_: JNIEnv, _: JClass, handle: jlong) {
    if handle != 0 {
        drop(unsafe { Box::from_raw(handle as *mut Handle) });
    }
}

/// `bands` is flat: kind, frequency, gain dB, Q (slope S for the slope shelves), channel for each band.
#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_configure(env: JNIEnv, _: JClass, handle: jlong, bands: JFloatArray, preamp_db: jfloat, crossfeed_db: jfloat) {
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
pub extern "system" fn Java_dev_flint_music_playback_Dsp_configureOutput(
    _: JNIEnv, _: JClass, handle: jlong, balance: jfloat, mono: jboolean, threshold_db: jfloat, release_ms: jfloat, lookahead_ms: jfloat,
) {
    if handle != 0 {
        unsafe { &*(handle as *const Handle) }.eq.lock().configure_output(balance as f64, mono != 0, threshold_db as f64, release_ms as f64, lookahead_ms as f64);
    }
}

/// The limiter meter: peak gain reduction in dB in the last buffer, 0 when it is off or idle. Lock-free, poll freely.
#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_gainReductionDb(_: JNIEnv, _: JClass, handle: jlong) -> jfloat {
    if handle == 0 {
        return 0.0;
    }
    f32::from_bits(unsafe { &*(handle as *const Handle) }.reduction_db.load(Ordering::Relaxed))
}

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_Dsp_reset(_: JNIEnv, _: JClass, handle: jlong) {
    if handle != 0 {
        unsafe { &*(handle as *const Handle) }.eq.lock().reset();
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

fn band(kind: EqKind, freq: f32, gain_db: f32, q: f32) -> EqBand {
    EqBand { kind, freq, gain_db, q }
}

/// The built-in curves, as data, so the UI (and the settings store) never holds a frequency of its own.
/// Every preset that boosts carries a pre-amp that pays the boost back, so a preset cannot clip on its own.
#[uniffi::export]
pub fn eq_presets() -> Vec<NamedPreset> {
    let preset = |name: &str, preamp_db: f32, bands: Vec<EqBand>| NamedPreset { name: name.to_string(), preamp_db, bands };
    vec![
        preset("Flat", 0.0, vec![]),
        preset("Bass boost", -6.0, vec![band(EqKind::LowShelf, 100.0, 6.0, 0.7), band(EqKind::Peaking, 60.0, 3.0, 1.0)]),
        preset("Bass cut", 0.0, vec![band(EqKind::LowShelf, 110.0, -6.0, 0.7)]),
        preset("Treble boost", -5.0, vec![band(EqKind::HighShelf, 6000.0, 5.0, 0.7)]),
        preset("Treble cut", 0.0, vec![band(EqKind::HighShelf, 6000.0, -5.0, 0.7)]),
        preset(
            "Vocal boost",
            -4.0,
            vec![band(EqKind::Peaking, 300.0, -2.0, 1.0), band(EqKind::Peaking, 2500.0, 4.0, 1.2), band(EqKind::Peaking, 5000.0, 2.0, 1.5)],
        ),
        // The equal-loudness smile: what quiet listening takes away at both ends.
        preset(
            "Loudness",
            -7.0,
            vec![band(EqKind::LowShelfSlope, 80.0, 7.0, 0.8), band(EqKind::Peaking, 1000.0, -2.0, 1.0), band(EqKind::HighShelfSlope, 10000.0, 5.0, 0.8)],
        ),
        // Phone and laptop drivers: throw away what they can only rattle on, then put the body back an octave up.
        preset(
            "Small speakers",
            -4.0,
            vec![band(EqKind::HighPass, 90.0, 0.0, 0.71), band(EqKind::Peaking, 220.0, 4.0, 1.0), band(EqKind::Peaking, 3000.0, 2.0, 1.2)],
        ),
    ]
}

impl From<&EqBand> for Band {
    fn from(b: &EqBand) -> Self {
        Band { kind: b.kind as i32, freq: b.freq as f64, gain_db: b.gain_db as f64, q: b.q as f64, channel: CH_BOTH }
    }
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

    fn tone_at(freq: f64, amplitude: f64) -> Vec<f32> {
        (0..48000).map(|i| (amplitude * (2.0 * std::f64::consts::PI * freq * i as f64 / 48000.0).sin()) as f32).collect()
    }

    fn b(kind: i32, freq: f64, gain_db: f64, q: f64) -> Band {
        Band { kind, freq, gain_db, q, channel: CH_BOTH }
    }

    fn gain_at(eq: &mut Equalizer, freq: f64) -> f64 {
        let x = tone(freq);
        let mut y = vec![0f32; x.len()];
        eq.reset();
        eq.process_f32(&x, &mut y);
        20.0 * (rms(&y[9600..]) / rms(&x[9600..])).log10()
    }

    /// Runs a stereo tone and returns the per-channel gain in dB.
    fn stereo_gain_at(eq: &mut Equalizer, freq: f64) -> (f64, f64) {
        let m = tone(freq);
        let x: Vec<f32> = m.iter().flat_map(|s| [*s, *s]).collect();
        let mut y = vec![0f32; x.len()];
        eq.reset();
        eq.process_f32(&x, &mut y);
        let l: Vec<f32> = y.iter().step_by(2).skip(4800).copied().collect();
        let r: Vec<f32> = y.iter().skip(1).step_by(2).skip(4800).copied().collect();
        let ref_rms = rms(&m[9600..]);
        (20.0 * (rms(&l) / ref_rms).log10(), 20.0 * (rms(&r) / ref_rms).log10())
    }

    fn peak(x: &[f32]) -> f64 {
        x.iter().fold(0f64, |m, v| m.max(v.abs() as f64))
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

        eq.configure(&[b(PEAKING, 1000.0, -12.0, 1.41)], 0.0, 0.0);
        assert!((gain_at(&mut eq, 1000.0) + 12.0).abs() < 0.5);
        assert!(gain_at(&mut eq, 8000.0).abs() < 0.5);
    }

    #[test]
    fn shelves_and_preamp() {
        let mut eq = Equalizer::new(48000, 1);
        eq.configure(&[b(LOW_SHELF, 200.0, 6.0, 0.71)], -3.0, 0.0);
        assert!((gain_at(&mut eq, 40.0) - 3.0).abs() < 0.5, "low shelf + preamp at 40 Hz");
        assert!((gain_at(&mut eq, 5000.0) + 3.0).abs() < 0.5, "only the preamp at 5 kHz");
        eq.configure(&[b(HIGH_SHELF, 4000.0, -6.0, 0.71)], 0.0, 0.0);
        assert!((gain_at(&mut eq, 16000.0) + 6.0).abs() < 0.6);
        assert!(gain_at(&mut eq, 200.0).abs() < 0.5);
    }

    #[test]
    fn slope_shelves_reach_their_gain_and_stay_out_of_the_other_end() {
        let mut eq = Equalizer::new(48000, 1);
        eq.configure(&[b(LOW_SHELF_SLOPE, 250.0, 8.0, 1.0)], 0.0, 0.0);
        assert!((gain_at(&mut eq, 40.0) - 8.0).abs() < 0.6, "low slope shelf at 40 Hz: {}", gain_at(&mut eq, 40.0));
        assert!((gain_at(&mut eq, 250.0) - 4.0).abs() < 0.6, "half the gain at the corner");
        assert!(gain_at(&mut eq, 8000.0).abs() < 0.3);

        eq.configure(&[b(HIGH_SHELF_SLOPE, 3000.0, -8.0, 1.0)], 0.0, 0.0);
        assert!((gain_at(&mut eq, 16000.0) + 8.0).abs() < 0.7);
        assert!(gain_at(&mut eq, 100.0).abs() < 0.3);
    }

    #[test]
    fn pass_filters_cut_the_far_side_and_leave_the_pass_band() {
        let mut eq = Equalizer::new(48000, 1);
        eq.configure(&[b(LOW_PASS, 1000.0, 0.0, 0.707)], 0.0, 0.0);
        assert!(gain_at(&mut eq, 100.0).abs() < 0.2, "low pass pass-band");
        assert!((gain_at(&mut eq, 1000.0) + 3.0).abs() < 0.6, "-3 dB at the corner");
        assert!(gain_at(&mut eq, 8000.0) < -15.0, "two poles, three octaves up");

        eq.configure(&[b(HIGH_PASS, 1000.0, 0.0, 0.707)], 0.0, 0.0);
        assert!(gain_at(&mut eq, 10000.0).abs() < 0.2);
        assert!((gain_at(&mut eq, 1000.0) + 3.0).abs() < 0.6);
        assert!(gain_at(&mut eq, 125.0) < -15.0);
    }

    #[test]
    fn band_pass_peaks_at_unity_and_notch_digs_a_hole() {
        let mut eq = Equalizer::new(48000, 1);
        eq.configure(&[b(BAND_PASS, 1000.0, 0.0, 2.0)], 0.0, 0.0);
        assert!(gain_at(&mut eq, 1000.0).abs() < 0.2, "constant 0 dB peak gain");
        assert!(gain_at(&mut eq, 100.0) < -12.0 && gain_at(&mut eq, 10000.0) < -12.0);

        eq.configure(&[b(NOTCH, 1000.0, 0.0, 8.0)], 0.0, 0.0);
        assert!(gain_at(&mut eq, 1000.0) < -20.0, "notch at the centre");
        assert!(gain_at(&mut eq, 250.0).abs() < 0.4 && gain_at(&mut eq, 4000.0).abs() < 0.4);
    }

    #[test]
    fn all_pass_keeps_the_level_and_moves_the_phase() {
        let mut eq = Equalizer::new(48000, 1);
        eq.configure(&[b(ALL_PASS, 1000.0, 0.0, 0.707)], 0.0, 0.0);
        for f in [100.0, 1000.0, 5000.0, 15000.0] {
            assert!(gain_at(&mut eq, f).abs() < 0.2, "all pass is flat at {f} Hz");
        }
        let x = tone(1000.0);
        let mut y = vec![0f32; x.len()];
        eq.reset();
        eq.process_f32(&x, &mut y);
        assert!(!eq.is_identity() && x[24000..] != y[24000..], "the phase moved even though the level did not");
    }

    #[test]
    fn bad_bands_are_ignored_rather_than_played() {
        let mut eq = Equalizer::new(48000, 2);
        eq.configure(
            &[
                b(PEAKING, f64::NAN, 6.0, 1.0),
                b(PEAKING, 30000.0, 6.0, 1.0),
                b(PEAKING, -100.0, 6.0, 1.0),
                b(PEAKING, 1000.0, f64::NAN, 1.0),
                b(99, 1000.0, 6.0, 1.0),
            ],
            f64::NAN,
            0.0,
        );
        assert!(eq.is_identity(), "nothing survived, so the processor can be skipped");
    }

    #[test]
    fn a_band_can_be_routed_to_one_side_only() {
        let mut eq = Equalizer::new(48000, 2);
        eq.configure(&[Band { channel: CH_LEFT, ..b(PEAKING, 1000.0, 12.0, 1.0) }], 0.0, 0.0);
        let (l, r) = stereo_gain_at(&mut eq, 1000.0);
        assert!((l - 12.0).abs() < 0.5 && r.abs() < 0.2, "left only: {l} / {r}");

        eq.configure(&[Band { channel: CH_RIGHT, ..b(PEAKING, 1000.0, -12.0, 1.0) }], 0.0, 0.0);
        let (l, r) = stereo_gain_at(&mut eq, 1000.0);
        assert!(l.abs() < 0.2 && (r + 12.0).abs() < 0.5, "right only: {l} / {r}");

        // A `both` band must leave centre content centred, sample for sample.
        eq.configure(&[b(HIGH_SHELF, 4000.0, 8.0, 0.71)], 0.0, 0.0);
        let m = tone(4000.0);
        let x: Vec<f32> = m.iter().flat_map(|s| [*s, *s]).collect();
        let mut y = vec![0f32; x.len()];
        eq.reset();
        eq.process_f32(&x, &mut y);
        assert!(y.chunks_exact(2).all(|f| f[0] == f[1]), "both-channel band kept the centre centred");

        // A mono stream has no sides, so a side band is dropped instead of half-applied.
        let mut mono = Equalizer::new(48000, 1);
        mono.configure(&[Band { channel: CH_RIGHT, ..b(PEAKING, 1000.0, 12.0, 1.0) }], 0.0, 0.0);
        assert!(mono.is_identity());
    }

    #[test]
    fn balance_trims_one_side_and_mono_keeps_the_level() {
        let mut eq = Equalizer::new(48000, 2);
        eq.configure(&[], 0.0, 0.0);
        eq.configure_output(0.5, false, 0.0, 100.0, 0.0);
        let (l, r) = stereo_gain_at(&mut eq, 1000.0);
        assert!((l + 12.0).abs() < 0.01 && r.abs() < 0.01, "half right is -12 dB on the left: {l} / {r}");

        eq.configure_output(-1.0, false, 0.0, 100.0, 0.0);
        let (l, r) = stereo_gain_at(&mut eq, 1000.0);
        assert!(l.abs() < 0.01 && r < -100.0, "hard left mutes the right: {l} / {r}");

        eq.configure_output(0.0, false, 0.0, 100.0, 0.0);
        assert!(eq.is_identity(), "centred balance costs nothing");

        // Two uncorrelated tones: the mono sum must keep the level, not lose 3 dB.
        eq.configure_output(0.0, true, 0.0, 100.0, 0.0);
        let (a, c) = (tone(440.0), tone(3700.0));
        let x: Vec<f32> = a.iter().zip(&c).flat_map(|(l, r)| [*l, *r]).collect();
        let mut y = vec![0f32; x.len()];
        eq.process_f32(&x, &mut y);
        let db = 20.0 * (rms(&y[19200..]) / rms(&x[19200..])).log10();
        assert!(db.abs() < 0.3, "mono sum moved the level by {db} dB");
        assert!(y.chunks_exact(2).all(|f| f[0] == f[1]), "both channels carry the same mono signal");
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

    #[test]
    fn the_limiter_is_bit_exact_below_the_threshold() {
        let mut eq = Equalizer::new(48000, 1);
        eq.configure(&[], 0.0, 0.0);
        eq.configure_output(0.0, false, -6.0, 120.0, 5.0);
        assert!(!eq.is_identity(), "the look-ahead delay alone means the processor must run");

        let x = tone_at(1000.0, 0.25); // -12 dBFS, well under the knee
        let mut y = vec![0f32; x.len()];
        eq.process_f32(&x, &mut y);
        let d = 240; // 5 ms at 48 kHz
        assert_eq!(&y[d..], &x[..x.len() - d], "below the knee the samples come back untouched");
        assert_eq!(eq.gain_reduction_db(), 0.0);
    }

    #[test]
    fn retuning_the_limiter_does_not_break_the_stream() {
        let mut eq = Equalizer::new(48000, 1);
        eq.configure(&[], 0.0, 0.0);
        eq.configure_output(0.0, false, -6.0, 120.0, 5.0);
        let x = tone_at(1000.0, 0.25);
        let (head, tail) = x.split_at(24000);
        let (mut a, mut b) = (vec![0f32; head.len()], vec![0f32; tail.len()]);
        eq.process_f32(head, &mut a);
        eq.configure_output(0.0, false, -3.0, 300.0, 5.0); // a slider moved mid-track
        eq.process_f32(tail, &mut b);
        let joined: Vec<f32> = a.into_iter().chain(b).collect();
        let d = 240;
        assert_eq!(&joined[d..], &x[..x.len() - d], "the delay line survived the new settings");
    }

    #[test]
    fn the_limiter_holds_the_ceiling_and_reports_the_reduction() {
        let mut eq = Equalizer::new(48000, 2);
        eq.configure(&[], 0.0, 0.0);
        eq.configure_output(0.0, false, -6.0, 80.0, 5.0);
        let m = tone_at(220.0, 1.0); // 0 dBFS into a -6 dBFS ceiling: 6 dB of reduction
        let x: Vec<f32> = m.iter().flat_map(|s| [*s, *s]).collect();
        let mut y = vec![0f32; x.len()];
        eq.process_f32(&x, &mut y);

        let ceiling = 10f64.powf(-6.0 / 20.0);
        assert!(peak(&y[9600..]) <= ceiling * 1.04, "peak {} over the {ceiling} ceiling", peak(&y[9600..]));
        assert!(peak(&y[9600..]) > ceiling * 0.9, "and it is not simply squashed flat");
        let gr = eq.gain_reduction_db() as f64;
        assert!((gr - 6.0).abs() < 0.5, "meter says {gr} dB, expected about 6");

        // A quiet buffer after the release has run its course reads back as no reduction at all.
        eq.reset();
        let quiet = tone_at(220.0, 0.1);
        let x: Vec<f32> = quiet.iter().flat_map(|s| [*s, *s]).collect();
        eq.process_f32(&x, &mut y);
        assert_eq!(eq.gain_reduction_db(), 0.0);
    }

    #[test]
    fn the_limiter_catches_what_a_boost_would_have_clipped() {
        let mut eq = Equalizer::new(48000, 1);
        // +10 dB of pre-amp on a -3 dBFS tone would leave the chain at +7 dBFS.
        eq.configure(&[], 10.0, 0.0);
        eq.configure_output(0.0, false, -1.0, 100.0, 5.0);
        let x = tone_at(700.0, 0.7);
        let mut y = vec![0f32; x.len()];
        eq.process_f32(&x, &mut y);
        assert!(peak(&y[9600..]) <= 10f64.powf(-1.0 / 20.0) * 1.04, "peak {}", peak(&y[9600..]));
    }

    /// The path the phone uses by default. Every other limiter test feeds floats, which is how 16-bit audio being
    /// turned down by 91 dB - silence - went unnoticed: the float path was always scaled right.
    #[test]
    fn the_limiter_on_16_bit_audio_passes_normal_music_and_only_catches_peaks() {
        let mut eq = Equalizer::new(48000, 2);
        eq.configure(&[], 0.0, 0.0);
        eq.configure_output(0.0, false, -1.0, 120.0, 5.0);
        let d = 240 * 2; // 5 ms at 48 kHz, two channels

        // -12 dBFS, far below the ceiling: must come out unchanged, only delayed.
        let quiet: Vec<i16> = tone_at(1000.0, 0.25).iter().flat_map(|s| { let v = (*s * 32767.0) as i16; [v, v] }).collect();
        let mut y = vec![0i16; quiet.len()];
        eq.process_i16(&quiet, &mut y);
        assert_eq!(&y[d..], &quiet[..quiet.len() - d], "16-bit audio below the ceiling must pass through untouched");
        assert_eq!(eq.gain_reduction_db(), 0.0, "and the meter must not claim a reduction");

        // Full scale into a -1 dB ceiling: about 1 dB of reduction, not 91.
        eq.reset();
        let loud: Vec<i16> = tone_at(220.0, 1.0).iter().flat_map(|s| { let v = (*s * 32767.0) as i16; [v, v] }).collect();
        let mut y = vec![0i16; loud.len()];
        eq.process_i16(&loud, &mut y);
        let gr = eq.gain_reduction_db() as f64;
        assert!(gr > 0.3 && gr < 2.0, "meter says {gr} dB on a full-scale tone into a -1 dB ceiling");
        let peak = y[9600..].iter().map(|v| (*v as f64).abs()).fold(0.0, f64::max) / 32768.0;
        assert!(peak > 0.8 && peak <= 10f64.powf(-1.0 / 20.0) * 1.04, "16-bit peak {peak}, should sit just under the ceiling");
    }

    #[test]
    fn limiter_input_is_clamped_rather_than_trusted() {
        let mut eq = Equalizer::new(48000, 2);
        eq.configure(&[], 0.0, 0.0);
        eq.configure_output(f64::NAN, false, f64::NAN, -5.0, f64::INFINITY);
        let x: Vec<f32> = tone(1000.0).iter().flat_map(|s| [*s, *s]).collect();
        let mut y = vec![0f32; x.len()];
        eq.process_f32(&x, &mut y);
        assert!(y.iter().all(|v| v.is_finite()), "bad settings must not poison the output");
    }

    #[test]
    fn presets_are_sane_and_flat_really_is_flat() {
        let presets = eq_presets();
        assert!(presets.iter().any(|p| p.name == "Flat" && p.bands.is_empty()));
        let mut names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), presets.len(), "preset names are unique");

        for p in &presets {
            assert!((-12.0..=0.0).contains(&p.preamp_db), "{} pre-amp {}", p.name, p.preamp_db);
            let boost = p.bands.iter().fold(0f32, |m, b| m.max(b.gain_db));
            assert!(p.preamp_db <= -boost, "{} boosts {boost} dB but only pays back {}", p.name, p.preamp_db);
            let mut eq = Equalizer::new(48000, 2);
            let bands: Vec<Band> = p.bands.iter().map(Band::from).collect();
            for band in &bands {
                assert!((20.0..=20000.0).contains(&band.freq), "{} band at {} Hz", p.name, band.freq);
                assert!(band.q > 0.0 && band.q <= 10.0, "{} band Q {}", p.name, band.q);
                assert!(band.gain_db.abs() <= 12.0, "{} band gain {}", p.name, band.gain_db);
            }
            eq.configure(&bands, p.preamp_db as f64, 0.0);
            assert_eq!(eq.is_identity(), p.bands.is_empty() && p.preamp_db == 0.0, "{}", p.name);
            // With its own pre-amp a preset must stay near unity everywhere, so picking one cannot clip on its own.
            for f in [30.0, 60.0, 100.0, 220.0, 440.0, 1000.0, 2500.0, 4000.0, 8000.0, 12000.0] {
                let g = gain_at(&mut eq, f);
                assert!(g.is_finite() && g < 3.5, "{} is {g} dB at {f} Hz", p.name);
            }
        }
    }

    /// The `kind` field crossing the JNI boundary is an `EqKind` ordinal; the two lists must not drift apart.
    #[test]
    fn eq_kind_ordinals_match_the_dsp_codes() {
        for (kind, code) in [
            (EqKind::Peaking, PEAKING),
            (EqKind::LowShelf, LOW_SHELF),
            (EqKind::HighShelf, HIGH_SHELF),
            (EqKind::LowPass, LOW_PASS),
            (EqKind::HighPass, HIGH_PASS),
            (EqKind::BandPass, BAND_PASS),
            (EqKind::Notch, NOTCH),
            (EqKind::AllPass, ALL_PASS),
            (EqKind::LowShelfSlope, LOW_SHELF_SLOPE),
            (EqKind::HighShelfSlope, HIGH_SHELF_SLOPE),
        ] {
            assert_eq!(kind as i32, code, "{kind:?}");
        }
    }
}
