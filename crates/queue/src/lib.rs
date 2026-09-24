//! The queue the app plays, owned here: the list and its order (playlist.rs over `nori_player::playlist`),
//! the songs in it by id (queue.rs), how it moves and what the controls do (rules.rs), refilling it past
//! its end (autofill.rs), the offline bridge (bridge.rs), which song the ear is on (heard.rs), counting
//! plays (scrobble.rs) and what playing something means (actions.rs). The calls that read the server or
//! the core's database handle are the core's.

pub mod actions;
pub mod autofill;
pub mod bridge;
pub mod heard;
pub mod playlist;
pub mod queue;
pub mod rules;
pub mod scrobble;
