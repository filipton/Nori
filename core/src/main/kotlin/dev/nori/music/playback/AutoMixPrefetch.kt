package dev.nori.music.playback

import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.cache.Cache
import androidx.media3.datasource.cache.ContentMetadata
import dalvik.annotation.optimization.CriticalNative

/** The measurer's doors (crates/android/src/measure.rs, over nori-engine's `Measurer`). */
internal object MeasureJni {
    init { System.loadLibrary("norimusic") }
    /** Makes the measurer; idle until it is asked. */
    @JvmStatic external fun start()
    /** The songs coming up may have changed: the core names them, and the same songs change nothing. */
    @JvmStatic @CriticalNative external fun update()
    /** A song has become whole in one of the caches. */
    @JvmStatic @CriticalNative external fun arrived()
    @JvmStatic @CriticalNative external fun stop()
}

/** What the measurer asks of the platform, from its own thread: where a song's bytes are, and that one was measured. */
@UnstableApi
internal object MeasureBridge {
    @Volatile var prefetch: AutoMixPrefetch? = null

    @JvmStatic fun whole(id: String): Array<String>? = prefetch?.whole(id)
    @JvmStatic fun measured() { prefetch?.onMeasured?.invoke() }
}

/**
 * Measures tracks before they are played, so a transition has both halves' tempo, beats and cue points
 * the first time those two songs meet. The streaming tap in [TransitionSink] only finishes a track as
 * it ends, which is one boundary too late: the mix out of a song the phone has never heard had nothing
 * to plan from and fell back to a plain fade.
 *
 * The measuring is the core's, for both players: nori-engine's measurer (crates/android/src/measure.rs)
 * decodes each song once, whole, on a thread of the lowest priority, reading its files straight. This
 * only says where a song's bytes are in media3's caches, and when one has become whole: the caches'
 * own callbacks say so as the precacher or the player writes the last of it, so the song after the one
 * playing is measured as soon as it is on the device, and nothing is ever measured while its bytes are
 * still coming. Nothing here touches the network.
 */
@UnstableApi
class AutoMixPrefetch(
    private val sources: MediaSources,
    /** A track has been measured: whatever was planned without it can be planned again. On the measuring thread. */
    internal val onMeasured: () -> Unit = {},
) {
    init {
        MeasureBridge.prefetch = this
        sources.onWhole = { MeasureJni.arrived() }
        MeasureJni.start()
    }

    /** The queue moved or was edited: the measurer asks the core which songs come up now. */
    fun update() = MeasureJni.update()

    fun release() {
        MeasureJni.stop()
        sources.onWhole = null
        if (MeasureBridge.prefetch === this) MeasureBridge.prefetch = null
    }

    /**
     * [id]'s bytes if they are all on the device: its cache key, then the files that hold them in order -
     * a download's, or else a streamed copy's. Null while any of it is missing.
     */
    internal fun whole(id: String): Array<String>? =
        files(sources.downloadCache, sources.downloadKey(id)) ?: runCatching { sources.streamKey(id) }.getOrNull()?.let { files(sources.streamCache, it) }

    /**
     * The files of [key] in [cache] from its first byte on, when they hold all of it but perhaps a short
     * tail (see [MediaSources.isWhole]); the key first.
     */
    private fun files(cache: Cache, key: String): Array<String>? {
        if (!MediaSources.isWhole(cache, key)) return null
        val spans = cache.getCachedSpans(key)
        val out = ArrayList<String>(spans.size + 1)
        out += key
        var at = 0L
        for (span in spans) {
            val file = span.file ?: break
            if (span.position != at) break
            out += file.path
            at += span.length
        }
        return if (at >= ContentMetadata.getContentLength(cache.getContentMetadata(key)) - MediaSources.TAIL) out.toTypedArray() else null
    }
}
