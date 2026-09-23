package dev.nori.music.playback

import android.util.Log
import androidx.annotation.Keep
import androidx.media3.common.C
import androidx.media3.common.MimeTypes
import androidx.media3.common.Timeline
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.ForwardingAudioSink
import java.nio.ByteBuffer

/**
 * Transitions between tracks inside the one player: plain crossfades and AutoMix (beat-matched, with a
 * bass swap, a filter sweep on the way out and a tempo stretch on the way in). The Rust core plans each
 * transition and does the per-sample work; this sink decides where the audio goes.
 *
 * Until the outgoing track reaches the planned start, audio passes straight through. From there it is held
 * (at most the length of the transition; anything past that the plan chose to skip is dropped). When the
 * next track begins, its opening is mixed into what was held and the result goes on. Nothing here runs
 * between transitions except one position check per buffer.
 *
 * When the two sides disagree on rate or channels, the incoming side is converted to the outgoing one
 * (cubic resample, mono/stereo either way) and the mix runs at the outgoing rate; the downstream format
 * switches once the mix is out. Anything else unconvertible falls back to the ending unmixed.
 *
 * Beyond a transition the same rule holds for the whole queue: the downstream format is latched on
 * the first PCM stream and every later stream is converted to it, so the AudioTrack below is never
 * recreated when the next song has another rate. Recreating it is a stop and a start - about half a
 * second of silence right where the new song plays alone - which is what a 48 -> 44.1 kHz boundary
 * sounded like. The latch clears on reset (a stopped player latches again on whatever comes next)
 * and never engages for non-PCM streams or while the service holds the output bit-perfect, where the
 * native format must reach the wire untouched; see [lockRate].
 *
 * It also hands every decoded buffer of a not-yet-analysed track to the streaming analyser, so the tempo,
 * beat grid and cue points come from audio the phone is decoding anyway.
 *
 * media3 calls [configure] once per decoded stream with the stream's media item, which is how the sink
 * knows which song it is decoding (decoding runs well ahead of what is heard).
 *
 * All of the above is decided in Rust, in `nori_player::engine` - shared with every platform, and
 * tested there against a simulated output. This class is the media3 side of it: it forwards each
 * AudioSink call to the engine, and the engine calls back into it for the real output below (`down*`)
 * and to nudge the UI when the heard song changes (`hostHeardChanged`). Plans, analyses and the log
 * are the core's own (crates/core/src/automix/planner.rs). Those callbacks are reached by name from
 * native code.
 */
@UnstableApi
class TransitionSink(sink: AudioSink) : ForwardingAudioSink(sink) {

    companion object {
        /**
         * A mix is being heard right now (the test bridge and logs ask). What the ear has lives in
         * nori-player and is read from there (HeardJni); nothing is copied out on every position query.
         */
        val mixing: Boolean get() = TransitionEngineJni.mixing()

        /** The ear is behind the player on a held ending. */
        val holding: Boolean get() = TransitionEngineJni.holding()

        /** Called on the playback thread when the ear leaves the player, or catches up with it. */
        @Volatile var onHeardChanged: (() -> Unit)? = null

        /**
         * Bytes handed to the output since the sink was made. The only honest answer to "is audio actually
         * flowing?": the media session's position is not updated periodically, and a controller in the
         * background can report a stale one, so a test watching either could pass in silence.
         */
        val bytesWritten: Long get() = TransitionEngineJni.bytesWritten()
    }


    private val engine = TransitionEngineJni.create()
    /** The engine's status words, written by Rust after every call into it: asking costs no call. */
    private val status = TransitionEngineJni.status(engine).order(java.nio.ByteOrder.nativeOrder())

    /**
     * Feeding the output in bursts (nori_player::burst): off while the output decodes by itself
     * (offload) or something needs low latency (the equalizer being tuned).
     */
    var bursting = true
        set(v) { field = v; TransitionEngineJni.setBurst(engine, v) }
    /** The formats handed to the engine, by token, until it sends one downstream or drops it. */
    private val configs = HashMap<Int, AudioSink.AudioSinkConfig>()
    private var nextToken = 0

    /**
     * The downstream format stays on whatever the first PCM stream brought. False only while the
     * service holds the output bit-perfect (or hi-res): then every stream passes through native and
     * the track below follows it. The service keeps this in step with `transitionsOff`.
     */
    @Volatile var lockRate = true
        set(v) { field = v; TransitionEngineJni.setLockRate(engine, v) }

