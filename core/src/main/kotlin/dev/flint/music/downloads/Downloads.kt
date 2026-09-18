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
    /** What this batch asked for, so the closing notification reports the batch and not the library. */
    private val batch = java.util.Collections.synchronizedSet(mutableSetOf<String>())
    val state: StateFlow<DownloadState> = _state

    val manager: DownloadManager by lazy {
        DownloadManager(context, sources.database, sources.downloadCache, sources.network, io).apply {
            maxParallelDownloads = 2
            addListener(object : DownloadManager.Listener {
                override fun onDownloadChanged(m: DownloadManager, d: Download, e: Exception?) {
                    if (d.state == Download.STATE_COMPLETED) io.execute { core.downloadDone(d.request.id); publish() }
                    // The foreground notification disappears with the service; a batch that took a while
                    // should still say how it went.
                    if (m.currentDownloads.isEmpty()) summarise(m)
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
            batch += s.id
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

    /** "12 songs downloaded" (or how many did not), once the batch that was asked for finishes. */
    private fun summarise(m: DownloadManager) {
        val wanted = synchronized(batch) { batch.toSet() }
        if (wanted.isEmpty()) return
        synchronized(batch) { batch.clear() }
        val all = m.downloadIndex.getDownloads().use { c -> generateSequence { if (c.moveToNext()) c.download else null }.toList() }
            .filter { it.request.id in wanted }
        val failed = all.count { it.state == Download.STATE_FAILED }
        val done = all.count { it.state == Download.STATE_COMPLETED }
        if (done == 0 && failed == 0) return
        val manager = context.getSystemService(android.app.NotificationManager::class.java) ?: return
        val text = listOfNotNull(
            "$done song${if (done == 1) "" else "s"} downloaded".takeIf { done > 0 },
            "$failed failed".takeIf { failed > 0 },
        ).joinToString(" · ")
        manager.notify(
            1002,
            NotificationCompat.Builder(context, "downloads")
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentTitle(text)
                .setAutoCancel(true)
                .build(),
        )
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
        val done = downloads.count { it.state == Download.STATE_COMPLETED }
        val failed = downloads.count { it.state == Download.STATE_FAILED }
        val total = downloads.size
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

        val current = downloads.firstOrNull { it.state == Download.STATE_DOWNLOADING }
            ?.request?.data?.decodeToString()?.substringAfter('\n')?.takeIf { it.isNotBlank() }
        val remaining = (expected - bytes).coerceAtLeast(0)
        val eta = if (speedBps > 1024 && remaining > 0) remaining / speedBps else -1.0
        fun time(seconds: Double) = when {
            seconds < 60 -> "${seconds.toInt()} s left"
            seconds < 3600 -> "${(seconds / 60).toInt()} min left"
            else -> "%.1f h left".format(seconds / 3600)
        }
        fun mb(v: Long) = if (v >= 1_000_000_000) "%.1f GB".format(v / 1e9) else "%.0f MB".format(v / 1e6)

        val headline = when {
            notMetRequirements != 0 -> "Waiting for the network"
            total > 1 -> "Downloading $done of $total songs"
            else -> "Downloading"
        }
        val detail = listOfNotNull(
            current,
            "${mb(bytes)} of ${mb(expected)}".takeIf { expected > 0 },
            "%.1f MB/s".format(speedBps / 1_000_000).takeIf { speedBps > 1024 },
            eta.takeIf { it >= 0 }?.let(::time),
            "$failed failed".takeIf { failed > 0 },
        ).joinToString(" · ")

        val percent = if (expected > 0) ((bytes * 100) / expected).toInt().coerceIn(0, 100) else -1
        return NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle(headline)
            // Collapsed, Android shows the title, the sub-text and the bar; expanded, it shows this too.
            .setContentText(detail.ifEmpty { "Starting…" })
            .setSubText(detail.ifEmpty { null })
            .setStyle(NotificationCompat.BigTextStyle().bigText(detail.ifEmpty { "Starting…" }))
            .setProgress(100, percent.coerceAtLeast(0), percent < 0)
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()
    }
}
