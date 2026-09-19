package dev.flint.music.downloads

import kotlinx.coroutines.flow.StateFlow

/** Where one song's download stands. QUEUED is never stored: it is any pending song without a mark. */
enum class DownloadPhase { QUEUED, DOWNLOADING, FAILED, DONE }

/**
 * What a download this session has touched is doing. [progress] is 0..1, or negative while the size is
 * not known; it moves at most four times a second and only by whole percents (see [ProgressGate]), so
 * a row drawing it never redraws per chunk, and it stands still once the phase is not DOWNLOADING.
 * [at] is when the phase last changed, elapsedRealtime.
 */
class DownloadMark(val phase: DownloadPhase, val progress: StateFlow<Float>, val at: Long)

/**
 * Lets through only the progress worth drawing: the first figure, the switch to or from "size
 * unknown", the finish, and otherwise a step of at least [minStep] no sooner than [minIntervalMs] after
 * the last one. The bytes arrive in chunks of a few kilobytes; handing each to the UI would recompose
 * a list hundreds of times a second for a ring that moves a pixel.
 */
class ProgressGate(private val minIntervalMs: Long = 250, private val minStep: Float = 0.01f) {
    private var last = Float.NaN
    private var lastAt = 0L

    /** True when [fraction] (negative: unknown) at [now] should be published. */
    fun offer(fraction: Float, now: Long): Boolean {
        val pass = when {
            last.isNaN() -> true
            fraction < 0f -> last >= 0f
            last < 0f -> true
            fraction >= 1f -> last < 1f
            else -> now - lastAt >= minIntervalMs && kotlin.math.abs(fraction - last) >= minStep
        }
        if (pass) { last = fraction; lastAt = now }
        return pass
    }
}

/**
 * How far along a download is: against the length the server stated when it stated one, otherwise
 * against [estimate] (what the song should weigh at the download quality), held short of full because
 * an estimate can be low and a ring that sits at 100 % while bytes still arrive looks stuck. Negative
 * when neither is known.
 */
fun downloadFraction(contentLength: Long, bytes: Long, estimate: Long): Float = when {
    contentLength > 0 -> (bytes.toDouble() / contentLength).toFloat().coerceIn(0f, 1f)
    estimate > 0 -> (bytes.toDouble() / estimate).toFloat().coerceIn(0f, ESTIMATE_CEILING)
    else -> -1f
}

private const val ESTIMATE_CEILING = 0.97f

/**
 * What a song should weigh once downloaded: the file itself at the original quality, else its length at
 * the transcoded bitrate. A transcoding server rarely sends a Content-Length, so this is what gives the
 * progress ring (and the notification's bar) something to fill against.
 */
fun expectedBytes(sizeBytes: Long, durationS: Long, bitRateKbps: Int): Long =
    if (bitRateKbps > 0 && durationS > 0) durationS * bitRateKbps * 125L else sizeBytes

/** The downloads screen's lists, in the order the queue will run them. */
data class DownloadSections<T>(val active: List<T>, val queued: List<T>, val failed: List<T>, val finished: List<T>)

/**
 * Splits the pending songs (newest first, as the index keeps them) into what is downloading, waiting
 * and failed, in queue order - oldest first, which is the order the download manager starts them in -
 * and lists this session's finished songs newest first. [done] and [pending] together are where a
 * finished song's details come from, so a song that has just completed is not missing from every list
 * for the moment between its mark and the index catching up.
 */
fun <T> downloadSections(pending: List<T>, done: List<T>, marks: Map<String, DownloadMark>, id: (T) -> String): DownloadSections<T> {
    val active = ArrayList<T>()
    val queued = ArrayList<T>()
    val failed = ArrayList<T>()
    for (song in pending.asReversed()) when (marks[id(song)]?.phase) {
        DownloadPhase.DOWNLOADING -> active += song
        DownloadPhase.FAILED -> failed += song
        DownloadPhase.DONE -> {}
        else -> queued += song
    }
    val byId = HashMap<String, T>()
    for (s in pending) byId[id(s)] = s
    for (s in done) byId[id(s)] = s
    val finished = marks.entries.asSequence().filter { it.value.phase == DownloadPhase.DONE }
        .sortedByDescending { it.value.at }.mapNotNull { byId[it.key] }.toList()
    return DownloadSections(active, queued, failed, finished)
}
