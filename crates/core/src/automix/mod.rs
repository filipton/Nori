//! AutoMix: DJ-style transitions (see docs/research/automix.md).
//!
//! - `analysis` + `tempo` + `structure` + `loudness`: one pass over a track's PCM gives a `TrackAnalysis`
//!   (tempo and beat grid, downbeats, phrase cues, key, loudness, silence and MixRamp points). Coarse uniffi call
//!   with the whole decoded track, or a JNI streaming tap on PCM the player already decodes.
//! - `store`: the `track_analysis` table and the `Core` methods around it.
//! - `plan`: a pure function from two analyses and the user's settings to a `TransitionPlan`.
//! - `mixer` and `stretch`: per-buffer JNI building blocks that render a plan (gain curves, bass swap, filter
//!   sweep; time-stretch of the incoming track).

pub mod analysis;
pub mod loudness;
pub mod mixer;
pub mod plan;
pub mod resample;
pub mod store;
pub mod stretch;
pub mod structure;
pub mod tempo;

#[cfg(test)]
mod tests;

use crate::{AutoMixSettings, TrackAnalysis, TransitionPlan};
use analysis::{Analyzer, Features};

/// Bump when the analysis changes enough that stored rows should be redone.
pub const ANALYSIS_VERSION: i32 = 2;
/// Below these the grid is not used for cue placement either (cues fall back to the energy envelope).
const CUE_MIN_CONFIDENCE: f32 = 0.4;
const CUE_MIN_STABILITY: f32 = 0.5;

/// Mean of a per-frame `curve` over `[from_s, to_s)`. Too short a window to say anything (under a
/// second of frames) falls back to the whole track rather than to noise.
fn window_mean(curve: &[f32], fps: f64, t0: f64, from_s: f64, to_s: f64) -> f32 {
    if curve.is_empty() || !fps.is_finite() || fps <= 0.0 {
        return 0.0;
    }
    let idx = |t: f64| ((t - t0) * fps).round().max(0.0) as usize;
    let (mut a, mut b) = (idx(from_s).min(curve.len()), idx(to_s).min(curve.len()));
    if b.saturating_sub(a) < fps as usize {
        (a, b) = (0, curve.len());
    }
    if b <= a {
        return 0.0;
    }
    curve[a..b].iter().sum::<f32>() / (b - a) as f32
}

/// A `TrackAnalysis` plus the working data behind it, for tests and diagnostics.
pub struct Analysis {
    pub track: TrackAnalysis,
    pub tempo: tempo::Tempo,
}

/// Runs the whole-track steps on the features of one track.
pub fn finish(song_id: &str, f: &Features) -> Analysis {
    let duration_ms = (f.duration_s * 1000.0).round() as i64;
    let (s0, s1) = loudness::silence_trim(&f.blocks_raw);
    let (s0, s1) = (s0.min(duration_ms), s1.min(duration_ms));
    let lufs = loudness::integrated(&f.blocks_k);
    let (mr0, mr1) = loudness::mixramp(&f.blocks_k, lufs).map_or((s0, s1), |(a, b)| (a.clamp(s0, s1.max(s0)), b.clamp(s0, s1.max(s0))));
    let silent = s1 <= s0;
    let music = if silent { (0.0, f.duration_s) } else { (s0 as f64 / 1000.0, s1 as f64 / 1000.0) };

    let t = if silent { tempo::Tempo::default() } else { tempo::estimate(&f.onset, f.fps, f.t0) };
    let db = structure::downbeat(&t, f, music);
    let grid_ok = t.bpm > 0.0 && t.confidence >= CUE_MIN_CONFIDENCE && t.stability >= CUE_MIN_STABILITY;
    let (intro, outro) = if silent { (0.0, 0.0) } else { structure::cues(&t, &db, f, music, grid_ok) };
    let (key, key_confidence) = if silent { (0, 0.0) } else { structure::key(&f.chroma) };
    // What the overlap windows sound like: vocal share and brightness of the outgoing outro and the
    // incoming intro, for the pair gates in `plan`. Silence has neither.
    let (outro_vocal, outro_centroid, intro_vocal, intro_centroid) = if silent {
        (0.0, 0.0, 0.0, 0.0)
    } else {
        (
            window_mean(&f.vocal, f.fps, f.t0, outro, music.1),
            window_mean(&f.centroid, f.fps, f.t0, outro, music.1),
            window_mean(&f.vocal, f.fps, f.t0, music.0, intro),
            window_mean(&f.centroid, f.fps, f.t0, music.0, intro),
        )
    };

    let track = TrackAnalysis {
        song_id: song_id.to_string(),
        analysis_version: ANALYSIS_VERSION,
        duration_ms,
        bpm: if t.bpm.is_finite() { t.bpm } else { 0.0 },
        bpm_confidence: t.confidence,
        beat_offset_ms: t.offset_s * 1000.0,
        stability: t.stability,
        downbeat_phase: db.phase,
        downbeat_confidence: db.confidence,
        lufs: lufs as f32,
        key,
        key_confidence,
        silence_start_ms: s0,
        silence_end_ms: s1,
        mixramp_start_ms: mr0,
        mixramp_end_ms: mr1,
        intro_end_ms: (intro * 1000.0).round() as i64,
        outro_start_ms: (outro * 1000.0).round() as i64,
        outro_vocal,
        intro_vocal,
        outro_centroid,
        intro_centroid,
        analysed_ms: crate::db::now_ms(),
    };
    Analysis { track, tempo: t }
}

