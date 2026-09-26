//! AutoMix on this side of the FFI: the analysis store (SQLite), the transition planner the audio path
//! asks (planner.rs), the engine's host (host.rs), the beat model's file
//! (beat_model.rs) and the calls Kotlin makes.
//! The analysis, planning and per-sample work itself lives in the player crate (`nori_player::automix`),
//! shared with every platform. The store's calls on the core's own database are the core's.

pub mod beat_model;
pub mod host;
pub mod store;
pub mod planner;

pub use nori_player::automix::*;

use nori_model::WindowSong;

/// `current` sits inside an album played in order (for ReplayGain's album mode); see
/// `nori_player::transitions::in_album_run`.
pub fn in_album_run(before: Option<WindowSong>, current: WindowSong, after: Option<WindowSong>, shuffling: bool) -> bool {
    nori_player::transitions::in_album_run(before.as_ref(), &current, after.as_ref(), shuffling)
}
