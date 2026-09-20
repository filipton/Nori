package dev.nori.music.playback

import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.util.UnstableApi
import java.nio.ByteBuffer

/** The Rust side of the equalizer; see crates/core/src/dsp.rs. */
internal object Dsp {
    init { System.loadLibrary("norimusic") }

    @JvmStatic external fun create(sampleRate: Int, channels: Int): Long
    @JvmStatic external fun destroy(handle: Long)
    /** [bands] is flat: kind, frequency, gain dB, Q, channel per band. */
    @JvmStatic external fun configure(handle: Long, bands: FloatArray, preampDb: Float, crossfeedDb: Float)

    /** The stage after the equalizer. [lookaheadMs] 0 or less turns the limiter off. */
    @JvmStatic external fun configureOutput(handle: Long, balance: Float, mono: Boolean, thresholdDb: Float, releaseMs: Float, lookaheadMs: Float)

    /** Peak gain reduction over the last buffer, in dB. Reads an atomic, so the UI never waits on the audio thread. */
    @JvmStatic external fun gainReductionDb(handle: Long): Float
    @JvmStatic external fun reset(handle: Long)
    @JvmStatic external fun process(handle: Long, input: ByteBuffer, inPos: Int, output: ByteBuffer, outPos: Int, bytes: Int, encoding: Int): Boolean
}

/**
 * The sample-domain chain: pre-amp, parametric equalizer, crossfeed. While [enabled] is false the
 * processor reports itself inactive and media3 leaves it out of the chain entirely, which is also
 * what lets playback stay offloaded. Flipping [enabled] takes effect the next time the sink is
 * configured; the service re-prepares the player to force that. Changing the curve is live.
 */
@UnstableApi
class Equalizer : BaseAudioProcessor() {
    @Volatile var enabled = false
    @Volatile private var bands = FloatArray(0)
    @Volatile private var preampDb = 0f
    @Volatile private var crossfeedDb = 0f
    @Volatile private var output = floatArrayOf(0f, 0f, -1f, 100f, 0f)
    @Volatile private var dirty = true
    private var handle = 0L

    fun setChain(bands: List<dev.nori.music.settings.Band>, preampDb: Float, crossfeedDb: Float) {
        this.bands = FloatArray(bands.size * 5).also { a ->
            bands.forEachIndexed { i, b ->
                a[i * 5] = b.kind.ordinal.toFloat(); a[i * 5 + 1] = b.freq; a[i * 5 + 2] = b.gainDb; a[i * 5 + 3] = b.q; a[i * 5 + 4] = b.channel.ordinal.toFloat()
            }
        }
        this.preampDb = preampDb
        this.crossfeedDb = crossfeedDb
        dirty = true
    }

    /** Balance, mono and the limiter; [limiterLookaheadMs] 0 means no limiter. */
    fun setOutput(balance: Float, mono: Boolean, thresholdDb: Float, releaseMs: Float, limiterLookaheadMs: Float) {
        output = floatArrayOf(balance, if (mono) 1f else 0f, thresholdDb, releaseMs, limiterLookaheadMs)
        dirty = true
    }

    companion object {
        /**
         * The chain the service is currently playing through, so a screen can read its meters without
         * reaching into the service. Null whenever nothing is playing through a processor.
         */
        @Volatile var active: Equalizer? = null
    }

    /** What the limiter is doing right now, for a meter. 0 when it is off or idle. */
    val gainReductionDb: Float get() = handle.takeIf { it != 0L }?.let { Dsp.gainReductionDb(it) } ?: 0f

    override fun onConfigure(format: AudioProcessor.AudioFormat): AudioProcessor.AudioFormat {
        if (!enabled) return AudioProcessor.AudioFormat.NOT_SET
        if (format.encoding != C.ENCODING_PCM_16BIT && format.encoding != C.ENCODING_PCM_FLOAT) {
            throw AudioProcessor.UnhandledAudioFormatException(format)
        }
        return format
    }

    override fun onFlush() {
        if (handle != 0L) Dsp.destroy(handle)
        handle = Dsp.create(inputAudioFormat.sampleRate, inputAudioFormat.channelCount)
        android.util.Log.i("nori", "equalizer in chain: ${inputAudioFormat.sampleRate} Hz x${inputAudioFormat.channelCount}")
        dirty = true
    }

    override fun queueInput(input: ByteBuffer) {
        val n = input.remaining()
        if (n == 0) return
        if (dirty) {
            Dsp.configure(handle, bands, preampDb, crossfeedDb)
            Dsp.configureOutput(handle, output[0], output[1] > 0.5f, output[2], output[3], output[4])
            dirty = false
        }
        val out = replaceOutputBuffer(n)
        if (input.isDirect && Dsp.process(handle, input, input.position(), out, out.position(), n, inputAudioFormat.encoding)) {
            input.position(input.limit())
            out.position(out.position() + n)
        } else {
            out.put(input)
        }
        out.flip()
    }

    override fun onReset() {
        if (handle != 0L) Dsp.destroy(handle)
        handle = 0
    }
}
