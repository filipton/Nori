package dev.flint.music.playback

import androidx.media3.common.C
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.util.UnstableApi
import java.nio.ByteBuffer

/** The Rust side of the equalizer; see crates/core/src/dsp.rs. */
internal object Dsp {
    init { System.loadLibrary("flintmusic") }

    @JvmStatic external fun create(sampleRate: Int, channels: Int): Long
    @JvmStatic external fun destroy(handle: Long)
    @JvmStatic external fun setGains(handle: Long, gains: FloatArray)
    @JvmStatic external fun reset(handle: Long)
    @JvmStatic external fun process(handle: Long, input: ByteBuffer, inPos: Int, output: ByteBuffer, outPos: Int, bytes: Int, encoding: Int): Boolean
}

/**
 * Ten-band equalizer. While [enabled] is false the processor reports itself
 * inactive and media3 leaves it out of the chain entirely, which is also what
 * lets playback stay offloaded. Flipping [enabled] takes effect the next time
 * the sink is configured; the service re-prepares the player to force that.
 */
@UnstableApi
class Equalizer : BaseAudioProcessor() {
    companion object {
        val FREQUENCIES = intArrayOf(31, 62, 125, 250, 500, 1000, 2000, 4000, 8000, 16000)
    }

    @Volatile var enabled = false
    @Volatile private var gains = FloatArray(FREQUENCIES.size)
    @Volatile private var dirty = true
    private var handle = 0L

    fun setGains(db: List<Float>) {
        gains = db.toFloatArray()
        dirty = true
    }

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
        android.util.Log.i("flint", "equalizer in chain: ${inputAudioFormat.sampleRate} Hz x${inputAudioFormat.channelCount}")
        dirty = true
    }

    override fun queueInput(input: ByteBuffer) {
        val n = input.remaining()
        if (n == 0) return
        if (dirty) { Dsp.setGains(handle, gains); dirty = false }
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
