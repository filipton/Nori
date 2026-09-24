//! The settings: stored one value per key with their defaults, ranges and wire formats (settings.rs),
//! kept in memory and written back to the app's database (settings_store.rs), and the settings screen as
//! data (settings_schema.rs). What follows them by itself lives here too: the sound chain the settings ask
//! for (dsp.rs, told of every change) and which streams the core's decoder takes (decoder.rs). Which
//! lyrics services there are, and which the settings ask (lyrics_sources.rs).

pub mod decoder;
pub mod dsp;
pub mod lyrics_sources;
pub mod settings;
pub mod settings_schema;
pub mod settings_store;
