package dev.nori.music.downloads

import android.app.Notification
import android.app.NotificationManager
import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.cache.CacheDataSource
import androidx.media3.exoplayer.offline.DefaultDownloadIndex
import androidx.media3.exoplayer.offline.DefaultDownloaderFactory
import androidx.media3.exoplayer.offline.Download
import androidx.media3.exoplayer.offline.DownloadManager
import androidx.media3.exoplayer.offline.DownloadRequest
import androidx.media3.exoplayer.offline.DownloadService
import androidx.media3.exoplayer.offline.Downloader
import androidx.media3.exoplayer.offline.DownloaderFactory
import androidx.media3.exoplayer.scheduler.Scheduler
import dev.nori.music.Nori
import dev.nori.music.ffi.Core
import dev.nori.music.ffi.Song
import dev.nori.music.playback.MediaSources
import dev.nori.music.settings.Settings
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

data class DownloadState(val done: List<Song> = emptyList(), val pending: List<Song> = emptyList()) {
    val doneIds: Set<String> = done.mapTo(HashSet()) { it.id }
    val pendingIds: Set<String> = pending.mapTo(HashSet()) { it.id }
}

/**
 * media3 moves and stores the bytes; the index in Rust remembers what each file is.
 *
 * media3 keeps its own queue in [DefaultDownloadIndex], which outlives the process: a force stop
 * mid-download leaves it saying "queued" for every song that had not finished. Everything this class
 * shows - the marks, the batch the notification counts - is rebuilt from that queue and from what the
 * manager reports, never kept only in memory, and the service is started again at launch whenever the
 * index says something is unfinished (see [resume]).
 */
@UnstableApi
class Downloads(private val context: Context, private val coreOf: () -> Core, lazySources: Lazy<MediaSources>, private val settings: Settings) {
    private val core get() = coreOf()
    private val sources by lazySources
    /** The index's bookkeeping. Downloads never run here: see [TrackedDownloaders]. */
    private val io = Executors.newFixedThreadPool(2)
    private val main = Handler(Looper.getMainLooper())
    private val _state = MutableStateFlow(DownloadState())
    val state: StateFlow<DownloadState> = _state

    /** The run of the queue the notification counts. Main thread only, where the manager reports. */
    private val batch = DownloadBatch()

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

    /** How fast the batch is moving, for the notification and the downloads screen. */
    private val _stats = MutableStateFlow(DownloadStats())
    val stats: StateFlow<DownloadStats> = _stats

    /** How fast each running song is arriving, written from its task's own thread. */
    private val _tempos = MutableStateFlow<Map<String, DownloadTempo>>(emptyMap())
    val tempos: StateFlow<Map<String, DownloadTempo>> = _tempos

    /** Latest byte counts per running download, for speed and ETA. Task threads only. */
    private val bytesOf = ConcurrentHashMap<String, Long>()
    private val totalOf = ConcurrentHashMap<String, Long>()
    private data class SpeedSample(var bytes: Long, var at: Long, var rate: Double)
    private val speedOf = ConcurrentHashMap<String, SpeedSample>()
    @Volatile private var lastTemposAt = 0L

    private fun progressOf(request: DownloadRequest) =
        progress.getOrPut(request.id) { MutableStateFlow(if (estimateOf(request) > 0) 0f else -1f) }

    val manager: DownloadManager by lazy {
        // media3's own wiring (DefaultDownloadIndex, DefaultDownloaderFactory over the download cache),
        // with each downloader wrapped so its progress reaches [progress]. media3 itself only tells
        // whoever asks for the list of downloads, and asking would mean polling.
        //
        // Runnable::run: each download copies its bytes on the thread media3 already gave it. Handing
        // them to a pool instead caps the downloads that actually move at the pool's size, whatever
        // maxParallelDownloads says - a pool of two was why "5 at once" moved two songs and left three
        // rings standing at nought.
        val upstream = CacheDataSource.Factory().setCache(sources.downloadCache).setUpstreamDataSourceFactory(sources.network)
        DownloadManager(context, DefaultDownloadIndex(sources.database), TrackedDownloaders(DefaultDownloaderFactory(upstream, Runnable::run))).apply {
            // The queue runs in the order songs were asked for, this many at a time; a failure or a
            // cancel frees its slot for the next in line. DownloadWorker keeps it in step with the setting.
            maxParallelDownloads = parallel()
            addListener(object : DownloadManager.Listener {
                // The queue as media3 restored it from its index: the downloads a force stop interrupted
                // start again from where their bytes end, and the batch counts them from here.
                override fun onInitialized(m: DownloadManager) {
                    for (d in m.currentDownloads) follow(d)
                }

                override fun onDownloadChanged(m: DownloadManager, d: Download, e: Exception?) {
                    follow(d)
                    if (d.state == Download.STATE_COMPLETED) io.execute {
                        // The download is the permanent copy now; the streamed one is the same bytes
                        // twice, so it goes. (A song streamed before it was downloaded lives in both.)
                        sources.dropStreamCopies(d.request.id)
                        core.downloadDone(d.request.id); publish()
                    }
                }

                override fun onDownloadRemoved(m: DownloadManager, d: Download) {
                    val wasOpen = batch.isOpen(d.request.id)
                    batch.removed(d.request.id)
                    unmark(listOf(d.request.id))
                    io.execute { core.downloadRemove(d.request.id); publish() }
                    if (wasOpen && batch.drained) summarise()
                }
            })
        }
    }

