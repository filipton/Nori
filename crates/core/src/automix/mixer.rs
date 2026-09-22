//! The transition mixer: two PCM streams of equal length in (the outgoing track's tail, the incoming track's start,
//! already time-stretched when the plan asks for it), one out. It keeps its own clock from the first frame of the
//! transition, so every curve lands on the planned sample:
//!
//! - gain curves (equal power, linear or sin²) per deck, plus a loudness trim on the incoming deck that glides back
//!   to 0 dB over the last quarter of the transition;
//! - the bass swap: both decks run through a 4th-order Linkwitz-Riley high-pass, and a raised-cosine crossfade
//!   between dry and high-passed moves the lows from the outgoing to the incoming track over `bass_swap_len`;
//! - the low-pass sweep on the outgoing deck, exponential in frequency, coefficients updated every 16 frames.
//!
//! Past the end of the transition the output is the incoming stream untouched, so a late call does no harm.
//! The per-buffer call allocates nothing and may run in place (`dest` equal to either input).

use jni::objects::{JByteBuffer, JClass, JFloatArray};
use jni::sys::{jint, jlong};
use jni::JNIEnv;
use parking_lot::Mutex;

use crate::{FadeCurve, TransitionPlan};

const MAX_CHANNELS: usize = 8;
/// Frames between low-pass coefficient updates.
const LP_STEP: u64 = 16;
/// The sweep fades its filter in over this long, so starting the filter never clicks.
const LP_ENTRY_MS: f64 = 50.0;

/// Layout of the flat parameter array (`automix_mixer_params`, JNI `configure`). Times in ms, relative to the start.
pub mod param {
    pub const DURATION: usize = 0;
    pub const CURVE: usize = 1;
    pub const OUT_FADE_START: usize = 2;
    pub const OUT_FADE_END: usize = 3;
    pub const IN_FADE_START: usize = 4;
    pub const IN_FADE_END: usize = 5;
    pub const OUT_GAIN_DB: usize = 6;
    pub const IN_GAIN_DB: usize = 7;
    pub const SWAP_START: usize = 8;
    pub const SWAP_LEN: usize = 9;
    pub const BASS_CUT_HZ: usize = 10;
    pub const LP_START: usize = 11;
    pub const LP_END: usize = 12;
    pub const LP_FROM_HZ: usize = 13;
    pub const LP_TO_HZ: usize = 14;
    /// Beat-synced echo on the outgoing deck: delay ms (one outgoing beat), feedback 0..1, wet dB.
    /// Delay -1 means off. Appended after the original 15; short arrays leave the echo off.
    pub const ECHO_DELAY: usize = 15;
    pub const ECHO_FB: usize = 16;
    pub const ECHO_WET: usize = 17;
    /// High-pass sweep on the outgoing deck (DJ filter-open). Start -1 means off.
    pub const HP_START: usize = 18;
    pub const HP_END: usize = 19;
    pub const HP_FROM_HZ: usize = 20;
    pub const HP_TO_HZ: usize = 21;
    pub const COUNT: usize = 22;
}

/// The plan as the flat array the mixer takes.
pub fn params(plan: &TransitionPlan) -> Vec<f32> {
    let mut p = vec![0f32; param::COUNT];
    p[param::DURATION] = plan.duration_ms as f32;
    p[param::CURVE] = plan.fade_curve as i32 as f32;
    p[param::OUT_FADE_START] = plan.out_fade_start_ms as f32;
    p[param::OUT_FADE_END] = plan.out_fade_end_ms as f32;
    p[param::IN_FADE_START] = plan.in_fade_start_ms as f32;
    p[param::IN_FADE_END] = plan.in_fade_end_ms as f32;
    p[param::OUT_GAIN_DB] = plan.out_gain_db;
    p[param::IN_GAIN_DB] = plan.in_gain_db;
    p[param::SWAP_START] = plan.bass_swap_ms as f32;
    p[param::SWAP_LEN] = plan.bass_swap_len_ms as f32;
    p[param::BASS_CUT_HZ] = plan.bass_cut_hz;
    p[param::LP_START] = plan.filter_start_ms as f32;
    p[param::LP_END] = plan.filter_end_ms as f32;
    p[param::LP_FROM_HZ] = plan.filter_from_hz;
    p[param::LP_TO_HZ] = plan.filter_to_hz;
    p[param::ECHO_DELAY] = plan.echo_delay_ms as f32;
    p[param::ECHO_FB] = plan.echo_feedback;
    p[param::ECHO_WET] = plan.echo_wet_db;
    p[param::HP_START] = plan.hp_start_ms as f32;
    p[param::HP_END] = plan.hp_end_ms as f32;
    p[param::HP_FROM_HZ] = plan.hp_from_hz;
    p[param::HP_TO_HZ] = plan.hp_to_hz;
    p
}

