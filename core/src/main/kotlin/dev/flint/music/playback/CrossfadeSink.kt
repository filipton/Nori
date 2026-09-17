package dev.flint.music.playback

import androidx.media3.common.C
import androidx.media3.common.MimeTypes
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.ForwardingAudioSink
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.PI
import kotlin.math.cos
import kotlin.math.sin

/**
 * Equal-power mix of the end of one track ([tail], 16-bit PCM) with the start of the next.
 * Pure buffer arithmetic, so it is unit-tested off the device.
 */
class Crossfader(private val tail: ByteBuffer, private val frameBytes: Int) {
    private val frames = tail.remaining() / frameBytes
    private var frame = 0
    val done get() = frame >= frames
    val framesMixed get() = frame

    /** Mixes as much of [input] as the tail still covers into [out]; both advance. */
    fun mix(input: ByteBuffer, out: ByteBuffer) {
        val n = minOf(input.remaining() / frameBytes, frames - frame)
        val channels = frameBytes / 2
        repeat(n) {
            val t = (frame + 0.5) / frames * (PI / 2)
            val (down, up) = cos(t) to sin(t)
            repeat(channels) {
                val v = tail.short * down + input.short * up
                out.putShort(v.toInt().coerceIn(-32768, 32767).toShort())
            }
            frame++
        }
    }

    /** The next track ended before the fade did: the rest of the tail, still fading out. */
    fun rest(out: ByteBuffer) {
        val channels = frameBytes / 2
        while (frame < frames) {
            val down = cos((frame + 0.5) / frames * (PI / 2))
            repeat(channels) { out.putShort((tail.short * down).toInt().toShort()) }
            frame++
        }
    }
}

/**
 * Crossfade inside one player. Audio passes through a delay line as long as the fade; when the
 * renderer announces the next track, what is still in the line is that track's predecessor's
 * ending, and it is mixed with the newcomer's beginning. No second player, no second decoder, and
 * the deep-buffer burst playback underneath keeps working. 16-bit PCM only; everything else
 * (float output, offload) passes straight through.
 */
@UnstableApi
class CrossfadeSink(sink: AudioSink) : ForwardingAudioSink(sink) {
    /** 0 turns it off; the line then drains and the sink becomes a pass-through. */
    @Volatile var seconds = 0

    /** Set by the service when the coming track change is within one album played in order: that boundary stays gapless. */
    @Volatile var keepNextBoundary = false

    private class Chunk(val data: ByteBuffer, val ptsUs: Long, val startsTrack: Boolean)

    private var rate = 0
    private var frameBytes = 0
    private val pcm get() = frameBytes > 0
    private val line = ArrayDeque<Chunk>()
    private var lineBytes = 0L
    /** Bytes at the head of the line that must go out regardless of the delay (mixed audio). */
    private var dueBytes = 0L
    private var out: Chunk? = null
    private var fade: Crossfader? = null
    private var fadePtsUs = 0L
    private var nextStartsTrack = false
    private var pendingConfig: AudioSink.AudioSinkConfig? = null
    private var ending = false
    private var endSent = false
    private val pool = ArrayList<ByteBuffer>()

    private fun targetBytes() = seconds.toLong() * rate * frameBytes
    private fun slackBytes() = rate.toLong() * frameBytes

    private fun buffer(size: Int): ByteBuffer {
        val i = pool.indexOfFirst { it.capacity() >= size }
        val b = if (i >= 0) pool.removeAt(i) else ByteBuffer.allocateDirect(maxOf(size, 8192)).order(ByteOrder.nativeOrder())
        b.clear()
        return b
    }

    private fun append(data: ByteBuffer, ptsUs: Long, due: Boolean, startsTrack: Boolean = false) {
        data.flip()
        if (!data.hasRemaining()) { pool += data; return }
        line += Chunk(data, ptsUs, startsTrack)
        lineBytes += data.remaining()
        if (due) dueBytes += data.remaining()
    }

    /** Pushes what is due into the real sink. False when the sink would not take it all yet. */
    private fun drain(keepBytes: Long): Boolean {
        while (true) {
            out?.let { c ->
                if (c.startsTrack && c.data.position() == 0) super.handleDiscontinuity()
                if (!super.handleBuffer(c.data, c.ptsUs, 1)) return false
                if (pool.size < 64) pool += c.data
                out = null
            }
            val head = line.firstOrNull() ?: return true
            if (dueBytes <= 0 && lineBytes <= keepBytes) return true
            line.removeFirst()
            lineBytes -= head.data.remaining()
            dueBytes = (dueBytes - head.data.remaining()).coerceAtLeast(0)
            out = head
        }
    }

