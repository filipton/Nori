package dev.flint.music.downloads

import android.app.Notification
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.exoplayer.offline.DefaultDownloadIndex
import androidx.media3.exoplayer.offline.DefaultDownloaderFactory
import androidx.media3.exoplayer.offline.Download
import androidx.media3.exoplayer.offline.DownloadManager
import androidx.core.app.NotificationCompat
import androidx.media3.exoplayer.offline.DownloadRequest
import androidx.media3.exoplayer.offline.DownloadService
import androidx.media3.exoplayer.offline.Downloader
import androidx.media3.exoplayer.offline.DownloaderFactory
import androidx.media3.exoplayer.scheduler.Scheduler
import dev.flint.music.Flint
import dev.flint.music.ffi.Core
import dev.flint.music.ffi.Song
import dev.flint.music.playback.MediaSources
import dev.flint.music.settings.Settings
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

data class DownloadState(val done: List<Song> = emptyList(), val pending: List<Song> = emptyList()) {
    val doneIds: Set<String> = done.mapTo(HashSet()) { it.id }
    val pendingIds: Set<String> = pending.mapTo(HashSet()) { it.id }
}

/** media3 moves and stores the bytes; the index in Rust remembers what each file is. */
@UnstableApi
class Downloads(private val context: Context, private val coreOf: () -> Core, lazySources: Lazy<MediaSources>, private val settings: Settings) {
    private val core get() = coreOf()
    private val sources by lazySources
    private val io = Executors.newFixedThreadPool(2)
    private val main = Handler(Looper.getMainLooper())
    private val _state = MutableStateFlow(DownloadState())
    /**
     * What this batch asked for and how it is going. media3 only hands the notification the downloads
     * that are still running, so counting "done" from that list always gives nought and the total
     * shrinks as songs finish - "0 of 8", then "0 of 7". These numbers are the app's own.
     */
    private val batch = java.util.Collections.synchronizedSet(mutableSetOf<String>())
    @Volatile var batchTotal = 0
        private set
    @Volatile var batchDone = 0
        private set
    @Volatile var batchFailed = 0
        private set
    val state: StateFlow<DownloadState> = _state

    private val _marks = MutableStateFlow<Map<String, DownloadMark>>(emptyMap())
    /**
     * The songs this session's downloads are doing something with: downloading, failed, or finished
     * lately (the last [RECENT] of them). A pending song with no mark is waiting its turn. It changes
     * when a download changes phase, never with its progress, which each mark carries in a flow of its
     * own - so a list watching this map is not redrawn per percent.
     */
    val marks: StateFlow<Map<String, DownloadMark>> = _marks

    /** The progress of each download that has a task, written from that task's own thread. */
    private val progress = ConcurrentHashMap<String, MutableStateFlow<Float>>()

    private fun progressOf(request: DownloadRequest) =
        progress.getOrPut(request.id) { MutableStateFlow(if (estimateOf(request) > 0) 0f else -1f) }

    val manager: DownloadManager by lazy {
        // media3's own wiring (DefaultDownloadIndex, DefaultDownloaderFactory over the download cache),
        // with each downloader wrapped so its progress reaches [progress]. media3 itself only tells
        // whoever asks for the list of downloads, and asking would mean polling.
        val upstream = CacheDataSource.Factory().setCache(sources.downloadCache).setUpstreamDataSourceFactory(sources.network)
        DownloadManager(context, DefaultDownloadIndex(sources.database), TrackedDownloaders(DefaultDownloaderFactory(upstream, io))).apply {
            // The queue runs in the order songs were asked for, this many at a time; a failure or a
            // cancel frees its slot for the next in line. DownloadWorker keeps it in step with the setting.
            maxParallelDownloads = parallel()
            addListener(object : DownloadManager.Listener {
                override fun onDownloadChanged(m: DownloadManager, d: Download, e: Exception?) {
                    if (d.request.id in batch) {
                        if (d.state == Download.STATE_COMPLETED) batchDone++
                        if (d.state == Download.STATE_FAILED) batchFailed++
                    }
                    mark(d)
                    if (d.state == Download.STATE_COMPLETED) io.execute { core.downloadDone(d.request.id); publish() }
                    // The service's own notification goes when the service does; the batch still owes the
                    // user a word about how it went, in the same place the progress was.
                    if (m.currentDownloads.isEmpty()) summarise()
                }

                override fun onDownloadRemoved(m: DownloadManager, d: Download) {
                    unmark(listOf(d.request.id))
                    io.execute { core.downloadRemove(d.request.id); publish() }
                }
            })
        }
    }

