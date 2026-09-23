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
import dev.nori.music.ffi.Client
import dev.nori.music.ffi.StreamQuality
import dev.nori.music.net.Http
import dev.nori.music.settings.Settings
import dev.nori.music.settings.Quality
import java.io.File

/** The stream cache's order of use, kept in the core (crates/core/src/stream_cache.rs). */
internal object StreamCacheJni {
    init { System.loadLibrary("norimusic") }
    @JvmStatic external fun touch(key: String)
    /** [keys] reordered in place into the order they leave the cache in. */
    @JvmStatic external fun order(keys: Array<String>)
}

/**
 * Eviction with a limit that follows the setting. media3's own evictor takes its maximum once, in the
 * constructor, so changing "Space for streamed music" would otherwise wait for a restart to mean
 * anything. What goes first is the core's (never used by this run, then least recently used); this
 * reports each use and, only when the cache is over its limit, drops whole resources in that order.
 */
class ResizableEvictor(@Volatile var maxBytes: Long) : CacheEvictor {
    override fun onCacheInitialized() {}
    override fun onStartFile(cache: Cache, key: String, position: Long, length: Long) = touch(cache, key)
    override fun onSpanAdded(cache: Cache, span: CacheSpan) = touch(cache, span.key!!)
    override fun onSpanRemoved(cache: Cache, span: CacheSpan) {}
    override fun onSpanTouched(cache: Cache, oldSpan: CacheSpan, newSpan: CacheSpan) = touch(cache, newSpan.key!!)
    override fun requiresCacheSpanTouches() = true

    private fun touch(cache: Cache, key: String) = synchronized(this) {
        StreamCacheJni.touch(key)
        trimLocked(cache)
    }

    /** Throws out whole resources until the cache fits. Runs wherever the caller is. */
    fun trim(cache: Cache) = synchronized(this) { trimLocked(cache) }

    private fun trimLocked(cache: Cache) {
        if (cache.cacheSpace <= maxBytes) return
        val keys = cache.keys.toTypedArray()
        StreamCacheJni.order(keys)
        for (key in keys) {
            if (cache.cacheSpace <= maxBytes) return
            runCatching { cache.removeResource(key) }
        }
    }
}

/**
 * Where audio bytes come from, in order: a finished download, the rolling
 * stream cache, the network. Both caches are keyed by song id and quality, never
 * by URL, so a replayed track costs no radio time at all.
 */
@UnstableApi
class MediaSources(context: Context, private val clientOf: () -> Client, private val http: Http, private val settings: Settings) {
    private val client get() = clientOf()
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
        // Which copy and at what quality (a download is the permanent copy; a stream follows the network the
        // phone is on and the second address's cap) is the core's, over its own settings. The network is
        // asked only for a stream: one binder call per track.
        val kept = id in downloaded
        val target = client.resolve(id, kept, !kept && http.metered)
        return dataSpec.buildUpon().setUri(Uri.parse(target.url)).setKey(target.key).build()
    }

    /** The key [resolve] would give a song that is not downloaded, without building its URL. */
    fun streamKey(id: String): String = settings.value.let { client.streamKey(id, http.metered, it.wifi.ffi(), it.mobile.ffi()) }

    private fun Quality.ffi() = StreamQuality(bitRate.coerceAtLeast(0).toUInt(), format)

    fun downloadKey(id: String) = dev.nori.music.ffi.downloadKey(id)

    fun downloadUrl(id: String): String = client.downloadTarget(id, settings.value.download.ffi()).url

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
        val keys = runCatching { streamCache.keys }.getOrDefault(emptySet())
        if (keys.isEmpty()) return
        // The key grammar is the core's; it picks this song's copies out of the cache's keys.
        for (key in dev.nori.music.ffi.streamCopies(id, keys.toList())) runCatching { streamCache.removeResource(key) }
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
