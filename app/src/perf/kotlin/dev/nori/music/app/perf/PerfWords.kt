package dev.nori.music.app.perf

/**
 * The perf build's Performance page: its buttons, headings and notes. A measuring tool for whoever builds
 * the app, in English like the report it shares, so its words are kept here rather than in the app's
 * string resources.
 */
object PerfWords {
    const val shareReport = "Share report"
    const val startFresh = "Start fresh"
    const val byState = "By state"
    const val nothingRecorded = "Nothing recorded yet. A stretch is kept when the app moves into another state: the screen goes off, music stops, the player opens."
    const val frames = "Frames"
    const val noFrames = "No frames counted yet: they are counted while the app is on screen."
    const val stretches = "Stretches"
    const val benchmarks = "Benchmarks"
    const val runCalls = "Run call benchmark"
    const val runCovers = "Run cover benchmark"
    const val calls = "Calls"
    const val covers = "Covers"
    /** A benchmark under way. */
    const val running = "running..."
    /** The app's own log and crashes, folded away until asked for. */
    const val log = "Log"
    const val showLog = "Show the log"
    const val hideLog = "Hide the log"
    const val logNote = "The app's own recent log and any crash, read when shown. The shared report ends with them."
    /** A stretch's events, unfolded. */
    const val hideEvents = "Hide events"

    /** The button that unfolds a stretch's events: "3 events". */
    fun events(count: Int): String = if (count == 1) "1 event" else "$count events"

    /** A benchmark that stopped with [error]. */
    fun failed(error: String): String = "failed: $error"
}
