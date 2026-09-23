package dev.nori.music.playback

import android.os.Handler
import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.util.UnstableApi
import androidx.media3.common.util.Util
import androidx.media3.decoder.CryptoConfig
import androidx.media3.decoder.DecoderException
import androidx.media3.decoder.DecoderInputBuffer
import androidx.media3.decoder.SimpleDecoder
import androidx.media3.decoder.SimpleDecoderOutputBuffer
import androidx.media3.exoplayer.audio.AudioRendererEventListener
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.DecoderAudioRenderer
import java.nio.ByteBuffer

/** The decoder in the core (crates/core/src/decoder.rs over nori_player::decode). */
internal object RustDecoderJni {
    init { System.loadLibrary("norimusic") }

    /** The codec number for [mime], or 0 when the platform's own decoder should take the stream. */
    @JvmStatic external fun takes(mime: String, codecs: String?, channels: Int, chipTakesIt: Boolean): Int
    /** [delayKnown]: the stream states its encoder delay (an MP3's LAME header), which media3 trims. */
    @JvmStatic external fun create(codec: Int, rate: Int, channels: Int, extra: ByteArray?, delayKnown: Boolean): Long
    @JvmStatic external fun destroy(h: Long)
    /** The next packet does not follow the last; [atStart]: back at the very beginning. */
    @JvmStatic external fun reset(h: Long, atStart: Boolean)
    /** `channels << 32 | rate` of what comes out. */
    @JvmStatic external fun shape(h: Long): Long
    @JvmStatic external fun maxBytes(codec: Int, channels: Int, float: Boolean): Int
    /** Bytes written; -1 skip the packet; -2 broken; below that, `-(bytes needed) - 2`, then [take]. */
    @JvmStatic external fun decode(h: Long, input: ByteBuffer, inPos: Int, inLen: Int, output: ByteBuffer, outPos: Int, outCap: Int, float: Boolean): Int
    @JvmStatic external fun take(h: Long, output: ByteBuffer, outPos: Int, outCap: Int, float: Boolean): Int
}

/**
 * One stream decoded in the core. media3 hands in each packet its extractor split off, in a direct buffer
 * it keeps; the samples are written straight into the output buffer, which is kept too, so a packet costs
 * one crossing and nothing is allocated on either side.
 */
@UnstableApi
class RustAudioDecoder(format: Format, private val codec: Int, private val float: Boolean) :
    SimpleDecoder<DecoderInputBuffer, SimpleDecoderOutputBuffer, DecoderException>(arrayOfNulls(BUFFERS), arrayOfNulls(BUFFERS)) {

    private var handle = RustDecoderJni.create(codec, format.sampleRate, format.channelCount, setupData(format), format.encoderDelay > 0)
    private val maxBytes = RustDecoderJni.maxBytes(codec, format.channelCount.coerceAtLeast(1), float)

    init {
        if (handle == 0L) throw DecoderException("the core cannot decode ${format.sampleMimeType}")
        setInitialInputBufferSize(if (format.maxInputSize != Format.NO_VALUE) format.maxInputSize else DEFAULT_INPUT_SIZE)
    }

    val channels: Int get() = (RustDecoderJni.shape(handle) ushr 32).toInt()
    val rate: Int get() = RustDecoderJni.shape(handle).toInt()

    override fun getName() = "nori-rust"

    override fun createInputBuffer() = DecoderInputBuffer(DecoderInputBuffer.BUFFER_REPLACEMENT_MODE_DIRECT)
    override fun createOutputBuffer() = SimpleDecoderOutputBuffer { releaseOutputBuffer(it) }
    override fun createUnexpectedDecodeException(error: Throwable) = DecoderException("unexpected decode error", error)

    override fun decode(input: DecoderInputBuffer, output: SimpleDecoderOutputBuffer, reset: Boolean): DecoderException? {
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
        super.release()
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
        if (RustDecoderJni.takes(mime, format.codecs, format.channelCount, chip) == 0) return C.FORMAT_UNSUPPORTED_SUBTYPE
        if (format.cryptoType != C.CRYPTO_TYPE_NONE) return C.FORMAT_UNSUPPORTED_DRM
        if (!sinkSupportsFormat(Util.getPcmFormat(encoding(), format.channelCount, format.sampleRate))) return C.FORMAT_UNSUPPORTED_SUBTYPE
        return C.FORMAT_HANDLED
    }

    override fun createDecoder(format: Format, cryptoConfig: CryptoConfig?): RustAudioDecoder {
        val codec = RustDecoderJni.takes(format.sampleMimeType.orEmpty(), format.codecs, format.channelCount, false)
        return RustAudioDecoder(format, codec, encoding() == C.ENCODING_PCM_FLOAT)
    }

    override fun getOutputFormat(decoder: RustAudioDecoder): Format = Util.getPcmFormat(encoding(), decoder.channels, decoder.rate)

    private fun encoding() = if (float()) C.ENCODING_PCM_FLOAT else C.ENCODING_PCM_16BIT
}
