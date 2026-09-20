package dev.flint.music.playback

import android.util.Log
import androidx.media3.common.C
import androidx.media3.common.MimeTypes
import androidx.media3.common.Timeline
import androidx.media3.common.util.UnstableApi
import androidx.media3.exoplayer.audio.AudioSink
import androidx.media3.exoplayer.audio.ForwardingAudioSink
import java.nio.ByteBuffer
import java.nio.ByteOrder

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
 * It also hands every decoded buffer of a not-yet-analysed track to the streaming analyser, so the tempo,
 * beat grid and cue points come from audio the phone is decoding anyway.
 *
 * media3 calls [configure] once per decoded stream with the stream's media item, which is how the sink
 * knows which song it is decoding (decoding runs well ahead of what is heard).
 */
@UnstableApi
class TransitionSink(sink: AudioSink, private val listener: Listener) : ForwardingAudioSink(sink) {

    companion object {
        /**
         * A transition is running right now. Apple's player puts a word in the middle of the seek row
         * while one is - theirs reads "Mixing" - and the seek row is the one thing on screen that already
         * ticks, so it can read this without anything new having to watch it. Written on the playback
         * thread, read on the main one; a stale answer for a fraction of a second means nothing here.
         */
        @Volatile var mixing = false
            private set
    }

    interface Listener {
        /** The transition out of [outgoingId], or null for gapless. Called on the playback thread; must be quick. */
        fun planFor(outgoingId: String): Plan?
        /** Whether [songId] still needs analysing. */
        fun wantsAnalysis(songId: String): Boolean
        /** The analyser heard all of [songId]. The listener owns [handle] from here and must destroy it. */
        fun analysed(songId: String, handle: Long, frames: Long, sampleRate: Int)
    }

    class Plan(
        val incomingId: String,
        val outStartUs: Long,
        val durationUs: Long,
        val inSkipUs: Long,
        val mixer: FloatArray,
        val tempoRatio: Float,
        val keepPitch: Boolean,
        val rampUs: Long,
    )

    private enum class Phase { PASS, HOLD, MIX }

    private class Chunk(val data: ByteBuffer, val ptsUs: Long, val resync: Boolean)

    private var rate = 0
    private var channels = 0
    private var encoding = 0
    private var frameBytes = 0
    private val pcm get() = frameBytes > 0
    private var pendingConfig: AudioSink.AudioSinkConfig? = null

    /** The song whose audio is arriving now, from the last [configure]. */
    private var currentId: String? = null
    private var offsetUs = 0L

    private var phase = Phase.PASS
        set(value) { field = value; mixing = value != Phase.PASS }
    private var plan: Plan? = null
    private var planFor: String? = null
    private var tail: ByteBuffer? = null
    private var tailLen = 0
    private var tailRead = 0
    private var skipLeft = 0L
    private var resyncNext = false
    /** Mixed and stretched audio carries its own continuous clock; real timestamps resume after a resync. */
    private var syntheticPtsUs = C.TIME_UNSET

    private var mixer = 0L
    private var mixerFormat = 0
    private var stretch = 0L
    /** Built with the plan, taken up when the next track begins; see [prepare]. */
    private var pendingStretch = 0L
    private var pendingKeepPitch = false
    private var scratch: ByteBuffer? = null

    private var analyzer = 0L
    private var analyzerFor: String? = null
    private var analysisTainted = false

    private val out = ArrayDeque<Chunk>()
    private val pool = ArrayList<ByteBuffer>()

    // ---- configuration ----

