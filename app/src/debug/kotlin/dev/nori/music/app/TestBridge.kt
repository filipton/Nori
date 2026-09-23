package dev.nori.music.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.util.Log

/**
 * Debug builds only: lets `adb shell am broadcast` drive the app directly.
 *
 *   adb shell am broadcast -a dev.nori.music.TEST --es cmd open --es arg settings/audio
 *   adb shell am broadcast -a dev.nori.music.TEST --es cmd state
 *   adb shell am broadcast -a dev.nori.music.TEST --es cmd set --es arg limiter --es value true
 *   adb shell am broadcast -a dev.nori.music.TEST --es cmd play --es arg "search:noise 1"
 *
 * Answers go to logcat under the tag `noritest`, one line, so a script can read them back.
 */
class TestBridge : BroadcastReceiver() {
    /** What the player itself knows, with no Activity involved. */
    private fun serviceState(context: Context): String {
        val nori = dev.nori.music.Nori.get(context)
        val st = nori.player.state.value
        return """{"route":"background","playing":${st.playing},"title":"${st.current?.title.orEmpty()}",""" +
            """"artist":"${st.current?.artist.orEmpty()}","positionMs":${nori.player.positionMs},""" +
            """"durationMs":${st.durationMs},"queue":${st.queue.size},"index":${st.index},"error":"${st.error.orEmpty()}",""" +
            """"dspActive":${dev.nori.music.playback.Equalizer.active != null},""" +
            """"gainReductionDb":${dev.nori.music.playback.Equalizer.active?.gainReductionDb ?: 0f},""" +
            """"sinkBytes":${dev.nori.music.playback.TransitionSink.bytesWritten}}"""
    }

    private fun watchStates() {
        val counts = HashMap<String, Int>()
        val handle = androidx.compose.runtime.snapshots.Snapshot.registerApplyObserver { changed, _ ->
            for (o in changed) {
                val name = o.toString().take(120)
                counts[name] = (counts[name] ?: 0) + 1
            }
        }
        Handler(Looper.getMainLooper()).postDelayed({
            handle.dispose()
            Log.i("noritest", "states changed: ${counts.values.sum()} changes over ${counts.size} states")
            // By kind, with the value dropped, so many states of one kind count together.
            counts.entries.groupBy { it.key.substringBefore("(") + "@" + it.key.substringAfterLast("@").length }
                .mapValues { e -> e.value.sumOf { it.value } }.entries.sortedByDescending { it.value }.take(6)
                .forEach { Log.i("noritest", "kind ${it.value}x ${it.key}") }
            counts.entries.sortedByDescending { it.value }.take(12).forEach { Log.i("noritest", "state ${it.value}x ${it.key}") }
            Log.i("noritest", "states done")
        }, 1000)
    }

    private fun bench(): String {
        val out = StringBuilder()
        var sink = 0L
        run(out, "empty loop", 200_000) { i -> sink += i }
        run(out, "kotlin seekStep", 200_000) { i -> sink += kotlinSeekStep(0.3f + i * 1e-7f, 0.31f, 0.016f, 900f, 0.003f) }
        run(out, "jni seekStep", 200_000) { i -> sink += dev.nori.music.look.CoverLook.seekStep(0.3f + i * 1e-7f, 0.31f, 0.016f, 900f, 0.003f) }
        run(out, "uniffi swipeTurn", 20_000) { i -> sink += dev.nori.music.ffi.swipeTurn(-20f - i % 7, -950f, 1000f, true, true, true) }
        val a = IntArray(dev.nori.music.look.CoverLook.LEN) { 0xFF102030.toInt() + it * 997 }
        val b = IntArray(dev.nori.music.look.CoverLook.LEN) { 0xFFF0E0D0.toInt() - it * 991 }
        val o = IntArray(a.size)
        run(out, "jni mix (all colours)", 50_000) { i -> dev.nori.music.look.CoverLook.mix(a, b, (i % 100) / 100f, o); sink += o[3] }
        run(out, "kotlin compose lerp (all colours)", 50_000) { i ->
            val t = (i % 100) / 100f
            for (k in a.indices) o[k] = androidx.compose.ui.graphics.lerp(androidx.compose.ui.graphics.Color(a[k]), androidx.compose.ui.graphics.Color(b[k]), t).value.toInt()
            sink += o[3]
        }
        run(out, "jni duration string", 50_000) { i -> sink += dev.nori.music.look.CoverLook.duration((i % 7000).toLong(), false).length }
        run(out, "uniffi duration string", 20_000) { i -> sink += dev.nori.music.ffi.duration((i % 7000).toLong()).length }
        return out.append("sink $sink").toString()
    }