    /** How many songs download at once, from the setting. */
    fun parallel() = settings.value.parallelDownloads.coerceIn(1, 10)

    /** Whether the queue has been handed back to media3 in this process; see [resume]. */
    @Volatile private var resumed = false

    init { io.execute { publish(); reconcile() } }

    private fun publish() {
        val next = DownloadState(core.downloads(true), core.downloads(false))
        sources.downloaded = next.doneIds
        _state.value = next
    }

    /**
     * Picks up what an earlier process left unfinished. Asked at launch (from the application, and
     * again once there is a screen, since a process started in the background may not start services).
     * Costs one query of media3's table when something is pending and nothing at all otherwise.
     */
    fun resume() {
        if (!resumed) io.execute { if (!resumed) reconcile() }
    }

    /**
     * Brings the Rust index and media3's queue back into agreement after the process died:
     * - finished in media3 but not recorded here (the process went between the two): recorded now;
     * - never reached media3 (the add was still in flight): asked for again;
     * - failed: marked failed, so the song reads as failed rather than waiting;
     * - queued or interrupted mid-download: the download service is started, which starts the manager,
     *   which restores and resumes them.
     */
    private fun reconcile() {
        val pending = _state.value.pending
        if (pending.isEmpty()) { resumed = true; return }
        val stored = runCatching {
            DefaultDownloadIndex(sources.database).getDownloads().use { c ->
                buildMap { while (c.moveToNext()) c.download.let { put(it.request.id, it) } }
            }
        }.getOrDefault(emptyMap())
        val failed = HashMap<String, DownloadMark>()
        val lost = ArrayList<Song>()
        var finished = false
        var unfinished = false
        for (s in pending) {
            val d = stored[s.id]
            when (d?.state) {
                null, Download.STATE_REMOVING -> lost += s
                Download.STATE_COMPLETED -> { core.downloadDone(s.id); finished = true; sources.dropStreamCopies(s.id) }
                Download.STATE_FAILED -> failed[s.id] = DownloadMark(
                    DownloadPhase.FAILED, MutableStateFlow(downloadFraction(d.contentLength, d.bytesDownloaded, estimateOf(d.request))), 0L,
                )
                else -> unfinished = true
            }
        }
        if (finished) publish()
        if (failed.isNotEmpty()) _marks.update { m -> failed.filterKeys { it !in m } + m }
        if (!unfinished && lost.isEmpty()) { resumed = true; return }
        Log.i(TAG, "resuming: ${pending.size} pending, ${lost.size} asked for again, ${failed.size} failed")
        main.post {
            resumed = runCatching {
                // The first intent brings the service - and with it the manager and its restored queue - up.
                if (lost.isEmpty()) DownloadService.start(context, DownloadWorker::class.java)
                for (s in lost) DownloadService.sendAddDownload(context, DownloadWorker::class.java, request(s), false)
            }.onFailure { Log.w(TAG, "could not start the download service yet", it) }.isSuccess
        }
    }

    /**
     * Follows one download into [batch] and [marks]. Runs on the main thread, where media3 reports.
     * When the last song of a batch settles, the batch says how it went.
     */
    private fun follow(d: Download) {
        val id = d.request.id
        val wasOpen = batch.isOpen(id)
        when (d.state) {
            Download.STATE_QUEUED, Download.STATE_DOWNLOADING, Download.STATE_RESTARTING, Download.STATE_STOPPED ->
                if (batch.queued(id, albumOf(d.request))) cancelResult()
            Download.STATE_COMPLETED -> batch.completed(id)
            Download.STATE_FAILED -> batch.failed(id)
        }
        mark(d)
        if (wasOpen && batch.drained) summarise()
    }

