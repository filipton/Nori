package dev.nori.music.app.perf

import android.app.Activity
import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.net.TrafficStats
import android.os.BatteryManager
import android.os.Build
import android.os.Bundle
import android.os.Debug
import android.os.Handler
import android.os.HandlerThread
import android.os.Looper
import android.os.PowerManager
import android.os.Process
import android.os.SystemClock
import android.system.Os
import android.system.OsConstants
import android.view.FrameMetrics
import android.view.Window
import androidx.compose.runtime.Composable
import androidx.compose.runtime.mutableStateOf
import androidx.core.content.ContextCompat
import dev.nori.music.Nori
import dev.nori.music.app.PerfHooks
import dev.nori.music.ffi.PerfCounters
import dev.nori.music.ffi.PerfDevice
import dev.nori.music.ffi.PerfFrames
import dev.nori.music.ffi.PerfOutput
import dev.nori.music.ffi.PerfPage
import dev.nori.music.ffi.PerfStretch
import dev.nori.music.ffi.PerfThread
import dev.nori.music.playback.PlaybackService
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.android.asCoroutineDispatcher
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import java.io.File

/**
 * The perf build's recorder: what the app costs, stretch by stretch, on the owner's own phone.
 *
 * A stretch is a span of one state (screen off and playing, the player open, charging, ...) with one set
 * of the settings that change the cost. Which state that is, what two readings of the counters make, how
 * the stretches add up and how the page and the report say it are the core's (perf_log.rs); this reads
 * Android's counters and draws the page. The counters are read when the state or those
 * settings change, which ends one stretch and starts the next, and when the Performance page opens
 * (the stretch so far). Nothing here ticks: every read is set off by a broadcast (screen on or off,
 * power connected, the service's play/pause), an activity starting or stopping, the player sheet or
 * a settings change. While the phone sleeps with music playing nothing here runs at all, so the
 * recorder adds no wakeups of its own to the numbers it records. Why a stretch cost what it did is read
 * at its ends as well: every thread's name, CPU time and wakeups, the AudioTrack the player opened, and
 * the bytes the app moved over the network.
 *
 * Everything happens on one background thread, which sleeps in its looper between events. The frame
 * listener is there only while an activity is started, so it costs nothing with the screen off.
 */
internal class Recorder(private val app: Application) : PerfHooks.Recorder {
    private val thread = HandlerThread("perf", Process.THREAD_PRIORITY_BACKGROUND).apply { start() }
    private val handler = Handler(thread.looper)
    private val main = Handler(Looper.getMainLooper())
    private val battery = app.getSystemService(BatteryManager::class.java)
    private val msPerTick = 1000.0 / Os.sysconf(OsConstants._SC_CLK_TCK)

    // What the app is doing now. Written and read on the perf thread only.
    private var screenOn = app.getSystemService(PowerManager::class.java).isInteractive
    private var playing = false
    private var foreground = false
    private var playerOpen = false
    private var charging = false
    private var settings = ""

    // The stretch under way: where it started, and the frames drawn since.
    private var start: PerfCounters? = null
    private var startKey = ""
    private var startCfg = ""
    private var frames = 0L
    private var janky = 0L
    private var worstNs = 0L
    private var frameBudgetNs = 16_666_667L

    /** What the page shows: the stretches kept and the one under way, read when it opens; none until then. */
    val shown = mutableStateOf<Shown?>(null)
    val callBench = mutableStateOf("")
    val coverBench = mutableStateOf("")
    /** The page's fixed words, the core's. */
    val words by lazy { dev.nori.music.ffi.words.wordsPerf() }

    private val events = object : BroadcastReceiver() {
        override fun onReceive(context: Context, intent: Intent) {
            when (intent.action) {
                Intent.ACTION_SCREEN_ON -> screenOn = true
                Intent.ACTION_SCREEN_OFF -> screenOn = false
                Intent.ACTION_POWER_CONNECTED -> charging = true
                Intent.ACTION_POWER_DISCONNECTED -> charging = false
                // The service says so on every song as well; only a change of playing counts.
                PlaybackService.ACTION_STATE -> playing = intent.getBooleanExtra(PlaybackService.EXTRA_PLAYING, playing)
            }
            changed()
        }
    }

    /**
     * Counted on the perf thread as the frames are drawn. A frame is janky when it took longer than
     * its deadline (Android 12 on) or than one refresh of the display (before).
     */
    private val frameListener = Window.OnFrameMetricsAvailableListener { _, m, dropped ->
        if (m.getMetric(FrameMetrics.FIRST_DRAW_FRAME) == 1L) return@OnFrameMetricsAvailableListener
        val total = m.getMetric(FrameMetrics.TOTAL_DURATION)
        val budget = if (Build.VERSION.SDK_INT >= 31) m.getMetric(FrameMetrics.DEADLINE) else frameBudgetNs
        // Frames the listener was too slow to be told about still happened; their times are unknown.
        frames += 1 + dropped
        if (total > budget) janky++
        if (total > worstNs) worstNs = total
    }

