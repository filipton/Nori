package dev.nori.music.app.perf

import android.os.Build
import org.json.JSONObject
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale

/**
 * One stretch as it is kept (perf_log.rs, as JSON): the state it was in, how long, and what it cost.
 * [uah] is the charge used by the battery's own counter, none where the phone does not keep one;
 * [pct] is the drop in the battery level, which every phone reports.
 */
internal class Stretch(
    val state: String,
    val startWall: Long,
    val ms: Long,
    val cpuMs: Long,
    val wakeups: Long,
    val allocBytes: Long,
    val gcs: Long,
    val pssKb: Long,
    val uah: Long?,
    val pct: Int,
    val gaugeMa: Double?,
    val tempMin: Int,
    val tempMax: Int,
    val frames: Long,
    val janky: Long,
    val worstMs: Double,
    val cfg: String,
) {
    fun json(): String = JSONObject().apply {
        put("s", state); put("t0", startWall); put("ms", ms); put("cpu", cpuMs); put("wk", wakeups)
        put("al", allocBytes); put("gc", gcs); put("pss", pssKb); uah?.let { put("uah", it) }; put("pct", pct)
        gaugeMa?.let { put("gma", it) }; put("tmin", tempMin); put("tmax", tempMax)
        put("fr", frames); put("jk", janky); put("worst", worstMs); put("cfg", cfg)
    }.toString()

    companion object {
        fun parse(json: String): Stretch? = runCatching {
            val o = JSONObject(json)
            Stretch(
                state = o.getString("s"), startWall = o.getLong("t0"), ms = o.getLong("ms"), cpuMs = o.getLong("cpu"),
                wakeups = o.getLong("wk"), allocBytes = o.getLong("al"), gcs = o.getLong("gc"), pssKb = o.getLong("pss"),
                uah = if (o.has("uah")) o.getLong("uah") else null, pct = o.getInt("pct"),
                gaugeMa = if (o.has("gma")) o.getDouble("gma") else null, tempMin = o.getInt("tmin"), tempMax = o.getInt("tmax"),
                frames = o.getLong("fr"), janky = o.getLong("jk"), worstMs = o.getDouble("worst"), cfg = o.getString("cfg"),
            )
        }.getOrNull()
    }
}

/** Every stretch of one state added up. Battery figures leave charging out: it runs the other way. */
internal class Totals(val state: String) {
    var count = 0
    var ms = 0L
    var cpuMs = 0L
    var wakeups = 0L
    var allocBytes = 0L
    var gcs = 0L
    var pssKb = 0L
    /** Charge used, over the stretches the counter measured, and how long those were. */
    var uah = 0L
    var uahMs = 0L
    var pct = 0
    var frames = 0L
    var janky = 0L

    fun add(s: Stretch) {
        count++; ms += s.ms; cpuMs += s.cpuMs; wakeups += s.wakeups; allocBytes += s.allocBytes; gcs += s.gcs
        pssKb = s.pssKb; pct += s.pct; frames += s.frames; janky += s.janky
        s.uah?.let { uah += it; uahMs += s.ms }
    }

    val battery get() = state != "charging"
    val cpuPct get() = if (ms > 0) cpuMs * 100.0 / ms else 0.0
    val wakeupsPerS get() = if (ms > 0) wakeups * 1000.0 / ms else 0.0
    val allocKbPerMin get() = if (ms > 0) allocBytes / 1024.0 / (ms / 60_000.0) else 0.0
    val mah get() = uah / 1000.0
    val mahPerH get() = if (uahMs > 0) mah / (uahMs / 3_600_000.0) else null
    val pctPerH get() = if (ms > 0) pct / (ms / 3_600_000.0) else 0.0
    val jankPct get() = if (frames > 0) janky * 100.0 / frames else null

    companion object {
        /** The stretches added up by state, in [Recorder.STATES]'s order; states never seen left out. */
        fun of(stretches: List<Stretch>): List<Totals> {
            val by = LinkedHashMap<String, Totals>()
            Recorder.STATES.keys.forEach { by[it] = Totals(it) }
            stretches.forEach { s -> by.getOrPut(s.state) { Totals(s.state) }.add(s) }
            return by.values.filter { it.count > 0 }
        }
    }
}

internal fun stateName(key: String) = Recorder.STATES[key] ?: key

internal fun f(pattern: String, vararg args: Any?) = String.format(Locale.ROOT, pattern, *args)

internal fun duration(ms: Long): String {
    val s = ms / 1000
    return when {
        s >= 3600 -> f("%d h %02d min", s / 3600, s / 60 % 60)
        s >= 60 -> f("%d min %02d s", s / 60, s % 60)
        else -> "$s s"
    }
}

