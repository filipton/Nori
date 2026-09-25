package dev.nori.music.playback

import android.os.Handler
import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.util.UnstableApi
import androidx.media3.common.util.Util
import androidx.media3.decoder.CryptoConfig
import androidx.media3.decoder.DecoderException
import androidx.media3.decoder.DecoderInputBuffer
import androidx.media3.decoder.Decoder
import androidx.media3.decoder.SimpleDecoderOutputBuffer
import androidx.media3.exoplayer.audio.AudioRendererEventListener
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DecoderAudioRenderer
import dalvik.annotation.optimization.CriticalNative
import dalvik.annotation.optimization.FastNative
import java.nio.ByteBuffer

/** The decoder in the core (crates/settings/src/decoder.rs over nori_player::decode). */
internal object RustDecoderJni {
    init { System.loadLibrary("norimusic") }

    /** The codec number for [mime] at [rate] Hz, or 0 when the platform's own decoder should take the stream. */
    @JvmStatic external fun takes(mime: String, codecs: String?, channels: Int, rate: Int, chipTakesIt: Boolean): Int
    /**
     * An AAC stream that says AAC-LC at 24 kHz or less: HE-AAC whose SBR is signalled only inside the
     * stream (a station's AAC+ over ADTS), in all likelihood. The platform's decoder plays it at its real
     * rate; the audio chip would be set up for AAC-LC at half of it.
     */
    @JvmStatic external fun implicitSbr(mime: String, codecs: String?, rate: Int): Boolean
    /** [takes] for a stream split by the platform's MediaExtractor (measuring ahead), whose setup data differs for some codecs. */
    @JvmStatic external fun takesExtracted(mime: String, codecs: String?, channels: Int): Int
    /** [delayKnown]: the stream states its encoder delay (an MP3's LAME header), which media3 trims. */
    @JvmStatic external fun create(codec: Int, rate: Int, channels: Int, extra: ByteArray?, delayKnown: Boolean): Long
    @JvmStatic @CriticalNative external fun destroy(h: Long)
    /** The next packet does not follow the last; [atStart]: back at the very beginning. */
    @JvmStatic @CriticalNative external fun reset(h: Long, atStart: Boolean)
    /** `channels << 32 | rate` of what comes out. */
    @JvmStatic @CriticalNative external fun shape(h: Long): Long
    @JvmStatic @CriticalNative external fun maxBytes(codec: Int, channels: Int, float: Boolean): Int
    /** Bytes written; -1 skip the packet; -2 broken; below that, `-(bytes needed) - 2`, then [take]. */
    @JvmStatic @FastNative external fun decode(h: Long, input: ByteBuffer, inPos: Int, inLen: Int, output: ByteBuffer, outPos: Int, outCap: Int, float: Boolean): Int
    @JvmStatic @FastNative external fun take(h: Long, output: ByteBuffer, outPos: Int, outCap: Int, float: Boolean): Int
}

/**
 * One stream decoded in the core. media3 hands in each packet its extractor split off, in a direct buffer
 * it keeps; the samples are written straight into the output buffer, which is kept too, so a packet costs
 * one crossing and nothing is allocated on either side.
 *
 * Decoded on the playback thread, in [queueInputBuffer], rather than on a thread of its own as media3's
 * SimpleDecoder does: a packet takes microseconds, and the handover to that thread and back cost two
 * wakeups a packet - most of what playing woke the phone for. The bookkeeping (end of stream, first
 * sample, output start time, skipped buffers, the reset after a flush) is SimpleDecoder's, kept exact.
 */