    override fun configure(config: AudioSink.AudioSinkConfig) {
        val f = config.format
        val id = config.mediaPeriodId?.let { pid ->
            runCatching {
                val t = config.timeline
                t.getWindow(t.getPeriodByUid(pid.periodUid, Timeline.Period()).windowIndex, Timeline.Window()).mediaItem.mediaId
            }.getOrNull()
        }
        val enc = if (f.sampleMimeType == MimeTypes.AUDIO_RAW && (f.pcmEncoding == C.ENCODING_PCM_16BIT || f.pcmEncoding == C.ENCODING_PCM_FLOAT)) f.pcmEncoding else 0
        val same = enc != 0 && enc == encoding && f.sampleRate == rate && f.channelCount == channels
        // One line per decoded stream. Audio handed to the DSP whole (offload) never arrives here as
        // samples, and then no transition is possible at all - which is worth saying out loud, because
        // every other sign of it is a boundary that simply passes.
        Log.i("flint", "sink: $id ${f.sampleMimeType} ${f.sampleRate} Hz${if (enc == 0) " - not PCM, no transitions" else ""}")
        if (id != null && id != currentId) onNewStream(id)
        if (same || (out.isEmpty() && phase == Phase.PASS)) apply(config, enc) else {
            // A different format while audio of the old one is held: the transition cannot mix across it.
            // Play the held audio out as it is, then switch.
            abandonTransition()
            pendingConfig = config
        }
    }

    private fun apply(config: AudioSink.AudioSinkConfig, enc: Int) {
        val f = config.format
        rate = if (enc != 0) f.sampleRate else 0
        channels = if (enc != 0) f.channelCount else 0
        encoding = enc
        frameBytes = if (enc == 0) 0 else f.channelCount * (if (enc == C.ENCODING_PCM_FLOAT) 4 else 2)
        super.configure(config)
    }

    override fun setOutputStreamOffsetUs(outputStreamOffsetUs: Long) {
        offsetUs = outputStreamOffsetUs
        super.setOutputStreamOffsetUs(outputStreamOffsetUs)
    }

    // ---- the audio path ----

    override fun handleBuffer(buffer: ByteBuffer, presentationTimeUs: Long, encodedAccessUnitCount: Int): Boolean {
        pendingConfig?.let { config ->
            if (!drain()) return false
            pendingConfig = null
            val f = config.format
            apply(config, if (f.sampleMimeType == MimeTypes.AUDIO_RAW && (f.pcmEncoding == C.ENCODING_PCM_16BIT || f.pcmEncoding == C.ENCODING_PCM_FLOAT)) f.pcmEncoding else 0)
        }
        if (!pcm) return super.handleBuffer(buffer, presentationTimeUs, encodedAccessUnitCount)
        if (!drain()) return false
        buffer.order(ByteOrder.nativeOrder())
        when (phase) {
            Phase.PASS -> {
                if (stretch != 0L) { feedAnalysis(buffer, buffer.position(), buffer.remaining()); stretchOut(buffer); drain(); return true }
                val p = planFor(currentId)
                val trackPos = presentationTimeUs - offsetUs
                val frames = buffer.remaining() / frameBytes
                val startFrame = if (p == null) Long.MAX_VALUE else (p.outStartUs - trackPos) * rate / 1_000_000
                // More than a second past the planned start (a late plan, or a seek): leave this transition alone.
                val skipTransition = startFrame < -rate
                if (startFrame >= frames || skipTransition) return pass(buffer, presentationTimeUs)
                val before = startFrame.coerceAtLeast(0).toInt() * frameBytes
                feedAnalysis(buffer, buffer.position(), buffer.remaining())
                if (before > 0) {
                    val head = buffer.duplicate().order(ByteOrder.nativeOrder())
                    head.limit(head.position() + before)
                    enqueue(copyOf(head), presentationTimeUs)
                    buffer.position(buffer.position() + before)
                }
                beginHold(p!!)
                hold(buffer)
            }
            Phase.HOLD -> { feedAnalysis(buffer, buffer.position(), buffer.remaining()); hold(buffer) }
            Phase.MIX -> { feedAnalysis(buffer, buffer.position(), buffer.remaining()); mix(buffer, presentationTimeUs) }
        }
        drain()
        return true
    }

    /**
     * Straight through. The sink below refuses buffers on purpose (see [BurstSink]) and the renderer then
     * offers the same audio again, so the analyser is only given what was actually taken.
     */
    private fun pass(buffer: ByteBuffer, ptsUs: Long): Boolean {
        if (out.isNotEmpty()) {
            feedAnalysis(buffer, buffer.position(), buffer.remaining())
            enqueue(copyOf(buffer), ptsUs)
            drain()
            return true
        }
        val from = buffer.position()
        val accepted = super.handleBuffer(buffer, ptsUs, 1)
        feedAnalysis(buffer, from, buffer.position() - from)
        return accepted
    }

