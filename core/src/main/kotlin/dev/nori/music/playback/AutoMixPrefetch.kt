package dev.nori.music.playback

import android.media.MediaCodec
import android.media.MediaDataSource
import android.media.MediaExtractor
import android.media.MediaFormat
import androidx.media3.common.util.UnstableApi
import androidx.media3.datasource.DataSource
import androidx.media3.datasource.DataSpec
import androidx.media3.datasource.cache.ContentMetadata
import dev.nori.music.ffi.Core
import java.util.concurrent.Executors
import java.util.concurrent.Future

/**
 * Measures tracks before they are played, so a transition has both halves' tempo, beats and cue points
 * the first time those two songs meet. The streaming tap in [TransitionSink] only finishes a track as
 * it ends, which is one boundary too late: the mix out of a song the phone has never heard had nothing
 * to plan from and fell back to a plain fade.
 *
 * Nothing here touches the network. A track is only measured once its bytes are already on the device -
 * downloaded, or fetched ahead into the stream cache by [Precacher] - so this costs radio time never and
 * CPU only on a thread that yields to everything else. A track that is not there yet is simply left for
 * the next time the queue moves, by which point the precacher has usually brought it in.
 *
 * Decoding is MediaCodec's, at whatever speed the CPU manages, reading through the same cache-first
 * [MediaSources] chain playback uses; the PCM goes straight into the streaming analyser and is never
 * held as a whole track.
 */