    /**
     * Something the plan was made without - an analysis measured since - has arrived, so the plan for
     * the track playing is asked for again at the next buffer.
     */
    fun replan() = TransitionEngineJni.replan(engine)

    private fun now() = android.os.SystemClock.elapsedRealtime()

    // ---- media3 into the engine ----

    override fun configure(config: AudioSink.AudioSinkConfig) {
        val f = config.format
        val enc = if (f.sampleMimeType == MimeTypes.AUDIO_RAW && (f.pcmEncoding == C.ENCODING_PCM_16BIT || f.pcmEncoding == C.ENCODING_PCM_FLOAT)) f.pcmEncoding else 0
        val token = nextToken++
        configs[token] = config
        // Only a few formats are ever live (the one below, a pending one, those staged ahead).
        if (configs.size > 16) configs.keys.filter { it < token - 16 }.forEach(configs::remove)
        TransitionEngineJni.configure(engine, this, now(), idOf(config), f.sampleRate, f.channelCount, enc, token)
    }

    private fun idOf(config: AudioSink.AudioSinkConfig): String? =
        config.mediaPeriodId?.let { pid ->
            runCatching {
                val t = config.timeline
                t.getWindow(t.getPeriodByUid(pid.periodUid, Timeline.Period()).windowIndex, Timeline.Window()).mediaItem.mediaId
            }.getOrNull()
        }

    override fun setOutputStreamOffsetUs(outputStreamOffsetUs: Long) {
        TransitionEngineJni.setOffset(engine, outputStreamOffsetUs)
        super.setOutputStreamOffsetUs(outputStreamOffsetUs)
    }

    override fun handleBuffer(buffer: ByteBuffer, presentationTimeUs: Long, encodedAccessUnitCount: Int): Boolean {
        if (!buffer.isDirect) return super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
        val positionBefore = buffer.position()
        // The output's clock goes in with the offer: whether it wants audio now is decided from it (bursts),
        // and asking for it here is cheaper than the engine calling back for it.
        val r = TransitionEngineJni.handleBuffer(engine, this, now(), buffer, positionBefore, buffer.remaining(), presentationTimeUs, super.getCurrentPositionUs(false))
        val used = (r and 0xffffffffL).toInt()
        // Passed straight through, the buffer went down itself and its position already moved; queued,
        // the engine copied it and says how much it took.
        if (buffer.position() == positionBefore) buffer.position(buffer.position() + used)
        return (r ushr 32) != 0L
    }

    override fun handleDiscontinuity() = TransitionEngineJni.handleDiscontinuity(engine, this, now())

    override fun getCurrentPositionUs(sourceEnded: Boolean): Long =
        TransitionEngineJni.position(engine, this, now(), sourceEnded, super.getCurrentPositionUs(sourceEnded))

    override fun playToEndOfStream() {
        if (TransitionEngineJni.playToEnd(engine, this, now())) super.playToEndOfStream()
    }

    override fun hasPendingData(): Boolean = status.getLong(0) != 0L || super.hasPendingData()

    // Resuming is a fresh start for the burst bookkeeping: the output may have been stopped and its
    // clock reset while the app sat in the background.
    override fun play() { TransitionEngineJni.restartBurst(engine); super.play() }
    override fun pause() { TransitionEngineJni.restartBurst(engine); super.pause() }
    override fun isEnded(): Boolean = TransitionEngineJni.queueEmpty(engine, this, now()) && super.isEnded()

    override fun flush() {
        TransitionEngineJni.flush(engine, this, now())
        super.flush()
    }

    override fun reset() {
        TransitionEngineJni.reset(engine, this, now())
        configs.clear()
        super.reset()
    }

    // ---- the engine into the output below ----

    @Keep private fun downConfigure(token: Int) {
        configs[token]?.let { super.configure(it) }
    }

    /** `(taken << 32) | bytes taken`. */
    @Keep private fun downHandleBuffer(buffer: ByteBuffer, ptsUs: Long): Long {
        val before = buffer.position()
        val taken = super.handleBuffer(buffer, ptsUs, 1)
        return (if (taken) 1L shl 32 else 0L) or (buffer.position() - before).toLong()
    }

    @Keep private fun downDiscontinuity() = super.handleDiscontinuity()

    @Keep private fun downPosition(sourceEnded: Boolean): Long = super.getCurrentPositionUs(sourceEnded)

    // ---- the engine into the app ----

    @Keep private fun hostHeardChanged() { onHeardChanged?.invoke() }

    protected fun finalize() = TransitionEngineJni.destroy(engine)
}