    fun install() {
        var started = 0
        app.registerActivityLifecycleCallbacks(object : Application.ActivityLifecycleCallbacks {
            override fun onActivityStarted(activity: Activity) {
                if (Build.VERSION.SDK_INT < 31) {
                    @Suppress("DEPRECATION")
                    val display = if (Build.VERSION.SDK_INT >= 30) activity.display else activity.windowManager.defaultDisplay
                    display?.refreshRate?.takeIf { it > 0f }?.let { frameBudgetNs = (1e9 / it).toLong() }
                }
                activity.window.addOnFrameMetricsAvailableListener(frameListener, handler)
                if (started++ == 0) handler.post { foreground = true; changed() }
            }

            override fun onActivityStopped(activity: Activity) {
                runCatching { activity.window.removeOnFrameMetricsAvailableListener(frameListener) }
                if (--started == 0) handler.post { foreground = false; changed() }
            }

            override fun onActivityCreated(activity: Activity, savedInstanceState: Bundle?) {}
            override fun onActivityResumed(activity: Activity) {}
            override fun onActivityPaused(activity: Activity) {}
            override fun onActivitySaveInstanceState(activity: Activity, outState: Bundle) {}
            override fun onActivityDestroyed(activity: Activity) {}
        })
        val filter = IntentFilter().apply {
            addAction(Intent.ACTION_SCREEN_ON)
            addAction(Intent.ACTION_SCREEN_OFF)
            addAction(Intent.ACTION_POWER_CONNECTED)
            addAction(Intent.ACTION_POWER_DISCONNECTED)
            addAction(PlaybackService.ACTION_STATE)
        }
        ContextCompat.registerReceiver(app, events, filter, null, handler, ContextCompat.RECEIVER_NOT_EXPORTED)
        handler.post {
            charging = batteryIntent()?.getIntExtra(BatteryManager.EXTRA_PLUGGED, 0)?.let { it != 0 } ?: false
            // A settings change that alters the cost ends the stretch as a change of state does. The line
            // is the core's, read from the settings it keeps, which have the change before this hears of it.
            val prefs = Nori.get(app).settings.prefs
            settings = cfg()
            CoroutineScope(handler.asCoroutineDispatcher()).launch {
                prefs.map { cfg() }.distinctUntilChanged().collect { settings = it; changed() }
            }
            changed()
        }
    }

    override fun playerOpen(open: Boolean) {
        handler.post { playerOpen = open; changed() }
    }

    @Composable
    override fun Page() = PerfPage(this)

    /** The state a stretch is filed under (the core's `perf_state`). */
    private fun key(): String = dev.nori.music.ffi.perfState(charging, screenOn, playing, foreground, playerOpen)

    /**
     * The settings that change what playing costs, in one line (the core's `perf_config`), the playback
     * path first: the one the service runs, which took the setting when it started; with no service yet,
     * the one it will start with.
     */
    private fun cfg(): String = dev.nori.music.ffi.perfConfig(PlaybackService.engine)

    /** Something happened: when it moved the app into another state, one stretch ends and the next begins. */
    private fun changed() {
        // The service may have started (or stopped) since, and with it the path that plays.
        settings = cfg()
        val key = key()
        if (start != null && key == startKey && settings == startCfg) return
        val now = counters()
        start?.let { s -> stretch(s, now, live = false)?.let { dev.nori.music.ffi.perfLogAdd(now.wallMs, it) } }
        begin(now, key)
    }

    private fun begin(now: PerfCounters, key: String) {
        start = now
        startKey = key
        startCfg = settings
        frames = 0; janky = 0; worstNs = 0
    }

    /** What the page shows, read again: the kept stretches and the one under way so far. */
    fun refresh() = handler.post {
        val live = start?.let { stretch(it, counters(), live = true) }
        val page = dev.nori.music.ffi.perfPage(live)
        main.post { shown.value = Shown(page, live) }
    }

