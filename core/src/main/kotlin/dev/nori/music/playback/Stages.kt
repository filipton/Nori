package dev.nori.music.playback

import androidx.media3.common.C
import androidx.media3.common.Format
import androidx.media3.common.PlaybackParameters
import androidx.media3.common.audio.AudioProcessor
import androidx.media3.common.audio.AudioProcessorChain
import androidx.media3.common.audio.BaseAudioProcessor
import androidx.media3.common.util.UnstableApi
import dalvik.annotation.optimization.CriticalNative
import dalvik.annotation.optimization.FastNative
import java.nio.ByteBuffer
import java.nio.ByteOrder

/** The Rust side of speed/pitch and silence skipping; see crates/android/src/stages.rs. */
internal object Stages {
    init { System.loadLibrary("norimusic") }

    @JvmStatic @CriticalNative external fun speed(sampleRate: Int, channels: Int, encoding: Int, speed: Float, pitch: Float): Long
    @JvmStatic @CriticalNative external fun silence(sampleRate: Int, channels: Int): Long
    @JvmStatic @CriticalNative external fun destroy(handle: Long)
    @JvmStatic @CriticalNative external fun flush(handle: Long)
    /** The output's length, with [MOVED] set when [output] must be asked again; -1 when [input] is not direct. */
    @JvmStatic @FastNative external fun process(handle: Long, input: ByteBuffer, pos: Int, bytes: Int, keep: Boolean): Int
    @JvmStatic @CriticalNative external fun end(handle: Long, keep: Boolean): Int
    /** A direct buffer over the stage's own output memory. */
    @JvmStatic external fun output(handle: Long): ByteBuffer
    /** With [handle] 0 (no stage made yet) these work from the nominal [speed]. */
    @JvmStatic @CriticalNative external fun mediaDurationUs(handle: Long, speed: Float, playoutUs: Long): Long
    @JvmStatic @CriticalNative external fun playoutDurationUs(handle: Long, speed: Float, mediaUs: Long): Long
    /** Whether speed and pitch change the sound at all (nori_player::speed::speed_active). */
    @JvmStatic @CriticalNative external fun speedActive(speed: Float, pitch: Float): Boolean
    /**
     * One tick of a volume fade (nori_player::transport::fade_step): the volume's bits in the low 32,
     * [FADE_DONE] set once it is over. See [fadeVolume] and [fadeDone].
     */
    @JvmStatic @CriticalNative external fun fadeStep(from: Float, to: Float, startMs: Long, nowMs: Long, ms: Int): Long

    const val FADE_DONE = 1L shl 32
    fun fadeVolume(step: Long): Float = java.lang.Float.intBitsToFloat(step.toInt())
    fun fadeDone(step: Long): Boolean = step and FADE_DONE != 0L
    @JvmStatic @CriticalNative external fun skippedFrames(handle: Long): Long

    const val MOVED = 1 shl 30
}

/**
 * A stage's output as media3 takes it: the stage's own memory, seen through one direct buffer that is
 * made again only if that memory moves. One call per buffer in, nothing copied or allocated.
 */
internal class StageOutput {
    private var view: ByteBuffer = AudioProcessor.EMPTY_BUFFER
    private var scratch: ByteBuffer = AudioProcessor.EMPTY_BUFFER
    /** Bytes produced and not yet handed on. */
    var waiting = 0
        private set

    fun feed(handle: Long, input: ByteBuffer) {
        val n = input.remaining()
        var src = input
        if (!input.isDirect) {
            if (scratch.capacity() < n) scratch = ByteBuffer.allocateDirect(n).order(ByteOrder.nativeOrder())
            src = scratch.also { it.clear(); it.put(input.duplicate()); it.flip() }
        }
        took(handle, Stages.process(handle, src, src.position(), n, waiting > 0))
        input.position(input.limit())
    }

    fun end(handle: Long) = took(handle, Stages.end(handle, waiting > 0))

    private fun took(handle: Long, r: Int) {
        if (r < 0) return
        if (r and Stages.MOVED != 0) view = Stages.output(handle).order(ByteOrder.nativeOrder())
        waiting = r and (Stages.MOVED - 1)
    }

    /** Everything produced, once; empty until there is more. */
    fun take(): ByteBuffer {
        if (waiting == 0) return AudioProcessor.EMPTY_BUFFER
        view.clear()
        view.limit(waiting)
        waiting = 0
        return view
    }

    /** A new stage: its memory is somewhere else. */
    fun reset() {
        view = AudioProcessor.EMPTY_BUFFER
        waiting = 0
    }
}

/**
 * Silence skipping, done by nori-player (an exact port of media3's, so it sounds the same): pauses longer
 * than 100 ms are shortened, faded rather than cut. 16-bit only, like media3's.
 */
@UnstableApi
class SilenceSkipping : BaseAudioProcessor() {
    @Volatile var enabled = false
    private var handle = 0L
    private val out = StageOutput()
    private var inputEnded = false

    val skippedFrames: Long get() = if (handle != 0L) Stages.skippedFrames(handle) else 0

    override fun onConfigure(format: AudioProcessor.AudioFormat): AudioProcessor.AudioFormat {
        if (format.encoding != C.ENCODING_PCM_16BIT) throw AudioProcessor.UnhandledAudioFormatException(format)
        if (format.sampleRate == Format.NO_VALUE) return AudioProcessor.AudioFormat.NOT_SET
        return format
    }

