//! Conversion between sample rates (and widths) for transitions whose two sides disagree: the
//! incoming side is converted to the outgoing side's format, so the mix itself always runs at one
//! rate. Cubic (Catmull-Rom) interpolation per channel; the fractional position and the last two
//! input frames carry across buffers. Mono and stereo both ways; anything else refuses (JNI 0).
//!
//! Two input frames' worth of history carry across buffers; neighbours past the buffer end clamp
//! to its last frame, so no content is ever skipped at a buffer edge.

use jni::{
    objects::{JByteBuffer, JClass},
    sys::{jint, jlong},
    JNIEnv,
};
use parking_lot::Mutex;

use super::{PCM_16, PCM_FLOAT};

pub struct Resampler {
    in_rate: f64,
    out_rate: f64,
    in_ch: usize,
    out_ch: usize,
    /// Input frames per output frame.
    step: f64,
    /// Fractional input-frame position of the next output frame (negative against `hist`).
    pos: f64,
    /// The latest input frames heard, input-channel layout, at most two frames.
    hist: Vec<f32>,
    /// Decoded input, reused across calls so the steady state allocates nothing.
    scratch: Vec<f32>,
}

impl Resampler {
    pub fn new(in_rate: i32, in_ch: i32, out_rate: i32, out_ch: i32) -> Option<Resampler> {
        if in_rate <= 0 || out_rate <= 0 || in_rate > 384_000 || out_rate > 384_000 {
            return None;
        }
        if !matches!(in_ch, 1 | 2) || !matches!(out_ch, 1 | 2) {
            return None;
        }
        Some(Resampler {
            in_rate: in_rate as f64,
            out_rate: out_rate as f64,
            in_ch: in_ch as usize,
            out_ch: out_ch as usize,
            step: in_rate as f64 / out_rate as f64,
            pos: 0.0,
            hist: Vec::new(),
            scratch: Vec::new(),
        })
    }

    /// One input frame mapped to output channels (the second lane is unused for mono out).
    fn fetch(&self, frames: &[f32], n: usize, idx: i64) -> [f32; 2] {
        let hn = (self.hist.len() / self.in_ch) as i64;
        let c = idx.clamp(-hn, n as i64 - 1);
        let base = if c < 0 {
            &self.hist[(hn + c) as usize * self.in_ch..]
        } else {
            &frames[c as usize * self.in_ch..]
        };
        if self.in_ch == self.out_ch {
            [base[0], if self.out_ch > 1 { base[1] } else { 0.0 }]
        } else if self.in_ch == 1 {
            [base[0], base[0]]
        } else {
            [(base[0] + base[1]) * 0.5, 0.0]
        }
    }

