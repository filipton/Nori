package dev.flint.music.playback

import android.os.SystemClock
import dev.flint.music.Flint
import dev.flint.music.ffi.Song
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/**
 * Counts a play once the configured share of the track (or four minutes) has actually been heard.
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
        val percent = flint.settings.value.scrobblePercent.coerceIn(10, 100)
        scope.launch(Dispatchers.IO) {
            // Both are writes: made offline, they wait in the pending queue and keep their original time.
            if (done != null && heard >= minOf(done.duration.toLong() * 10 * percent, 240_000L).coerceAtLeast(10_000L)) runCatching { flint.library.scrobble(done.id, submission = true, timeMs = at) }
            if (next != null) runCatching { flint.library.nowPlaying(next.id) }
        }
    }
}
