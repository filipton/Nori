//! Tempo change of the incoming track during a beat-matched transition: hold `ratio` for the overlap, ramp back to
//! 1 over a few bars, then hand over to the untouched signal and stop costing anything.
//!
//! Two engines: Signalsmith Stretch (MIT, C++ through the `signalsmith-stretch` crate) keeps the pitch; varispeed
//! (cubic Hermite resampling, "vinyl") moves pitch with tempo and costs almost nothing. Both stream: any number of
//! input frames in, whatever fits out, `(consumed, produced)` back.
//!
//! Timing contract: output frame `j` carries the input content at `∫ ratio` — the start latency of the stretcher is
//! dropped internally, so the first output frame is the first input frame and a beat grid computed on the input
//! stays valid on the output. Once the ramp has reached 1 and the stretcher has crossfaded to the plain signal,
//! `bypassed()` turns true: call `drain` once to collect the few frames still inside, then stop calling `process`
//! and pass the track straight through. That hand-over is sample-exact.
//!
//! `process` never allocates; everything is sized in `new`.


/// Input frames per engine call; the tempo is updated this often (about 6 ms).
pub const BLOCK: usize = 256;
/// Slowest ratio accepted; bounds the output of one block.
const MIN_RATIO: f64 = 0.5;
const MAX_RATIO: f64 = 2.0;
const MAX_OUT: usize = (BLOCK as f64 / MIN_RATIO) as usize + 8;
/// Crossfade from the stretched to the plain signal, frames.
const XFADE: usize = 1024;
pub const MAX_CHANNELS: usize = 8;

#[derive(Clone, Copy, Debug, PartialEq)]
enum State {
    /// ratio 1 and no ramp: a plain copy.
    Direct,
    Active,
    Fading(usize),
    Bypass,
}

struct Vari {
    /// Interleaved frames; `pos` is fractional within it.
    buf: Vec<f32>,
    len: usize,
    pos: f64,
    /// Fractional part dropped at the start of the hand-over crossfade.
    phase: f64,
}

enum Engine {
    Signalsmith(signalsmith_stretch::Stretch),
    Vari(Vari),
}

pub struct Stretcher {
    ch: usize,
    engine: Engine,
    ratio0: f64,
    hold: u64,
    ramp: u64,
    out_pos: u64,
    /// Frames the engine has produced, pre-roll included.
    synth: u64,
    drop_total: u64,
    /// How far ahead of what it returns the engine synthesises (its output latency), frames.
    lead: u64,
    frac: f64,
    drop: usize,
    state: State,
    /// Output produced by the last engine call and not yet handed out.
    pend: Vec<f32>,
    pend_read: usize,
    pend_len: usize,
    /// Plain input delayed by the stretcher's latency, for the hand-over (Signalsmith only).
    raw: Vec<f32>,
    raw_pos: usize,
    /// Frames of `raw` still to hand out by `drain`.
    raw_left: usize,
    latency: usize,
    stage: Vec<f32>,
}

#[inline]
fn hermite(y0: f32, y1: f32, y2: f32, y3: f32, t: f32) -> f32 {
    let c1 = 0.5 * (y2 - y0);
    let c2 = y0 - 2.5 * y1 + 2.0 * y2 - 0.5 * y3;
    let c3 = 0.5 * (y3 - y0) + 1.5 * (y1 - y2);
    ((c3 * t + c2) * t + c1) * t + y1
}

impl Vari {
    fn new(ch: usize) -> Self {
        Vari { buf: vec![0.0; (BLOCK * 2 + 8) * ch], len: 0, pos: 1.0, phase: 0.0 }
    }