@UnstableApi
class RustAudioDecoder(format: Format, private val codec: Int, private val float: Boolean) :
    Decoder<DecoderInputBuffer, SimpleDecoderOutputBuffer, DecoderException> {

    private var handle = RustDecoderJni.create(codec, format.sampleRate, format.channelCount, setupData(format), format.encoderDelay > 0)
    private val maxBytes = RustDecoderJni.maxBytes(codec, format.channelCount.coerceAtLeast(1), float)

    private val input = DecoderInputBuffer(DecoderInputBuffer.BUFFER_REPLACEMENT_MODE_DIRECT)
    private var inputOut = false
    /** Output buffers free to decode into; the renderer hands each back through its owner when done. */
    private val free = ArrayDeque<SimpleDecoderOutputBuffer>(BUFFERS)
    private val ready = ArrayDeque<SimpleDecoderOutputBuffer>(BUFFERS)
    private var flushed = false
    private var skipped = 0
    private var outputStartTimeUs = C.TIME_UNSET
    private var error: DecoderException? = null

    init {
        if (handle == 0L) throw DecoderException("the core cannot decode ${format.sampleMimeType}")
        input.ensureSpaceForWrite(if (format.maxInputSize != Format.NO_VALUE) format.maxInputSize else DEFAULT_INPUT_SIZE)
        repeat(BUFFERS) { free.addLast(SimpleDecoderOutputBuffer { out -> out.clear(); free.addLast(out) }) }
    }

    val channels: Int get() = (RustDecoderJni.shape(handle) ushr 32).toInt()
    val rate: Int get() = RustDecoderJni.shape(handle).toInt()

    override fun getName() = "nori-rust"

    override fun setOutputStartTimeUs(outputStartTimeUs: Long) { this.outputStartTimeUs = outputStartTimeUs }

    override fun dequeueInputBuffer(): DecoderInputBuffer? {
        error?.let { throw it }
        if (inputOut || free.isEmpty()) return null
        inputOut = true
        return input
    }

    override fun queueInputBuffer(inputBuffer: DecoderInputBuffer) {
        error?.let { throw it }
        check(inputBuffer === input && inputOut)
        inputOut = false
        val out = free.removeLast()
        if (input.isEndOfStream) {
            out.addFlag(C.BUFFER_FLAG_END_OF_STREAM)
        } else {
            out.timeUs = input.timeUs
            if (input.isFirstSample) out.addFlag(C.BUFFER_FLAG_FIRST_SAMPLE)
            if (outputStartTimeUs != C.TIME_UNSET && input.timeUs < outputStartTimeUs) out.shouldBeSkipped = true
            val e = try { decode(input, out, flushed) } catch (e: RuntimeException) { DecoderException("unexpected decode error", e) }
            flushed = false
            if (e != null) { error = e; input.clear(); out.release(); return }
        }
        input.clear()
        if (out.shouldBeSkipped) {
            skipped++
            out.release()
        } else {
            out.skippedOutputBufferCount = skipped
            skipped = 0
            ready.addLast(out)
        }
    }

    override fun dequeueOutputBuffer(): SimpleDecoderOutputBuffer? {
        error?.let { throw it }
        return ready.removeFirstOrNull()
    }

    override fun flush() {
        flushed = true
        skipped = 0
        inputOut = false
        input.clear()
        while (ready.isNotEmpty()) ready.removeFirst().release()
    }

    private fun decode(input: DecoderInputBuffer, output: SimpleDecoderOutputBuffer, reset: Boolean): DecoderException? {
        if (reset) RustDecoderJni.reset(handle, input.timeUs == 0L)
        val data = input.data ?: return null
        var out = output.init(input.timeUs, maxBytes)
        var n = RustDecoderJni.decode(handle, data, data.position(), data.remaining(), out, 0, out.capacity(), float)
        if (n < -2) {
            // A packet larger than any the codec is meant to make: grow once and take it.
            out = output.grow(-(n + 2))
            n = RustDecoderJni.take(handle, out, 0, out.capacity(), float)
        }
        when {
            n >= 0 -> { out.position(0); out.limit(n) }
            n == -1 -> output.shouldBeSkipped = true
            else -> return DecoderException("the core could not go on decoding")
        }
        return null
    }

    override fun release() {
        RustDecoderJni.destroy(handle)
        handle = 0
    }

    private companion object {
        const val BUFFERS = 16
        const val DEFAULT_INPUT_SIZE = 64 * 1024

        /** The codec's own setup, as media3's extractors give it: joined when it comes in pieces. */
        fun setupData(format: Format): ByteArray? {
            val parts = format.initializationData
            if (parts.isEmpty()) return null
            if (parts.size == 1) return parts[0]
            return ByteArray(parts.sumOf { it.size }).also { all -> var at = 0; for (p in parts) { p.copyInto(all, at); at += p.size } }
        }
    }
}

/**
 * Audio decoded in the core rather than by the platform's MediaCodec, for the streams it takes (see
 * `decoder.rs`): no buffer objects made per packet, no hop to the codec's own process, and the same
 * samples on every platform. It stands aside for what it cannot decode, and for a stream the output
 * hands to the audio chip whole; MediaCodec, after it in line, takes those.
 */
@UnstableApi
class RustAudioRenderer(handler: Handler?, listener: AudioRendererEventListener?, private val sink: AudioSink, private val float: () -> Boolean) :
    DecoderAudioRenderer<RustAudioDecoder>(handler, listener, sink) {

    override fun getName() = "RustAudioRenderer"

    override fun supportsFormatInternal(format: Format): Int {
        val mime = format.sampleMimeType ?: return C.FORMAT_UNSUPPORTED_TYPE
        val chip = runCatching { sink.getFormatOffloadSupport(format).isFormatSupported }.getOrDefault(false)
        if (RustDecoderJni.takes(mime, format.codecs, format.channelCount, format.sampleRate, chip) == 0) return C.FORMAT_UNSUPPORTED_SUBTYPE
        if (format.cryptoType != C.CRYPTO_TYPE_NONE) return C.FORMAT_UNSUPPORTED_DRM
        if (!sinkSupportsFormat(Util.getPcmFormat(encoding(), format.channelCount, format.sampleRate))) return C.FORMAT_UNSUPPORTED_SUBTYPE
        return C.FORMAT_HANDLED
    }

    override fun createDecoder(format: Format, cryptoConfig: CryptoConfig?): RustAudioDecoder {
        val codec = RustDecoderJni.takes(format.sampleMimeType.orEmpty(), format.codecs, format.channelCount, format.sampleRate, false)
        return RustAudioDecoder(format, codec, encoding() == C.ENCODING_PCM_FLOAT)
    }

    override fun getOutputFormat(decoder: RustAudioDecoder): Format = Util.getPcmFormat(encoding(), decoder.channels, decoder.rate)

    private fun encoding() = if (float()) C.ENCODING_PCM_FLOAT else C.ENCODING_PCM_16BIT
}
