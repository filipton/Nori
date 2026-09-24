package dev.nori.music.playback

import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.cache.CacheWriter
import java.util.concurrent.Executors
import java.util.concurrent.Future

/**
 * Fetches whole upcoming tracks into the stream cache right when a track starts, which is a moment
 * the radio is awake anyway; when their turn comes they play from disk and the modem stays asleep.
 * One track at a time, in queue order, abandoned as soon as the queue moves on. Provider tracks from
 * octo-fiesta are never fetched ahead: asking for one makes the server download it.
 */
@UnstableApi
class Precacher(
    private val sources: MediaSources,
    /** A song has just been fetched whole: what waits for a song to be on the device (measuring) can look again. */
    private val onFetched: () -> Unit = {},
) {
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "nori-precache").apply { priority = Thread.MIN_PRIORITY } }
    private var running: Future<*>? = null
    @Volatile private var writer: CacheWriter? = null

    /**
     * [songs] are the core's (`Client::precache_targets`): which ones, in order, and where each comes from.
     * None the download queue is fetching or has fetched: a download arrives permanently, and pulling it
     * into the rolling cache too keeps it twice.
     */
    fun update(songs: List<dev.nori.music.ffi.net.Fetch>) {
        cancel()
        if (songs.isEmpty()) return
        running = worker.submit {
            for (song in songs) {
                if (Thread.currentThread().isInterrupted) return@submit
                // A download may have finished while the songs before it were fetched.
                if (sources.isDownloaded(song.id)) continue
                var fetched = false
                val listener = CacheWriter.ProgressListener { _, _, new -> if (new > 0) fetched = true }
                val whole = runCatching { CacheWriter(sources.precaching.createDataSource(), sources.spec(song), null, listener).also { writer = it }.cache() }.isSuccess
                writer = null
                if (whole && fetched) onFetched()
            }
        }
    }

    fun cancel() {
        writer?.cancel()
        running?.cancel(true)
        running = null
    }

    fun release() { cancel(); worker.shutdownNow() }
}