    /// Converts `input` (interleaved, `in_enc`) to the output format, appending to `output`.
    /// Returns (consumed input bytes, produced output bytes), or None when the output does not fit.
    pub fn process(&mut self, input: &[u8], in_enc: i32, output: &mut [u8], out_enc: i32) -> Option<(usize, usize)> {
        let wi = match in_enc {
            PCM_16 => 2,
            PCM_FLOAT => 4,
            _ => return None,
        };
        let wo = match out_enc {
            PCM_16 => 2,
            PCM_FLOAT => 4,
            _ => return None,
        };
        let n = input.len() / (wi * self.in_ch);
        if n == 0 {
            return Some((0, 0));
        }
        let cap = output.len() / (wo * self.out_ch);
        // Frames this call can emit, plus one (the clamp at the edge costs nothing to reserve).
        let need = ((n as f64 - self.pos) / self.step).ceil().max(0.0) as usize + 1;
        if need > cap {
            return None;
        }
        self.scratch.resize(n * self.in_ch, 0.0);
        if wi == 4 {
            for (d, b) in self.scratch.iter_mut().zip(input.chunks_exact(4)) {
                *d = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
        } else {
            for (d, b) in self.scratch.iter_mut().zip(input.chunks_exact(2)) {
                *d = i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0;
            }
        }
        let mut made = 0usize;
        // Every position centred in this buffer is emitted; neighbours past either edge clamp
        // (left to the previous buffer via `hist`, right to this buffer's last frame).
        while self.pos < n as f64 {
            let i = self.pos.floor() as i64;
            let f = self.pos - i as f64;
            let n0 = self.fetch(&self.scratch, n, i - 1);
            let n1 = self.fetch(&self.scratch, n, i);
            let n2 = self.fetch(&self.scratch, n, i + 1);
            let n3 = self.fetch(&self.scratch, n, i + 2);
            for c in 0..self.out_ch {
                let v = cubic(n0[c], n1[c], n2[c], n3[c], f);
                let at = (made * self.out_ch + c) * wo;
                if wo == 4 {
                    output[at..at + 4].copy_from_slice(&v.to_le_bytes());
                } else {
                    let q = v.clamp(-1.0, 1.0) * 32768.0;
                    output[at..at + 2].copy_from_slice(&(q.round().clamp(-32768.0, 32767.0) as i16).to_le_bytes());
                }
            }
            made += 1;
            self.pos += self.step;
        }
        self.pos -= n as f64;
        self.hist.extend_from_slice(&self.scratch[n.saturating_sub(2) * self.in_ch..]);
        let keep = 2 * self.in_ch;
        if self.hist.len() > keep {
            self.hist.drain(..self.hist.len() - keep);
        }
        Some((n * wi * self.in_ch, made * wo * self.out_ch))
    }
}

fn cubic(p0: f32, p1: f32, p2: f32, p3: f32, f: f64) -> f32 {
    let f = f as f32;
    0.5 * ((2.0 * p1) + (-p0 + p2) * f + (2.0 * p0 - 5.0 * p1 + 4.0 * p2 - p3) * f * f + (-p0 + 3.0 * p1 - 3.0 * p2 + p3) * f * f * f)
}

// ---- JNI: dev.flint.music.playback.AutoMixResample ---------------------------------------------------------------

fn handle(h: jlong) -> Option<&'static Mutex<Resampler>> {
    (h != 0).then(|| unsafe { &*(h as *const Mutex<Resampler>) })
}

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_AutoMixResample_create(
    _: JNIEnv, _: JClass, in_rate: jint, in_ch: jint, out_rate: jint, out_ch: jint,
) -> jlong {
    Resampler::new(in_rate, in_ch, out_rate, out_ch).map_or(0, |r| Box::into_raw(Box::new(Mutex::new(r))) as jlong)
}

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_AutoMixResample_destroy(_: JNIEnv, _: JClass, h: jlong) {
    if h != 0 {
        drop(unsafe { Box::from_raw(h as *mut Mutex<Resampler>) });
    }
}

