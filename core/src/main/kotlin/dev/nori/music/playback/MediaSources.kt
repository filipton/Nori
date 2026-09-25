package dev.nori.music.playback

import android.content.Context
import dalvik.annotation.optimization.FastNative
import android.net.Uri
import androidx.media3.common.util.UnstableApi
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.cache.Cache
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.cache.CacheEvictor
import androidx.media3.datasource.cache.CacheSpan
import androidx.media3.datasource.cache.ContentMetadata
import androidx.media3.datasource.cache.NoOpCacheEvictor
import androidx.media3.datasource.cache.SimpleCache
import androidx.media3.datasource.okhttp.OkHttpDataSource
import dalvik.annotation.optimization.CriticalNative
import dev.nori.music.downloads.DownloadsJni
import dev.nori.music.ffi.Client
import dev.nori.music.ffi.net.StreamQuality
import dev.nori.music.net.Http
import dev.nori.music.settings.Settings
import dev.nori.music.settings.Quality
import java.io.File

/** The stream cache's keys and their order of use, kept in the core (crates/transfers/src/stream_cache.rs). */
internal object StreamCacheJni {
    init { System.loadLibrary("norimusic") }
    @JvmStatic @FastNative external fun touch(key: String)
    /** What the cache held when this process first looked; told once. */
    @JvmStatic external fun seed(keys: Array<String>)
    /** The next key to drop, forgotten by the core as it is handed out; null when there is none. */
    @JvmStatic @FastNative external fun next(): String?
    /** A song's streamed copies, forgotten by the core as they are handed out. */
    @JvmStatic external fun copies(id: String): Array<String>
    @JvmStatic @CriticalNative external fun clear()
}

/**
 * Eviction with a limit that follows the setting. media3's own evictor takes its maximum once, in the
 * constructor, so changing "Space for streamed music" would otherwise wait for a restart to mean
 * anything. What goes first is the core's (never used by this run, then least recently used); this
 * reports each use and, only when the cache is over its limit, drops whole resources in the order the
 * core names them. The core knows the keys, so they are handed over once rather than on every trim.
 */