internal fun whenStarted(wall: Long): String = SimpleDateFormat("MM-dd HH:mm", Locale.ROOT).format(Date(wall))

/** One state's figures on one line, as the page and the report show them. */
internal fun Totals.line(): String = buildString {
    append(duration(ms)).append(f(", CPU %.2f %%, %.1f wakeups/s, %.0f KB/min allocated, %d GCs, PSS %d MB", cpuPct, wakeupsPerS, allocKbPerMin, gcs, pssKb / 1024))
    if (battery) {
        mahPerH?.let { append(f(", %.1f mAh (%.1f mAh/h)", mah, it)) } ?: append(f(", %d %% (%.2f %%/h)", pct, pctPerH))
    }
    jankPct?.let { append(f(", %d frames, %.1f %% janky", frames, it)) }
}

internal fun Stretch.line(): String = buildString {
    append(whenStarted(startWall)).append("  ").append(duration(ms))
    append(f(", CPU %.2f %%, %.1f wakeups/s, %.0f KB/min, %d GCs, PSS %d MB", if (ms > 0) cpuMs * 100.0 / ms else 0.0,
        if (ms > 0) wakeups * 1000.0 / ms else 0.0, if (ms > 0) allocBytes / 1024.0 / (ms / 60_000.0) else 0.0, gcs, pssKb / 1024))
    if (state != "charging") {
        uah?.let { append(f(", %.1f mAh (%.1f mAh/h)", it / 1000.0, it / 1000.0 / (ms / 3_600_000.0))) } ?: append(", $pct %")
    }
    gaugeMa?.let { append(f(", gauge %.0f mA", it)) }
    append(f(", %.1f-%.1f °C", tempMin / 10.0, tempMax / 10.0))
    if (frames > 0) append(f(", %d frames, %d janky, worst %.0f ms", frames, janky, worstMs))
    append(" [").append(cfg).append(']')
}

/** The report the Share button sends: plain text, so it reads the same in a chat, a mail or an issue. */
internal fun report(shown: Shown, calls: String, covers: String): String = buildString {
    val all = shown.kept + listOfNotNull(shown.live)
    val app = dev.nori.music.app.BuildConfig.VERSION_NAME
    val sha = dev.nori.music.app.BuildConfig.GIT_SHA
    appendLine("Nori perf report")
    appendLine("Device: ${Build.MANUFACTURER} ${Build.MODEL} (${Build.DEVICE}), Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})")
    appendLine("Build: $app ($sha, ${dev.nori.music.app.BuildConfig.BUILD_TYPE})")
    if (all.isNotEmpty()) appendLine("Recorded: ${whenStarted(all.first().startWall)} to ${whenStarted(all.last().startWall + all.last().ms)}, ${all.size} stretches")
    appendLine("Battery counter: " + if (all.any { it.uah != null }) "yes (mAh)" else "no (battery % only)")
    appendLine()
    appendLine("By state")
    append(table(Totals.of(all)))
    appendLine()
    appendLine("Stretches, newest first")
    all.asReversed().take(60).forEach { s -> appendLine("${stateName(s.state)}: ${s.line()}") }
    if (calls.isNotEmpty()) appendLine().appendLine("Call benchmark: $calls")
    if (covers.isNotEmpty()) appendLine().appendLine("Cover benchmark: $covers")
}

/** The states side by side in padded columns, for a monospaced reader; "-" where a figure does not apply. */
internal fun table(totals: List<Totals>): String {
    val head = listOf("state", "time", "CPU %", "wakeups/s", "KB/min", "GCs", "PSS MB", "mAh", "mAh/h", "%/h", "frames", "janky %")
    val rows = totals.map { t ->
        listOf(
            stateName(t.state), duration(t.ms), f("%.2f", t.cpuPct), f("%.1f", t.wakeupsPerS), f("%.0f", t.allocKbPerMin),
            "${t.gcs}", "${t.pssKb / 1024}",
            if (t.battery && t.uahMs > 0) f("%.1f", t.mah) else "-",
            t.mahPerH?.takeIf { t.battery }?.let { f("%.1f", it) } ?: "-",
            if (t.battery) f("%.2f", t.pctPerH) else "-",
            if (t.frames > 0) "${t.frames}" else "-",
            t.jankPct?.let { f("%.1f", it) } ?: "-",
        )
    }
    val widths = head.indices.map { i -> (rows.map { it[i] } + head[i]).maxOf { it.length } }
    return buildString {
        (listOf(head) + rows).forEach { r -> appendLine(r.mapIndexed { i, c -> if (i == 0) c.padEnd(widths[i]) else c.padStart(widths[i]) }.joinToString("  ").trimEnd()) }
    }
}
