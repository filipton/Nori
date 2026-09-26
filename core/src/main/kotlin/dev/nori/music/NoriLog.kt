package dev.nori.music

/**
 * The app's own log lines (logcat, tag `nori`), for events only. Each is kept too with the core's in memory
 * (nori-model's alog, the last few hundred lines), so that the perf build's copy of the app's lines taken as
 * an invariant breaks has what the Kotlin said beside what the Rust did: logcat's buffer is the whole
 * system's, and a codec's chatter turns it over in minutes.
 */
object NoriLog {
    private const val TAG = "nori"

    /** False once the library could not be reached (a JVM unit test has none): logcat alone from then on. */
    @Volatile private var keeps = true

    fun i(message: String) {
        android.util.Log.i(TAG, message)
        keep(message)
    }

    fun w(message: String, error: Throwable? = null) {
        if (error == null) android.util.Log.w(TAG, message) else android.util.Log.w(TAG, message, error)
        keep(if (error == null) message else "$message: $error")
    }

    private fun keep(line: String) {
        if (!keeps) return
        try {
            dev.nori.music.ffi.model.alogKeep(line)
        } catch (e: Throwable) {
            keeps = false
        }
    }
}
