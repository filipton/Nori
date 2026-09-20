package dev.nori.music.downloads

/**
 * One run of the download queue: everything queued since the queue was last empty. Its total holds
 * still while songs finish ("12 of 49", never "1 of 47"), and it is fed only by what the download
 * manager reports - queued, finished, failed, removed - so it is rebuilt the same way after the
 * process has died and media3 has restored its queue. Not thread-safe: the manager reports on the
 * main thread and the notification is built there, so that is the only thread that touches it.
 */
class DownloadBatch {
    /** Queued or downloading: what the batch is still waiting for. */
    private val open = LinkedHashSet<String>()
    private val failedIds = HashSet<String>()
    private val labels = HashMap<String, String>()

    var total = 0; private set
    var done = 0; private set
    var failed = 0; private set

    /** Nothing left to wait for. A batch that has drained is replaced by the next song queued. */
    val drained get() = open.isEmpty()

    /** Songs that have finished either way. */
    val finished get() = done + failed

    fun isOpen(id: String) = id in open

    /**
     * A song entered the queue. [label] is what the batch is called if every song shares it (an
     * album's name). Returns true when this starts a new batch.
     */
    fun queued(id: String, label: String? = null): Boolean {
        if (id in open) return false
        val fresh = open.isEmpty()
        if (fresh) reset()
        if (failedIds.remove(id)) failed-- // tried again: the same song, not one more
        else total++
        open += id
        if (!label.isNullOrBlank()) labels[id] = label
        return fresh
    }

    fun completed(id: String) {
        if (open.remove(id)) done++
    }

    fun failed(id: String) {
        if (open.remove(id)) { failed++; failedIds += id }
    }

    /** Stopped before it finished, or a failure given up on: it no longer counts at all. */
    fun removed(id: String) {
        when {
            open.remove(id) -> total--
            failedIds.remove(id) -> { failed--; total-- }
        }
        labels.remove(id)
    }

    /** "12 of 49": the song being worked on now, counted from one, never past the total. */
    fun position(): Int = (finished + 1).coerceAtMost(total)

    /** How far the whole batch is, 0..1: finished songs plus [inFlight], the sum of the running ones' fractions. */
    fun fraction(inFlight: Double): Float =
        if (total <= 0) 0f else ((finished + inFlight.coerceIn(0.0, open.size.toDouble())) / total).toFloat().coerceIn(0f, 1f)

    /** The one name every song of the batch shares, when they all share one. */
    fun label(): String? {
        if (labels.size < total) return null
        return labels.values.distinct().singleOrNull()
    }

    private fun reset() {
        failedIds.clear(); labels.clear()
        total = 0; done = 0; failed = 0
    }
}
