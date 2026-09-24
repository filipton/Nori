//! AutoMix on this side of the FFI: the analysis store (SQLite), the transition planner the audio path
//! asks (planner.rs), the engine's host (host.rs), measuring ahead (ahead.rs) and the calls Kotlin makes.
//! The analysis, planning and per-sample work itself lives in the player crate (`nori_player::automix`),
//! shared with every platform. The store's calls on the core's own database are the core's.

pub mod ahead;
pub mod host;
pub mod store;
pub mod planner;

pub use nori_player::automix::*;

use nori_model::{AutoMixSettings, TrackAnalysis, TransitionPlan, WindowSong};

/// Analyses a whole decoded track without storing it: interleaved little-endian PCM (`encoding` 2 = 16-bit,
/// 4 = float) at any rate and channel count, exactly as MediaCodec hands it out. A `ByteArray` on the Kotlin side,
/// so a track crosses as one copy rather than millions of boxed floats. One call per track, off the main thread;
/// about 0.14 CPU-seconds for 4 minutes on a desktop.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn automix_analyse(song_id: String, pcm: Vec<u8>, sample_rate: i32, channels: i32, encoding: i32) -> TrackAnalysis {
    analyse_bytes(&song_id, &pcm, sample_rate, channels, encoding).track
}

/// How to get from `outgoing` to `incoming`; either may be unknown. Durations in ms.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn plan_transition(
    outgoing: Option<TrackAnalysis>, incoming: Option<TrackAnalysis>, out_duration_ms: i64, in_duration_ms: i64, settings: AutoMixSettings,
) -> TransitionPlan {
    nori_player::automix::plan::plan(outgoing.as_ref(), incoming.as_ref(), out_duration_ms, in_duration_ms, &settings)
}

/// The plan as the flat array `AutoMixMixer.configure` takes.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn automix_mixer_params(plan: TransitionPlan) -> Vec<f32> {
    mixer::params(&plan)
}

/// `bpm` folded by ×2/×½ to the octave nearest `reference`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn automix_fold_tempo(reference: f64, bpm: f64) -> f64 {
    tempo::fold(reference, bpm)
}

/// Playback speed that locks `in_bpm` to `out_bpm` after octave folding (1.0 when either is unknown).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn automix_tempo_ratio(out_bpm: f64, in_bpm: f64) -> f64 {
    tempo::match_ratio(out_bpm, in_bpm)
}

/// "8B", "11A", or empty for an unknown key.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn automix_key_name(key: i32) -> String {
    structure::camelot_name(key)
}

/// Steps between two keys on the Camelot wheel (0 same, 1 compatible); -1 when either is unknown.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn automix_key_distance(a: i32, b: i32) -> i32 {
    structure::key_distance(a, b)
}

/// The defaults the settings screen starts from.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn automix_default_settings() -> AutoMixSettings {
    AutoMixSettings::default()
}

/// `current` sits inside an album played in order (for ReplayGain's album mode); see
/// `nori_player::transitions::in_album_run`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn in_album_run(before: Option<WindowSong>, current: WindowSong, after: Option<WindowSong>, shuffling: bool) -> bool {
    nori_player::transitions::in_album_run(before.as_ref(), &current, after.as_ref(), shuffling)
}
