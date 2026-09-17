package dev.flint.music.playback

import android.os.SystemClock
import dev.flint.music.Flint
import dev.flint.music.ffi.Song
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/**
 * Counts a play once half the track (or four minutes) has actually been heard.
 * There is no timer: listening time is summed from play/pause edges and judged
 * when the track ends, which is a moment the radio is awake anyway.
 */
class Scrobbler(private val flint: Flint, private val scope: CoroutineScope) {
    private var song: Song? = null
    private var startedAt = 0L
    private var heardMs = 0L
    private var playingSince = 0L

    fun onPlaying(playing: Boolean) {
        val now = SystemClock.elapsedRealtime()
        if (playing && playingSince == 0L) playingSince = now
        if (!playing && playingSince != 0L) { heardMs += now - playingSince; playingSince = 0 }
    }

    /** [next] is null when playback ends. */
    fun onTrack(next: Song?, playing: Boolean) {
        onPlaying(false)
        val done = song
        val heard = heardMs
        val at = startedAt
        song = next
        heardMs = 0
        startedAt = System.currentTimeMillis()
        onPlaying(playing)
        if (!flint.settings.value.scrobble) return
        scope.launch(Dispatchers.IO) {
            if (done != null && heard >= minOf(done.duration.toLong() * 500, 240_000L).coerceAtLeast(10_000L)) submit(done.id, at)
            if (next != null) runCatching { flint.library.scrobble(next.id, submission = false) }
        }
    }

    private suspend fun submit(id: String, at: Long) {
        try {
            flint.library.scrobble(id, submission = true, timeMs = at)
            for (p in flint.core.scrobblePending()) {
                flint.library.scrobble(p.songId, submission = true, timeMs = p.timeMs)
                flint.core.scrobbleDone(p.rowId)
            }
        } catch (e: Exception) {
            // Offline: keep it for the next time a scrobble gets through.
            runCatching { flint.core.scrobbleEnqueue(id, at) }
        }
    }
}