    /** How many songs download at once, from the setting. */
    fun parallel() = settings.value.parallelDownloads.coerceIn(1, 10)

    init { io.execute { publish(); seedFailed() } }

    private fun publish() {
        val next = DownloadState(core.downloads(true), core.downloads(false))
        sources.downloaded = next.doneIds
        _state.value = next
    }

    /** Follows one download's phase into [marks]. Runs on the main thread, where media3 reports. */
    private fun mark(d: Download) {
        val id = d.request.id
        val now = SystemClock.elapsedRealtime()
        when (d.state) {
            Download.STATE_DOWNLOADING -> {
                val flow = progressOf(d.request)
                _marks.update { if (it[id]?.phase == DownloadPhase.DOWNLOADING) it else it + (id to DownloadMark(DownloadPhase.DOWNLOADING, flow, now)) }
            }
            Download.STATE_COMPLETED -> {
                progress.remove(id)
                _marks.update { recentOnly(it + (id to DownloadMark(DownloadPhase.DONE, MutableStateFlow(1f), now))) }
            }
            Download.STATE_FAILED -> {
                val last = progress.remove(id)?.value ?: -1f
                _marks.update { it + (id to DownloadMark(DownloadPhase.FAILED, MutableStateFlow(last), now)) }
            }
            // Queued, stopped, restarting or on its way out: no mark, so a pending song reads as waiting.
            else -> if (id in _marks.value) _marks.update { it - id }
        }
    }

    private fun unmark(ids: Collection<String>) {
        ids.forEach { progress.remove(it) }
        _marks.update { m -> if (ids.none { it in m }) m else m - ids.toSet() }
    }

    /** Finished marks beyond the latest [RECENT] go; the song's own "downloaded" state carries on. */
    private fun recentOnly(m: Map<String, DownloadMark>): Map<String, DownloadMark> {
        val done = m.values.count { it.phase == DownloadPhase.DONE }
        if (done <= RECENT) return m
        val drop = m.entries.filter { it.value.phase == DownloadPhase.DONE }.sortedBy { it.value.at }
            .take(done - RECENT).mapTo(HashSet()) { it.key }
        return m - drop
    }

    /**
     * Downloads that failed in an earlier session are still pending in the index; without this they
     * would read as waiting their turn. One query of media3's own table, once, off the main thread.
     */
    private fun seedFailed() {
        val pending = _state.value.pendingIds
        if (pending.isEmpty()) return
        val failed = runCatching {
            DefaultDownloadIndex(sources.database).getDownloads(Download.STATE_FAILED).use { c ->
                buildMap {
                    while (c.moveToNext()) {
                        val d = c.download
                        if (d.request.id !in pending) continue
                        val f = downloadFraction(d.contentLength, d.bytesDownloaded, estimateOf(d.request))
                        put(d.request.id, DownloadMark(DownloadPhase.FAILED, MutableStateFlow(f), 0L))
                    }
                }
            }
        }.getOrDefault(emptyMap())
        if (failed.isNotEmpty()) _marks.update { m -> failed.filterKeys { it !in m } + m }
    }

    fun download(songs: List<Song>) = io.execute {
        val known = _state.value.let { it.doneIds + it.pendingIds }
        for (s in songs) {
            if (s.id in known) continue
            core.downloadAdd(s)
            enqueue(s)
        }
        publish()
    }