    private fun mark(d: Download) {
        val id = d.request.id
        val now = SystemClock.elapsedRealtime()
        when (d.state) {
            Download.STATE_DOWNLOADING -> {
                val flow = progressOf(d.request)
                if (_marks.value[id]?.phase != DownloadPhase.DOWNLOADING) {
                    _marks.update { it + (id to DownloadMark(DownloadPhase.DOWNLOADING, flow, now)) }
                    Log.d(TAG, "start $id: ${running()} downloading at once")
                }
            }
            Download.STATE_COMPLETED -> {
                progress.remove(id)
                dropTempo(id)
                _marks.update { recentOnly(it + (id to DownloadMark(DownloadPhase.DONE, MutableStateFlow(1f), now))) }
            }
            Download.STATE_FAILED -> {
                val last = progress.remove(id)?.value ?: -1f
                dropTempo(id)
                _marks.update { it + (id to DownloadMark(DownloadPhase.FAILED, MutableStateFlow(last), now)) }
                Log.w(TAG, "failed $id")
            }
            // Queued, stopped, restarting or on its way out: no mark, so a pending song reads as waiting.
            else -> if (id in _marks.value) _marks.update { it - id }
        }
    }

    /**
     * What a task's thread reports: bytes so far and what the song should weigh. Folds the delta
     * into a smoothed per-song rate and, at most twice a second, publishes every running song's
     * tempo for the downloads screen.
     */
    private fun noteBytes(id: String, bytes: Long, total: Long, now: Long) {
        bytesOf[id] = bytes
        if (total > 0) totalOf[id] = total
        val sample = speedOf.getOrPut(id) { SpeedSample(bytes, now, 0.0) }
        val dt = (now - sample.at) / 1000.0
        if (dt >= 0.4 && bytes >= sample.bytes) {
            val instant = (bytes - sample.bytes) / dt
            sample.rate = if (sample.rate <= 0) instant else sample.rate * 0.7 + instant * 0.3
            sample.bytes = bytes
            sample.at = now
        }
        if (now - lastTemposAt >= 500) {
            lastTemposAt = now
            _tempos.value = bytesOf.entries.associate { (song, b) ->
                val t = (totalOf[song] ?: 0L) - b
                song to DownloadTempo(speedOf[song]?.rate?.toLong() ?: 0L, t.coerceAtLeast(0L))
            }
        }
    }

    private fun dropTempo(id: String) {
        bytesOf.remove(id); totalOf.remove(id); speedOf.remove(id)
        if (id in _tempos.value) _tempos.value = _tempos.value - id
        if (speedOf.isEmpty()) _stats.value = DownloadStats()
    }

    /** How many songs are downloading right now. */
    fun running(): Int = _marks.value.values.count { it.phase == DownloadPhase.DOWNLOADING }

