package dev.nori.music.app.perf

import android.app.Activity
import android.app.Application
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
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
import dev.nori.music.playback.PlaybackService
import dev.nori.music.settings.Prefs
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.android.asCoroutineDispatcher
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import java.io.File

/**
 * The perf build's recorder: what the app costs, stretch by stretch, on the owner's own phone.
 *
 * A stretch is a span of one state (screen off and playing, the player open, charging, ... see [key])
 * with one set of the settings that change the cost. The counters are read when the state or those
 * settings change, which ends one stretch and starts the next, and when the Performance page opens
 * (the stretch so far). Nothing here ticks: every read is set off by a broadcast (screen on or off,
 * power connected, the service's play/pause), an activity starting or stopping, the player sheet or
 * a settings change. While the phone sleeps with music playing nothing here runs at all, so the
 * recorder adds no wakeups of its own to the numbers it records.
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
    private var start: Counters? = null
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
            // A settings change that alters the cost ends the stretch as a change of state does.
            val prefs = Nori.get(app).settings.prefs
            settings = cfg(prefs.value)
            CoroutineScope(handler.asCoroutineDispatcher()).launch {
                prefs.map(::cfg).distinctUntilChanged().collect { settings = it; changed() }
            }
            changed()
        }
    }

    override fun playerOpen(open: Boolean) {
        handler.post { playerOpen = open; changed() }
    }

    @Composable
    override fun Page() = PerfPage(this)

    /** The state a stretch is filed under; the names are [STATES]'s. */
    private fun key(): String = when {
        charging -> "charging"
        !screenOn -> if (playing) "off-playing" else "off-paused"
        !playing -> "on-paused"
        !foreground -> "on-playing-away"
        playerOpen -> "on-playing-player"
        else -> "on-playing-app"
    }

    /**
     * The settings that change what playing costs, in one line, the playback path first: ExoPlayer (with
     * the Rust sink, or offloaded to the audio chip) or the Rust player. The path is the one the service
     * runs, which took the setting when it started; with no service yet, the one it will start with.
     * Last, which decoder draws the covers, since scrolling a library costs what they cost.
     */
    private fun cfg(p: Prefs) = "engine ${PlaybackService.engine ?: if (p.playbackEngine == 1) "rust" else "exoplayer"}, " +
        "eq ${onOff(p.eqEnabled)}, automix ${onOff(p.autoMix)}, crossfade ${p.crossfadeSec} s, " +
        "offload ${onOff(p.offload)}, hi-res ${onOff(p.hiRes)}, bit-perfect ${onOff(p.bitPerfect)}, " +
        "covers ${if (p.coreCovers) "rust" else "android"}"

    private fun onOff(b: Boolean) = if (b) "on" else "off"

    /** Something happened: when it moved the app into another state, one stretch ends and the next begins. */
    private fun changed() {
        // The service may have started (or stopped) since, and with it the path that plays.
        settings = cfg(Nori.get(app).settings.value)
        val key = key()
        if (start != null && key == startKey && settings == startCfg) return
        val now = counters()
        start?.let { s -> stretch(s, now)?.let { dev.nori.music.ffi.perfLogAdd(now.wallMs, it.json()) } }
        begin(now, key)
    }

    private fun begin(now: Counters, key: String) {
        start = now
        startKey = key
        startCfg = settings
        frames = 0; janky = 0; worstNs = 0
    }

    /** What the page shows, read again: the kept stretches and the one under way so far. */
    fun refresh() = handler.post {
        val live = start?.let { stretch(it, counters(), shortest = 0) }
        val kept = dev.nori.music.ffi.perfLogRows(0).mapNotNull(Stretch::parse)
        main.post { shown.value = Shown(kept, live) }
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
        into.value = "running..."
        Thread({
            val result = runCatching(body).getOrElse { "failed: $it" }
            main.post { into.value = result }
        }, "bench").start()
    }

    /** The difference between two readings, filed under the state that began at [a]; none for a blink. */
    private fun stretch(a: Counters, b: Counters, shortest: Long = SHORTEST_MS): Stretch? {
        val ms = b.elapsedMs - a.elapsedMs
        if (ms < shortest) return null
        // Only threads alive at both ends: one that ended in between would take its whole count with it.
        var wakeups = 0L
        for ((tid, n) in b.switches) a.switches[tid]?.let { wakeups += n - it }
        val gauge = listOfNotNull(a.gaugeUa, b.gaugeUa).takeIf { it.isNotEmpty() }?.let { g -> g.sumOf { kotlin.math.abs(it) } / g.size / 1000.0 }
        return Stretch(
            state = startKey, startWall = a.wallMs, ms = ms, cpuMs = b.cpuMs - a.cpuMs, wakeups = wakeups,
            allocBytes = b.allocBytes - a.allocBytes, gcs = b.gcs - a.gcs, pssKb = b.pssKb,
            uah = if (a.chargeUah != null && b.chargeUah != null) a.chargeUah - b.chargeUah else null,
            pct = a.capacityPct - b.capacityPct, gaugeMa = gauge,
            tempMin = minOf(a.tempDeci, b.tempDeci), tempMax = maxOf(a.tempDeci, b.tempDeci),
            frames = frames, janky = janky, worstMs = worstNs / 1e6,
            cfg = startCfg + ", " + (if (PlaybackService.offloadWanted) "offloaded" else "on the CPU"),
        )
    }

    private fun batteryIntent(): Intent? = app.registerReceiver(null, IntentFilter(Intent.ACTION_BATTERY_CHANGED))

    /** Everything a stretch is measured by, read now. A few milliseconds, most of it the PSS. */
    private fun counters(): Counters {
        val stat = runCatching { File("/proc/self/stat").readText().substringAfterLast(") ").split(' ') }.getOrNull()
        // utime and stime, fields 14 and 15 of the whole line: 11 and 12 once the name is cut off.
        val ticks = stat?.let { (it.getOrNull(11)?.toLongOrNull() ?: 0L) + (it.getOrNull(12)?.toLongOrNull() ?: 0L) } ?: 0L
        val switches = HashMap<Int, Long>()
        File("/proc/self/task").listFiles()?.forEach { task ->
            val tid = task.name.toIntOrNull() ?: return@forEach
            runCatching {
                File(task, "status").useLines { lines ->
                    lines.firstOrNull { it.startsWith("voluntary_ctxt_switches") }?.substringAfter(':')?.trim()?.toLongOrNull()
                }
            }.getOrNull()?.let { switches[tid] = it }
        }
        val memory = Debug.MemoryInfo().also { Debug.getMemoryInfo(it) }
        val b = batteryIntent()
        fun prop(id: Int) = battery.getIntProperty(id).takeIf { it != Int.MIN_VALUE && it != 0 }
        return Counters(
            elapsedMs = SystemClock.elapsedRealtime(), wallMs = System.currentTimeMillis(),
            cpuMs = (ticks * msPerTick).toLong(), switches = switches,
            allocBytes = runtimeStat("art.gc.bytes-allocated"), gcs = runtimeStat("art.gc.gc-count"),
            pssKb = memory.totalPss.toLong(),
            chargeUah = prop(BatteryManager.BATTERY_PROPERTY_CHARGE_COUNTER)?.toLong(),
            capacityPct = battery.getIntProperty(BatteryManager.BATTERY_PROPERTY_CAPACITY),
            gaugeUa = (prop(BatteryManager.BATTERY_PROPERTY_CURRENT_AVERAGE) ?: prop(BatteryManager.BATTERY_PROPERTY_CURRENT_NOW))?.toLong(),
            tempDeci = b?.getIntExtra(BatteryManager.EXTRA_TEMPERATURE, 0) ?: 0,
        )
    }

    private fun runtimeStat(name: String) = Debug.getRuntimeStat(name)?.toLongOrNull() ?: 0L

    companion object {
        /** Stretches shorter than this are the blinks between two states (the screen going off stops the activity too). */
        const val SHORTEST_MS = 3_000L

        /** Every state a stretch is filed under, in the order the page lists them, with its name. */
        val STATES = linkedMapOf(
            "off-playing" to "Screen off, playing",
            "off-paused" to "Screen off, paused",
            "on-playing-player" to "Screen on, playing, player open",
            "on-playing-app" to "Screen on, playing, other page",
            "on-playing-away" to "Screen on, playing, another app",
            "on-paused" to "Screen on, paused",
            "charging" to "Charging",
        )
    }
}

/** One reading of every counter. */
internal class Counters(
    val elapsedMs: Long,
    val wallMs: Long,
    val cpuMs: Long,
    /** Voluntary context switches per thread: each is a thread going to sleep and being woken again. */
    val switches: Map<Int, Long>,
    val allocBytes: Long,
    val gcs: Long,
    val pssKb: Long,
    /** What is left in the battery, where the phone counts it (µAh). */
    val chargeUah: Long?,
    val capacityPct: Int,
    /** The fuel gauge's current, its own average where it keeps one (µA, either sign by maker). */
    val gaugeUa: Long?,
    /** Battery temperature in tenths of a degree. */
    val tempDeci: Int,
)

internal class Shown(val kept: List<Stretch>, val live: Stretch?)
