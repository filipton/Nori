package dev.flint.music.downloads

import android.app.Notification
import android.content.Context
import android.net.Uri
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.offline.Download
import androidx.media3.exoplayer.offline.DownloadManager
import androidx.media3.exoplayer.offline.DownloadNotificationHelper
import androidx.media3.exoplayer.offline.DownloadRequest
import androidx.media3.exoplayer.offline.DownloadService
import androidx.media3.exoplayer.scheduler.Scheduler
import dev.flint.music.Flint
import dev.flint.music.ffi.Core
import dev.flint.music.ffi.Song
import dev.flint.music.playback.MediaSources
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.util.concurrent.Executors

data class DownloadState(val done: List<Song> = emptyList(), val pending: List<Song> = emptyList()) {
    val doneIds: Set<String> = done.mapTo(HashSet()) { it.id }
    val pendingIds: Set<String> = pending.mapTo(HashSet()) { it.id }
}

/** media3 moves and stores the bytes; the index in Rust remembers what each file is. */
@UnstableApi
class Downloads(private val context: Context, lazyCore: Lazy<Core>, lazySources: Lazy<MediaSources>) {
    private val core by lazyCore
    private val sources by lazySources
    private val io = Executors.newFixedThreadPool(2)
    private val _state = MutableStateFlow(DownloadState())
    val state: StateFlow<DownloadState> = _state

    val manager: DownloadManager by lazy {
        DownloadManager(context, sources.database, sources.downloadCache, sources.network, io).apply {
            maxParallelDownloads = 2
            addListener(object : DownloadManager.Listener {
                override fun onDownloadChanged(m: DownloadManager, d: Download, e: Exception?) {
                    if (d.state == Download.STATE_COMPLETED) io.execute { core.downloadDone(d.request.id); publish() }
                }

                override fun onDownloadRemoved(m: DownloadManager, d: Download) {
                    io.execute { core.downloadRemove(d.request.id); publish() }
                }
            })
        }
    }

    init { io.execute(::publish) }

    private fun publish() {
        val next = DownloadState(core.downloads(true), core.downloads(false))
        sources.downloaded = next.doneIds
        _state.value = next
    }

    fun download(songs: List<Song>) = io.execute {
        val known = _state.value.let { it.doneIds + it.pendingIds }
        for (s in songs) {
            if (s.id in known) continue
            core.downloadAdd(s)
            val request = DownloadRequest.Builder(s.id, Uri.parse(sources.downloadUrl(s.id))).setCustomCacheKey(sources.downloadKey(s.id)).build()
            DownloadService.sendAddDownload(context, DownloadWorker::class.java, request, false)
        }
        publish()
    }

    fun remove(ids: List<String>) = ids.forEach { DownloadService.sendRemoveDownload(context, DownloadWorker::class.java, it, false) }
}

@UnstableApi
class DownloadWorker : DownloadService(1001, 1000L, "downloads", androidx.media3.exoplayer.R.string.exo_download_notification_channel_name, 0) {
    override fun getDownloadManager(): DownloadManager = Flint.get(this).downloads.manager
    override fun getScheduler(): Scheduler? = null
    override fun getForegroundNotification(downloads: MutableList<Download>, notMetRequirements: Int): Notification =
        DownloadNotificationHelper(this, "downloads").buildProgressNotification(this, android.R.drawable.stat_sys_download, null, null, downloads, notMetRequirements)
}