    /**
     * Something the plan was made without - an analysis measured since - has arrived, so the plan for
     * the track playing is asked for again at the next buffer. A transition is planned as soon as a
     * track's audio starts arriving, which is well before the track it was measured against has been
     * measured; without this the listener would be stuck with the answer it gave in the first second.
     */
    fun replan() { replanWanted = true }
    @Volatile private var replanWanted = false

    private fun planFor(id: String?): Plan? {
        if (id == null) return null
        val again = replanWanted
        if (planFor != id || again) {
            replanWanted = false
            plan = listener.planFor(id)
            planFor = id
            plan?.let(::prepare)
        }
        return plan
    }

    /**
     * Everything the transition needs, built when the plan is made rather than when it starts. A plan is
     * asked for as soon as a track's audio begins arriving, which is minutes of slack; the mix itself
     * begins between two buffers, and a megabyte of direct memory and a stretcher's tables allocated
     * right then are exactly the kind of work that leaves the sink with nothing to write - the catch in
     * the sound that the mixes had.
     */
    private fun prepare(p: Plan) {
        if (!pcm) return
        val bytes = (p.durationUs * rate / 1_000_000).toInt() * frameBytes
        if (bytes > 0 && (tail?.capacity() ?: 0) < bytes) tail = ByteBuffer.allocateDirect(bytes).order(ByteOrder.nativeOrder())
        if (mixer == 0L || mixerFormat != rate * 100 + channels) {
            if (mixer != 0L) AutoMixMixer.destroy(mixer)
            mixer = AutoMixMixer.create(rate, channels)
            mixerFormat = rate * 100 + channels
        }
        val stretching = kotlin.math.abs(p.tempoRatio - 1f) > 1e-4f
        if (stretching && pendingStretch != 0L && pendingKeepPitch != p.keepPitch) {
            AutoMixStretch.destroy(pendingStretch)
            pendingStretch = 0L
        }
        if (stretching && pendingStretch == 0L) {
            pendingStretch = AutoMixStretch.create(rate, channels, p.keepPitch)
            pendingKeepPitch = p.keepPitch
        }
        // The mixed audio is handed on in chunks; a pool that is already full means the mix itself
        // allocates nothing at all.
        while (pool.size < 8) pool += ByteBuffer.allocateDirect(16384).order(ByteOrder.nativeOrder())
    }

    private fun beginHold(p: Plan) {
        val bytes = (p.durationUs * rate / 1_000_000).toInt() * frameBytes
        tail = tail?.takeIf { it.capacity() >= bytes } ?: ByteBuffer.allocateDirect(bytes).order(ByteOrder.nativeOrder())
        tail!!.clear().limit(bytes)
        tailLen = 0
        phase = Phase.HOLD
    }

    /** Outgoing audio from the start of the transition; whatever does not fit is audio the plan skips. */
    private fun hold(buffer: ByteBuffer) {
        val t = tail!!
        val n = minOf(buffer.remaining(), t.limit() - tailLen)
        if (n > 0) {
            val slice = buffer.duplicate()
            slice.limit(slice.position() + n)
            t.position(tailLen)
            t.put(slice)
            tailLen += n
        }
        buffer.position(buffer.limit())
    }