    private fun unmark(ids: Collection<String>) {
        ids.forEach { progress.remove(it); dropTempo(it) }
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
     * Queues what is not downloaded yet. A song the index calls pending but the queue is not working on
     * (failed, or lost to a process that died before media3 heard of it) is asked for again rather than
     * skipped, so the download button always does something.
     */
    fun download(songs: List<Song>) = io.execute {
        val st = _state.value
        val fresh = songs.filter { it.id !in st.doneIds && it.id !in st.pendingIds }.distinctBy { it.id }
        val again = songs.filter { it.id in st.pendingIds }
        for (s in fresh) core.downloadAdd(s)
        if (fresh.isNotEmpty()) publish()
        val requests = fresh.map(::request)
        val retries = again.map(::request)
        main.post {
            for (r in requests) add(r)
            for (r in retries) if (!batch.isOpen(r.id)) { unmark(listOf(r.id)); add(r) }
        }
    }

    /** Sends failed downloads round again; they rejoin the queue at its end. */
    fun retry(songs: List<Song>) {
        if (songs.isEmpty()) return
        unmark(songs.map { it.id })
        io.execute {
            for (s in songs) core.downloadAdd(s)
            publish()
            val requests = songs.map(::request)
            main.post { requests.forEach(::add) }
        }
    }

    private fun add(r: DownloadRequest) {
        runCatching { DownloadService.sendAddDownload(context, DownloadWorker::class.java, r, false) }
            .onFailure { Log.w(TAG, "could not queue ${r.id}", it) }
    }

    /**
     * The title, the album and the expected size ride along with the request: the notification and the
     * progress ring need them, and a transcoding server usually answers without a Content-Length, which
     * leaves media3's own progress empty and the download looking stuck.
     */
    private fun request(s: Song): DownloadRequest {
        val expected = expectedBytes(s.size.toLong(), s.duration.toLong(), settings.value.download.bitRate)
        return DownloadRequest.Builder(s.id, Uri.parse(sources.downloadUrl(s.id)))
            .setCustomCacheKey(sources.downloadKey(s.id))
            .setData("$expected\n${s.title.replace('\n', ' ')}\n${s.album.replace('\n', ' ')}".toByteArray())
            .build()
    }

    fun remove(ids: List<String>) = ids.forEach { DownloadService.sendRemoveDownload(context, DownloadWorker::class.java, it, false) }

    /**
     * Stops downloads that have not finished and forgets them. The index entry goes at once as well as
     * through media3's callback: a song left pending by an earlier session may have no media3 download
     * to remove, and would otherwise sit in the queue for ever.
     */
    fun cancel(ids: List<String>) {
        if (ids.isEmpty()) return
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
        unmark(ids)
        main.post { ids.forEach { manager.removeDownload(it) } }
        io.execute { ids.forEach { core.downloadRemove(it) }; publish() }
    }

    /**
     * The download notification while a batch runs: where it is ("Downloading: 12 of 49"), the song
     * in flight, how fast the bytes arrive and how long they should take, and a bar over the whole
     * batch that moves with the bytes of the songs in flight. The service asks once a second;
     * nothing else redraws it. The batch's aggregate speed is published for the downloads screen.
     */
    internal fun progressNotification(context: Context, downloads: List<Download>, notMetRequirements: Int): Notification {
        val running = downloads.filter { it.state == Download.STATE_DOWNLOADING }
        val total = maxOf(batch.total, batch.finished + downloads.size)
        if (total == 0) {
            // Nothing left: the service is on its way out, and the batch's own summary (its own id,
            // so stopping the service does not take it) says how it went. No bar here.
            return NotificationCompat.Builder(context, CHANNEL)
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentTitle("Downloads complete")
                .setContentIntent(openDownloads(context))
                .setAutoCancel(true)
                .setOnlyAlertOnce(true)
                .setSilent(true)
                .setShowWhen(false)
                .build()
        }
        val inFlight = running.sumOf { d -> downloadFraction(d.contentLength, d.bytesDownloaded, estimateOf(d.request)).coerceAtLeast(0f).toDouble() }
        val fraction = batch.fraction(inFlight)
        val position = (batch.finished + 1).coerceAtMost(total.coerceAtLeast(1))
        val label = batch.label()
        val current = running.minByOrNull { it.startTimeMs }?.let { titleOf(it.request) }
        val now = SystemClock.elapsedRealtime()
        val speed = speedOf.values.sumOf { if (now - it.at < 3_000) it.rate else 0.0 }.toLong()
        val expected = downloads.map { if (it.contentLength > 0) it.contentLength else estimateOf(it.request) }
        val unlisted = (total - batch.finished - downloads.size).coerceAtLeast(0)
        val avg = expected.filter { it > 0 }.average().takeIf { !it.isNaN() }?.toLong() ?: 8_000_000L
        val remaining = expected.zip(downloads).sumOf { (e, d) -> (e - d.bytesDownloaded).coerceAtLeast(0L) } + unlisted * avg
        val eta = if (speed > 0 && remaining > 0) remaining / speed else null
        _stats.value = DownloadStats(speed, remaining, eta)
        val title = when {
            notMetRequirements != 0 -> "Waiting for a network"
            total == 1 -> current?.let { "Downloading “$it”" } ?: "Downloading 1 song"
            else -> "Downloading: $position of $total"
        }
        val speedText = formatSpeed(speed)
        val etaText = formatEta(eta)
        val text = if (total == 1) {
            listOfNotNull(current, speedText.ifEmpty { null }, etaText.ifEmpty { null }).joinToString(" · ")
        } else {
            listOfNotNull(current, label?.let { "“$it”" }, speedText.ifEmpty { null }, etaText.ifEmpty { null }).joinToString(" · ")
        }
        return NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle(title)
            .setContentText(text.ifEmpty { null })
            .setProgress(1000, (fraction * 1000).toInt(), false)
            .setContentIntent(openDownloads(context))
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Cancel", cancelIntent(context))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setShowWhen(false)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .build()
    }

    /**
     * How the batch went, once it has. Its own notification id: the service takes the progress one with
     * it when it stops, and would take this too if it shared the id. All done: a quiet line that goes
     * by itself. Something failed: it stays, and a tap shows which.
     */
    private fun summarise() {
        val done = batch.done
        val failed = batch.failed
        if (done == 0 && failed == 0) return
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        val label = batch.label()
        val title = when {
            failed > 0 -> if (failed == 1) "1 song couldn’t be downloaded" else "$failed songs couldn’t be downloaded"
            label != null && done > 1 -> "“$label” downloaded"
            done == 1 -> "1 song downloaded"
            else -> "$done songs downloaded"
        }
        val text = when {
            failed > 0 && done > 0 -> "$done downloaded · tap to see what failed"
            failed > 0 -> "Tap to try again"
            else -> null
        }
        val b = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(if (failed > 0) android.R.drawable.stat_notify_error else android.R.drawable.stat_sys_download_done)
            .setContentTitle(title)
            .setContentText(text)
            .setContentIntent(openDownloads(context))
            .setAutoCancel(true)
            .setSilent(true)
            .setOnlyAlertOnce(true)
        if (failed == 0) b.setTimeoutAfter(8_000)
        runCatching { nm.notify(DOWNLOAD_RESULT_NOTIFICATION, b.build()) }
    }

    /** A new batch starting takes the last one's result away: the progress notification replaces it. */
    private fun cancelResult() {
        context.getSystemService(NotificationManager::class.java)?.cancel(DOWNLOAD_RESULT_NOTIFICATION)
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
                    noteBytes(request.id, bytes, if (length > 0) length else estimate, SystemClock.elapsedRealtime())
                }
                override fun cancel() = downloader.cancel()
                override fun remove() = downloader.remove()
            }
        }
    }

    companion object {
        /** Finished downloads the downloads screen keeps listing this session. */
        const val RECENT = 50
        internal const val TAG = "noridl"
    }
}

