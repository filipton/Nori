package dev.nori.music.playback

import androidx.media3.common.MediaItem
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSpec
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
class Precacher(private val sources: MediaSources) {
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "nori-precache").apply { priority = Thread.MIN_PRIORITY } }
    private var running: Future<*>? = null
    @Volatile private var writer: CacheWriter? = null

    fun update(upcoming: List<MediaItem>, skip: (String) -> Boolean = { false }) {
        cancel()
        // A song the download queue is already fetching arrives permanently; pulling it into the
        // rolling cache too keeps it twice.
        val ids = dev.nori.music.ffi.queueFetchable(upcoming.map { it.mediaId }).filterNot(skip)
        if (ids.isEmpty()) return
        running = worker.submit {
            for (id in ids) {
                if (Thread.currentThread().isInterrupted) return@submit
                if (id in sources.downloaded) continue
                runCatching { CacheWriter(sources.streamCached.createDataSource(), sources.resolve(DataSpec(songUri(id))), null, null).also { writer = it }.cache() }
                writer = null
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
