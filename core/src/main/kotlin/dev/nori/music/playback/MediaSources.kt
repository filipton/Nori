package dev.nori.music.playback

import android.content.Context
import android.net.Uri
import androidx.media3.common.util.UnstableApi
import androidx.media3.database.StandaloneDatabaseProvider
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.TransferListener
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.datasource.cache.LeastRecentlyUsedCacheEvictor
import androidx.media3.datasource.cache.NoOpCacheEvictor
import androidx.media3.datasource.cache.SimpleCache
import androidx.media3.datasource.okhttp.OkHttpDataSource
import dev.nori.music.ffi.Core
import dev.nori.music.net.Http
import dev.nori.music.settings.Settings
import dev.nori.music.settings.Quality
import java.io.File

/**
 * Where audio bytes come from, in order: a finished download, the rolling
 * stream cache, the network. Both caches are keyed by song id and quality, never
 * by URL, so a replayed track costs no radio time at all.
 */
@UnstableApi
class MediaSources(context: Context, private val coreOf: () -> Core, private val http: Http, private val settings: Settings, private val onSecondAddress: () -> Boolean = { false }) {
    private val core get() = coreOf()
    val database = StandaloneDatabaseProvider(context)
    val streamCache = SimpleCache(File(context.cacheDir, "stream"), LeastRecentlyUsedCacheEvictor(settings.value.cacheMb * 1024L * 1024L), database)
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