/** What a request says the song will weigh, its title and its album: the lines of its data (see [Downloads.request]). */
private fun dataLine(request: DownloadRequest, n: Int): String? = request.data.decodeToString().split('\n').getOrNull(n)?.takeIf(String::isNotBlank)
private fun estimateOf(request: DownloadRequest): Long = dataLine(request, 0)?.toLongOrNull() ?: 0L
private fun titleOf(request: DownloadRequest): String? = dataLine(request, 1)
private fun albumOf(request: DownloadRequest): String? = dataLine(request, 2)

/** Asks the app to open on its downloads screen. The activity answers it; the core only names it. */
const val ACTION_OPEN_DOWNLOADS = "dev.nori.music.OPEN_DOWNLOADS"

/** The notification's Cancel: everything not yet downloaded leaves the queue, finished songs stay. */
private const val ACTION_CANCEL_DOWNLOADS = "dev.nori.music.CANCEL_DOWNLOADS"

private const val CHANNEL = "downloads"

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

@UnstableApi
private fun cancelIntent(context: Context): PendingIntent =
    PendingIntent.getService(
        context, 2, Intent(context, DownloadWorker::class.java).setAction(ACTION_CANCEL_DOWNLOADS),
        PendingIntent.FLAG_UPDATE_CURRENT or PendingIntent.FLAG_IMMUTABLE,
    )

/**
 * The progress of a download batch, for as long as the service runs. Not 1001: that is media3's
 * playback notification id, and sharing it replaced the now-playing notification while downloading.
 */
const val DOWNLOAD_NOTIFICATION = 2001

/** How the batch went, once it has: a separate id, so the service stopping does not take it away. */
const val DOWNLOAD_RESULT_NOTIFICATION = 2002

@UnstableApi
class DownloadWorker : DownloadService(DOWNLOAD_NOTIFICATION, 1000L, CHANNEL, androidx.media3.exoplayer.R.string.exo_download_notification_channel_name, 0) {
    override fun getDownloadManager(): DownloadManager = Nori.get(this).downloads.manager
    override fun getScheduler(): Scheduler? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        if (intent?.action == ACTION_CANCEL_DOWNLOADS) {
            Nori.get(this).downloads.cancelAll()
            return super.onStartCommand(Intent(intent).setAction(DownloadService.ACTION_INIT), flags, startId)
        }
        return super.onStartCommand(intent, flags, startId)
    }

    /**
     * Asked once a second while anything downloads (media3 throttles it to the interval above), on the
     * main thread the manager lives on - which also makes it the place to follow a change to
     * "Downloads at once" without a listener of its own.
     */
    override fun getForegroundNotification(downloads: MutableList<Download>, notMetRequirements: Int): Notification {
        val all = Nori.get(this).downloads
        all.parallel().let { if (all.manager.maxParallelDownloads != it) all.manager.maxParallelDownloads = it }
        return all.progressNotification(this, downloads, notMetRequirements)
    }
}