#[derive(Clone, Copy, Default)]
struct Coef {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Coef {
    fn low_pass(rate: f64, hz: f64) -> Self {
        Self::rbj(rate, hz, true)
    }

    fn high_pass(rate: f64, hz: f64) -> Self {
        Self::rbj(rate, hz, false)
    }

    /// Butterworth (Q = 1/√2) RBJ low- or high-pass.
    fn rbj(rate: f64, hz: f64, low: bool) -> Self {
        let hz = hz.clamp(10.0, rate * 0.49);
        let w = 2.0 * std::f64::consts::PI * hz / rate;
        let (s, c) = w.sin_cos();
        let alpha = s / std::f64::consts::SQRT_2;
        let a0 = 1.0 + alpha;
        let (b0, b1) = if low { ((1.0 - c) / 2.0, 1.0 - c) } else { ((1.0 + c) / 2.0, -(1.0 + c)) };
        Coef { b0: b0 / a0, b1: b1 / a0, b2: b0 / a0, a1: -2.0 * c / a0, a2: (1.0 - alpha) / a0 }
    }

    #[inline]
    fn run(&self, s: &mut [f64; 2], x: f64) -> f64 {
        let y = self.b0 * x + s[0];
        s[0] = self.b1 * x - self.a1 * y + s[1];
        s[1] = self.b2 * x - self.a2 * y;
        y
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
struct Span {
    start: u64,
    end: u64,
}

impl Span {
    /// 0 before, 1 after, linear in between.
    #[inline]
    fn progress(&self, p: u64) -> f64 {
        if p <= self.start {
            0.0
        } else if p >= self.end {
            1.0
        } else {
            (p - self.start) as f64 / (self.end - self.start) as f64
        }
    }
}

pub struct Mixer {
    rate: f64,
    ch: usize,
    pos: u64,
    len: u64,
    curve: FadeCurve,
    out_fade: Span,
    in_fade: Span,
    out_gain: f64,
    in_gain_db: f64,
    trim_glide: Span,
    swap: Option<Span>,
    hp: Coef,
    /// Two cascaded sections per deck per channel: [deck][section][channel].
    hp_state: [[[[f64; 2]; MAX_CHANNELS]; 2]; 2],
    /// DJ filter-open: high-pass sweep on the outgoing deck only (separate from the bass-swap HP).
    hp_sweep: Option<(Span, f64, f64)>,
    hp_sweep_coef: Coef,
    hp_sweep_state: [[[f64; 2]; MAX_CHANNELS]; 2],
    hp_sweep_from: u64,
    lp: Option<(Span, f64, f64)>,
    lp_coef: Coef,
    lp_state: [[[f64; 2]; MAX_CHANNELS]; 2],
    lp_entry: u64,
    /// Where the sweep's fade-in is measured from: the sweep's start, or a seek that landed inside it.
    lp_from: u64,
    /// Beat-synced echo on the outgoing deck: delay frames, feedback, wet gain, and the ring per channel.
    echo: Option<(usize, f64, f64)>,
    echo_buf: Vec<f64>,
    echo_pos: usize,
}

impl Mixer {
    pub fn new(rate: u32, channels: usize) -> Self {
        let mut m = Mixer {
            rate: rate.max(1) as f64,
            ch: channels.clamp(1, MAX_CHANNELS),
            pos: 0,
            len: 0,
            curve: FadeCurve::EqualPower,
            out_fade: Span { start: 0, end: 0 },
            in_fade: Span { start: 0, end: 0 },
            out_gain: 1.0,
            in_gain_db: 0.0,
            trim_glide: Span { start: 0, end: 0 },
            swap: None,
            hp: Coef::default(),
            hp_state: [[[[0.0; 2]; MAX_CHANNELS]; 2]; 2],
            hp_sweep: None,
            hp_sweep_coef: Coef::default(),
            hp_sweep_state: [[[0.0; 2]; MAX_CHANNELS]; 2],
            hp_sweep_from: 0,
            lp: None,
            lp_coef: Coef::default(),
            lp_state: [[[0.0; 2]; MAX_CHANNELS]; 2],
            lp_entry: 0,
            lp_from: 0,
            echo: None,
            echo_buf: Vec::new(),
            echo_pos: 0,
        };
        m.configure(&[0.0; param::COUNT]);
        m
    }

    /// Takes the flat parameter array (see `param`); short or bad arrays leave a plain equal-power fade.
    /// Restarts the clock.
    pub fn configure(&mut self, p: &[f32]) {
        let get = |i: usize| p.get(i).copied().filter(|v| v.is_finite()).unwrap_or(-1.0) as f64;
        let frames = |ms: f64| (ms.max(0.0) * self.rate / 1000.0).round() as u64;
        self.pos = 0;
        self.len = frames(get(param::DURATION));
        self.curve = match get(param::CURVE) as i32 {
            1 => FadeCurve::Linear,
            2 => FadeCurve::SineSquared,
            _ => FadeCurve::EqualPower,
        };
        let span = |a: f64, b: f64, len: u64| -> Span {
            if a < 0.0 || b < 0.0 || b < a {
                Span { start: 0, end: len }
            } else {
                Span { start: frames(a).min(len), end: frames(b).min(len) }
            }
        };
        self.out_fade = span(get(param::OUT_FADE_START), get(param::OUT_FADE_END), self.len);
        self.in_fade = span(get(param::IN_FADE_START), get(param::IN_FADE_END), self.len);
        let gain = |i: usize| p.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0) as f64;
        self.out_gain = 10f64.powf(gain(param::OUT_GAIN_DB).clamp(-24.0, 12.0) / 20.0);
        self.in_gain_db = gain(param::IN_GAIN_DB).clamp(-12.0, 12.0);
        self.trim_glide = Span { start: self.len * 3 / 4, end: self.len };
        let (ss, sl) = (get(param::SWAP_START), get(param::SWAP_LEN));
        self.swap = (ss >= 0.0).then(|| Span { start: frames(ss).min(self.len), end: (frames(ss) + frames(sl.max(1.0))).min(self.len.max(1)) });
        self.hp = Coef::high_pass(self.rate, if get(param::BASS_CUT_HZ) > 0.0 { get(param::BASS_CUT_HZ) } else { 180.0 });
        let (hs, he, hf, ht) = (get(param::HP_START), get(param::HP_END), get(param::HP_FROM_HZ), get(param::HP_TO_HZ));
        self.hp_sweep = (hs >= 0.0 && he >= hs && hf > 0.0 && ht > 0.0).then(|| {
            (span(hs, he, self.len), hf.min(self.rate * 0.45), ht.min(self.rate * 0.45))
        });
        self.hp_sweep_from = self.hp_sweep.map_or(0, |(s, _, _)| s.start);
        if let Some((_, from, _)) = self.hp_sweep {
            self.hp_sweep_coef = Coef::high_pass(self.rate, from);
        }
        let (ls, le, lf, lt) = (get(param::LP_START), get(param::LP_END), get(param::LP_FROM_HZ), get(param::LP_TO_HZ));
        self.lp = (ls >= 0.0 && le >= ls && lf > 0.0 && lt > 0.0).then(|| (span(ls, le, self.len), lf.min(self.rate * 0.45), lt.min(self.rate * 0.45)));
        self.lp_entry = frames(LP_ENTRY_MS).max(1);
        self.lp_from = self.lp.map_or(0, |(s, _, _)| s.start);
        // One outgoing beat of delay, capped at a second (about 1.5 MB float at 48 kHz stereo, reused across transitions).
        let (ed, ef, ew) = (get(param::ECHO_DELAY), p.get(param::ECHO_FB).copied().unwrap_or(0.5), p.get(param::ECHO_WET).copied().unwrap_or(-6.0));
        self.echo = (ed >= 1.0).then(|| {
            let d = (frames(ed).max(1).min(self.rate as u64) as usize).max(1);
            (d, (ef as f64).clamp(0.0, 0.9), 10f64.powf((ew as f64).clamp(-24.0, 0.0) / 20.0))
        });
        if let Some((d, _, _)) = self.echo {
            let need = d * self.ch;
            if self.echo_buf.len() < need {
                self.echo_buf.resize(need, 0.0);
            }
            self.echo_buf[..need].fill(0.0);
            self.echo_pos = 0;
        }
        self.hp_state = [[[[0.0; 2]; MAX_CHANNELS]; 2]; 2];
        self.hp_sweep_state = [[[0.0; 2]; MAX_CHANNELS]; 2];
        self.lp_state = [[[0.0; 2]; MAX_CHANNELS]; 2];
    }

    pub fn position(&self) -> u64 {
        self.pos
    }

    /// Starts the clock `frames` into the transition instead of at its first sample: a listener who
    /// seeks into the overlap hears the curves where they would have been, not a fresh fade with too
    /// little tail left to finish it. Filters start clean from here; the sweep fades in from here too.
    pub fn seek(&mut self, frames: u64) {
        self.pos = frames.min(self.len);
        if let Some((span, from, to)) = self.lp {
            if self.pos >= span.start {
                let hz = from * (to / from).powf(span.progress(self.pos));
                self.lp_coef = Coef::low_pass(self.rate, hz);
                self.lp_from = self.pos;
            }
        }
        if let Some((span, from, to)) = self.hp_sweep {
            if self.pos >= span.start {
                let hz = from * (to / from).powf(span.progress(self.pos));
                self.hp_sweep_coef = Coef::high_pass(self.rate, hz);
                self.hp_sweep_from = self.pos;
            }
        }
    }

    pub fn done(&self) -> bool {
        self.pos >= self.len
    }

    #[inline]
    fn fade(curve: FadeCurve, x: f64, down: bool) -> f64 {
        let x = if down { 1.0 - x } else { x };
        match curve {
            FadeCurve::EqualPower => (x * std::f64::consts::FRAC_PI_2).sin(),
            FadeCurve::Linear => x,
            FadeCurve::SineSquared => (x * std::f64::consts::FRAC_PI_2).sin().powi(2),
        }
    }

    /// Mixes `frames` frames. Raw pointers so `dst` may alias either input: each frame is read before it is written.
    ///
    /// # Safety
    /// All three pointers must be valid for `frames * channels` elements.
    unsafe fn run<T: Copy>(&mut self, out: *const T, inc: *const T, dst: *mut T, frames: usize, load: impl Fn(T) -> f64, store: impl Fn(f64) -> T) {
        let ch = self.ch;
        let mut xo = [0f64; MAX_CHANNELS];
        let mut xi = [0f64; MAX_CHANNELS];
        for f in 0..frames {
            for c in 0..ch {
                xo[c] = load(*out.add(f * ch + c));
                xi[c] = load(*inc.add(f * ch + c));
            }
            let p = self.pos;
            if p >= self.len {
                for c in 0..ch {
                    *dst.add(f * ch + c) = store(xi[c]);
                }
                self.pos += 1;
                continue;
            }
            let g_out = Self::fade(self.curve, self.out_fade.progress(p), true) * self.out_gain;
            let trim = self.in_gain_db * (1.0 - self.trim_glide.progress(p));
            let g_in = Self::fade(self.curve, self.in_fade.progress(p), false) * 10f64.powf(trim / 20.0);
            // Raised-cosine swap: `k_in` is how much of the incoming lows is still cut, `k_out` how much of the outgoing.
            let (k_in, k_out) = match self.swap {
                Some(s) => {
                    let x = 0.5 - 0.5 * (s.progress(p) * std::f64::consts::PI).cos();
                    (1.0 - x, x)
                }
                None => (0.0, 0.0),
            };
            let lp_wet = match self.lp {
                Some((span, from, to)) if p >= span.start => {
                    if (p - span.start) % LP_STEP == 0 {
                        let hz = from * (to / from).powf(span.progress(p));
                        self.lp_coef = Coef::low_pass(self.rate, hz);
                    }
                    (p.saturating_sub(self.lp_from) as f64 / self.lp_entry as f64).min(1.0)
                }
                _ => 0.0,
            };
            let hp_wet = match self.hp_sweep {
                Some((span, from, to)) if p >= span.start => {
                    if (p - span.start) % LP_STEP == 0 {
                        let hz = from * (to / from).powf(span.progress(p));
                        self.hp_sweep_coef = Coef::high_pass(self.rate, hz);
                    }
                    (p.saturating_sub(self.hp_sweep_from) as f64 / self.lp_entry as f64).min(1.0)
                }
                _ => 0.0,
            };
            for c in 0..ch {
                let mut o = xo[c];
                if hp_wet > 0.0 {
                    let h = self.hp_sweep_coef.run(&mut self.hp_sweep_state[0][c], o);
                    let h = self.hp_sweep_coef.run(&mut self.hp_sweep_state[1][c], h);
                    o = o * (1.0 - hp_wet) + h * hp_wet;
                }
                if lp_wet > 0.0 {
                    let l = self.lp_coef.run(&mut self.lp_state[0][c], o);
                    let l = self.lp_coef.run(&mut self.lp_state[1][c], l);
                    o = o * (1.0 - lp_wet) + l * lp_wet;
                }
                if let Some((d, fb, wet)) = self.echo {
                    // Post-fader send: the dry deck fades, the repeats decay on their own inside the overlap.
                    let idx = self.echo_pos * ch + c;
                    let rep = self.echo_buf[idx];
                    self.echo_buf[idx] = o + rep * fb;
                    if c + 1 == ch {
                        self.echo_pos = (self.echo_pos + 1) % d;
                    }
                    o = o * g_out + rep * wet;
                    *dst.add(f * ch + c) = store(o + xi[c] * g_in);
                    continue;
                }
                let mut i = xi[c];
                if self.swap.is_some() {
                    let ho = self.hp.run(&mut self.hp_state[0][0][c], o);
                    let ho = self.hp.run(&mut self.hp_state[0][1][c], ho);
                    let hi = self.hp.run(&mut self.hp_state[1][0][c], i);
                    let hi = self.hp.run(&mut self.hp_state[1][1][c], hi);
                    o = o * (1.0 - k_out) + ho * k_out;
                    i = i * (1.0 - k_in) + hi * k_in;
                }
                *dst.add(f * ch + c) = store(o * g_out + i * g_in);
            }
            self.pos += 1;
        }
    }

    pub fn process_f32(&mut self, out: &[f32], inc: &[f32], dst: &mut [f32]) {
        let frames = out.len().min(inc.len()).min(dst.len()) / self.ch;
        unsafe { self.run(out.as_ptr(), inc.as_ptr(), dst.as_mut_ptr(), frames, |x| x as f64, |y| y as f32) }
    }

    pub fn process_i16(&mut self, out: &[i16], inc: &[i16], dst: &mut [i16]) {
        let frames = out.len().min(inc.len()).min(dst.len()) / self.ch;
        unsafe { self.run(out.as_ptr(), inc.as_ptr(), dst.as_mut_ptr(), frames, |x| x as f64, |y| y.round().clamp(-32768.0, 32767.0) as i16) }
    }
}

// ---- JNI: dev.nori.music.playback.AutoMixMixer ----------------------------------------------------------------

const PCM_16: jint = 2;
const PCM_FLOAT: jint = 4;

fn mixer<'a>(h: jlong) -> Option<&'a Mutex<Mixer>> {
    (h != 0).then(|| unsafe { &*(h as *const Mutex<Mixer>) })
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_AutoMixMixer_create(_: JNIEnv, _: JClass, rate: jint, channels: jint) -> jlong {
    Box::into_raw(Box::new(Mutex::new(Mixer::new(rate.max(1) as u32, channels.max(1) as usize)))) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_AutoMixMixer_destroy(_: JNIEnv, _: JClass, h: jlong) {
    if h != 0 {
        drop(unsafe { Box::from_raw(h as *mut Mutex<Mixer>) });
    }
}

/// `params` is `automix_mixer_params(plan)`. Restarts the transition clock at 0.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_AutoMixMixer_configure(env: JNIEnv, _: JClass, h: jlong, params: JFloatArray) {
    let Some(m) = mixer(h) else { return };
    let mut p = [0f32; param::COUNT];
    let n = (env.get_array_length(&params).unwrap_or(0).max(0) as usize).min(param::COUNT);
    if env.get_float_array_region(&params, 0, &mut p[..n]).is_err() {
        return;
    }
    m.lock().configure(&p[..n]);
}

/// Frames mixed since `configure`.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_AutoMixMixer_position(_: JNIEnv, _: JClass, h: jlong) -> jlong {
    mixer(h).map_or(0, |m| m.lock().position() as jlong)
}

/// Moves the transition clock to `frames` in; see [Mixer::seek].
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_AutoMixMixer_seek(_: JNIEnv, _: JClass, h: jlong, frames: jlong) {
    if let Some(m) = mixer(h) {
        m.lock().seek(frames.max(0) as u64);
    }
}

/// Mixes `frames` frames of `outgoing[out_pos..]` and `incoming[in_pos..]` into `dest[dest_pos..]` (byte positions;
/// all direct buffers, `dest` may be either input). Returns false when the buffers cannot be used.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_AutoMixMixer_process(
    env: JNIEnv, _: JClass, h: jlong, outgoing: JByteBuffer, out_pos: jint, incoming: JByteBuffer, in_pos: jint, dest: JByteBuffer, dest_pos: jint,
    frames: jint, encoding: jint,
) -> bool {
    let (Ok(o), Ok(i), Ok(d)) =
        (env.get_direct_buffer_address(&outgoing), env.get_direct_buffer_address(&incoming), env.get_direct_buffer_address(&dest))
    else {
        return false;
    };
    let Some(m) = mixer(h) else { return false };
    if o.is_null() || i.is_null() || d.is_null() || frames < 0 {
        return false;
    }
    let (Ok(ocap), Ok(icap), Ok(dcap)) =
        (env.get_direct_buffer_capacity(&outgoing), env.get_direct_buffer_capacity(&incoming), env.get_direct_buffer_capacity(&dest))
    else {
        return false;
    };
    let mut m = m.lock();
    let width = match encoding {
        PCM_16 => 2,
        PCM_FLOAT => 4,
        _ => return false,
    };
    let bytes = frames as usize * m.ch * width;
    if out_pos < 0 || in_pos < 0 || dest_pos < 0 || out_pos as usize + bytes > ocap || in_pos as usize + bytes > icap || dest_pos as usize + bytes > dcap {
        return false;
    }
    let (o, i, d) = unsafe { (o.add(out_pos as usize), i.add(in_pos as usize), d.add(dest_pos as usize)) };
    if o as usize % width != 0 || i as usize % width != 0 || d as usize % width != 0 {
        return false;
    }
    let n = frames as usize;
    unsafe {
        if width == 2 {
            m.run(o as *const i16, i as *const i16, d as *mut i16, n, |x| x as f64, |y| y.round().clamp(-32768.0, 32767.0) as i16);
        } else {
            m.run(o as *const f32, i as *const f32, d as *mut f32, n, |x| x as f64, |y| y as f32);
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TransitionKind;

    const RATE: f64 = 48000.0;

    fn plan() -> TransitionPlan {
        TransitionPlan {
            kind: TransitionKind::EqualPowerFade,
            out_start_ms: 0,
            in_start_ms: 0,
            duration_ms: 1000,
            tempo_ratio: 1.0,
            tempo_ramp_beats: 0,
            tempo_ramp_ms: 0,
            keep_pitch: true,
            fade_curve: FadeCurve::EqualPower,
            out_fade_start_ms: 0,
            out_fade_end_ms: 1000,
            in_fade_start_ms: 0,
            in_fade_end_ms: 1000,
            out_gain_db: 0.0,
            in_gain_db: 0.0,
            bass_swap_ms: -1,
            bass_swap_len_ms: 0,
            bass_cut_hz: 180.0,
            filter_start_ms: -1,
            filter_end_ms: -1,
            filter_from_hz: 0.0,
            filter_to_hz: 0.0,
            echo_delay_ms: -1,
            echo_feedback: 0.5,
            echo_wet_db: -6.0,
            out_loop_ms: -1,
            in_loop_ms: -1,
            hp_start_ms: -1,
            hp_end_ms: -1,
            hp_from_hz: 0.0,
            hp_to_hz: 0.0,
            reason: String::new(),
        }
    }

    fn sine(freq: f64, frames: usize) -> Vec<f32> {
        (0..frames).map(|i| (0.5 * (2.0 * std::f64::consts::PI * freq * i as f64 / RATE).sin()) as f32).collect()
    }

    fn rms(x: &[f32]) -> f64 {
        (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
    }

    /// RMS of each 10 ms slice of the mono output.
    fn envelope(y: &[f32]) -> Vec<f64> {
        y.chunks(480).map(rms).collect()
    }

    #[test]
    fn equal_power_keeps_uncorrelated_power_and_ends_on_the_incoming_track() {
        let mut m = Mixer::new(RATE as u32, 1);
        m.configure(&params(&plan()));
        let a = sine(440.0, 72000);
        let b = sine(1234.5, 72000);
        let mut y = vec![0f32; a.len()];
        m.process_f32(&a, &b, &mut y);
        let env = envelope(&y[..48000]);
        let r = rms(&a);
        for (i, e) in env.iter().enumerate() {
            assert!((e / r - 1.0).abs() < 0.08, "slice {i}: {e} vs {r}");
        }
        assert_eq!(&y[48000..], &b[48000..], "after the fade it is the incoming track, bit for bit");
        assert!(m.done());
        assert!(y[..10].iter().zip(&a).all(|(y, a)| (y - a).abs() < 1e-3), "starts on the outgoing track");
    }

    #[test]
    fn sine_squared_keeps_correlated_amplitude_and_runs_in_place() {
        let mut p = plan();
        p.fade_curve = FadeCurve::SineSquared;
        let mut m = Mixer::new(RATE as u32, 2);
        m.configure(&params(&p));
        let a: Vec<f32> = sine(440.0, 48000).iter().flat_map(|v| [*v, *v]).collect();
        let mut y = a.clone();
        let b = a.clone();
        let out_ptr = y.as_ptr();
        unsafe { m.run(out_ptr, b.as_ptr(), y.as_mut_ptr(), 48000, |x| x as f64, |v| v as f32) };
        for (u, v) in y.iter().zip(&a) {
            assert!((u - v).abs() < 1e-5, "sin² + cos² of the same signal is the signal");
        }
    }

    #[test]
    fn a_seek_into_the_mix_picks_the_curves_up_where_they_would_be() {
        let mut p = plan();
        p.fade_curve = FadeCurve::Linear;
        let ones = vec![1f32; 48000];
        let zeros = vec![0f32; 48000];
        // The whole transition, as a listener who played into it hears it.
        let mut m = Mixer::new(RATE as u32, 1);
        m.configure(&params(&p));
        let mut whole = vec![0f32; 48000];
        m.process_f32(&ones, &zeros, &mut whole);
        // The same transition entered 300 ms in: what comes out is the tail of the one above.
        let mut m = Mixer::new(RATE as u32, 1);
        m.configure(&params(&p));
        m.seek(14400);
        assert_eq!(m.position(), 14400);
        let mut late = vec![0f32; 48000 - 14400];
        m.process_f32(&ones[14400..], &zeros[14400..], &mut late);
        for (i, (l, w)) in late.iter().zip(&whole[14400..]).enumerate() {
            assert!((l - w).abs() < 1e-5, "frame {i}: {l} vs {w}");
        }
        assert!(m.done());
    }

    #[test]
    fn fades_follow_their_windows_and_trims() {
        let mut p = plan();
        p.fade_curve = FadeCurve::Linear;
        (p.out_fade_start_ms, p.out_fade_end_ms, p.in_fade_start_ms, p.in_fade_end_ms) = (500, 1000, 0, 500);
        p.in_gain_db = -6.0;
        let mut m = Mixer::new(RATE as u32, 1);
        m.configure(&params(&p));
        let ones = vec![1f32; 48000];
        let zeros = vec![0f32; 48000];
        let mut y = vec![0f32; 48000];
        m.process_f32(&ones, &zeros, &mut y);
        assert!((y[12000] - 1.0).abs() < 1e-6 && (y[36000] - 0.5).abs() < 1e-3, "outgoing holds, then falls linearly");
        m.configure(&params(&p));
        m.process_f32(&zeros, &ones, &mut y);
        assert!((y[12000] as f64 - 0.5 * 0.501).abs() < 2e-3, "half way up, at -6 dB: {}", y[12000]);
        assert!((y[30000] as f64 - 0.501).abs() < 2e-3, "fully up, still trimmed: {}", y[30000]);
        assert!((y[47999] - 1.0).abs() < 2e-3, "the trim has glided back to 0 dB by the end: {}", y[47999]);
    }

    #[test]
    fn the_bass_swap_moves_the_lows_from_one_deck_to_the_other() {
        let mut p = plan();
        p.fade_curve = FadeCurve::Linear;
        // Both decks at full gain the whole time, so only the filters act.
        (p.out_fade_start_ms, p.out_fade_end_ms, p.in_fade_start_ms, p.in_fade_end_ms) = (1000, 1000, 0, 0);
        (p.bass_swap_ms, p.bass_swap_len_ms, p.bass_cut_hz) = (500, 100, 200.0);
        let mut m = Mixer::new(RATE as u32, 1);
        m.configure(&params(&p));
        let bass = sine(50.0, 48000);
        let zeros = vec![0f32; 48000];
        let mut y = vec![0f32; 48000];
        m.process_f32(&bass, &zeros, &mut y);
        let r = rms(&bass[..4800]);
        assert!((rms(&y[12000..24000]) / r - 1.0).abs() < 0.02, "outgoing bass untouched before the swap");
        assert!(rms(&y[30000..42000]) / r < 0.03, "outgoing bass gone after it");
        m.configure(&params(&p));
        m.process_f32(&zeros, &bass, &mut y);
        assert!(rms(&y[12000..24000]) / r < 0.03, "incoming bass cut before the swap");
        assert!((rms(&y[30000..42000]) / r - 1.0).abs() < 0.02, "and whole after it");
        // Highs pass on both decks throughout.
        let hi = sine(3000.0, 48000);
        m.configure(&params(&p));
        m.process_f32(&zeros, &hi, &mut y);
        assert!((rms(&y[2400..24000]) / rms(&hi) - 1.0).abs() < 0.03);
    }

    #[test]
    fn the_echo_repeats_the_outgoing_deck_on_the_beat() {
        let mut p = plan();
        (p.out_fade_start_ms, p.out_fade_end_ms, p.in_fade_start_ms, p.in_fade_end_ms) = (0, 480, 480, 960);
        (p.echo_delay_ms, p.echo_feedback, p.echo_wet_db) = (240, 0.5, 0.0);
        let mut m = Mixer::new(RATE as u32, 1);
        m.configure(&params(&p));
        // One click, then silence: the repeats land one delay apart, halving each time.
        let mut click = vec![0f32; 48000];
        click[0] = 1.0;
        let zeros = vec![0f32; 48000];
        let mut y = vec![0f32; 48000];
        m.process_f32(&click, &zeros, &mut y);
        let d = (0.24 * RATE) as usize;
        let at = |n: usize| y[n * d..n * d + 24].iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        assert!((at(0) - 1.0).abs() < 0.01, "dry click: {}", at(0));
        assert!((at(1) - 1.0).abs() < 0.1, "first repeat at full wet: {}", at(1));
        assert!((at(2) - 0.5).abs() < 0.1, "second repeat halved: {}", at(2));
        assert!((at(3) - 0.25).abs() < 0.1, "third repeat quartered: {}", at(3));
    }

    #[test]
    fn the_low_pass_sweep_darkens_the_outgoing_track() {
        let mut p = plan();
        (p.out_fade_start_ms, p.out_fade_end_ms, p.in_fade_start_ms, p.in_fade_end_ms) = (1000, 1000, 1000, 1000);
        (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) = (200, 600, 20000.0, 300.0);
        let mut m = Mixer::new(RATE as u32, 1);
        m.configure(&params(&p));
        let hi = sine(4000.0, 48000);
        let zeros = vec![0f32; 48000];
        let mut y = vec![0f32; 48000];
        m.process_f32(&hi, &zeros, &mut y);
        let r = rms(&hi);
        assert!((rms(&y[..9000]) / r - 1.0).abs() < 0.01, "untouched before the sweep");
        assert!(rms(&y[31000..47000]) / r < 0.02, "4 kHz is 24 dB+ under a 300 Hz 4th-order low-pass");
        let jump = y.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
        assert!(jump < 0.5 * 2.0 * std::f32::consts::PI * 4000.0 / 48000.0 * 1.1, "no clicks: {jump}");
    }

    #[test]
    fn bad_parameters_fall_back_to_a_plain_fade() {
        let mut m = Mixer::new(44100, 2);
        m.configure(&[f32::NAN, 7.0]);
        assert!(m.done(), "no duration, nothing to mix");
        let a = vec![0.25f32; 64];
        let b = vec![0.5f32; 64];
        let mut y = vec![0f32; 64];
        m.process_f32(&a, &b, &mut y);
        assert_eq!(y, b);
    }

    /// Mixer cost, 44.1 kHz stereo, everything on. `cargo test --release -p norimusic mixer_cost -- --ignored --nocapture`
    #[test]
    #[ignore]
    fn mixer_cost() {
        let mut p = plan();
        p.duration_ms = 30_000;
        (p.bass_swap_ms, p.bass_swap_len_ms) = (15_000, 500);
        (p.filter_start_ms, p.filter_end_ms, p.filter_from_hz, p.filter_to_hz) = (15_000, 30_000, 18_000.0, 300.0);
        let mut m = Mixer::new(44100, 2);
        m.configure(&params(&p));
        let a: Vec<i16> = (0..44100 * 30 * 2).map(|i| ((i * 7919) % 20000) as i16 - 10000).collect();
        let b = a.clone();
        let mut y = vec![0i16; a.len()];
        let t = std::time::Instant::now();
        for ((a, b), y) in a.chunks(4096).zip(b.chunks(4096)).zip(y.chunks_mut(4096)) {
            m.process_i16(a, b, y);
        }
        let per_s = t.elapsed().as_secs_f64() / 30.0;
        println!("mixer: {:.3} ms CPU per second of audio ({:.3} % of one core)", per_s * 1000.0, per_s * 100.0);
    }
}