    /**
     * The report, made on this thread (it reads every stretch kept) and handed to the system's share
     * sheet: the stretches as the page last read them, and the benchmarks' results.
     */
    fun share(context: Context, calls: String, covers: String) {
        val live = shown.value?.live
        handler.post {
            val device = PerfDevice(
                Build.MANUFACTURER, Build.MODEL, Build.DEVICE, Build.VERSION.RELEASE, Build.VERSION.SDK_INT,
                dev.nori.music.app.BuildConfig.VERSION_NAME, dev.nori.music.app.BuildConfig.GIT_SHA, dev.nori.music.app.BuildConfig.BUILD_TYPE,
            )
            val text = dev.nori.music.ffi.perfReport(live, device, calls, covers)
            main.post {
                context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text), null))
            }
        }
    }

    /** "Start fresh": everything kept is forgotten, and the stretch under way starts again from now. */
    fun clear() = handler.post {
        dev.nori.music.ffi.perfLogClear()
        begin(counters(), key())
        refresh()
    }

    fun runCalls() = bench(callBench) { dev.nori.music.app.Bench.calls() }

    fun runCovers() = bench(coverBench) { dev.nori.music.app.Bench.covers(app) }

    /** A benchmark takes seconds, so on a thread of its own; its cost lands in the stretch under way. */
    private fun bench(into: androidx.compose.runtime.MutableState<String>, body: () -> String) {
        into.value = words.running
        Thread({
            val result = runCatching(body).getOrElse { dev.nori.music.ffi.words.wordsPerfFailed(it.toString()) }
            main.post { into.value = result }
        }, "bench").start()
    }

    /**
     * The difference between two readings, filed under the state that began at [a] (the core's
     * `perf_stretch`); none for a blink, unless it is the [live] one the page shows.
     */
    private fun stretch(a: PerfCounters, b: PerfCounters, live: Boolean): PerfStretch? =
        dev.nori.music.ffi.perfStretch(startKey, startCfg, a, b, PerfFrames(frames, janky, worstNs), PlaybackService.offloadWanted, output(), live)

    /**
     * The AudioTrack the player last opened, as the platform describes it now, against what was asked of
     * it; none with no player. A track released since reads as what it last said, or not at all.
     */
    private fun output(): PerfOutput? {
        val opened = PlaybackService.track ?: return null
        val t = opened.track
        return runCatching {
            val device = t.routedDevice
            PerfOutput(
                engine = opened.engine, rate = t.sampleRate, channels = t.channelCount, encoding = t.audioFormat,
                askedBytes = opened.askedBytes.toLong(), sizeFrames = t.bufferSizeInFrames.toLong(),
                capacityFrames = t.bufferCapacityInFrames.toLong(), modeAsked = opened.askedMode, mode = t.performanceMode,
                offloaded = Build.VERSION.SDK_INT >= 29 && t.isOffloadedPlayback,
                deviceType = device?.type ?: 0, deviceName = device?.productName?.toString().orEmpty(),
                underruns = t.underrunCount, playState = t.playState,
            )
        }.getOrNull()
    }

    private fun batteryIntent(): Intent? = app.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))

    /**
     * Everything a stretch is measured by, read now. A few milliseconds, most of it the PSS and a file or
     * two per thread (its name and CPU time in `stat`, its wakeups in `status`).
     */
    private fun counters(): PerfCounters {
        val stat = runCatching { File("/proc/self/stat").readText().substringAfterLast(") ").split(' ') }.getOrNull()
        // utime and stime, fields 14 and 15 of the whole line: 11 and 12 once the name is cut off.
        val ticks = stat?.let { (it.getOrNull(11)?.toLongOrNull() ?: 0L) + (it.getOrNull(12)?.toLongOrNull() ?: 0L) } ?: 0L
        val threads = ArrayList<PerfThread>()
        File("/proc/self/task").listFiles()?.forEach { task ->
            val tid = task.name.toIntOrNull() ?: return@forEach
            runCatching {
                // The name is the one in brackets, which may itself hold spaces and brackets: up to the last one.
                val line = File(task, "stat").readText()
                val name = line.substringAfter('(').substringBeforeLast(')')
                val f = line.substringAfterLast(") ").split(' ')
                val cpu = ((f.getOrNull(11)?.toLongOrNull() ?: 0L) + (f.getOrNull(12)?.toLongOrNull() ?: 0L)) * msPerTick
                val switches = File(task, "status").useLines { lines ->
                    lines.firstOrNull { it.startsWith("voluntary_ctxt_switches") }?.substringAfter(':')?.trim()?.toLongOrNull()
                } ?: return@runCatching
                threads += PerfThread(tid, name, cpu.toLong(), switches)
            }
        }
        val memory = Debug.MemoryInfo().also { Debug.getMemoryInfo(it) }
        val b = batteryIntent()
        fun prop(id: Int) = battery.getIntProperty(id).takeIf { it != Int.MIN_VALUE && it != 0 }
        return PerfCounters(
            elapsedMs = SystemClock.elapsedRealtime(), wallMs = System.currentTimeMillis(),
            cpuMs = (ticks * msPerTick).toLong(), threads = threads,
            allocBytes = runtimeStat("art.gc.bytes-allocated"), gcs = runtimeStat("art.gc.gc-count"),
            pssKb = memory.totalPss.toLong(),
            chargeUah = prop(BatteryManager.BATTERY_PROPERTY_CHARGE_COUNTER)?.toLong(),
            capacityPct = battery.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY),
            gaugeUa = (prop(BatteryManager.BATTERY_PROPERTY_CURRENT_AVERAGE) ?: prop(BatteryManager.BATTERY_PROPERTY_CURRENT_NOW))?.toLong(),
            tempDeci = b?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, 0) ?: 0,
            rxBytes = TrafficStats.getUidRxBytes(Process.myUid()).takeIf { it != TrafficStats.UNSUPPORTED.toLong() },
            txBytes = TrafficStats.getUidTxBytes(Process.myUid()).takeIf { it != TrafficStats.UNSUPPORTED.toLong() },
        )
    }

    private fun runtimeStat(name: String) = Debug.getRuntimeStat(name)?.toLongOrNull() ?: 0L

}

/** The page as the core laid it out, and the stretch under way it was read with (for the report). */
internal class Shown(val page: PerfPage, val live: PerfStretch?)