/// Analyses one whole track of mono samples at `sample_rate`.
pub fn analyse(song_id: &str, pcm: &[f32], sample_rate: u32) -> Analysis {
    let mut a = Analyzer::new(sample_rate, (pcm.len() as u64 * 1000) / sample_rate.max(1) as u64);
    a.feed(pcm);
    let f = a.take_features();
    finish(song_id, &f)
}

/// `C.ENCODING_PCM_16BIT` and `C.ENCODING_PCM_FLOAT`, as media3 numbers them.
pub const PCM_16: i32 = 2;
pub const PCM_FLOAT: i32 = 4;

/// Analyses interleaved little-endian PCM bytes, 16-bit or float, as a decoder hands them out.
pub fn analyse_bytes(song_id: &str, pcm: &[u8], sample_rate: i32, channels: i32, encoding: i32) -> Analysis {
    let ch = channels.clamp(1, 8) as usize;
    let rate = sample_rate.max(1) as u32;
    let width = if encoding == PCM_FLOAT { 4 } else { 2 };
    let frames = pcm.len() / width / ch;
    let mut a = Analyzer::new(rate, frames as u64 * 1000 / rate as u64);
    // Decoded in slices so a whole track never exists as f32 on top of the bytes.
    let chunk = 1024 * ch * width;
    for part in pcm.chunks(chunk) {
        let part = &part[..part.len() / (ch * width) * (ch * width)];
        if encoding == PCM_FLOAT {
            let mut s = [0f32; 1024 * 8];
            for (d, b) in s.iter_mut().zip(part.chunks_exact(4)) {
                *d = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
            }
            a.feed_interleaved(&s[..part.len() / 4], ch, |v| v);
        } else {
            let mut s = [0i16; 1024 * 8];
            for (d, b) in s.iter_mut().zip(part.chunks_exact(2)) {
                *d = i16::from_le_bytes([b[0], b[1]]);
            }
            a.feed_interleaved(&s[..part.len() / 2], ch, |v| v as f32 / 32768.0);
        }
    }
    finish(song_id, &a.take_features())
}

/// Analyses a whole decoded track without storing it: interleaved little-endian PCM (`encoding` 2 = 16-bit,
/// 4 = float) at any rate and channel count, exactly as MediaCodec hands it out. A `ByteArray` on the Kotlin side,
/// so a track crosses as one copy rather than millions of boxed floats. One call per track, off the main thread;
/// about 0.14 CPU-seconds for 4 minutes on a desktop.
#[uniffi::export]
pub fn automix_analyse(song_id: String, pcm: Vec<u8>, sample_rate: i32, channels: i32, encoding: i32) -> TrackAnalysis {
    analyse_bytes(&song_id, &pcm, sample_rate, channels, encoding).track
}

/// How to get from `outgoing` to `incoming`; either may be unknown. Durations in ms.
#[uniffi::export]
pub fn plan_transition(
    outgoing: Option<TrackAnalysis>, incoming: Option<TrackAnalysis>, out_duration_ms: i64, in_duration_ms: i64, settings: AutoMixSettings,
) -> TransitionPlan {
    plan::plan(outgoing.as_ref(), incoming.as_ref(), out_duration_ms, in_duration_ms, &settings)
}

/// The plan as the flat array `AutoMixMixer.configure` takes.
#[uniffi::export]
pub fn automix_mixer_params(plan: TransitionPlan) -> Vec<f32> {
    mixer::params(&plan)
}

/// `bpm` folded by ×2/×½ to the octave nearest `reference`.
#[uniffi::export]
pub fn automix_fold_tempo(reference: f64, bpm: f64) -> f64 {
    tempo::fold(reference, bpm)
}

/// Playback speed that locks `in_bpm` to `out_bpm` after octave folding (1.0 when either is unknown).
#[uniffi::export]
pub fn automix_tempo_ratio(out_bpm: f64, in_bpm: f64) -> f64 {
    tempo::match_ratio(out_bpm, in_bpm)
}

/// "8B", "11A", or empty for an unknown key.
#[uniffi::export]
pub fn automix_key_name(key: i32) -> String {
    structure::camelot_name(key)
}

/// Steps between two keys on the Camelot wheel (0 same, 1 compatible); -1 when either is unknown.
#[uniffi::export]
pub fn automix_key_distance(a: i32, b: i32) -> i32 {
    structure::key_distance(a, b)
}

/// The defaults the settings screen starts from.
#[uniffi::export]
pub fn automix_default_settings() -> AutoMixSettings {
    AutoMixSettings::default()
}