    /// Appends `input`, writes as many frames at `ratio` as the buffer allows. `fade`: (done, total) of the hand-over.
    fn run(&mut self, ch: usize, input: &[f32], ratio: f64, out: &mut [f32], fade: Option<(usize, usize)>) -> usize {
        let n = input.len() / ch;
        if self.len == 0 {
            // One frame of history before the first sample, so the first output is exactly frame 0.
            self.buf[..ch].fill(0.0);
            self.len = 1;
        }
        self.buf[self.len * ch..(self.len + n) * ch].copy_from_slice(input);
        self.len += n;
        let mut made = 0;
        let max = out.len() / ch;
        while made < max && (self.pos.floor() as usize) + 2 < self.len {
            let i = self.pos.floor() as usize;
            let t = (self.pos - i as f64) as f32;
            let at = |k: usize, c: usize| self.buf[k * ch + c];
            for c in 0..ch {
                let v = hermite(at(i - 1, c), at(i, c), at(i + 1, c), at(i + 2, c), t);
                out[made * ch + c] = match fade {
                    Some((done, total)) => {
                        // The plain signal is the same stream `phase` frames earlier, on whole frames.
                        let exact = self.pos - self.phase;
                        let e = exact.floor() as usize;
                        let te = (exact - e as f64) as f32;
                        let plain = hermite(at(e.max(1) - 1, c), at(e, c), at(e + 1, c), at(e + 2, c), te);
                        let w = ((done + made) as f32 / total as f32).min(1.0);
                        v * (1.0 - w) + plain * w
                    }
                    None => v,
                };
            }
            made += 1;
            self.pos += ratio;
        }
        // Keep one frame of history before the read position.
        let keep_from = (self.pos.floor() as usize).saturating_sub(1);
        if keep_from > 0 {
            self.buf.copy_within(keep_from * ch..self.len * ch, 0);
            self.len -= keep_from;
            self.pos -= keep_from as f64;
        }
        made
    }
}

impl Stretcher {
    /// `keep_pitch` picks Signalsmith Stretch; otherwise varispeed.
    pub fn new(rate: u32, channels: usize, keep_pitch: bool) -> Self {
        let ch = channels.clamp(1, MAX_CHANNELS);
        let engine = if keep_pitch {
            Engine::Signalsmith(signalsmith_stretch::Stretch::preset_cheaper(ch as u32, rate.max(8000)))
        } else {
            Engine::Vari(Vari::new(ch))
        };
        let latency = match &engine {
            Engine::Signalsmith(s) => s.input_latency() + s.output_latency(),
            Engine::Vari(_) => 0,
        };
        Stretcher {
            ch,
            engine,
            ratio0: 1.0,
            hold: 0,
            ramp: 0,
            out_pos: 0,
            synth: 0,
            drop_total: 0,
            lead: 0,
            frac: 0.0,
            drop: 0,
            state: State::Direct,
            pend: vec![0.0; MAX_OUT * ch],
            pend_read: 0,
            pend_len: 0,
            raw: vec![0.0; latency.max(1) * ch],
            raw_pos: 0,
            raw_left: 0,
            latency,
            stage: vec![0.0; MAX_OUT * ch],
        }
    }

    /// Starts a schedule from the next frame: `ratio` (playback speed, >1 faster) for `hold` output frames, then a
    /// linear ramp to 1 over `ramp` output frames, then the hand-over. Resets the engine.
    pub fn configure(&mut self, ratio: f64, hold: u64, ramp: u64) {
        let ratio = if ratio.is_finite() { ratio.clamp(MIN_RATIO, MAX_RATIO) } else { 1.0 };
        (self.ratio0, self.hold, self.ramp, self.out_pos, self.frac) = (ratio, hold, ramp, 0, 0.0);
        (self.pend_read, self.pend_len, self.raw_pos) = (0, 0, 0);
        self.raw.fill(0.0);
        if (ratio - 1.0).abs() < 1e-6 && ramp == 0 {
            self.state = State::Direct;
            self.drop = 0;
            return;
        }
        self.state = State::Active;
        match &mut self.engine {
            Engine::Signalsmith(s) => {
                s.reset();
                // Output frame j holds input (j - out_latency) * ratio - in_latency: drop the difference once.
                self.drop = (s.input_latency() as f64 / ratio + s.output_latency() as f64).round() as usize;
                self.lead = s.output_latency() as u64;
            }
            Engine::Vari(v) => {
                *v = Vari::new(self.ch);
                (self.drop, self.lead) = (0, 0);
            }
        }
        (self.synth, self.drop_total) = (0, self.drop as u64);
    }

