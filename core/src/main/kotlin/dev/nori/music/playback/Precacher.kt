package dev.nori.music.playback

import androidx.media3.common.C
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.cache.CacheWriter
import androidx.media3.datasource.cache.ContentMetadata
import java.util.concurrent.Executors
import java.util.concurrent.Future

/**
 * Fetches whole upcoming tracks into the stream cache right when a track starts, which is a moment
 * the radio is awake anyway; when their turn comes they play from disk and the modem stays asleep.
 * One track at a time, in queue order, abandoned as soon as the queue moves on. Provider tracks from
 * octo-fiesta are never fetched ahead: asking for one makes the server download it.
 *
 * A song the player is writing into the cache right then is left to the player: it is fetching it
 * itself. Waiting for it held every song after it back for as long as the player kept the song open,
 * and woke the precacher each time anything at all was written to the cache.
 */
@UnstableApi
class Precacher(private val sources: MediaSources) {
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
                if (beingWritten(song.key)) {
                    android.util.Log.i("nori", "precache: ${song.key} is being fetched by the player, left to it")
                    continue
                }
                runCatching { CacheWriter(sources.precaching.createDataSource(), sources.spec(song), null, null).also { writer = it }.cache() }
                writer = null
            }
        }
    }

    /**
     * Whether someone else holds the first stretch of [key] the cache is missing: the player, loading
     * it. Asked once per song, without waiting.
     */
    private fun beingWritten(key: String): Boolean = runCatching {
        val cache = sources.streamCache
        val length = ContentMetadata.getContentLength(cache.getContentMetadata(key))
        val from = if (length > 0) cache.getCachedLength(key, 0, length).coerceAtLeast(0) else 0L
        if (length > 0 && from >= length) return@runCatching false
        val span = cache.startReadWriteNonBlocking(key, from, if (length > 0) length - from else C.LENGTH_UNSET.toLong()) ?: return@runCatching true
        if (!span.isCached) cache.releaseHoleSpan(span)
        false
    }.getOrDefault(false)

    fun cancel() {
        writer?.cancel()
        running?.cancel(true)
        running = null
    }

    fun release() { cancel(); worker.shutdownNow() }
}
