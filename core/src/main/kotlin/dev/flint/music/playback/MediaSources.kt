package dev.flint.music.playback

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
import dev.flint.music.ffi.Core
import dev.flint.music.net.Http
import dev.flint.music.settings.Settings
import java.io.File

/**
 * Where audio bytes come from, in order: a finished download, the rolling
 * stream cache, the network. Both caches are keyed by song id and quality, never
 * by URL, so a replayed track costs no radio time at all.
 */
@UnstableApi
class MediaSources(context: Context, private val core: Core, private val http: Http, private val settings: Settings) {
    val database = StandaloneDatabaseProvider(context)
    val streamCache = SimpleCache(File(context.cacheDir, "stream"), LeastRecentlyUsedCacheEvictor(settings.value.cacheMb * 1024L * 1024L), database)
    val downloadCache = SimpleCache(File(context.getExternalFilesDir(null) ?: context.filesDir, "downloads"), NoOpCacheEvictor(), database)

    /** Ids whose download is complete; kept by [dev.flint.music.downloads.Downloads]. */
    @Volatile var downloaded: Set<String> = emptySet()

    val network: DataSource.Factory = OkHttpDataSource.Factory(http.stream)

    private val cached: DataSource.Factory = CacheDataSource.Factory()
        .setCache(downloadCache)
        .setCacheWriteDataSinkFactory(null)
        .setUpstreamDataSourceFactory(
            CacheDataSource.Factory().setCache(streamCache).setUpstreamDataSourceFactory(network)
                .setFlags(CacheDataSource.FLAG_IGNORE_CACHE_ON_ERROR)
        )

    val factory = DataSource.Factory { Switch(cached.createDataSource(), network.createDataSource()) }

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
            val id = dataSpec.uri.lastPathSegment!!
            val resolved = if (id in downloaded) {
                dataSpec.buildUpon().setUri(Uri.parse(downloadUrl(id))).setKey(downloadKey(id)).build()
            } else {
                val q = if (http.metered) settings.value.mobile else settings.value.wifi
                dataSpec.buildUpon().setUri(Uri.parse(core.streamUrl(id, q.bitRate.toUInt(), q.format))).setKey("$id:${q.key}").build()
            }
            return songs.also { active = it }.open(resolved)
        }

        override fun read(buffer: ByteArray, offset: Int, length: Int) = active!!.read(buffer, offset, length)
        override fun addTransferListener(l: TransferListener) { songs.addTransferListener(l); plain.addTransferListener(l) }
        override fun getUri() = active?.uri
        override fun getResponseHeaders() = active?.responseHeaders ?: emptyMap()
        override fun close() { active?.close(); active = null }
    }
}