/// Converts `in_bytes` of `input[in_pos..]` (`in_enc`) into `output[out_pos..]` (at most `out_cap` bytes,
/// `out_enc`). Returns `(consumed_bytes << 32) | produced_bytes`, or -1 when the buffers cannot be used.
#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_AutoMixResample_process(
    env: JNIEnv, _: JClass, h: jlong, input: JByteBuffer, in_pos: jint, in_bytes: jint, output: JByteBuffer, out_pos: jint, out_cap: jint,
    in_enc: jint, out_enc: jint,
) -> jlong {
    let (Ok(src), Ok(dst)) = (env.get_direct_buffer_address(&input), env.get_direct_buffer_address(&output)) else { return -1 };
    let Some(h) = handle(h) else { return -1 };
    if src.is_null() || dst.is_null() || in_pos < 0 || out_pos < 0 || in_bytes < 0 || out_cap < 0 {
        return -1;
    }
    let (Ok(icap), Ok(ocap)) = (env.get_direct_buffer_capacity(&input), env.get_direct_buffer_capacity(&output)) else { return -1 };
    if in_pos as usize + in_bytes as usize > icap || out_pos as usize + out_cap as usize > ocap {
        return -1;
    }
    let wi = match in_enc {
        PCM_16 => 2,
        PCM_FLOAT => 4,
        _ => return -1,
    };
    let wo = match out_enc {
        PCM_16 => 2,
        PCM_FLOAT => 4,
        _ => return -1,
    };
    if in_pos as usize % wi != 0 || out_pos as usize % wo != 0 {
        return -1;
    }
    let (i, o) = unsafe {
        (
            std::slice::from_raw_parts(src.add(in_pos as usize), in_bytes as usize),
            std::slice::from_raw_parts_mut(dst.add(out_pos as usize), out_cap as usize),
        )
    };
    h.lock().process(i, in_enc, o, out_enc).map_or(-1, |(used, made)| ((used as jlong) << 32) | made as jlong)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(rate: u32, hz: f64, frames: usize) -> Vec<i16> {
        (0..frames).map(|i| ((i as f64 * hz * std::f64::consts::TAU / rate as f64).sin() * 20000.0) as i16).collect()
    }

    fn bytes_of(s: &[i16]) -> Vec<u8> {
        s.iter().flat_map(|v| v.to_le_bytes()).collect()
    }

    fn shorts_of(b: &[u8]) -> Vec<i16> {
        b.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect()
    }

    #[test]
    fn same_rate_is_a_passthrough() {
        let mut r = Resampler::new(44100, 2, 44100, 2).unwrap();
        let l = sine(44100, 440.0, 1000);
        let stereo: Vec<i16> = l.iter().flat_map(|v| [*v, (*v / 2)]).collect();
        let input = bytes_of(&stereo);
        let mut out = vec![0u8; 20000];
        let (used, made) = r.process(&input, PCM_16, &mut out, PCM_16).unwrap();
        assert_eq!(used, input.len());
        // All but the clamped edge come back (the first frame leans on a clamped neighbour).
        let back = shorts_of(&out[..made]);
        assert_eq!(back.len(), stereo.len());
        let err: i64 = back.iter().zip(&stereo).skip(4).map(|(a, b)| (a - b).abs() as i64).sum::<i64>() / (back.len() - 4) as i64;
        assert!(err <= 2, "avg error {err}");
    }

    #[test]
    fn rate_change_keeps_the_tone() {
        let mut r = Resampler::new(44100, 1, 48000, 1).unwrap();
        let input = bytes_of(&sine(44100, 1000.0, 4410));
        let mut out = vec![0u8; 20000];
        let (used, made) = r.process(&input, PCM_16, &mut out, PCM_16).unwrap();
        assert_eq!(used, input.len());
        let frames = made / 2;
        assert!((frames as i64 - 4800).abs() <= 2, "{frames}");
        // A 1 kHz tone resampled still correlates with a 1 kHz tone (no gross aliasing or drift).
        let back = shorts_of(&out[..made]);
        let corr: f64 = back.iter().enumerate().map(|(i, v)| *v as f64 * (i as f64 * 1000.0 * std::f64::consts::TAU / 48000.0).sin()).sum::<f64>()
            / back.iter().map(|v| (*v as f64).powi(2)).sum::<f64>().sqrt()
            / (back.len() as f64 / 2.0).sqrt();
        assert!(corr > 0.9, "{corr}");
    }

    #[test]
    fn split_buffers_match_one_whole_one() {
        let whole_in = bytes_of(&sine(48000, 500.0, 4800));
        let mut whole = Resampler::new(48000, 1, 44100, 1).unwrap();
        let mut out_whole = vec![0u8; 30000];
        let (_, made_whole) = whole.process(&whole_in, PCM_16, &mut out_whole, PCM_16).unwrap();
        let mut split = Resampler::new(48000, 1, 44100, 1).unwrap();
        let mut out_split = vec![0u8; 30000];
        let mut at = 0usize;
        for half in whole_in.chunks(whole_in.len() / 3) {
            let (_, made) = split.process(half, PCM_16, &mut out_split[at..], PCM_16).unwrap();
            at += made;
        }
        assert!((at as i64 - made_whole as i64).abs() <= 4, "{at} vs {made_whole}");
        let (a, b) = (shorts_of(&out_whole[..made_whole]), shorts_of(&out_split[..at]));
        let n = a.len().min(b.len()).saturating_sub(8);
        let err: i64 = a.iter().zip(&b).skip(4).take(n).map(|(x, y)| (x - y).abs() as i64).sum::<i64>() / n.max(1) as i64;
        assert!(err <= 3, "avg error {err}");
    }

    #[test]
    fn mono_to_stereo_and_back() {
        let mut r = Resampler::new(44100, 1, 44100, 2).unwrap();
        let input = bytes_of(&sine(44100, 440.0, 500));
        let mut out = vec![0u8; 20000];
        let (_, made) = r.process(&input, PCM_16, &mut out, PCM_16).unwrap();
        assert_eq!(made, 500 * 2 * 2);
        let back = shorts_of(&out[..made]);
        assert!(back.chunks_exact(2).skip(2).all(|c| (c[0] - c[1]).abs() <= 2));
        let mut r = Resampler::new(44100, 2, 44100, 1).unwrap();
        let mut mono = vec![0u8; 20000];
        let (_, made) = r.process(&out[..made], PCM_16, &mut mono, PCM_16).unwrap();
        assert_eq!(made, 500 * 2);
    }

    #[test]
    fn nonsense_is_refused() {
        assert!(Resampler::new(0, 2, 44100, 2).is_none());
        assert!(Resampler::new(44100, 6, 44100, 2).is_none());
        assert!(Resampler::new(44100, 2, 44100, 6).is_none());
        let mut r = Resampler::new(44100, 2, 48000, 2).unwrap();
        assert!(r.process(&[0u8; 100], 7, &mut [0u8; 10000], PCM_16).is_none());
        assert!(r.process(&[0u8; 100], PCM_16, &mut [0u8; 4], PCM_16).is_none());
    }
}
