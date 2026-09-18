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

        /**
         * Bytes this sink has handed to the AudioTrack since the process started. The only honest
         * answer to "is audio actually flowing?": the media session's position is deliberately not
         * updated periodically here, and a controller in the background can report a stale one, so a
         * test that watches either of those can pass while the device is silent - which is exactly the
         * bug this counter was added to catch.
         */
        @Volatile var bytesWritten = 0L
    }

    /** False while playback is offloaded or something needs low latency (the equalizer being tuned). */
    @Volatile var enabled = true
    private var filling = true
    private var writtenUntilUs = C.TIME_UNSET

    override fun handleBuffer(buffer: ByteBuffer, presentationTimeUs: Long, encodedAccessUnitCount: Int): Boolean {
        // Offloaded playback goes straight through, but it still has to be counted: a test that watches
        // these bytes must not go blind the moment the audio chip takes over.
        if (!enabled) {
            val before = buffer.remaining()
            return super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
                .also { if (it) bytesWritten += before - buffer.remaining() }
        }
        if (!filling) {
            // The sink has no position to report while it is stopped, paused before it ever played, or
            // freshly restarted. Subtracting CURRENT_POSITION_NOT_SET (Long.MIN_VALUE) from what we wrote
            // produces an enormous "queued" figure, and this sink then refuses every buffer for ever -
            // which is silence that only a sink rebuild (any audio setting) recovers from.
            val position = getCurrentPositionUs(false)
            val known = writtenUntilUs != C.TIME_UNSET && position != AudioSink.CURRENT_POSITION_NOT_SET
            if (known && writtenUntilUs - position > LOW_US) return false
            filling = true
        }
        val before = buffer.remaining()
        val taken = super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
        if (taken) { writtenUntilUs = presentationTimeUs; bytesWritten += before - buffer.remaining() } else filling = false
        return taken
    }

    private fun restart() { filling = true; writtenUntilUs = C.TIME_UNSET }

    // Resuming is a fresh start for the burst bookkeeping: the track may have been stopped and its
    // position reset while the app sat in the background, so what was written before means nothing now.
    override fun play() { restart(); super.play() }
    override fun pause() { restart(); super.pause() }

    override fun configure(config: AudioSink.AudioSinkConfig) { restart(); super.configure(config) }
    override fun handleDiscontinuity() { restart(); super.handleDiscontinuity() }
    override fun flush() { restart(); super.flush() }
    override fun reset() { restart(); super.reset() }
}
