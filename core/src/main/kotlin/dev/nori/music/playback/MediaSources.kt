package dev.nori.music.playback

import android.content.Context
import android.net.Uri
import androidx.media3.common.util.UnstableApi
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.TransferListener
import androidx.media3.datasource.cache.Cache
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.cache.CacheEvictor
import androidx.media3.datasource.cache.CacheSpan
import androidx.media3.datasource.cache.NoOpCacheEvictor
import androidx.media3.datasource.cache.SimpleCache
import androidx.media3.datasource.okhttp.OkHttpDataSource
import dev.nori.music.ffi.Core
import dev.nori.music.net.Http
import dev.nori.music.settings.Settings
import dev.nori.music.settings.Quality
import java.io.File

/**
 * Least-recently-used eviction with a limit that follows the setting. media3's own evictor takes its
 * maximum once, in the constructor, so changing "Space for streamed music" would otherwise wait for a
 * restart to mean anything. Trimming mirrors what that evictor does - the stalest whole resources go
 * until the cache fits - from the write callbacks (which is where media3 calls it) and on demand.
 */
class ResizableEvictor(@Volatile var maxBytes: Long) : CacheEvictor {
    private val order = LinkedHashMap<String, Unit>(16, 0.75f, true)

    override fun onCacheInitialized() {}
    override fun onStartFile(cache: Cache, key: String, position: Long, length: Long) = touch(cache, key)
    override fun onSpanAdded(cache: Cache, span: CacheSpan) = touch(cache, span.key!!)
    override fun onSpanRemoved(cache: Cache, span: CacheSpan) {}
    override fun onSpanTouched(cache: Cache, oldSpan: CacheSpan, newSpan: CacheSpan) = touch(cache, newSpan.key!!)
    override fun requiresCacheSpanTouches() = true

    private fun touch(cache: Cache, key: String) = synchronized(this) {
        order[key] = Unit
        trimLocked(cache)
    }

    /** Throws out the stalest whole resources until the cache fits. Runs wherever the caller is. */
    fun trim(cache: Cache) = synchronized(this) { trimLocked(cache) }

    private fun trimLocked(cache: Cache) {
        if (cache.cacheSpace <= maxBytes) return
        val dropping = order.keys.toList()
        for (key in dropping) {
            if (cache.cacheSpace <= maxBytes) return
            order.remove(key)
            if (key in cache.keys) runCatching { cache.removeResource(key) }
        }
        // Keys an earlier process wrote and this one never touched are not in the order above;
        // untouched since the restart is the stalest there is, so they go first.
        for (key in cache.keys) {
            if (cache.cacheSpace <= maxBytes) return
            if (key !in order) runCatching { cache.removeResource(key) }
        }
    }
}

/**
 * Where audio bytes come from, in order: a finished download, the rolling
 * stream cache, the network. Both caches are keyed by song id and quality, never
 * by URL, so a replayed track costs no radio time at all.
 */
@UnstableApi
class MediaSources(context: Context, private val coreOf: () -> Core, private val http: Http, private val settings: Settings, private val onSecondAddress: () -> Boolean = { false }) {
    private val core get() = coreOf()
    val database = StandaloneDatabaseProvider(context)
    val streamEvictor = ResizableEvictor(settings.value.cacheMb * 1024L * 1024L)
    val streamCache = SimpleCache(File(context.cacheDir, "stream"), streamEvictor, database)
    val downloadCache = SimpleCache(File(context.getExternalFilesDir(null) ?: context.filesDir, "downloads"), NoOpCacheEvictor(), database)

    /** Ids whose download is complete; kept by [dev.nori.music.downloads.Downloads]. */
    @Volatile var downloaded: Set<String> = emptySet()

    val network: DataSource.Factory = OkHttpDataSource.Factory(http.streamFactory)

    /** The rolling cache over the network; also what the precacher writes through. */
    val streamCached: CacheDataSource.Factory = CacheDataSource.Factory().setCache(streamCache).setUpstreamDataSourceFactory(network)
        .setFlags(CacheDataSource.FLAG_IGNORE_CACHE_ON_ERROR)