    private fun allocated() = android.os.Debug.getRuntimeStat("art.gc.bytes-allocated")?.toLongOrNull() ?: 0L

    /** Inline, so the loop itself boxes nothing and costs next to nothing: what is measured is the body. */
    private inline fun run(out: StringBuilder, name: String, n: Int, body: (Int) -> Unit) {
        repeat(3) { for (i in 0 until n / 10) body(i) } // warm up the JIT
        val a0 = allocated(); val t0 = System.nanoTime()
        for (i in 0 until n) body(i)
        val ns = (System.nanoTime() - t0).toDouble() / n; val bytes = (allocated() - a0).toDouble() / n
        out.append(String.format(java.util.Locale.ROOT, "%s: %.0f ns, %.0f B per call; ", name, ns, bytes))
    }

    /** nori_look::motion::seek_step, written out in Kotlin for the comparison. */
    private fun kotlinSeekStep(bar: Float, target: Float, dt: Float, widthPx: Float, speed: Float): Long {
        val width = widthPx.coerceAtLeast(1f)
        val gap = (target - bar) * width
        val next = if (kotlin.math.abs(gap) < 0.5f) bar else if (kotlin.math.abs(gap) < 2f) target else {
            val eased = bar + (target - bar) * (1f - kotlin.math.exp(-dt / 0.14f))
            if (kotlin.math.abs((target - eased) * width) < 2f) target else return (eased.toRawBits().toLong() shl 32)
        }
        if (speed <= 0f) return (next.toRawBits().toLong() shl 32) or 0xFFFF_FFFFL
        val ahead = ((target - next) * width).coerceIn(0f, 1f)
        val wait = ((1f - ahead) / (width * speed) * 1000f).toInt().coerceIn(16, 1000)
        return (next.toRawBits().toLong() shl 32) or wait.toLong()
    }

    override fun onReceive(context: Context, intent: Intent) {
        val cmd = intent.getStringExtra("cmd") ?: return
        val arg = intent.getStringExtra("arg").orEmpty()
        val value = intent.getStringExtra("value").orEmpty()
        Handler(Looper.getMainLooper()).post {
            val reply = when (cmd) {
                "open" -> TestHooks.open?.let { it(arg); "ok" } ?: "no ui"
                // Playback state must be answerable with the app in the background, because that is
                // where the interesting bugs are: resuming from a notification, a lock screen, a
                // headset button. The UI's richer answer is used when there is a UI.
                "state" -> TestHooks.state?.invoke() ?: serviceState(context)
                "set" -> TestHooks.set?.let { if (it(arg, value)) "ok" else "unknown setting $arg" } ?: "no ui"
                "play" -> TestHooks.play?.let { it(arg); "ok" } ?: "no ui"
                "login" -> TestHooks.login?.let { it(arg); "ok" } ?: "no ui"
                "do" -> TestHooks.act?.let { it(arg); "ok" } ?: "no ui"
                // Which Compose states change in the next second, and how often: a screen that redraws
                // when nothing on it moves has one of these changing every frame.
                "states" -> { watchStates(); "watching" }
                // What a crossing into the core costs, by kind, against the same work done in Kotlin.
                "bench" -> bench()
                else -> "unknown command $cmd"
            }
            Log.i("noritest", reply)
        }
    }
}