@UnstableApi
class AutoMixPrefetch(
    private val sources: MediaSources,
    private val coreOf: () -> Core,
    /** A track has been measured: whatever was planned without it can be planned again. */
    private val onMeasured: () -> Unit = {},
) {
    private val worker = Executors.newSingleThreadExecutor { Thread(it, "nori-analyse-ahead").apply { priority = Thread.MIN_PRIORITY } }
    private var running: Future<*>? = null

    /**
     * [ids] are the songs coming up, the one playing first. Whatever was being measured for an older
     * queue is abandoned: the point of this is the next boundary, not completeness.
     */
    fun update(ids: List<String>) {
        cancel()
        val wanted = ids.filterNot { it.startsWith("ext-") || it.startsWith("pl-") || it.startsWith(RADIO_PREFIX) }
        if (wanted.isEmpty()) return
        running = worker.submit {
            val missing = runCatching { coreOf().analysisMissing(wanted) }.getOrDefault(emptyList())
            val waiting = missing.count { !onDevice(it) }
            // One line per queue move, and only while AutoMix is on: which of the tracks coming up have
            // never been measured, and how many of those are not on the device yet to measure.
            android.util.Log.i("nori", "measuring ahead: ${missing.size} of ${wanted.size} unmeasured, $waiting not on the device yet")
            for (id in missing) {
                if (Thread.currentThread().isInterrupted) return@submit
                if (!onDevice(id)) continue
                runCatching { measure(id); onMeasured() }.onFailure { android.util.Log.i("nori", "analysing $id ahead failed: $it") }
            }
        }
    }

    fun cancel() {
        running?.cancel(true)
        running = null
    }

    fun release() { cancel(); worker.shutdownNow() }

    /** Whether the whole file is already on the device, as a download or as a complete cache entry. */
    private fun onDevice(id: String): Boolean {
        if (id in sources.downloaded) return true
        val key = runCatching { sources.resolve(DataSpec(songUri(id))).key }.getOrNull() ?: return false
        val length = ContentMetadata.getContentLength(sources.streamCache.getContentMetadata(key))
        return length > 0 && sources.streamCache.getCachedBytes(key, 0, length) >= length
    }

    /** Decodes the whole track into the streaming analyser and stores what comes out. */
    private fun measure(id: String) {
        val source = CachedTrack(sources.factory.createDataSource(), id)
        val extractor = MediaExtractor()
        var analyser = 0L
        try {
            extractor.setDataSource(source)
            val track = (0 until extractor.trackCount).firstOrNull {
                extractor.getTrackFormat(it).getString(MediaFormat.KEY_MIME)?.startsWith("audio/") == true
            } ?: return
            val format = extractor.getTrackFormat(track)
            extractor.selectTrack(track)
            val codec = MediaCodec.createDecoderByType(format.getString(MediaFormat.KEY_MIME)!!)
            val ended = try {
                codec.configure(format, null, null, 0)
                codec.start()
                // Handed over as it is created, so a decode that throws half way still has its handle freed.
                decode(codec, extractor, if (format.containsKey(MediaFormat.KEY_DURATION)) format.getLong(MediaFormat.KEY_DURATION) / 1000 else 0L) { analyser = it }
            } finally {
                runCatching { codec.stop() }
                codec.release()
            }
            // Only a whole song is an analysis of it (nori_player::transitions::whole_song decides, in
            // analysisFinishWhole); an interrupted decode is not even offered.
            val expectedMs = if (format.containsKey(MediaFormat.KEY_DURATION)) format.getLong(MediaFormat.KEY_DURATION) / 1000 else 0L
            if (!ended || analyser == 0L) {
                android.util.Log.i("nori", "measuring $id ahead stopped before its end: not stored")
                return
            }
            val a = coreOf().analysisFinishWhole(id, analyser, expectedMs)
            android.util.Log.i("nori", "analysed $id ahead: ${a?.let { "%.2f bpm (conf %.2f, stab %.2f), key %s".format(it.bpm, it.bpmConfidence, it.stability, dev.nori.music.ffi.automixKeyName(it.key)) } ?: "not stored: not the whole song, or too short"}")
        } finally {
            if (analyser != 0L) AutoMixAnalyzer.destroy(analyser)
            extractor.release()
            source.close()
        }
    }

    /**
     * The plain synchronous decode loop; every output buffer goes into the analyser and is released
     * again. True once the decoder has reached the end of the stream, false when it was interrupted
     * first. How much was heard is the analyser's own count; whether that is the whole song is the
     * core's call (analysisFinishWhole). The output format is read when it changes, not per buffer.
     */
    private fun decode(codec: MediaCodec, extractor: MediaExtractor, expectedMs: Long, created: (Long) -> Unit): Boolean {
        val info = MediaCodec.BufferInfo()
        var analyser = 0L
        var fed = false
        var rate = 0
        var channels = 0
        while (!Thread.currentThread().isInterrupted) {
            if (!fed) {
                val index = codec.dequeueInputBuffer(TIMEOUT_US)
                if (index >= 0) {
                    val buffer = codec.getInputBuffer(index)!!
                    val read = extractor.readSampleData(buffer, 0)
                    if (read < 0) {
                        codec.queueInputBuffer(index, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM)
                        fed = true
                    } else {
                        codec.queueInputBuffer(index, 0, read, extractor.sampleTime, 0)
                        extractor.advance()
                    }
                }
            }
            when (val index = codec.dequeueOutputBuffer(info, TIMEOUT_US)) {
                MediaCodec.INFO_TRY_AGAIN_LATER -> {}
                MediaCodec.INFO_OUTPUT_FORMAT_CHANGED -> {
                    val out = codec.outputFormat
                    rate = out.getInteger(MediaFormat.KEY_SAMPLE_RATE, 0)
                    channels = out.getInteger(MediaFormat.KEY_CHANNEL_COUNT, 0)
                }
                else -> if (index >= 0) {
                    if (info.size > 0) {
                        if (rate == 0) codec.outputFormat.let { rate = it.getInteger(MediaFormat.KEY_SAMPLE_RATE, 0); channels = it.getInteger(MediaFormat.KEY_CHANNEL_COUNT, 0) }
                        if (analyser == 0L && rate > 0 && channels > 0) {
                            analyser = AutoMixAnalyzer.create(rate, channels, expectedMs)
                            created(analyser)
                        }
                        val buffer = codec.getOutputBuffer(index)
                        // 16-bit is what a decoder hands out unless it is asked for float, which this never does.
                        if (analyser != 0L && buffer != null) AutoMixAnalyzer.feed(analyser, buffer, info.offset, info.size, PCM_16)
                    }
                    codec.releaseOutputBuffer(index, false)
                    if (info.flags and MediaCodec.BUFFER_FLAG_END_OF_STREAM != 0) return true
                }
            }
        }
        return false
    }

    /**
     * One song's bytes as [MediaExtractor] wants them: random access, by position. The media3 chain
     * underneath is sequential, so a jump backwards (which an extractor only does while reading a
     * header) reopens it at the new position; a read that carries on where the last one stopped costs
     * nothing extra.
     */
    private class CachedTrack(private val source: DataSource, private val id: String) : MediaDataSource() {
        private var open = false
        private var at = -1L
        private var length = -1L

        private fun openAt(position: Long) {
            close()
            val spec = DataSpec.Builder().setUri(songUri(id)).setPosition(position).build()
            val left = source.open(spec)
            open = true
            at = position
            if (left != androidx.media3.common.C.LENGTH_UNSET.toLong() && length < 0) length = position + left
        }

        override fun readAt(position: Long, buffer: ByteArray, offset: Int, size: Int): Int {
            if (size == 0) return 0
            if (!open || position != at) openAt(position)
            val n = source.read(buffer, offset, size)
            if (n > 0) at += n
            return n
        }

        override fun getSize(): Long {
            if (length < 0) runCatching { openAt(0) }
            return length
        }

        override fun close() {
            if (open) runCatching { source.close() }
            open = false
            at = -1L
        }
    }

    private companion object {
        const val TIMEOUT_US = 10_000L
        /** `C.ENCODING_PCM_16BIT`, as the Rust analyser numbers encodings. */
        const val PCM_16 = 2
    }
}
