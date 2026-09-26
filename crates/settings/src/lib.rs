//! The settings: stored one value per key with their defaults, ranges and wire formats (settings.rs),
//! kept in memory and written back to the app's database (settings_store.rs), and the model a client
//! builds its own settings screen on (settings_model.rs): every setting's name, kind, options as values and
//! default, the values now, and what the core's rules make of them. The sound settings' answers the
//! player asks for (dsp.rs). Which lyrics services there are, and which the settings ask
//! (lyrics_sources.rs). The credits of what the core is built from (credits.rs).
//!
//! No settings screen is here: its pages, rows, order, search and every word on it are each client's.

mod codec;
pub mod credits;
pub mod dsp;
pub mod lyrics_sources;
pub mod settings_model;
pub mod settings;
pub mod settings_store;
