package dev.nori.music.playback

import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.util.UnstableApi
import dalvik.annotation.optimization.CriticalNative
import dalvik.annotation.optimization.FastNative
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** The Rust side of the equalizer; see crates/core/src/dsp.rs. */
object Dsp {
    init { System.loadLibrary("norimusic") }

    @JvmStatic @CriticalNative external fun create(sampleRate: Int, channels: Int): Long
    @JvmStatic @CriticalNative external fun destroy(handle: Long)
    /**
     * Peak gain reduction over the last buffer processed, in dB. An atomic of its own, not the chain's: the
     * UI reads it while the playback thread may be freeing that chain, so no handle is passed.
     */
    @JvmStatic @CriticalNative external fun meter(): Float
    /** Frames the chain holds back (the limiter's look-ahead), drained at the end of a stream. */
    @JvmStatic @CriticalNative external fun delayFrames(handle: Long): Int
    @JvmStatic @CriticalNative external fun reset(handle: Long)
    @JvmStatic @FastNative external fun process(handle: Long, input: ByteBuffer, inPos: Int, output: ByteBuffer, outPos: Int, bytes: Int, encoding: Int): Boolean
    /** The pre-amp in effect (the core's `SoundSettings::effective_preamp_db`); [preampDb] is ignored when [automatic]. */
    @JvmStatic @FastNative external fun effectivePreampDb(eqEnabled: Boolean, preampDb: Float, automatic: Boolean, kinds: IntArray, gains: FloatArray): Float
    /** Whether anything in the sound chain is switched on; see nori_player::sound::sound_on. */
    @JvmStatic @CriticalNative external fun soundOn(eqEnabled: Boolean, crossfeedDb: Float, balance: Float, mono: Boolean, limiter: Boolean): Boolean
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

        /**
         * Whether a sound chain is in the samples' path now, whichever player plays: this processor in
         * ExoPlayer's sink, or the Rust engine's own chain (which [active] knows nothing of).
         */
        val inChain: Boolean get() = PlaybackService.rustPlayer?.chainIn ?: (active != null)

        /** The limiter's meter, whichever player plays: what it takes off right now, dB. */
        val meterDb: Float get() = PlaybackService.rustPlayer?.gainReductionDb ?: active?.gainReductionDb ?: 0f
    }

    /** What the limiter is doing right now, for a meter. 0 when it is off or idle. */
    val gainReductionDb: Float get() = if (handle != 0L) Dsp.meter() else 0f

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

    /** Where a heap buffer's samples are copied so the core can reach them; grown once, then kept. */
    private var scratch: ByteBuffer = AudioProcessor.EMPTY_BUFFER

    override fun queueInput(input: ByteBuffer) {
        val n = input.remaining()
        if (n == 0) return
        val out = replaceOutputBuffer(n)
        // The core reads samples only through a direct buffer's address. A heap buffer used to go
        // through untouched, which played that stretch with the equalizer silently off.
        var src = input
        if (!input.isDirect) {
            if (scratch.capacity() < n) scratch = ByteBuffer.allocateDirect(n).order(ByteOrder.nativeOrder())
            src = scratch.also { it.clear(); it.put(input.duplicate()); it.flip() }
        }
        if (Dsp.process(handle, src, src.position(), out, out.position(), n, inputAudioFormat.encoding)) {
            input.position(input.limit())
            out.position(out.position() + n)
        } else {
            out.put(input)
        }
        out.flip()
    }

    /**
     * The end of the queue: the limiter still holds its look-ahead (5 ms), and with no more music coming
     * nothing would push it out, so the last of the last song was never heard. Silence pushed through
     * brings it out; the stage then ends once that is taken.
     */
    override fun onQueueEndOfStream() {
        val frames = if (handle != 0L) Dsp.delayFrames(handle) else 0
        if (frames <= 0) return
        val n = frames * inputAudioFormat.bytesPerFrame
        if (scratch.capacity() < n) scratch = ByteBuffer.allocateDirect(n).order(ByteOrder.nativeOrder())
        val silence = scratch.also { it.clear(); while (it.position() < n) it.put(0); it.flip() }
        val out = replaceOutputBuffer(n)
        if (Dsp.process(handle, silence, 0, out, out.position(), n, inputAudioFormat.encoding)) out.position(out.position() + n)
        out.flip()
    }

    override fun onReset() {
        if (handle != 0L) Dsp.destroy(handle)
        handle = 0
        scratch = AudioProcessor.EMPTY_BUFFER
    }
}