    pub fn latency_frames(&self) -> usize {
        self.latency
    }

    pub fn bypassed(&self) -> bool {
        matches!(self.state, State::Bypass | State::Direct)
    }

    /// The schedule runs on the output index the frames synthesised now will have once they leave the stretcher:
    /// Signalsmith hands back frames it synthesised `output_latency` earlier, so the rate chosen now shows up that
    /// much later. Indexing by what was emitted would stretch `output_latency` frames too many at the old rate.
    fn ratio_now(&self) -> f64 {
        let at = (self.synth + self.lead).saturating_sub(self.drop_total);
        if at < self.hold {
            self.ratio0
        } else if at < self.hold + self.ramp {
            let x = (at - self.hold) as f64 / self.ramp as f64;
            self.ratio0 + (1.0 - self.ratio0) * x
        } else {
            1.0
        }
    }

    /// One block of at most `BLOCK` input frames into `pend`.
    fn run_block(&mut self, input: &[f32]) {
        let ch = self.ch;
        let n = input.len() / ch;
        let r = self.ratio_now();
        if r == 1.0 && self.state == State::Active && self.out_pos >= self.hold + self.ramp + (self.latency + BLOCK) as u64 {
            self.state = State::Fading(0);
            if let Engine::Vari(v) = &mut self.engine {
                v.phase = v.pos - v.pos.round();
            }
        }
        let produced = match self.state {
            State::Direct => {
                self.pend[..n * ch].copy_from_slice(input);
                n
            }
            State::Bypass => match &mut self.engine {
                Engine::Signalsmith(_) => {
                    let lat = self.latency;
                    for f in 0..n {
                        let slot = self.raw_pos * ch;
                        for c in 0..ch {
                            self.pend[f * ch + c] = self.raw[slot + c];
                            self.raw[slot + c] = input[f * ch + c];
                        }
                        self.raw_pos = if self.raw_pos + 1 == lat { 0 } else { self.raw_pos + 1 };
                    }
                    n
                }
                Engine::Vari(v) => v.run(ch, input, 1.0, &mut self.pend, None),
            },
            State::Active | State::Fading(_) => {
                let fade = if let State::Fading(done) = self.state { Some((done, XFADE)) } else { None };
                let made = match &mut self.engine {
                    Engine::Signalsmith(s) => {
                        let want = n as f64 / r + self.frac;
                        let m = (want.floor() as usize).min(MAX_OUT);
                        self.frac = want - m as f64;
                        s.process(input, &mut self.pend[..m * ch]);
                        // The plain signal, delayed to line up with the stretcher at ratio 1.
                        let lat = self.latency.max(1);
                        for f in 0..n {
                            let slot = self.raw_pos * ch;
                            for c in 0..ch {
                                self.stage[f * ch + c] = self.raw[slot + c];
                                self.raw[slot + c] = input[f * ch + c];
                            }
                            self.raw_pos = if self.raw_pos + 1 == lat { 0 } else { self.raw_pos + 1 };
                        }
                        if let Some((done, total)) = fade {
                            for f in 0..m.min(n) {
                                let w = ((done + f) as f32 / total as f32).min(1.0);
                                for c in 0..ch {
                                    let i = f * ch + c;
                                    self.pend[i] = self.pend[i] * (1.0 - w) + self.stage[i] * w;
                                }
                            }
                        }
                        m
                    }
                    Engine::Vari(v) => v.run(ch, input, r, &mut self.pend, fade),
                };
                if let State::Fading(done) = self.state {
                    let done = done + made;
                    self.state = if done >= XFADE { State::Bypass } else { State::Fading(done) };
                    if self.state == State::Bypass {
                        self.raw_left = self.latency;
                        if let Engine::Vari(v) = &mut self.engine {
                            v.pos -= v.phase;
                            v.phase = 0.0;
                        }
                    }
                }
                made
            }
        };
        self.synth += produced as u64;
        // Start latency: the first `drop` frames are the stretcher's pre-roll.
        let skip = self.drop.min(produced);
        self.drop -= skip;
        self.pend_read = skip;
        self.pend_len = produced;
        self.out_pos += (produced - skip) as u64;
    }