    /** Sends failed downloads round again; they rejoin the queue at its end. */
    fun retry(songs: List<Song>) {
        if (songs.isEmpty()) return
        unmark(songs.map { it.id })
        io.execute {
            for (s in songs) {
                core.downloadAdd(s)
                // A retry inside the batch it failed in is the same song, not one more.
                if (s.id in batch && batchFailed > 0) {
                    batchFailed--
                    DownloadService.sendAddDownload(context, DownloadWorker::class.java, request(s), false)
                } else enqueue(s)
            }
            publish()
        }
    }

    private fun enqueue(s: Song) {
        // A batch that has finished starts the count again rather than adding to the last one. The
        // counters answer that on their own: DownloadManager may only be asked from the main thread,
        // and this runs on the download executor.
        if (batchTotal > 0 && batchDone + batchFailed >= batchTotal) {
            batch.clear(); batchTotal = 0; batchDone = 0; batchFailed = 0
        }
        batch += s.id
        batchTotal++
        DownloadService.sendAddDownload(context, DownloadWorker::class.java, request(s), false)
    }

    /**
     * The title and the expected size ride along with the request: the notification and the progress
     * ring need both, and a transcoding server usually answers without a Content-Length, which leaves
     * media3's own progress empty and the download looking stuck.
     */
    private fun request(s: Song): DownloadRequest {
        val expected = expectedBytes(s.size.toLong(), s.duration.toLong(), settings.value.download.bitRate)
        return DownloadRequest.Builder(s.id, Uri.parse(sources.downloadUrl(s.id)))
            .setCustomCacheKey(sources.downloadKey(s.id))
            .setData("$expected\n${s.title}".toByteArray())
            .build()
    }

    /**
     * The same notification the progress was in, turned into the result: replacing id 1001 means the
     * bar does not sit there empty next to a second notification once the service has let go of it.
     */
    private fun summarise() {
        if (batchTotal == 0) return
        val done = batchDone
        val failed = batchFailed
        batch.clear(); batchTotal = 0; batchDone = 0; batchFailed = 0
        if (done == 0 && failed == 0) return
        val manager = context.getSystemService(android.app.NotificationManager::class.java) ?: return
        val text = listOfNotNull(
            "$done song${if (done == 1) "" else "s"} downloaded".takeIf { done > 0 },
            "$failed failed".takeIf { failed > 0 },
        ).joinToString(" · ")
        // The service may still hold the foreground notification for a moment; replacing it after it
        // has gone leaves exactly one, which is what a finished download should look like.
        io.execute {
            Thread.sleep(1200)
            manager.notify(
                DOWNLOAD_NOTIFICATION,
                NotificationCompat.Builder(context, "downloads")
                    .setSmallIcon(android.R.drawable.stat_sys_download_done)
                    .setContentTitle(text)
                    .setContentIntent(openDownloads(context))
                    .setAutoCancel(true)
                    .setOngoing(false)
                    .build(),
            )
        }
    }

    fun remove(ids: List<String>) = ids.forEach { DownloadService.sendRemoveDownload(context, DownloadWorker::class.java, it, false) }

    /**
     * Stops downloads that have not finished and forgets them. The index entry goes at once as well as
     * through media3's callback: a song left pending by an earlier session may have no media3 download
     * to remove, and would otherwise sit in the queue for ever.
     */
    fun cancel(ids: List<String>) {
        if (ids.isEmpty()) return
        dropFromBatch(ids)
        unmark(ids)
        remove(ids)
        io.execute { ids.forEach { core.downloadRemove(it) }; publish() }
    }

    /**
     * Everything not yet downloaded, in one go: straight to the manager rather than one service intent
     * per song, which for a whole library queued would be thousands. Finished downloads stay, which
     * media3's own "remove all" would not leave.
     */
    fun cancelAll() {
        val ids = _state.value.pending.map { it.id }
        if (ids.isEmpty()) return
        dropFromBatch(ids)
        unmark(ids)
        main.post { ids.forEach { manager.removeDownload(it) } }
        io.execute { ids.forEach { core.downloadRemove(it) }; publish() }
    }

