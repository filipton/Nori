package dev.nori.music.playback

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
        /** Slack for the clock moving a little further than the count between two readings. */
        private const val JUMP_US = 250_000L

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

    /**
     * How much audio is in the track, counted from what was handed to it and how far its clock has
     * moved - never from timestamps. It used to be the last buffer's timestamp less the clock, which
     * is wrong across a jump: a mix is stamped in the next song's time (TransitionSink), so the moment
     * its first chunk went in the timestamps leapt ahead by the length of the mix while the clock only
     * follows once that chunk is heard. The buffer looked twelve seconds deeper than it was, this sink
     * stopped feeding it for twice as long as it should, and the track ran dry near the end of the mix:
     * the cut heard on an AutoMix change ("audio underrun ... 10 s since last feed").
     *
     * A jump of the clock is not playing, so a move larger than what could have been queued is left
     * out. Anything that makes the count unsure starts it again at nothing, which can only make this
     * sink feed a little early, never late.
     */
    private var writtenUs = 0L
    private var playedUs = 0L
    private var lastPositionUs = AudioSink.CURRENT_POSITION_NOT_SET
    private var lastReadAtMs = 0L
    private var frameBytes = 0
    private var sampleRate = 0

    private fun queuedUs(): Long? {
        val position = getCurrentPositionUs(false)
        if (position == AudioSink.CURRENT_POSITION_NOT_SET) return null
        val last = lastPositionUs
        val now = android.os.SystemClock.elapsedRealtime()
        val wall = (now - lastReadAtMs) * 1000
        lastPositionUs = position
        lastReadAtMs = now
        if (last != AudioSink.CURRENT_POSITION_NOT_SET) {
            val moved = position - last
            val queued = writtenUs - playedUs
            playedUs += when {
                moved <= 0 -> 0L
                moved <= queued + JUMP_US -> moved
                // The clock jumped. What played meanwhile is at most the time that passed.
                else -> minOf(wall, queued)
            }
        }
        return (writtenUs - playedUs).coerceAtLeast(0)
    }

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
            // freshly restarted; then nothing is known and it takes audio rather than wait for ever.
            val queued = queuedUs()
            if (queued != null && queued > LOW_US) return false
            filling = true
        }
        val before = buffer.remaining()
        val taken = super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
        val bytes = before - buffer.remaining()
        bytesWritten += bytes
        if (frameBytes > 0 && sampleRate > 0) writtenUs += bytes.toLong() / frameBytes * 1_000_000L / sampleRate
        queuedUs()
        if (!taken) filling = false
        return taken
    }

    private fun restart() { filling = true; writtenUs = 0L; playedUs = 0L; lastPositionUs = AudioSink.CURRENT_POSITION_NOT_SET }

    // Resuming is a fresh start for the burst bookkeeping: the track may have been stopped and its
    // position reset while the app sat in the background, so what was written before means nothing now.
    override fun play() { restart(); super.play() }
    override fun pause() { restart(); super.pause() }

    override fun configure(config: AudioSink.AudioSinkConfig) {
        restart()
        val f = config.format
        val pcm = f.sampleMimeType == androidx.media3.common.MimeTypes.AUDIO_RAW && f.pcmEncoding != androidx.media3.common.Format.NO_VALUE
        frameBytes = if (pcm) androidx.media3.common.util.Util.getPcmFrameSize(f.pcmEncoding, f.channelCount) else 0
        sampleRate = if (pcm) f.sampleRate else 0
        super.configure(config)
    }
    // A timestamp discontinuity leaves the audio already written in the track, so the count stands;
    // the clock's jump when it is reached is left out by [queuedUs].
    override fun handleDiscontinuity() { filling = true; super.handleDiscontinuity() }
    override fun flush() { restart(); super.flush() }
    override fun reset() { restart(); super.reset() }
}