    /// Interleaved f32. Returns (input frames consumed, output frames written).
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) -> (usize, usize) {
        let ch = self.ch;
        let (inf, cap) = (input.len() / ch, output.len() / ch);
        let (mut used, mut made) = (0, 0);
        loop {
            let take = (self.pend_len - self.pend_read).min(cap - made);
            output[made * ch..(made + take) * ch].copy_from_slice(&self.pend[self.pend_read * ch..(self.pend_read + take) * ch]);
            self.pend_read += take;
            made += take;
            if made == cap || used == inf {
                break;
            }
            let n = BLOCK.min(inf - used);
            self.run_block(&input[used * ch..(used + n) * ch]);
            used += n;
        }
        (used, made)
    }

    /// Frames still inside the stretcher at the end of the stream (or right after `bypassed()` turns true).
    /// Returns frames written; call again while it returns a full buffer.
    pub fn drain(&mut self, output: &mut [f32]) -> usize {
        let ch = self.ch;
        let cap = output.len() / ch;
        let mut made = 0;
        let pending = (self.pend_len - self.pend_read).min(cap);
        output[..pending * ch].copy_from_slice(&self.pend[self.pend_read * ch..(self.pend_read + pending) * ch]);
        self.pend_read += pending;
        made += pending;
        if made == cap || self.pend_read < self.pend_len {
            return made;
        }
        match (&mut self.engine, self.state) {
            (_, State::Direct) => {}
            (Engine::Signalsmith(_), State::Bypass) => {
                // The delay line, oldest first; it may take several calls.
                let lat = self.latency;
                while made < cap && self.raw_left > 0 {
                    let slot = self.raw_pos * ch;
                    output[made * ch..(made + 1) * ch].copy_from_slice(&self.raw[slot..slot + ch]);
                    self.raw_pos = if self.raw_pos + 1 == lat { 0 } else { self.raw_pos + 1 };
                    self.raw_left -= 1;
                    made += 1;
                }
                if self.raw_left == 0 {
                    self.state = State::Direct;
                }
            }
            (Engine::Signalsmith(s), _) => {
                // The stream ended mid-stretch: flush what the stretcher holds (the input latency is lost).
                let m = s.output_latency().min(cap - made);
                s.flush(&mut output[made * ch..(made + m) * ch]);
                let skip = self.drop.min(m);
                output.copy_within((made + skip) * ch..(made + m) * ch, made * ch);
                made += m - skip;
                self.state = State::Direct;
            }
            (Engine::Vari(v), _) => {
                // What is left after the read position, at whole frames.
                let from = v.pos.round() as usize;
                let n = v.len.saturating_sub(from).min(cap - made);
                output[made * ch..(made + n) * ch].copy_from_slice(&v.buf[from * ch..(from + n) * ch]);
                made += n;
                *v = Vari::new(ch);
                self.state = State::Direct;
            }
        }
        (self.pend_read, self.pend_len) = (0, 0);
        made
    }
}

