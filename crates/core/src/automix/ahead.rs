//! Measuring the songs coming up before they play, so a transition has both halves' tempo, beats and cue
//! points the first time two songs meet. Which songs is `rules::queue_measure`; this is what the platform's
//! worker does with that list. Nothing here touches the network: a song is measured only once its bytes
//! are all on the device (downloaded, or fetched whole into the stream cache by the precacher), and one
//! that is not is left for the next time the queue moves.

/// Whether a song's whole file is on the device: downloaded, or a stream cache entry of known length
/// (`content_length`, -1 or 0 when the cache does not know it) holding every byte of it (`cached_bytes`).
///
/// Twin of `AutoMixPrefetch.onDevice` (core/.../playback/AutoMixPrefetch.kt), which Android keeps next to
/// media3's cache.
pub fn whole_on_device(downloaded: bool, content_length: i64, cached_bytes: i64) -> bool {
    downloaded || (content_length > 0 && cached_bytes >= content_length)
}

/// One pass of measuring ahead.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AheadPlan {
    /// The songs to measure now, in the order they come up.
    pub measure: Vec<String>,
    /// How many unmeasured songs are not on the device yet, for the one log line a pass writes.
    pub waiting: usize,
}

/// Of the songs coming up that have never been measured (`missing`, from `analysis_missing`, in queue
/// order), those whose bytes are on the device now ([`whole_on_device`], asked through `on_device`).
///
/// Twin of `AutoMixPrefetch.update` (core/.../playback/AutoMixPrefetch.kt), which Android keeps on its
/// worker thread.
pub fn plan(missing: Vec<String>, on_device: impl Fn(&str) -> bool) -> AheadPlan {
    let (measure, waiting): (Vec<String>, Vec<String>) = missing.into_iter().partition(|id| on_device(id));
    AheadPlan { measure, waiting: waiting.len() }
}
