package dev.nori.music.app.ui

import dev.nori.music.look.CoverPixels

/**
 * One view's asking for one cover, decided without Android (CoverFetchTest): what is kept in memory is
 * shown at once, anything else is asked of the loader, and a load that fails is asked again once, a
 * moment later, before the view gives up on the picture. [CoverImage] is this over the app's loader.
 *
 * A failure used to be final: the player's sleeve drew its placeholder for the rest of the song after a
 * single cover fetch that timed out, or one the loader answered with nothing because the song had been
 * skipped away from and back while it was on the wire. Now a failure the next try can mend (the network,
 * the server, a file cut short, a fetch nobody waited for) waits [RETRY_MS] and asks again, with the view
 * still showing that the picture is on its way; one that cannot be mended (a format that is not decoded)
 * is given up at once. Every failure is told to [Source.failed], which the perf build logs with its reason.
 */
internal class CoverFetch<P : Any>(
    val url: String?,
    private val source: Source<P>,
    initial: P?,
    private val shown: (P) -> Unit,
    private val missing: () -> Unit,
) {
    /** What a fetch asks of the platform. */
    interface Source<P : Any> {
        /** The picture kept in memory for [url], and whether it is enough for a view [width] x [height]; null for none. */
        fun kept(url: String, width: Int, height: Int): Pair<P, Boolean>?

        /** Asks for the picture; [done] with it and [CoverPixels.OK], or null and why not, unless cancelled first. */
        fun load(url: String, width: Int, height: Int, done: (P?, Int) -> Unit): Handle

        /** Runs [run] after [ms] on the same thread, unless cancelled first. */
        fun later(ms: Long, run: () -> Unit): Handle

        /** A load of [url] failed with [status]; [again]: it is asked again in a moment. */
        fun failed(url: String, status: Int, again: Boolean)
    }

    fun interface Handle {
        fun cancel()
    }

    /** What is showing: [initial], the picture kept when the view was composed, until another comes. */
    var picture: P? = initial
        private set
    private var request: Handle? = null
    private var retry: Handle? = null
    private var failures = 0
    private var width = 0
    private var height = 0

    /** Asks for the picture a view [width] x [height] pixels draws, unless what is kept will do. */
    fun want(width: Int, height: Int) {
        val url = url ?: return
        this.width = width
        this.height = height
        retry?.cancel()
        retry = null
        val k = source.kept(url, width, height)
        if (k != null && picture == null) show(k.first)
        if (k != null && k.second) {
            if (picture !== k.first) show(k.first)
            request?.cancel()
            request = null
            return
        }
        request?.cancel()
        request = source.load(url, width, height) { p, status -> answered(url, p, status) }
    }

    /** The view has gone, or wants another size: whatever is on its way is let go, a try to come too. */
    fun stop() {
        request?.cancel()
        request = null
        retry?.cancel()
        retry = null
    }

    /** Whether a request or a try again is on its way. */
    val asking: Boolean get() = request != null || retry != null

    private fun answered(url: String, p: P?, status: Int) {
        request = null
        if (p != null) {
            failures = 0
            show(p)
            return
        }
        val again = failures < RETRIES && mendable(status)
        failures++
        source.failed(url, status, again)
        if (again) retry = source.later(RETRY_MS) { retry = null; want(width, height) }
        else if (picture == null) missing()
    }

    private fun show(p: P) {
        picture = p
        shown(p)
    }

    companion object {
        /** How many times a failed load is asked again. */
        const val RETRIES = 1

        /** How long after a failure it is asked again: past a moment's outage, not so long the user waits on a plate. */
        const val RETRY_MS = 2_000L

        /** Whether asking again may bring the picture: not for a format that is not decoded, nor a Bitmap that cannot be made. */
        fun mendable(status: Int): Boolean = status != CoverPixels.UNKNOWN && status != CoverPixels.BAD_BITMAP
    }
}