    override fun configure(config: AudioSink.AudioSinkConfig) {
        val f = config.format
        val is16 = f.sampleMimeType == MimeTypes.AUDIO_RAW && f.pcmEncoding == C.ENCODING_PCM_16BIT
        val same = pcm && is16 && f.sampleRate == rate && f.channelCount * 2 == frameBytes
        if (same || (line.isEmpty() && out == null)) {
            rate = if (is16) f.sampleRate else 0
            frameBytes = if (is16) f.channelCount * 2 else 0
            super.configure(config)
        } else {
            // A different format is coming and the line still holds the old one: play that out first.
            finishFade()
            pendingConfig = config
        }
    }

    override fun handleBuffer(buffer: ByteBuffer, presentationTimeUs: Long, encodedAccessUnitCount: Int): Boolean {
        pendingConfig?.let { config ->
            if (!drain(0)) return false
            pendingConfig = null
            configure(config)
        }
        if (!pcm || (seconds == 0 && line.isEmpty() && out == null && fade == null)) return super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
        if (!drain(targetBytes()) && lineBytes > targetBytes() + slackBytes()) return false

        fade?.let { f ->
            val mixed = buffer(buffer.remaining())
            val before = f.framesMixed
            f.mix(buffer.order(ByteOrder.nativeOrder()), mixed)
            append(mixed, fadePtsUs + before * 1_000_000L / rate, due = true)
            if (f.done) fade = null
        }
        if (buffer.hasRemaining()) {
            val copy = buffer(buffer.remaining())
            copy.put(buffer)
            append(copy, presentationTimeUs, due = false, startsTrack = nextStartsTrack)
            nextStartsTrack = false
        }
        return true
    }

    override fun handleDiscontinuity() {
        if (!pcm || seconds == 0 || line.isEmpty()) return super.handleDiscontinuity()
        if (keepNextBoundary) {
            // Same album, next track: whatever is in the line simply plays out, and the newcomer follows without a fade.
            keepNextBoundary = false
            dueBytes = lineBytes
            nextStartsTrack = true
            return
        }
        finishFade()
        // Everything not yet due is the old track's ending: lift it out of the line and mix the newcomer into it.
        val keep = ArrayDeque<Chunk>()
        var due = dueBytes
        while (due > 0 && line.isNotEmpty()) { val c = line.removeFirst(); due -= c.data.remaining(); keep += c }
        val tailBytes = line.sumOf { it.data.remaining() }
        if (tailBytes > 0) {
            val tail = ByteBuffer.allocateDirect(tailBytes).order(ByteOrder.nativeOrder())
            fadePtsUs = line.first().ptsUs
            line.forEach { tail.put(it.data); if (pool.size < 64) pool += it.data }
            tail.flip()
            fade = Crossfader(tail, frameBytes)
        }
        line.clear()
        line.addAll(keep)
        lineBytes = keep.sumOf { it.data.remaining() }.toLong()
        nextStartsTrack = true
    }

    /** A fade that cannot complete (end of queue, format change): let the rest of the ending play, fading. */
    private fun finishFade() {
        val f = fade ?: return
        val rest = buffer((targetBytes() + slackBytes()).toInt().coerceAtLeast(frameBytes))
        val from = f.framesMixed
        f.rest(rest)
        append(rest, fadePtsUs + from * 1_000_000L / rate, due = true)
        fade = null
    }

    override fun playToEndOfStream() {
        ending = true
        finishFade()
        pump()
    }

    private fun pump() {
        if (ending && !endSent && drain(0)) { super.playToEndOfStream(); endSent = true }
    }

    override fun isEnded(): Boolean { pump(); return (!ending || endSent) && super.isEnded() }
    override fun hasPendingData(): Boolean { pump(); return line.isNotEmpty() || out != null || super.hasPendingData() }

    private fun clear() {
        line.clear(); lineBytes = 0; dueBytes = 0; out = null; fade = null
        nextStartsTrack = false; pendingConfig = null; ending = false; endSent = false
    }

    override fun flush() { clear(); super.flush() }
    override fun reset() { clear(); pool.clear(); super.reset() }
}
