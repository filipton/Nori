package dev.flint.music.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.os.Handler
import android.os.Looper
import android.util.Log

/**
 * Debug builds only: lets `adb shell am broadcast` drive the app directly.
 *
 *   adb shell am broadcast -a dev.flint.music.TEST --es cmd open --es arg settings/audio
 *   adb shell am broadcast -a dev.flint.music.TEST --es cmd state
 *   adb shell am broadcast -a dev.flint.music.TEST --es cmd set --es arg limiter --es value true
 *   adb shell am broadcast -a dev.flint.music.TEST --es cmd play --es arg "search:noise 1"
 *
 * Answers go to logcat under the tag `flinttest`, one line, so a script can read them back.
 */
class TestBridge : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        val cmd = intent.getStringExtra("cmd") ?: return
        val arg = intent.getStringExtra("arg").orEmpty()
        val value = intent.getStringExtra("value").orEmpty()
        Handler(Looper.getMainLooper()).post {
            val reply = when (cmd) {
                "open" -> TestHooks.open?.let { it(arg); "ok" } ?: "no ui"
                "state" -> TestHooks.state?.invoke() ?: "no ui"
                "set" -> TestHooks.set?.let { if (it(arg, value)) "ok" else "unknown setting $arg" } ?: "no ui"
                "play" -> TestHooks.play?.let { it(arg); "ok" } ?: "no ui"
                "login" -> TestHooks.login?.let { it(arg); "ok" } ?: "no ui"
                "do" -> TestHooks.act?.let { it(arg); "ok" } ?: "no ui"
                else -> "unknown command $cmd"
            }
            Log.i("flinttest", reply)
        }
    }
}