    override fun handleDiscontinuity() {
        val p = plan
        if (phase == Phase.HOLD && p != null && tailLen > 0) {
            // The next track begins: its opening is mixed into what was held.
            // Both were built when the plan was made; either is still made here if something changed
            // under them (a format switch) since.
            if (mixer == 0L || mixerFormat != rate * 100 + channels) {
                if (mixer != 0L) AutoMixMixer.destroy(mixer)
                mixer = AutoMixMixer.create(rate, channels)
                mixerFormat = rate * 100 + channels
            }
            AutoMixMixer.configure(mixer, p.mixer)
            if (kotlin.math.abs(p.tempoRatio - 1f) > 1e-4f) {
                stretch = if (pendingStretch != 0L && pendingKeepPitch == p.keepPitch) pendingStretch.also { pendingStretch = 0L }
                else AutoMixStretch.create(rate, channels, p.keepPitch)
                AutoMixStretch.configure(stretch, p.tempoRatio, p.durationUs * rate / 1_000_000, p.rampUs * rate / 1_000_000)
            }
            skipLeft = p.inSkipUs * rate / 1_000_000 * frameBytes
            tailRead = 0
            resyncNext = true
            phase = Phase.MIX
        } else {
            abandonTransition()
            super.handleDiscontinuity()
        }
        plan = null
        planFor = null
    }

    /** The incoming track, mixed into the held ending of the outgoing one. */
    private fun mix(buffer: ByteBuffer, ptsUs: Long) {
        if (skipLeft > 0) {
            val n = minOf(skipLeft, buffer.remaining().toLong()).toInt()
            buffer.position(buffer.position() + n)
            skipLeft -= n
            if (!buffer.hasRemaining()) return
        }
        val src = if (stretch != 0L) stretched(buffer) ?: return else buffer
        val t = tail!!
        val frames = minOf(src.remaining(), tailLen - tailRead) / frameBytes
        if (frames > 0) {
            AutoMixMixer.process(mixer, t, tailRead, src, src.position(), t, tailRead, frames, encoding)
            val mixed = t.duplicate().order(ByteOrder.nativeOrder())
            mixed.limit(tailRead + frames * frameBytes).position(tailRead)
            enqueue(copyOf(mixed), stamp(ptsUs, frames))
            tailRead += frames * frameBytes
            src.position(src.position() + frames * frameBytes)
        }
        if (src.hasRemaining()) enqueue(copyOf(src), stamp(ptsUs, src.remaining() / frameBytes))
        if (tailRead >= tailLen) phase = Phase.PASS
    }

    /** Timestamps while mixing and stretching are the sink's own running clock, so a stretch never reads as a jump. */
    private fun stamp(ptsUs: Long, frames: Int): Long {
        if (stretch == 0L && syntheticPtsUs == C.TIME_UNSET) return ptsUs
        val at = if (syntheticPtsUs == C.TIME_UNSET) ptsUs else syntheticPtsUs
        syntheticPtsUs = at + frames * 1_000_000L / rate
        return at
    }

    private fun stretched(input: ByteBuffer): ByteBuffer? {
        val need = input.remaining() * 3 + AutoMixStretch.latencyFrames(stretch) * frameBytes + 8192
        val s = scratch?.takeIf { it.capacity() >= need } ?: ByteBuffer.allocateDirect(need).order(ByteOrder.nativeOrder()).also { scratch = it }
        s.clear()
        var produced = 0
        while (input.hasRemaining()) {
            val r = AutoMixStretch.process(stretch, input, input.position(), input.remaining(), s, produced, s.capacity() - produced, encoding)
            if (r < 0) { input.position(input.limit()); break }
            val consumed = (r ushr 32).toInt()
            val made = (r and 0xffffffffL).toInt()
            input.position(input.position() + consumed)
            produced += made
            if (consumed == 0 && made == 0) break
        }
        if (AutoMixStretch.bypassed(stretch)) finishStretch(s, produced).let { produced = it }
        s.position(0).limit(produced)
        return if (produced == 0) null else s
    }

    /** After a mix the incoming track keeps going through the stretcher until its tempo is back to normal. */
    private fun stretchOut(buffer: ByteBuffer) {
        val s = stretched(buffer) ?: return
        enqueue(copyOf(s), syntheticPtsUs.takeIf { it != C.TIME_UNSET } ?: 0L)
        syntheticPtsUs += (s.remaining() / frameBytes) * 1_000_000L / rate
    }

    private fun finishStretch(s: ByteBuffer, produced: Int): Int {
        val more = AutoMixStretch.drain(stretch, s, produced, s.capacity() - produced, encoding)
        AutoMixStretch.destroy(stretch)
        stretch = 0L
        // Back on the track's own timestamps: tell the real sink to take the next one as a new reference.
        resyncNext = true
        syntheticPtsUs = C.TIME_UNSET
        return produced + more.coerceAtLeast(0)
    }