    private val cached: DataSource.Factory = CacheDataSource.Factory()
        .setCache(downloadCache)
        .setCacheWriteDataSinkFactory(null)
        .setUpstreamDataSourceFactory(streamCached)

    val factory = DataSource.Factory { Switch(cached.createDataSource(), network.createDataSource()) }

    /**
     * nori://song/<id> becomes a real URL and a cache key, at the moment the bytes are needed: the quality
     * follows the network the phone is on right then.
     */
    fun resolve(dataSpec: DataSpec): DataSpec {
        applyStreamLimit()
        val id = dataSpec.uri.lastPathSegment!!
        if (id in downloaded) return dataSpec.buildUpon().setUri(Uri.parse(downloadUrl(id))).setKey(downloadKey(id)).build()
        var q = if (http.metered) settings.value.mobile else settings.value.wifi
        // Through the profile's second (usually public) address an optional ceiling applies on top.
        val cap = settings.value.server?.altMaxBitRate ?: 0
        if (onSecondAddress() && cap > 0 && (q.bitRate == 0 || q.bitRate > cap)) q = Quality(cap, q.format.ifEmpty { "opus" })
        return dataSpec.buildUpon().setUri(Uri.parse(core.streamUrl(id, q.bitRate.toUInt(), q.format))).setKey("$id:${q.key}").build()
    }

    fun downloadKey(id: String) = "dl:$id"

    fun downloadUrl(id: String): String = settings.value.download.let { core.streamUrl(id, it.bitRate.toUInt(), it.format) }

    /**
     * The limit follows the setting without a restart; checked whenever a track is opened, so the
     * player picks a change up at the next song at the latest. A settings change applies it at once
     * through [setStreamLimitMb].
     */
    private fun applyStreamLimit() {
        val want = settings.value.cacheMb * 1024L * 1024L
        if (streamEvictor.maxBytes != want) setStreamLimitMb(settings.value.cacheMb)
    }

    /** Sets the streamed-music limit now; call off the main thread, it touches the disk. */
    fun setStreamLimitMb(mb: Int) {
        streamEvictor.maxBytes = mb * 1024L * 1024L
        streamEvictor.trim(streamCache)
    }

    fun streamBytes(): Long = runCatching { streamCache.cacheSpace }.getOrDefault(0L)
    fun downloadBytes(): Long = runCatching { downloadCache.cacheSpace }.getOrDefault(0L)

    /**
     * Forgets every streamed copy of [id], whatever quality it was fetched at. A finished download is
     * the permanent copy; the streamed one is the same bytes twice. Call off the main thread.
     */
    fun dropStreamCopies(id: String) {
        for (key in runCatching { streamCache.keys }.getOrDefault(emptySet())) {
            // Keys are "$id:<quality>" and the quality never holds a colon, so this is exact.
            if (key.substringBeforeLast(':') == id) runCatching { streamCache.removeResource(key) }
        }
    }

    /** Empties the streamed-music cache; downloads, covers and the index stay. Call off the main thread. */
    fun clearStream() {
        for (key in runCatching { streamCache.keys }.getOrDefault(emptySet())) runCatching { streamCache.removeResource(key) }
    }

    /**
     * Songs are resolved when they are opened, not when they are queued, so the
     * quality follows the network the phone is on at that moment. Anything
     * else (internet radio) goes straight to the network, uncached.
     */
    private inner class Switch(private val songs: DataSource, private val plain: DataSource) : DataSource {
        private var active: DataSource? = null

        override fun open(dataSpec: DataSpec): Long {
            if (dataSpec.uri.scheme != SONG_SCHEME) return plain.also { active = it }.open(dataSpec)
            val resolved = resolve(dataSpec)
            return songs.also { active = it }.open(resolved)
        }

        override fun read(buffer: ByteArray, offset: Int, length: Int) = active!!.read(buffer, offset, length)
        override fun addTransferListener(l: TransferListener) { songs.addTransferListener(l); plain.addTransferListener(l) }
        override fun getUri() = active?.uri
        override fun getResponseHeaders() = active?.responseHeaders ?: emptyMap()
        override fun close() { active?.close(); active = null }
    }
}
