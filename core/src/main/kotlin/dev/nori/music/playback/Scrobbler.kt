package dev.nori.music.playback

import android.os.SystemClock
import dev.nori.music.Nori
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch

/**
 * Counting plays is the core's (crates/core/src/scrobble.rs): it sums listening time from play and
 * pause edges, judges a song when it is left, and records it in the local history. This hands it the
 * edges and sends the server what the core says to.
 */
class Scrobbler(private val nori: Nori, private val scope: CoroutineScope) {
    fun onPlaying(playing: Boolean) = dev.nori.music.ffi.scrobblePlaying(playing, SystemClock.elapsedRealtime())

    /** [nextId] is null when playback ends. */
    fun onTrack(nextId: String?, playing: Boolean) {
        val p = nori.settings.value
        val wall = System.currentTimeMillis()
        val send = dev.nori.music.ffi.scrobbleTrack(
            nextId, playing, SystemClock.elapsedRealtime(), wall, java.util.TimeZone.getDefault().getOffset(wall),
            p.tasteModel, p.scrobble, p.scrobblePercent,
        )
        if (send.submitId == null && send.nowPlayingId == null) return
        scope.launch(Dispatchers.IO) {
            // Both are writes: made offline, they wait in the pending queue and keep their original time.
            send.submitId?.let { runCatching { nori.library.scrobble(it, submission = true, timeMs = send.submitAt) } }
            send.nowPlayingId?.let { runCatching { nori.library.nowPlaying(it) } }
        }
    }
}