// ---- JNI: dev.nori.music.playback.AutoMixStretch --------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    /// Clicks every `every` frames, stereo.
    fn clicks(frames: usize, every: usize) -> Vec<f32> {
        let mut x = vec![0f32; frames * 2];
        for f in (every / 2..frames).step_by(every) {
            for k in 0..8 {
                let v = if k % 2 == 0 { 0.8 } else { -0.8 } * (1.0 - k as f32 / 8.0);
                if f + k < frames {
                    x[(f + k) * 2] = v;
                    x[(f + k) * 2 + 1] = v;
                }
            }
        }
        x
    }

    fn run_all(s: &mut Stretcher, x: &[f32], chunk: usize) -> Vec<f32> {
        let mut out = Vec::new();
        let mut buf = vec![0f32; 4096 * 2];
        let mut i = 0;
        while i < x.len() {
            let end = (i + chunk * 2).min(x.len());
            let mut pos = i;
            while pos < end {
                let (u, m) = s.process(&x[pos..end], &mut buf);
                out.extend_from_slice(&buf[..m * 2]);
                pos += u * 2;
            }
            i = end;
        }
        loop {
            let m = s.drain(&mut buf);
            out.extend_from_slice(&buf[..m * 2]);
            if m * 2 < buf.len() {
                break;
            }
        }
        out
    }

    /// Frame index of each energy peak, one per `every`-frame window.
    fn peaks(y: &[f32], every: usize) -> Vec<usize> {
        let frames = y.len() / 2;
        let env: Vec<f32> = (0..frames).map(|f| y[f * 2].abs()).collect();
        let mut out = Vec::new();
        let mut f = 0;
        while f + every <= frames {
            let (i, v) = env[f..f + every].iter().enumerate().fold((0, 0f32), |m, (i, v)| if *v > m.1 { (i, *v) } else { m });
            if v > 0.1 {
                out.push(f + i);
            }
            f += every;
        }
        out
    }

    #[test]
    fn direct_mode_is_a_copy() {
        for keep in [true, false] {
            let mut s = Stretcher::new(44100, 2, keep);
            s.configure(1.0, 1000, 0);
            let x = clicks(44100, 11025);
            let y = run_all(&mut s, &x, 777);
            assert_eq!(x, y);
            assert!(s.bypassed());
        }
    }

    /// At a constant ratio the clicks come out at input / ratio, from the very first one: the start latency is gone.
    #[test]
    fn stretched_output_keeps_the_timeline() {
        for keep in [true, false] {
            for ratio in [1.04, 0.97] {
                let mut s = Stretcher::new(44100, 2, keep);
                s.configure(ratio, 10 * 44100, 0);
                let every = 22050;
                let x = clicks(44100 * 6, every);
                let y = run_all(&mut s, &x, 1000);
                let got = peaks(&y, (every as f64 / ratio) as usize);
                let want: Vec<f64> = (every / 2..44100 * 6).step_by(every).map(|f| f as f64 / ratio).collect();
                assert!(got.len() >= want.len() - 1, "{keep} {ratio}: {got:?}");
                for (g, w) in got.iter().zip(&want) {
                    assert!((*g as f64 - w).abs() < 0.003 * 44100.0, "keep_pitch {keep} ratio {ratio}: click at {g}, expected {w}");
                }
                let expect_len = x.len() as f64 / ratio;
                assert!((y.len() as f64 - expect_len).abs() < 0.01 * expect_len, "{} vs {expect_len}", y.len());
            }
        }
    }

    /// After hold + ramp the stretcher hands over to the plain signal and drains to exactly the input, delayed by
    /// the time the ramp gained.
    #[test]
    fn the_ramp_ends_in_a_seamless_bypass() {
        for keep in [true, false] {
            let mut s = Stretcher::new(44100, 2, keep);
            s.configure(1.03, 44100, 44100);
            let x: Vec<f32> = (0..44100 * 8).flat_map(|i| {
                let v = (0.3 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / 44100.0).sin()) as f32;
                [v, v]
            }).collect();
            let mut out = Vec::new();
            let mut buf = vec![0f32; 2048 * 2];
            let mut pos = 0;
            while pos < x.len() && !s.bypassed() {
                let end = (pos + 512 * 2).min(x.len());
                let (u, m) = s.process(&x[pos..end], &mut buf);
                out.extend_from_slice(&buf[..m * 2]);
                pos += u * 2;
            }
            assert!(s.bypassed(), "keep_pitch {keep}: never handed over");
            loop {
                let m = s.drain(&mut buf);
                out.extend_from_slice(&buf[..m * 2]);
                if m * 2 < buf.len() {
                    break;
                }
            }
            // From here the caller passes the input straight through; the join must be continuous.
            out.extend_from_slice(&x[pos..]);
            let frames = out.len() / 2;
            // Consumed = produced + ∫(ratio - 1): 1 s at 3 % plus a 1 s ramp at 1.5 % on average = 1985 frames.
            let gained = x.len() / 2 - frames;
            assert!((gained as i64 - 1985).abs() < 40, "keep_pitch {keep}: gained {gained}");
            let join = (out.len() - (x.len() - pos)) / 2;
            let tail: Vec<f32> = out[(join - 200) * 2..(join + 200) * 2].iter().step_by(2).copied().collect();
            let jump = tail.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0f32, f32::max);
            // A 220 Hz sine at 0.3 moves at most 0.3 * 2π * 220 / 44100 = 0.0094 per sample.
            assert!(jump < 0.02, "keep_pitch {keep}: discontinuity {jump} at the hand-over");
        }
    }

    /// Single-sample impulses: at a constant ratio every one comes out within a few frames of input / ratio.
    #[test]
    fn the_timeline_is_sample_accurate() {
        for (keep, ratio) in [(true, 1.0001f64), (true, 1.03), (true, 1.06), (true, 0.95), (false, 1.02), (false, 0.98)] {
            let mut s = Stretcher::new(44100, 2, keep);
            s.configure(ratio, 1 << 40, 0);
            let every = 11025;
            let mut x = vec![0f32; 44100 * 4 * 2];
            for f in (every / 2..44100 * 4).step_by(every) {
                x[f * 2] = 1.0;
                x[f * 2 + 1] = 1.0;
            }
            let mut out = Vec::new();
            let mut buf = vec![0f32; 8192];
            let mut pos = 0;
            while pos < x.len() {
                let (u, m) = s.process(&x[pos..(pos + 2000).min(x.len())], &mut buf);
                out.extend_from_slice(&buf[..m * 2]);
                pos += u * 2;
            }
            let env: Vec<f32> = out.iter().step_by(2).map(|v| v.abs()).collect();
            let mut errs = Vec::new();
            for k in 0..14 {
                let want = (every / 2 + k * every) as f64 / ratio;
                let w = want as usize;
                if w + 600 > env.len() || w < 600 {
                    continue;
                }
                let (i, _) = env[w - 600..w + 600].iter().enumerate().fold((0, 0f32), |m, (i, v)| if *v > m.1 { (i, *v) } else { m });
                errs.push((w - 600 + i) as f64 - want);
            }
            assert!(errs.len() >= 12, "{keep} {ratio}: {errs:?}");
            assert!(errs.iter().all(|e| e.abs() <= 12.0), "keep_pitch {keep} ratio {ratio}: {errs:?}");
        }
    }

    #[test]
    fn bad_ratios_are_clamped() {
        let mut s = Stretcher::new(48000, 2, false);
        s.configure(f64::NAN, 100, 0);
        assert!(s.bypassed());
        s.configure(100.0, 1000, 0);
        assert_eq!(s.ratio0, MAX_RATIO);
    }

    /// CPU cost per second of 44.1 kHz stereo. Run with
    /// `cargo test --release -p nori-player stretch_cost -- --ignored --nocapture`.
    #[test]
    #[ignore]
    fn stretch_cost() {
        let x: Vec<f32> = (0..44100 * 30).flat_map(|i| {
            let v = (0.3 * (2.0 * std::f64::consts::PI * 220.0 * i as f64 / 44100.0).sin() + 0.05 * ((i * 7919 % 1000) as f64 / 1000.0 - 0.5)) as f32;
            [v, v * 0.9]
        }).collect();
        for keep in [true, false] {
            let mut s = Stretcher::new(44100, 2, keep);
            s.configure(1.05, u64::MAX / 4, 0);
            let mut buf = vec![0f32; 4096 * 2];
            let t = std::time::Instant::now();
            let mut pos = 0;
            while pos < x.len() {
                let end = (pos + 1024 * 2).min(x.len());
                let (u, _) = s.process(&x[pos..end], &mut buf);
                pos += u * 2;
            }
            let per_s = t.elapsed().as_secs_f64() / 30.0;
            println!("{}: {:.2} ms CPU per second of audio ({:.2} % of one core), latency {} frames",
                if keep { "signalsmith (cheaper preset)" } else { "varispeed" }, per_s * 1000.0, per_s * 100.0, s.latency_frames());
        }
    }
}