    /** The held audio goes out unmixed: the format changed, or no next track came. */
    private fun abandonTransition() {
        if (phase == Phase.HOLD && tailLen > 0) {
            val t = tail!!.duplicate().order(ByteOrder.nativeOrder())
            t.position(0).limit(tailLen)
            enqueue(copyOf(t), 0)
        }
        if (phase != Phase.PASS) Log.i("flint", "transition abandoned in $phase")
        phase = Phase.PASS
        tailLen = 0
    }

    // ---- analysis tap ----

    private fun onNewStream(id: String) {
        finishAnalysis()
        currentId = id
        analysisTainted = false
        analyzerFor = null
    }

    private fun feedAnalysis(buffer: ByteBuffer, from: Int, bytes: Int) {
        val id = currentId ?: return
        if (analysisTainted || bytes <= 0) return
        if (analyzerFor != id) {
            analyzerFor = id
            if (analyzer != 0L) { AutoMixAnalyzer.destroy(analyzer); analyzer = 0L }
            if (listener.wantsAnalysis(id)) analyzer = AutoMixAnalyzer.create(rate, channels, 0)
        }
        if (analyzer != 0L) AutoMixAnalyzer.feed(analyzer, buffer, from, bytes, encoding)
    }

    private fun finishAnalysis() {
        val h = analyzer
        val id = analyzerFor
        analyzer = 0L
        if (h == 0L || id == null) return
        if (analysisTainted) { AutoMixAnalyzer.destroy(h); return }
        listener.analysed(id, h, AutoMixAnalyzer.frames(h), rate)
    }

    // ---- output queue ----

    private fun enqueue(data: ByteBuffer, ptsUs: Long) {
        if (!data.hasRemaining()) return
        out += Chunk(data, ptsUs, resyncNext)
        resyncNext = false
    }

    private fun copyOf(src: ByteBuffer): ByteBuffer {
        val n = src.remaining()
        val i = pool.indexOfFirst { it.capacity() >= n }
        val b = if (i >= 0) pool.removeAt(i) else ByteBuffer.allocateDirect(maxOf(n, 16384)).order(ByteOrder.nativeOrder())
        b.clear()
        b.put(src.duplicate())
        b.flip()
        return b
    }

    /** Sends what is queued. False when the real sink would not take it all yet (it asks again). */
    private fun drain(): Boolean {
        while (true) {
            val c = out.firstOrNull() ?: return true
            if (c.resync && c.data.position() == 0) super.handleDiscontinuity()
            if (!super.handleBuffer(c.data, c.ptsUs, 1)) return false
            out.removeFirst()
            if (pool.size < 32) pool += c.data
        }
    }

    override fun playToEndOfStream() {
        abandonTransition()
        finishAnalysis()
        if (drain()) super.playToEndOfStream()
    }

    override fun hasPendingData(): Boolean = out.isNotEmpty() || super.hasPendingData()
    override fun isEnded(): Boolean { if (out.isNotEmpty()) drain(); return out.isEmpty() && super.isEnded() }

    private fun clear() {
        out.clear()
        phase = Phase.PASS
        tailLen = 0; tailRead = 0; skipLeft = 0
        plan = null; planFor = null
        resyncNext = false
        syntheticPtsUs = C.TIME_UNSET
        if (stretch != 0L) { AutoMixStretch.destroy(stretch); stretch = 0L }
        if (pendingStretch != 0L) { AutoMixStretch.destroy(pendingStretch); pendingStretch = 0L }
        // A seek: the analyser has not heard this track continuously any more.
        if (analyzer != 0L) { AutoMixAnalyzer.destroy(analyzer); analyzer = 0L }
        analysisTainted = true
    }

    override fun flush() { clear(); super.flush() }

    override fun reset() {
        clear()
        if (mixer != 0L) { AutoMixMixer.destroy(mixer); mixer = 0L }
        pool.clear()
        tail = null
        scratch = null
        super.reset()
    }
}
