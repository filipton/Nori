package dev.flint.music.downloads

import android.app.Notification
import android.content.Context
import android.net.Uri
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.offline.Download
import androidx.media3.exoplayer.offline.DownloadManager
import androidx.core.app.NotificationCompat
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
class Downloads(private val context: Context, private val coreOf: () -> Core, lazySources: Lazy<MediaSources>) {
    private val core get() = coreOf()
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
            // The title and the expected size ride along with the request: the notification needs both,
            // and a transcoding server usually answers without a Content-Length, which leaves media3's
            // own progress bar empty and the download looking stuck.
            val request = DownloadRequest.Builder(s.id, Uri.parse(sources.downloadUrl(s.id)))
                .setCustomCacheKey(sources.downloadKey(s.id))
                .setData("${s.size}\n${s.title}".toByteArray())
                .build()
            DownloadService.sendAddDownload(context, DownloadWorker::class.java, request, false)
        }
        publish()
    }

    fun remove(ids: List<String>) = ids.forEach { DownloadService.sendRemoveDownload(context, DownloadWorker::class.java, it, false) }
}

@UnstableApi
class DownloadWorker : DownloadService(1001, 1000L, CHANNEL, androidx.media3.exoplayer.R.string.exo_download_notification_channel_name, 0) {
    companion object {
        private const val CHANNEL = "downloads"
        /** Last sample of total bytes and when it was taken, for a speed that means something. */
        private var lastBytes = 0L
        private var lastAt = 0L
        private var speedBps = 0.0
    }

    override fun getDownloadManager(): DownloadManager = Flint.get(this).downloads.manager
    override fun getScheduler(): Scheduler? = null

    /**
     * media3's stock notification shows a percentage it cannot compute when the server omits
     * Content-Length, which is most of the time here: an empty bar, no names, no idea whether anything
     * is happening. This one counts the bytes itself, sizes them against what the server said each song
     * weighs, and says what is being fetched, how fast, and how much longer.
     */
    override fun getForegroundNotification(downloads: MutableList<Download>, notMetRequirements: Int): Notification {
        val active = downloads.filter { it.state == Download.STATE_DOWNLOADING || it.state == Download.STATE_QUEUED }
        val bytes = downloads.sumOf { it.bytesDownloaded }
        val expected = downloads.sumOf { d ->
            val stated = d.contentLength.takeIf { it > 0 } ?: 0L
            if (stated > 0) stated else d.request.data.decodeToString().substringBefore('\n').toLongOrNull() ?: 0L
        }
        val now = android.os.SystemClock.elapsedRealtime()
        if (lastAt > 0 && now > lastAt && bytes >= lastBytes) {
            // Smoothed, or the figure jumps about wildly between one-second updates.
            val sample = (bytes - lastBytes) * 1000.0 / (now - lastAt)
            speedBps = if (speedBps == 0.0) sample else speedBps * 0.6 + sample * 0.4
        }
        lastBytes = bytes
        lastAt = now

        val current = active.firstOrNull { it.state == Download.STATE_DOWNLOADING }
            ?.request?.data?.decodeToString()?.substringAfter('\n')?.takeIf { it.isNotBlank() }
        val remaining = (expected - bytes).coerceAtLeast(0)
        val eta = if (speedBps > 1024 && remaining > 0) remaining / speedBps else -1.0
        val detail = listOfNotNull(
"${downloads.count { it.state == Download.STATE_COMPLETED } + 1}/${downloads.size}".takeIf { downloads.size > 1 },
            "%.1f MB/s".format(speedBps / 1_000_000).takeIf { speedBps > 1024 },
            when {
                eta < 0 -> null
                eta < 60 -> "${eta.toInt()} s left"
                else -> "${(eta / 60).toInt()} min left"
            },
        ).joinToString(" · ")

        val percent = if (expected > 0) ((bytes * 100) / expected).toInt().coerceIn(0, 100) else -1
        // Collapsed, Android shows the title, the sub-text and the bar, and drops the content text - so
        // the numbers worth reading at a glance go in the sub-text, not below it.
        val line = listOfNotNull(
            if (expected > 0) "%.0f of %.0f MB".format(bytes / 1e6, expected / 1e6) else null,
            detail.ifEmpty { null },
        ).joinToString(" · ")
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle(current ?: if (downloads.size > 1) "Downloading ${downloads.size} songs" else "Downloading")
            .setContentText(line.ifEmpty { if (notMetRequirements != 0) "Waiting for the network" else "Starting…" })
            .setSubText(line.ifEmpty { null })
            .setProgress(100, percent.coerceAtLeast(0), percent < 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()
    }
}
