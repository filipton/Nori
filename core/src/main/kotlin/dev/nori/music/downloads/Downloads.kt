package dev.nori.music.downloads

import android.app.Notification
import dalvik.annotation.optimization.FastNative
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
import dalvik.annotation.optimization.CriticalNative
import dev.nori.music.Nori
import dev.nori.music.ffi.Core
import dev.nori.music.ffi.transfers.DownloadKnown
import dev.nori.music.ffi.transfers.DownloadQueued
import dev.nori.music.ffi.model.Song
import dev.nori.music.playback.MediaSources
import dev.nori.music.settings.Settings
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import java.util.concurrent.ConcurrentHashMap
import java.util.concurrent.Executors

/**
 * The downloads table as the screens read it: how many songs are downloaded and how many are still to
 * come, and whether one song is either - asked of the core's memory of the table, not copied here, so a
 * song finishing costs the same with ten downloads as with ten thousand. A new value whenever the table
 * changes. The songs themselves are only read when something asks for them, off the main thread.
 */
class DownloadState internal constructor(
    val doneCount: Int = 0,
    val pendingCount: Int = 0,
    private val version: Long = 0,
    private val songs: (Boolean) -> List<Song> = { emptyList() },
    private val ids: (Boolean) -> List<String> = { emptyList() },
) {
    val doneIds: Set<String> = Membership(DownloadsJni.DONE, doneCount)
    val pendingIds: Set<String> = Membership(DownloadsJni.PENDING, pendingCount)

    /** Every downloaded song, newest first: read from the core the first time it is asked, never on the main thread. */
    val done: List<Song> by lazy { songs(true) }
    /** Every song still to download, newest first; read as [done] is. */
    val pending: List<Song> by lazy { songs(false) }

    override fun equals(other: Any?) = other is DownloadState && other.version == version
    override fun hashCode() = version.hashCode()

    /**
     * One side of the table as a set: a lookup is one question to the core. Equal only to itself, so
     * nothing compares two of them song by song; a change to the table is a new [DownloadState].
     */
    private inner class Membership(private val kind: Int, override val size: Int) : AbstractSet<String>() {
        override fun contains(element: String) = size > 0 && DownloadsJni.held(element) == kind
        override fun iterator() = ids(kind == DownloadsJni.DONE).iterator()
        override fun equals(other: Any?) = this === other
        override fun hashCode() = System.identityHashCode(this)
    }
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

    private val _marks = MutableStateFlow<Map<String, DownloadMark>>(emptyMap())
    /**
     * The songs this session's downloads are doing something with: downloading, failed, or finished
     * lately. A pending song with no mark is waiting its turn. It changes when a download changes phase,
     * never with its progress, which each mark carries in a flow of its own - so a list watching this
     * map is not redrawn per percent. The phases and the bookkeeping are the core's
     * (crates/transfers/src/transfers.rs); this mirrors them for the screens.
     */
    val marks: StateFlow<Map<String, DownloadMark>> = _marks

    /** Each marked song's progress, 0..1 or negative while its size is unknown. */
    private val progress = ConcurrentHashMap<String, MutableStateFlow<Float>>()

    private fun progressOf(id: String) = progress.getOrPut(id) { MutableStateFlow(DownloadsJni.startFraction(id)) }

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
                    if (d.state == Download.STATE_COMPLETED) settle(d.request.id, true)
                }

                override fun onDownloadRemoved(m: DownloadManager, d: Download) {
                    val flags = DownloadsJni.removed(d.request.id)
                    progress.remove(d.request.id)
                    if (flags and DownloadsJni.MARKS != 0) refreshMarks()
                    settle(d.request.id, false)
                    if (flags and DownloadsJni.DRAINED != 0) summarise()
                }
            })
        }
    }

    /** How many songs download at once, from the setting (its range is the core's). */
    fun parallel() = settings.value.parallelDownloads

    /** Whether the queue has been handed back to media3 in this process; see [resume]. */
    @Volatile private var resumed = false

    init {
        io.execute { publish(); reconcile() }
    }

    /** The table's counts, from the core's memory of it: nothing is read or copied, however many songs it holds. */
    private fun publish() {
        val n = core.downloadCounts()
        _state.value = DownloadState(n.done.toInt(), n.pending.toInt(), n.version.toLong(), ::songs, ::ids)
    }

    private fun songs(done: Boolean): List<Song> = runCatching { core.downloads(done) }.getOrDefault(emptyList())
    private fun ids(done: Boolean): List<String> = runCatching { core.downloadIds(done) }.getOrDefault(emptyList())

    /** Downloads that finished (true) or left the queue (false) and are not written down yet, in order. */
    private val settledIds = ArrayList<String>()
    private val settledDone = ArrayList<Boolean>()

    /**
     * Writes down that a download finished or went. They are gathered and written together, in one call
     * and one transaction, followed by one [publish]: stopping a library's worth of downloads reports each
     * song on its own, and writing each on its own was a call, a statement and a count per song.
     */
    private fun settle(id: String, finished: Boolean) {
        val first = synchronized(settledIds) {
            settledIds.add(id); settledDone.add(finished)
            settledIds.size == 1
        }
        if (first) io.execute(::writeSettled)
    }

    private fun writeSettled() {
        val (ids, done) = synchronized(settledIds) {
            (ArrayList(settledIds) to ArrayList(settledDone)).also { settledIds.clear(); settledDone.clear() }
        }
        // A finished download is the permanent copy; the streamed one is the same bytes twice, so it
        // goes. (A song streamed before it was downloaded lives in both.)
        for (i in ids.indices) if (done[i]) sources.dropStreamCopies(ids[i])
        runCatching { core.downloadSettle(ids, done) }.onFailure { Log.w(TAG, "could not write ${ids.size} settled downloads", it) }
        publish()
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
     * Brings the Rust index and media3's queue back into agreement after the process died. What each
     * song left pending needs is the core's (`download_recover`); this reads media3's table for it and
     * does what it says: the finished ones' streamed copies go, the failed ones show how far they got,
     * the lost ones are asked for again, and when anything is queued or was interrupted the download
     * service is started, which starts the manager, which restores and resumes them.
     */
    private fun reconcile() {
        val pending = _state.value.pendingIds
        if (pending.isEmpty()) { resumed = true; return }
        val known = runCatching {
            DefaultDownloadIndex(sources.database).getDownloads().use { c ->
                buildList {
                    while (c.moveToNext()) c.download.let { if (it.request.id in pending) add(DownloadKnown(it.request.id, it.state, it.contentLength, it.bytesDownloaded)) }
                }
            }
        }.getOrDefault(emptyList())
        val r = runCatching { core.downloadRecover(known) }.getOrElse { Log.w(TAG, "could not recover the queue", it); return }
        for (id in r.finished) sources.dropStreamCopies(id)
        if (r.finished.isNotEmpty()) publish()
        for (f in r.failed) progress.getOrPut(f.id) { MutableStateFlow(f.progress) }
        if (r.failed.isNotEmpty()) main.post { refreshMarks() }
        if (!r.unfinished && r.lost.isEmpty()) { resumed = true; return }
        Log.i(TAG, "resuming: ${pending.size} pending, ${r.lost.size} asked for again, ${r.failed.size} failed")
        main.post {
            resumed = runCatching {
                // The first intent brings the service - and with it the manager and its restored queue - up.
                if (r.lost.isEmpty()) DownloadService.start(context, DownloadWorker::class.java)
                for (id in r.lost) DownloadService.sendAddDownload(context, DownloadWorker::class.java, request(id), false)
            }.onFailure { Log.w(TAG, "could not start the download service yet", it) }.isSuccess
        }
    }

    /**
     * Hands one download's state to the core, which keeps the batch and the phases, and follows what it
     * says: a new batch takes the last one's result away, the phases changed, or the batch is over and
     * says how it went. Runs on the main thread, where media3 reports.
     */
    private fun follow(d: Download) {
        val id = d.request.id
        val flags = DownloadsJni.followed(id, d.state, SystemClock.elapsedRealtime())
        if (flags and DownloadsJni.NEW_BATCH != 0) cancelResult()
        when (d.state) {
            Download.STATE_DOWNLOADING -> progressOf(id)
            Download.STATE_COMPLETED -> progress.getOrPut(id) { MutableStateFlow(1f) }.value = 1f
        }
        if (flags and DownloadsJni.MARKS != 0) refreshMarks()
        if (flags and DownloadsJni.DRAINED != 0) summarise()
    }

    /**
     * The core's phases as the screens read them, each with its song's progress flow. Only the marks that
     * moved since the last time come over; a song that lost its mark and is not waiting any more also
     * loses its progress. Runs on the main thread, the only one that applies them.
     */
    private fun refreshMarks() {
        val m = dev.nori.music.ffi.transfers.downloadMarksChanged()
        if (m.ids.isEmpty()) return
        val next = HashMap(_marks.value)
        for (i in m.ids.indices) {
            val id = m.ids[i]
            val phase = when (m.phases[i]) { 0 -> null; 1 -> DownloadPhase.DOWNLOADING; 2 -> DownloadPhase.FAILED; else -> DownloadPhase.DONE }
            if (phase != null) next[id] = DownloadMark(phase, progressOf(id), m.at[i])
            else if (next.remove(id) != null && DownloadsJni.held(id) != DownloadsJni.PENDING) progress.remove(id)
        }
        _marks.value = next
    }

    private fun unmark(ids: Collection<String>) {
        var changed = false
        for (id in ids) { progress.remove(id); if (DownloadsJni.unmark(id) and DownloadsJni.MARKS != 0) changed = true }
        if (changed) main.post { refreshMarks() }
    }

    /**
     * Queues what is not downloaded yet. Which songs are new and which are asked for again is the core's
     * (`download_queue`); one asked again that media3 is still working on is left to it.
     */
    fun download(songs: List<Song>) = io.execute { queued(runCatching { core.downloadQueue(songs) }) }

    /** Every song of the offline index, in one call to the core; sync the library first so the index is complete. */
    fun downloadLibrary() = io.execute { queued(runCatching { core.downloadQueueLibrary() }) }

    private fun queued(result: Result<DownloadQueued>) {
        val q = result.getOrElse { Log.w(TAG, "could not queue downloads", it); return }
        if (q.fresh.isNotEmpty()) publish()
        val requests = q.fresh.map(::request)
        val retries = q.again.map(::request)
        main.post {
            for (r in requests) add(r)
            if (retries.isEmpty()) return@post
            // Listed once: media3 hands out a fresh copy of its list each time it is asked.
            val working = manager.currentDownloads.mapTo(HashSet()) { it.request.id }
            for (r in retries) if (r.id !in working) { unmark(listOf(r.id)); add(r) }
        }
    }

    /** Sends failed downloads round again; they rejoin the queue at its end. */
    fun retry(songs: List<Song>) {
        if (songs.isEmpty()) return
        unmark(songs.map { it.id })
        io.execute {
            val q = runCatching { core.downloadQueue(songs) }.getOrElse { Log.w(TAG, "could not retry downloads", it); return@execute }
            publish()
            val requests = (q.fresh + q.again).map(::request)
            main.post { requests.forEach(::add) }
        }
    }

    private fun add(r: DownloadRequest) {
        runCatching { DownloadService.sendAddDownload(context, DownloadWorker::class.java, r, false) }
            .onFailure { Log.w(TAG, "could not queue ${r.id}", it) }
    }

    /** What the song is and how much it should weigh the core knows from its own downloads table. */
    private fun request(id: String): DownloadRequest =
        DownloadRequest.Builder(id, Uri.parse(sources.downloadUrl(id))).setCustomCacheKey(sources.downloadKey(id)).build()

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
        for (id in ids) settle(id, false)
    }

    /**
     * Everything not yet downloaded, in one go: the core takes it out of the table and forgets its marks
     * in one call and says which songs they were, and they go straight to the manager rather than one
     * service intent per song, which for a whole library queued would be thousands. Finished downloads
     * stay, which media3's own "remove all" would not leave.
     */
    fun cancelAll() = io.execute {
        val ids = runCatching { core.downloadCancelAll() }.getOrElse { Log.w(TAG, "could not stop the downloads", it); return@execute }
        if (ids.isEmpty()) return@execute
        for (id in ids) progress.remove(id)
        publish()
        main.post {
            refreshMarks()
            for (id in ids) manager.removeDownload(id)
        }
    }

    /**
     * The download notification while a batch runs: where it is ("Downloading: 12 of 49"), the song
     * in flight, how fast the bytes arrive and how long they should take, and a bar over the whole
     * batch that moves with the bytes of the songs in flight. The service asks once a second;
     * nothing else redraws it. The batch's aggregate speed is published for the downloads screen.
     */
    internal fun progressNotification(context: Context, downloads: List<Download>, notMetRequirements: Int): Notification {
        // The words and the bar are the core's; asked once a second, it says whether they changed, and
        // an unchanged notification is handed back as it was rather than built again.
        when (DownloadsJni.notice(downloads.size, notMetRequirements, SystemClock.elapsedRealtime())) {
            0 -> lastProgress?.let { return it }
            2 -> return complete ?: NotificationCompat.Builder(context, CHANNEL)
                // Nothing left: the service is on its way out, and the batch's own summary (its own id,
                // so stopping the service does not take it) says how it went. No bar here.
                .setSmallIcon(android.R.drawable.stat_sys_download_done)
                .setContentTitle(noticeWords.complete)
                .setContentIntent(openDownloads(context))
                .setAutoCancel(true)
                .setOnlyAlertOnce(true)
                .setSilent(true)
                .setShowWhen(false)
                .build().also { complete = it }
        }
        val words = DownloadsJni.noticeWords()
        val cut = words.indexOf('\n')
        return NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(android.R.drawable.stat_sys_download)
            .setContentTitle(words.substring(0, cut))
            .setContentText(words.substring(cut + 1).ifEmpty { null })
            .setProgress(1000, DownloadsJni.noticePermille(), false)
            .setContentIntent(openDownloads(context))
            .addAction(android.R.drawable.ic_menu_close_clear_cancel, noticeWords.cancel, cancelIntent(context))
            .setOngoing(true)
            .setOnlyAlertOnce(true)
            .setSilent(true)
            .setShowWhen(false)
            .setCategory(NotificationCompat.CATEGORY_PROGRESS)
            .build().also { lastProgress = it }
    }

    private var lastProgress: Notification? = null
    /** The notification's fixed words, the core's (`words_download_notice`). */
    private val noticeWords by lazy { dev.nori.music.ffi.words.wordsDownloadNotice() }
    private var complete: Notification? = null

    /**
     * How the batch went, once it has. Its own notification id: the service takes the progress one with
     * it when it stops, and would take this too if it shared the id. All done: a quiet line that goes
     * by itself. Something failed: it stays, and a tap shows which.
     */
    private fun summarise() {
        val summary = DownloadsJni.summary()
        if (summary.isEmpty()) return
        val nm = context.getSystemService(NotificationManager::class.java) ?: return
        val failed = DownloadsJni.summaryFailed() > 0
        val b = NotificationCompat.Builder(context, CHANNEL)
            .setSmallIcon(if (failed) android.R.drawable.stat_notify_error else android.R.drawable.stat_sys_download_done)
            .setContentTitle(summary.substringBefore('\n'))
            .setContentText(summary.substringAfter('\n').ifEmpty { null })
            .setContentIntent(openDownloads(context))
            .setAutoCancel(true)
            .setSilent(true)
            .setOnlyAlertOnce(true)
        if (!failed) b.setTimeoutAfter(noticeWords.resultTimeoutMs)
        runCatching { nm.notify(DOWNLOAD_RESULT_NOTIFICATION, b.build()) }
    }

    /** A new batch starting takes the last one's result away: the progress notification replaces it. */
    private fun cancelResult() {
        context.getSystemService(NotificationManager::class.java)?.cancel(DOWNLOAD_RESULT_NOTIFICATION)
    }

    /**
     * Reports each downloader's chunks to the core against a slot of its own - three numbers a chunk,
     * nothing looked up or allocated - and passes on only the progress the core says is worth drawing.
     */
    private inner class TrackedDownloaders(private val inner: DownloaderFactory) : DownloaderFactory {
        override fun createDownloader(request: DownloadRequest): Downloader {
            val downloader = inner.createDownloader(request)
            val flow = progressOf(request.id)
            return object : Downloader {
                override fun download(listener: Downloader.ProgressListener?) {
                    val slot = DownloadsJni.open(request.id, SystemClock.elapsedRealtime())
                    downloader.download { length, bytes, percent ->
                        listener?.onProgress(length, bytes, percent)
                        val f = DownloadsJni.note(slot, length, bytes, SystemClock.elapsedRealtime())
                        if (!f.isNaN()) flow.value = f
                    }
                }
                override fun cancel() = downloader.cancel()
                override fun remove() = downloader.remove()
            }
        }
    }

    companion object {
        internal const val TAG = "noridl"
    }
}