    override fun isActive() = super.isActive() && enabled

    override fun queueInput(input: ByteBuffer) {
        if (input.hasRemaining() && handle != 0L) out.feed(handle, input)
    }

    override fun onQueueEndOfStream() {
        inputEnded = true
        if (handle != 0L) out.end(handle)
    }

    // The output is the stage's own memory, not a buffer of this class's: see StageOutput.
    override fun getOutput(): ByteBuffer = out.take()

    override fun isEnded() = inputEnded && out.waiting == 0

    override fun onFlush() {
        // Fresh for each stream: the format may have changed, and media3 counts skipped frames per stream.
        if (handle != 0L) Stages.destroy(handle)
        out.reset()
        inputEnded = false
        handle = if (isActive) Stages.silence(inputAudioFormat.sampleRate, inputAudioFormat.channelCount) else 0
        if (handle != 0L) android.util.Log.i("nori", "silence skipping in chain: ${inputAudioFormat.sampleRate} Hz x${inputAudioFormat.channelCount}")
    }

    override fun onReset() {
        enabled = false
        if (handle != 0L) Stages.destroy(handle)
        handle = 0
        out.reset()
        inputEnded = false
    }
}

/**
 * Playback speed and pitch, done by nori-player (media3's Sonic, ported exactly). Follows media3's
 * SonicAudioProcessor: out of the chain at 1x and pitch 1, new settings take effect at the next flush.
 */
@UnstableApi
class SpeedPitch : AudioProcessor {
    private var speed = 1f
    private var pitch = 1f
    private var pendingFormat = AudioProcessor.AudioFormat.NOT_SET
    private var format = AudioProcessor.AudioFormat.NOT_SET
    private var handle = 0L
    private val out = StageOutput()
    private var inputEnded = false

    fun set(speed: Float, pitch: Float) {
        require(speed > 0f && pitch > 0f)
        this.speed = speed
        this.pitch = pitch
    }

    fun mediaDurationUs(playoutUs: Long): Long =
        Stages.mediaDurationUs(handle, speed, playoutUs)

    override fun getDurationAfterProcessorApplied(durationUs: Long): Long =
        Stages.playoutDurationUs(handle, speed, durationUs)

    override fun configure(input: AudioProcessor.AudioFormat): AudioProcessor.AudioFormat {
        if (input.encoding != C.ENCODING_PCM_16BIT && input.encoding != C.ENCODING_PCM_FLOAT) {
            throw AudioProcessor.UnhandledAudioFormatException(input)
        }
        pendingFormat = input
        return input
    }

    override fun isActive() =
        pendingFormat.sampleRate != Format.NO_VALUE && Stages.speedActive(speed, pitch)

    override fun queueInput(input: ByteBuffer) {
        if (input.hasRemaining() && handle != 0L) out.feed(handle, input)
    }

    override fun queueEndOfStream() {
        if (handle != 0L) out.end(handle)
        inputEnded = true
    }

    override fun getOutput(): ByteBuffer = out.take()

    override fun isEnded() = inputEnded && out.waiting == 0

    @Deprecated("media3 calls flush(StreamMetadata)")
    override fun flush() = flush(AudioProcessor.StreamMetadata.DEFAULT)

    override fun flush(streamMetadata: AudioProcessor.StreamMetadata) {
        // A fresh engine at the current settings, as media3 recreates its Sonic on every flush.
        if (handle != 0L) Stages.destroy(handle)
        handle = 0
        out.reset()
        if (isActive) {
            format = pendingFormat
            handle = Stages.speed(format.sampleRate, format.channelCount, format.encoding, speed, pitch)
            android.util.Log.i("nori", "speed in chain: x$speed pitch $pitch")
        }
        inputEnded = false
    }

    override fun reset() {
        speed = 1f
        pitch = 1f
        pendingFormat = AudioProcessor.AudioFormat.NOT_SET
        format = AudioProcessor.AudioFormat.NOT_SET
        if (handle != 0L) Stages.destroy(handle)
        handle = 0
        out.reset()
        inputEnded = false
    }
}

/**
 * The processors the sink runs on 16-bit PCM, in media3's order: the equalizer, then silence skipping,
 * then speed/pitch. Everything that shapes the samples is nori-player's, so another app built on it
 * sounds the same.
 */
@UnstableApi
class SoundChain(equalizer: Equalizer) : AudioProcessorChain {
    private val silence = SilenceSkipping()
    private val speed = SpeedPitch()
    private val processors = arrayOf(equalizer, silence, speed)

    override fun getAudioProcessors(): Array<AudioProcessor> = processors

    override fun applyPlaybackParameters(parameters: PlaybackParameters): PlaybackParameters {
        speed.set(parameters.speed, parameters.pitch)
        return parameters
    }

    override fun applySkipSilenceEnabled(enabled: Boolean): Boolean {
        silence.enabled = enabled
        return enabled
    }

    override fun getMediaDuration(playoutDuration: Long): Long =
        if (speed.isActive) speed.mediaDurationUs(playoutDuration) else playoutDuration

    override fun getSkippedOutputFrameCount(): Long = silence.skippedFrames
}