class ResizableEvictor(
    @Volatile var maxBytes: Long,
    /** A piece of a song was written: [MediaSources] looks whether the song is whole now. */
    private val added: (Cache, String) -> Unit = { _, _ -> },
) : CacheEvictor {
    override fun onCacheInitialized() {}
    override fun onStartFile(cache: Cache, key: String, position: Long, length: Long) = touch(cache, key)
    override fun onSpanAdded(cache: Cache, span: CacheSpan) {
        touch(cache, span.key!!)
        added(cache, span.key!!)
    }
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
        seed(cache)
        while (cache.cacheSpace > maxBytes) {
            val key = StreamCacheJni.next() ?: return
            runCatching { cache.removeResource(key) }
        }
    }

    @Volatile private var seeded = false

    /**
     * Tells the core, once, what an earlier run left in the cache; from then on it hears of every key
     * through the callbacks above. Only asked when something is to be dropped, so a run that never
     * trims never lists the cache. Told twice in a race it is the same: the core only adds keys it
     * does not know.
     */
    fun seed(cache: Cache) {
        if (seeded) return
        seeded = true
        StreamCacheJni.seed(cache.keys.toTypedArray())
    }

    /** The core forgets every key: the cache was emptied, and whatever it still holds is told again. */
    fun forget() {
        StreamCacheJni.clear()
        seeded = false
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
    val streamEvictor = ResizableEvictor(settings.value.cacheMb * 1024L * 1024L, ::added)
    val streamCache = SimpleCache(File(context.cacheDir, "stream"), streamEvictor, database)
    val downloadCache = SimpleCache(File(context.getExternalFilesDir(null) ?: context.filesDir, "downloads"), Arrivals(::added), database)

    private val motionDir = File(context.cacheDir, "motion")

    /**
     * Moving covers (MotionPlayer): each loop is played from here after its first pass, so the video is
     * fetched once rather than once a loop. Built the first time one plays - never while they are
     * switched off - and least recently played first out past 64 MB, a dozen or so albums.
     */
    val motionCache: Cache by lazy { SimpleCache(motionDir, androidx.media3.datasource.cache.LeastRecentlyUsedCacheEvictor(64L * 1024 * 1024), database) }

    /**
     * Told when a song has become whole in either cache, on the thread that wrote its last piece (AutoMix's
     * measuring ahead, which only measures a song once it is all on the device). Null while nobody asks.
     */
    @Volatile var onWhole: (() -> Unit)? = null

    private fun added(cache: Cache, key: String) {
        val tell = onWhole ?: return
        if (isWhole(cache, key)) tell()
    }

    /** A download cache keeps everything, and says when a piece of a song was written. */
    private class Arrivals(private val added: (Cache, String) -> Unit) : CacheEvictor by NoOpCacheEvictor() {
        override fun onSpanAdded(cache: Cache, span: CacheSpan) = added(cache, span.key!!)
    }

    /** Whether [id]'s download is complete, from the core's memory of its downloads table. */
    fun isDownloaded(id: String): Boolean = DownloadsJni.held(id) == DownloadsJni.DONE

    /**
     * The stream cache's copy of [id] (a whole one first) by its key, and whether it is whole; none. For the
     * perf build's timeline, which says where a song played from; asked once a song.
     */
    fun streamCopy(id: String): Pair<String, Boolean>? {
        val keys = streamCache.keys.filter { it.startsWith("$id:") }
        return keys.firstOrNull { isWhole(streamCache, it) }?.let { it to true } ?: keys.firstOrNull()?.let { it to false }
    }

    val network: DataSource.Factory = OkHttpDataSource.Factory(http.streamFactory)

    /** The rolling cache over the network. */
    val streamCached: CacheDataSource.Factory = CacheDataSource.Factory().setCache(streamCache).setUpstreamDataSourceFactory(network)
        .setFlags(CacheDataSource.FLAG_IGNORE_CACHE_ON_ERROR)

    /**
     * What the precacher writes through: the same, except that a stretch of a song the player is writing
     * right then is waited for rather than fetched past the cache. The player loads the next song itself
     * as the one before is whole, and the precacher, finding it locked, downloaded the rest of it a
     * second time without keeping any of it.
     */
    val precaching: CacheDataSource.Factory = CacheDataSource.Factory().setCache(streamCache).setUpstreamDataSourceFactory(network)
        .setFlags(CacheDataSource.FLAG_IGNORE_CACHE_ON_ERROR or CacheDataSource.FLAG_BLOCK_ON_CACHE)

    private val cached: DataSource.Factory = CacheDataSource.Factory()
        .setCache(downloadCache)
        .setCacheWriteDataSinkFactory(null)
        .setUpstreamDataSourceFactory(streamCached)

    /** The songs to fetch ahead now, each with its address and key (`Client::precache_targets`). */
    fun precacheTargets(): List<dev.nori.music.ffi.net.Fetch> = client.precacheTargets(http.metered)

    /** Where the precacher reads [song] from, already resolved by the core. */
    fun spec(song: dev.nori.music.ffi.net.Fetch): DataSpec {
        applyStreamLimit()
        return DataSpec.Builder().setUri(Uri.parse(song.url)).setKey(song.key).build()
    }

    /**
     * A song's bytes from [from] on, at [url] under the cache key [key] as the core resolved them (the Rust
     * player opens its songs so): a download, then the stream cache, then the network. The source and how
     * many bytes are left (C.LENGTH_UNSET unknown).
     */
    fun openResolved(url: String, key: String, from: Long): Pair<DataSource, Long> {
        applyStreamLimit()
        val source = cached.createDataSource()
        return source to source.open(DataSpec.Builder().setUri(Uri.parse(url)).setKey(key).setPosition(from).build())
    }

    /**
     * A radio station's stream at [url], straight from the network (a live stream is never cached), with
     * the station's announcements asked for: the source, and the bytes of music between two announcements
     * as the station answers (`icy-metaint`; 0 when it sends none).
     */
    fun openLive(url: String): Pair<DataSource, Int> {
        val source = network.createDataSource()
        source.open(DataSpec.Builder().setUri(Uri.parse(url)).setHttpRequestHeaders(mapOf("Icy-MetaData" to "1")).build())
        val every = source.responseHeaders.entries.firstOrNull { it.key.equals("icy-metaint", ignoreCase = true) }?.value?.firstOrNull()?.trim()?.toIntOrNull()
        return source to (every ?: 0)
    }

    /** The key [resolve] would give a song that is not downloaded, without building its URL. */
    fun streamKey(id: String): String = settings.value.let { client.streamKey(id, http.metered, it.wifi.ffi(), it.mobile.ffi()) }

    private fun Quality.ffi() = StreamQuality(bitRate.coerceAtLeast(0).toUInt(), format)

    fun downloadKey(id: String) = dev.nori.music.ffi.net.downloadKey(id)

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
        runCatching { streamEvictor.seed(streamCache) }
        // The key grammar is the core's, and so is the list of keys: it names this song's copies.
        for (key in StreamCacheJni.copies(id)) runCatching { streamCache.removeResource(key) }
    }

    /** Empties the streamed-music cache; downloads, covers and the index stay. Call off the main thread. */
    fun clearStream() {
        for (key in runCatching { streamCache.keys }.getOrDefault(emptySet())) runCatching { streamCache.removeResource(key) }
        streamEvictor.forget()
    }

    companion object {
        /**
         * How much may be missing at the end of a song for it to count as whole: tags after the audio. The
         * player stops reading an MP3 where its frames end, so the ID3v1 tag after them (128 bytes) is never
         * fetched, and a song streamed through the player was never whole in the cache.
         */
        const val TAIL = 16 * 1024L

        /** Whether [cache] holds [key] from its first byte to its end, but perhaps a [TAIL]. */
        fun isWhole(cache: Cache, key: String): Boolean {
            val length = ContentMetadata.getContentLength(cache.getContentMetadata(key))
            return length > 0 && cache.getCachedLength(key, 0, length) >= length - TAIL
        }
    }
}
