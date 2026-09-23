package dev.nori.music.playback

import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.util.UnstableApi
import java.nio.ByteBuffer

/** The Rust side of the equalizer; see crates/core/src/dsp.rs. */
object Dsp {
    init { System.loadLibrary("norimusic") }

    @JvmStatic external fun create(sampleRate: Int, channels: Int): Long
    @JvmStatic external fun destroy(handle: Long)
    /** Peak gain reduction over the last buffer, in dB. Reads an atomic, so the UI never waits on the audio thread. */
    @JvmStatic external fun gainReductionDb(handle: Long): Float
    @JvmStatic external fun reset(handle: Long)
    @JvmStatic external fun process(handle: Long, input: ByteBuffer, inPos: Int, output: ByteBuffer, outPos: Int, bytes: Int, encoding: Int): Boolean
    /** The automatic pre-amp for these bands (kinds as BandKind ordinals); see nori_player::dsp::auto_preamp_db. */
    @JvmStatic external fun autoPreampDb(kinds: IntArray, gains: FloatArray): Float
    /** Whether anything in the sound chain is switched on; see nori_player::sound::sound_on. */
    @JvmStatic external fun soundOn(eqEnabled: Boolean, crossfeedDb: Float, balance: Float, mono: Boolean, limiter: Boolean): Boolean
    /** Where a volume fade from [from] to [to] stands at [t] (0..1); see nori_player::policy::fade. */
    @JvmStatic external fun fadeVolume(from: Float, to: Float, t: Float): Float
}

/**
 * The sample-domain chain: pre-amp, parametric equalizer, crossfeed, balance, mono, limiter. The chain
 * follows the settings in the core by itself (crates/core/src/dsp.rs): a change reaches the next buffer.
 * While [enabled] is false the processor reports itself inactive and media3 leaves it out of the
 * chain entirely, which is what lets playback stay offloaded. The service keeps [enabled] true on
 * every PCM path (even with a flat curve) so toggling EQ or moving a band is live — no sink
 * rebuild, no gap. Flipping [enabled] itself still waits for the next sink configuration.
 */
@UnstableApi
class Equalizer : BaseAudioProcessor() {
    @Volatile var enabled = false
    private var handle = 0L

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
    }

    override fun queueInput(input: ByteBuffer) {
        val n = input.remaining()
        if (n == 0) return
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
