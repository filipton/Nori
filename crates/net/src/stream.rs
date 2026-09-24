//! Where a song's audio comes from and under which name it is cached. Songs are resolved when they are
//! opened, not when they are queued, so the quality follows the network the phone is on at that moment.
//! Both caches are keyed by song id and quality, never by URL, so a replayed track costs no radio time.
//! Which address and quality the client picks for a song is the core's (its stream.rs).

use std::sync::atomic::{AtomicBool, Ordering};

/// Whether the phone's network is metered, as the platform last said ([`network_metered`]).
static METERED: AtomicBool = AtomicBool::new(false);

/// The network the phone is on changed: metered or not. Told whenever it changes, so a song opened in
/// Rust ([`resolve_now`]) streams at the right quality without asking the platform.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn network_metered(metered: bool) {
    METERED.store(metered, Ordering::Relaxed);
}

/// Whether the phone's network is metered, as the platform last said.
pub fn metered() -> bool {
    METERED.load(Ordering::Relaxed)
}

/// A quality setting: `bit_rate` 0 and an empty `format` mean the original file.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StreamQuality {
    pub bit_rate: u32,
    pub format: String,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StreamTarget {
    pub url: String,
    /// The cache key: `<id>:<bit rate><format>` in the rolling stream cache, `dl:<id>` for downloads.
    pub key: String,
}

/// A song to fetch whole into the stream cache ahead of its turn: where from and under which key.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Fetch {
    pub id: String,
    pub url: String,
    pub key: String,
}

/// A finished download's cache key.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn download_key(id: String) -> String {
    format!("dl:{id}")
}

/// Whether `key` is a streamed copy of `id`, at whatever quality it was fetched. Keys are
/// `<id>:<quality>` and the quality never holds a colon, so this is exact.
pub fn is_copy(id: &str, key: &str) -> bool {
    key.rsplit_once(':').is_some_and(|(before, _)| before == id)
}