/** The core's side of the downloads; see crates/transfers/src/transfers.rs. */
internal object DownloadsJni {
    init { System.loadLibrary("norimusic") }

    const val NEW_BATCH = 1
    const val DRAINED = 2
    const val MARKS = 4

    /** What [held] says of a song. */
    const val PENDING = 1
    const val DONE = 2

    /** Whether a song is in the downloads table: 0 no, [PENDING] queued or failed, [DONE] downloaded. */
    @JvmStatic @FastNative external fun held(id: String): Int
    @JvmStatic external fun followed(id: String, state: Int, now: Long): Int
    @JvmStatic external fun removed(id: String): Int
    @JvmStatic external fun unmark(id: String): Int
    @JvmStatic external fun startFraction(id: String): Float
    @JvmStatic external fun open(id: String, now: Long): Int
    /** Per chunk: the progress to show, or NaN when it has not moved enough to draw. */
    @JvmStatic @CriticalNative external fun note(slot: Int, length: Long, bytes: Long, now: Long): Float
    /** 0 unchanged, 1 changed, 2 the batch is over. */
    @JvmStatic @CriticalNative external fun notice(listed: Int, waiting: Int, now: Long): Int
    /** The notification's title and text, "title\ntext": one crossing for both. */
    @JvmStatic @FastNative external fun noticeWords(): String
    @JvmStatic @CriticalNative external fun noticePermille(): Int
    /** "title\ntext", or empty when there is nothing to say. */
    @JvmStatic @FastNative external fun summary(): String
    @JvmStatic @CriticalNative external fun summaryFailed(): Int
}

/**
 * The downloads screen's lines, in the core's words (crates/transfers/src/transfers.rs). A running row's line
 * is asked whenever its ring moves and the summary once a second, so they come over JNI, written in a
 * buffer the core keeps.
 */
object DownloadLines {
    init { System.loadLibrary("norimusic") }

    /** A running song's second line: its artist, then "45% · 2.1 MB/s · 1:20 left". */
    @JvmStatic @FastNative external fun row(id: String): String
    /** "2 downloading · 14 waiting · 1 failed · 3.2 MB/s · 12:34 left", or "Nothing downloading". */
    @JvmStatic @FastNative external fun summary(active: Int, queued: Int, failed: Int): String
}

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
