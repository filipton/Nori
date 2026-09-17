package dev.flint.music.playback

import androidx.media3.common.C
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.ForwardingAudioSink
import java.nio.ByteBuffer

/**
 * Makes CPU-decoded playback run in bursts. Left alone, the player tops the
 * AudioTrack up one codec frame at a time, which wakes four threads dozens of
 * times a second for as long as music plays. This sink lets the (deliberately
 * deep) AudioTrack buffer drain to [LOW_US] before accepting audio again, then
 * takes everything until it is full: a fraction of a second of work every few
 * seconds, and real sleep in between.
 *
 * Offloaded playback already sleeps on its own and is passed straight through.
 */
@UnstableApi
class BurstSink(sink: AudioSink) : ForwardingAudioSink(sink) {
    companion object {
        /** Depth of the AudioTrack buffer this is meant to be used with. */
        const val BUFFER_US = 10_000_000
        private const val LOW_US = 2_000_000L
    }

    /** False while playback is offloaded or something needs low latency (the equalizer being tuned). */
    @Volatile var enabled = true
    private var filling = true
    private var writtenUntilUs = C.TIME_UNSET

    override fun handleBuffer(buffer: ByteBuffer, presentationTimeUs: Long, encodedAccessUnitCount: Int): Boolean {
        if (!enabled) return super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
        if (!filling) {
            val queuedUs = if (writtenUntilUs == C.TIME_UNSET) 0 else writtenUntilUs - getCurrentPositionUs(false)
            if (queuedUs > LOW_US) return false
            filling = true
        }
        val taken = super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
        if (taken) writtenUntilUs = presentationTimeUs else filling = false
        return taken
    }

    private fun restart() { filling = true; writtenUntilUs = C.TIME_UNSET }

    override fun configure(config: AudioSink.AudioSinkConfig) { restart(); super.configure(config) }
    override fun handleDiscontinuity() { restart(); super.handleDiscontinuity() }
    override fun flush() { restart(); super.flush() }
    override fun reset() { restart(); super.reset() }
}