    /** A song stopped before it finished no longer counts towards "3 of 8" in the notification. */
    private fun dropFromBatch(ids: Collection<String>) {
        val marks = _marks.value
        for (id in ids) if (batch.remove(id)) {
            batchTotal = (batchTotal - 1).coerceAtLeast(0)
            if (marks[id]?.phase == DownloadPhase.FAILED && batchFailed > 0) batchFailed--
        }
    }

    /** Hands each downloader's progress to [progress], through a [ProgressGate] so it arrives at a drawable rate. */
    private inner class TrackedDownloaders(private val inner: DownloaderFactory) : DownloaderFactory {
        override fun createDownloader(request: DownloadRequest): Downloader {
            val downloader = inner.createDownloader(request)
            val estimate = estimateOf(request)
            val flow = progressOf(request)
            val gate = ProgressGate()
            return object : Downloader {
                override fun download(listener: Downloader.ProgressListener?) = downloader.download { length, bytes, percent ->
                    listener?.onProgress(length, bytes, percent)
                    val f = downloadFraction(length, bytes, estimate)
                    if (gate.offer(f, SystemClock.elapsedRealtime())) flow.value = f
                }
                override fun cancel() = downloader.cancel()
                override fun remove() = downloader.remove()
            }
        }
    }

    companion object {
        /** Finished downloads the downloads screen keeps listing this session. */
        const val RECENT = 50
    }
}

/** What a request says the song will weigh: the first line of its data (see [Downloads.request]). */
private fun estimateOf(request: DownloadRequest): Long =
    request.data.decodeToString().substringBefore('\n').toLongOrNull() ?: 0L

/** Asks the app to open on its downloads screen. The activity answers it; the core only names it. */
const val ACTION_OPEN_DOWNLOADS = "dev.flint.music.OPEN_DOWNLOADS"

@Volatile private var openDownloadsIntent: PendingIntent? = null

/**
 * Where a tap on a download notification goes: the app's launcher activity, asked to show its
 * downloads. The action differs from the launcher's own, so a running app is handed the request
 * (onNewIntent) instead of only being brought to the front.
 */
fun openDownloads(context: Context): PendingIntent? = openDownloadsIntent ?: run {
    val launcher = context.packageManager.getLaunchIntentForPackage(context.packageName)?.component ?: return null
    val intent = Intent(ACTION_OPEN_DOWNLOADS).setComponent(launcher)
        .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP)
    PendingIntent.getActivity(context, 1, intent, PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE)
        .also { openDownloadsIntent = it }
}

/** One id for the whole life of a download batch: progress, then the result, in the same place. */
const val DOWNLOAD_NOTIFICATION = 1001

@UnstableApi
class DownloadWorker : DownloadService(DOWNLOAD_NOTIFICATION, 1000L, CHANNEL, androidx.media3.exoplayer.R.string.exo_download_notification_channel_name, 0) {
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
     * is happening. This one counts the bytes itself, sizes them against what each song should weigh,
     * and says what is being fetched, how fast, and how much longer.
     */
    override fun getForegroundNotification(downloads: MutableList<Download>, notMetRequirements: Int): Notification {
        val downloads2 = Flint.get(this).downloads
        // This runs once a second while anything downloads, on the main thread the manager lives on: the
        // place to follow a change to "Downloads at once" without a listener of its own.
        downloads2.parallel().let { if (downloads2.manager.maxParallelDownloads != it) downloads2.manager.maxParallelDownloads = it }
        val done = downloads2.batchDone
        val failed = downloads2.batchFailed
        val total = downloads2.batchTotal.coerceAtLeast(downloads.size)
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

        val running = downloads.filter { it.state == Download.STATE_DOWNLOADING }
            .mapNotNull { it.request.data.decodeToString().substringAfter('\n').takeIf(String::isNotBlank) }
        val current = when (running.size) {
            0 -> null
            1 -> running[0]
            else -> "${running[0]} and ${running.size - 1} more"
        }
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
            total > 1 -> "Downloading ${(done + 1).coerceAtMost(total)} of $total songs"
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
            .setContentIntent(openDownloads(this))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .build()
    }
}
