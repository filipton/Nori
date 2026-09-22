package dev.nori.music.downloads

import kotlinx.coroutines.flow.StateFlow

/** Where one song's download stands. QUEUED is never stored: it is any pending song without a mark. */
enum class DownloadPhase { QUEUED, DOWNLOADING, FAILED, DONE }

/**
 * What a download this session has touched is doing. [progress] is 0..1, or negative while the size is
 * not known; it moves at most four times a second and only by whole percents (the core's gate, see
 * crates/core/src/transfers.rs), so a row drawing it never redraws per chunk. [at] is when the phase
 * last changed, elapsedRealtime.
 */
class DownloadMark(val phase: DownloadPhase, val progress: StateFlow<Float>, val at: Long)
