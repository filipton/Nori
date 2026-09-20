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

        /** How little sound may be left in the sink before a held ending is let go rather than mixed. */
        private const val DRY_US = 1_500_000L
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
    /** The output timestamp holding began at: everything from here on is inside this sink, unheard. */
    private var heldFromUs = C.TIME_UNSET
    private var heldAt = 0L
    /** How much of the outgoing track has been swallowed into the hold, in microseconds. */
    private var heldUs = 0L
    /** The last position given to the player, which may never go backwards. */
    private var reported = Long.MIN_VALUE
    private var tailLen = 0
    private var tailRead = 0
    /** The output timestamp the queued mix runs to, so a cut-short mix resumes the ending after it. */
    private var mixedEndUs = C.TIME_UNSET
    private var skipLeft = 0L
    private var resyncNext = false
    /** Mixed and stretched audio carries its own continuous clock; real timestamps resume after a resync. */
    private var syntheticPtsUs = C.TIME_UNSET

    private var mixer = 0L
    private var mixerFormat = 0
    private var stretch = 0L
    /** What the live stretcher was built for: rate, channels, encoding, bytes per frame. */
    private var stretchRate = 0
    private var stretchCh = 0
    private var stretchEnc = 0
    private var stretchFrameBytes = 0
    /** Built with the plan, taken up when the next track begins; see [prepare]. */
    private var pendingStretch = 0L
    private var pendingKeepPitch = false
    private var scratch: ByteBuffer? = null

    private var analyzer = 0L
    private var analyzerFor: String? = null
    private var analyzerRate = 0
    private var analysisTainted = false

    /** The incoming stream's format while a transition spans two formats; the mix runs at the outgoing one. */
    private var inRate = 0
    private var inChannels = 0
    private var inEncoding = 0
    private var inFrameBytes = 0
    /** Which stream id the converter was armed for: buffers flowing now are always in this format. */
    private var convId: String? = null
    private var resample = 0L
    private var resampleBuf: ByteBuffer? = null
    private val converting get() = resample != 0L
    /** Formats decoded ahead while a transition runs; armed once their buffers flow, never downstream. */
    private val staged = ArrayDeque<AudioSink.AudioSinkConfig>()
    /** The id whose buffers the mix is consuming, so a further decode-ahead configure never re-arms it. */
    private var mixSourceId: String? = null
    /**
     * The downstream format stays on whatever the first PCM stream brought. False only while the
     * service holds the output bit-perfect (or hi-res): then every stream passes through native and
     * the track below follows it, as before. The service keeps this in step with `transitionsOff`.
     */
    @Volatile var lockRate = true

    private val out = ArrayDeque<Chunk>()
    private val pool = ArrayList<ByteBuffer>()

    // ---- configuration ----

    override fun configure(config: AudioSink.AudioSinkConfig) {
        val f = config.format
        val id = idOf(config)
        val enc = if (f.sampleMimeType == MimeTypes.AUDIO_RAW && (f.pcmEncoding == C.ENCODING_PCM_16BIT || f.pcmEncoding == C.ENCODING_PCM_FLOAT)) f.pcmEncoding else 0
        // One line per decoded stream. Audio handed to the DSP whole (offload) never arrives here as
        // samples, and then no transition is possible at all - which is worth saying out loud, because
        // every other sign of it is a boundary that simply passes.
        Log.i("flint", "sink: $id ${f.sampleMimeType} ${f.sampleRate} Hz${if (enc == 0) " - not PCM, no transitions" else ""}")
        val forCurrent = id == null || id == currentId
        if (id != null && id != currentId) onNewStream(id)
        if (enc == 0) {
            // Not samples: there is nothing to convert, so the downstream format has to follow.
            // Anything held is played out as it is first.
            dropConverter()
            if (out.isEmpty() && phase == Phase.PASS) { apply(config, 0); return }
            abandonTransition()
            pendingConfig = config
            return
        }
        if (!lockRate) {
            // Bit-perfect or hi-res: the native format reaches the wire; the track below follows.
            dropConverter()
            if (f.sampleRate == rate && f.channelCount == channels && enc == encoding) return
            abandonTransition()
            pendingConfig = config
            return
        }
        if (rate == 0) {
            // The first PCM stream sets the downstream format for the whole queue.
            apply(config, enc)
            Log.i("flint", "sink pins ${f.sampleRate} Hz x${f.channelCount} for the queue")
            return
        }
        if (f.sampleRate == rate && f.channelCount == channels && enc == encoding) {
            // At the pinned format already. While its buffers flow the track below is told, as
            // always (a same-format configure never recreates it); decode-ahead only waits.
            if (phase == Phase.PASS && forCurrent) {
                // The flowing stream is at the pinned format: any converter is stale.
                dropConverter()
                apply(config, enc)
                return
            }
            staged += config
            return
        }
        if (phase == Phase.PASS && forCurrent) {
            // The stream whose buffers are flowing changed format: convert it from here on.
            if (!armConversion(id, f.sampleRate, f.channelCount, enc)) {
                abandonTransition()
                pendingConfig = config
            }
            return
        }
        // Decode-ahead (or a same-format stranger): the mix is never killed for it. Armed once
        // its buffers flow - at the discontinuity, or when the mix is out.
        if (phase == Phase.PASS || phase == Phase.HOLD || phase == Phase.MIX) { staged += config; return }
        abandonTransition()
        pendingConfig = config
    }

    private fun idOf(config: AudioSink.AudioSinkConfig): String? =
        config.mediaPeriodId?.let { pid ->
            runCatching {
                val t = config.timeline
                t.getWindow(t.getPeriodByUid(pid.periodUid, Timeline.Period()).windowIndex, Timeline.Window()).mediaItem.mediaId
            }.getOrNull()
        }

    /**
     * Converts the stream [id] (native [srcRate] x[srcCh], [srcEnc]) to the pinned format, from
     * whose buffers everything downstream is now computed. The stretcher works in the incoming
     * domain from here on (converted after), so the outgoing-domain one built in [prepare] is
     * dropped. False when the pair cannot be converted.
     */
    private fun armConversion(id: String?, srcRate: Int, srcCh: Int, srcEnc: Int): Boolean {
        if (srcEnc == 0) return false
        inRate = srcRate; inChannels = srcCh; inEncoding = srcEnc
        inFrameBytes = srcCh * (if (srcEnc == C.ENCODING_PCM_FLOAT) 4 else 2)
        if (resample != 0L) AutoMixResample.destroy(resample)
        resample = AutoMixResample.create(inRate, inChannels, rate, channels)
        convId = id
        if (resample == 0L) { inRate = 0; convId = null; return false }
        if (pendingStretch != 0L) { AutoMixStretch.destroy(pendingStretch); pendingStretch = 0L }
        Log.i("flint", "converting $inRate Hz x$inChannels -> $rate Hz x$channels")
        return true
    }

    /** No conversion: buffers flow at the pinned format already. Keeps the latch. */
    private fun dropConverter() {
        if (resample != 0L) { AutoMixResample.destroy(resample); resample = 0L }
        inRate = 0; inChannels = 0; inEncoding = 0; inFrameBytes = 0
        convId = null
    }

    /** A seek: the converter keeps its formats but its stream position starts over. */
    private fun resetConverter() {
        if (resample != 0L && inRate != 0) {
            AutoMixResample.destroy(resample)
            resample = AutoMixResample.create(inRate, inChannels, rate, channels)
            if (resample == 0L) inRate = 0
        }
    }

    /**
     * The staged format of [id] (its buffers flow now): arm it, or drop the converter when it is
     * already at the pinned format. Anything staged for another id waits its turn.
     */
    private fun armStagedFor(id: String?) {
        val it = staged.iterator()
        var cfg: AudioSink.AudioSinkConfig? = null
        while (it.hasNext()) {
            val c = it.next()
            if (idOf(c) == id || (id == null && cfg == null)) { it.remove(); cfg = c }
        }
        val c = cfg ?: return
        val f = c.format
        val enc = if (f.sampleMimeType == MimeTypes.AUDIO_RAW && (f.pcmEncoding == C.ENCODING_PCM_16BIT || f.pcmEncoding == C.ENCODING_PCM_FLOAT)) f.pcmEncoding else 0
        if (enc == 0 || (f.sampleRate == rate && f.channelCount == channels && enc == encoding)) {
            if (convId != null || enc == 0) dropConverter()
            return
        }
        // Already converting this stream: re-arming would drop the two frames of history the
        // interpolator carries and click.
        if (convId == id && inRate == f.sampleRate && inChannels == f.channelCount && inEncoding == enc) return
        if (!armConversion(id, f.sampleRate, f.channelCount, enc)) {
            abandonTransition()
            pendingConfig = c
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
        // The analyser hears the native stream; everything below runs at the pinned format.
        val (aRate, aCh, aEnc) = if (converting) Triple(inRate, inChannels, inEncoding) else Triple(rate, channels, encoding)
        when (phase) {
            Phase.PASS -> {
                // Converted audio lives in a shared buffer that the next call reuses: anything kept
                // is copied, and the sink below is only ever offered copies, never the live buffer.
                // The stretcher works in the native domain, so it always sees the native buffer.
                if (converting || stretch != 0L) feedAnalysis(buffer, buffer.position(), buffer.remaining(), aRate, aCh, aEnc)
                if (stretch != 0L) { stretchOut(buffer); drain(); return true }
                val buf = if (converting) toPinned(buffer) else buffer
                if (converting && buf == null) { drain(); return true }
                val p = planFor(currentId)
                val trackPos = presentationTimeUs - offsetUs
                val frames = buf!!.remaining() / frameBytes
                val startFrame = if (p == null) Long.MAX_VALUE else (p.outStartUs - trackPos) * rate / 1_000_000
                // More than a second past the planned start (a late plan, or a seek): leave this transition alone.
                val skipTransition = startFrame < -rate
                if (startFrame >= frames || skipTransition) return pass(buf, presentationTimeUs, converting)
                val before = startFrame.coerceAtLeast(0).toInt() * frameBytes
                if (!converting) feedAnalysis(buf, buf.position(), buf.remaining())
                if (before > 0) {
                    val head = buf.duplicate().order(ByteOrder.nativeOrder())
                    head.limit(head.position() + before)
                    enqueue(copyOf(head), presentationTimeUs)
                    buf.position(buf.position() + before)
                }
                beginHold(p!!)
                heldFromUs = presentationTimeUs + before.toLong() / frameBytes * 1_000_000L / rate
                heldAt = android.os.SystemClock.elapsedRealtime()
                Log.i("flint", "holding the ending, ${(heldFromUs - super.getCurrentPositionUs(false)) / 1000} ms of sound still in the sink")
                hold(buf)
            }
            Phase.HOLD -> {
                feedAnalysis(buffer, buffer.position(), buffer.remaining(), aRate, aCh, aEnc)
                val buf = if (converting) toPinned(buffer) else buffer
                if (buf != null) hold(buf)
            }
            Phase.MIX -> {
                feedAnalysis(buffer, buffer.position(), buffer.remaining(), aRate, aCh, aEnc)
                mix(buffer, presentationTimeUs)
            }
        }
        drain()
        return true
    }

    /**
     * Straight through. The sink below refuses buffers on purpose (see [BurstSink]) and the renderer then
     * offers the same audio again, so the analyser is only given what was actually taken.
     *
     * Converted audio ([copy]) lives in the shared converter buffer: it always goes through the
     * queue as a copy, because a refused buffer offered again would be converted over itself.
     */
    private fun pass(buffer: ByteBuffer, ptsUs: Long, copy: Boolean = false): Boolean {
        if (out.isNotEmpty() || copy) {
            // The queue path is already fed by the caller (with the native buffer); the fast path feeds here.
            if (!copy) feedAnalysis(buffer, buffer.position(), buffer.remaining())
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
        heldUs += buffer.remaining().toLong() / frameBytes * 1_000_000L / rate
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
            // The next track begins: its opening is mixed into what was held. Its format was staged
            // while it decoded ahead; arm it now, so the stretcher and the skip below measure the
            // incoming track in its own domain and the mix runs at the pinned one.
            armStagedFor(currentId)
            mixSourceId = currentId
            // Both were built when the plan was made; either is still made here if something changed
            // under them (a format switch) since.
            if (mixer == 0L || mixerFormat != rate * 100 + channels) {
                if (mixer != 0L) AutoMixMixer.destroy(mixer)
                mixer = AutoMixMixer.create(rate, channels)
                mixerFormat = rate * 100 + channels
            }
            AutoMixMixer.configure(mixer, p.mixer)
            // The stretcher works in the incoming domain when the sides disagree (converted after
            // stretching), else in the outgoing one as before; the skip is in the same domain.
            val sRate = if (converting) inRate else rate
            val sCh = if (converting) inChannels else channels
            val sEnc = if (converting) inEncoding else encoding
            val sFrameBytes = if (converting) inFrameBytes else frameBytes
            if (kotlin.math.abs(p.tempoRatio - 1f) > 1e-4f) {
                if (converting && pendingStretch != 0L) { AutoMixStretch.destroy(pendingStretch); pendingStretch = 0L }
                stretch = if (!converting && pendingStretch != 0L && pendingKeepPitch == p.keepPitch) pendingStretch.also { pendingStretch = 0L }
                else AutoMixStretch.create(sRate, sCh, p.keepPitch)
                stretchRate = sRate; stretchCh = sCh; stretchEnc = sEnc; stretchFrameBytes = sFrameBytes
                AutoMixStretch.configure(stretch, p.tempoRatio, p.durationUs * sRate / 1_000_000, p.rampUs * sRate / 1_000_000)
            }
            Log.i("flint", "mixing: the next track arrived ${android.os.SystemClock.elapsedRealtime() - heldAt} ms into the hold with ${(heldFromUs - super.getCurrentPositionUs(false)) / 1000} ms of sound left")
            // The held audio is about to go out as the mix, so it stops counting as played-but-unheard.
            // What was already reported stands until the sound really catches up with it.
            heldUs = 0L
            skipLeft = p.inSkipUs * sRate / 1_000_000 * sFrameBytes
            tailRead = 0
            mixedEndUs = C.TIME_UNSET
            resyncNext = true
            phase = Phase.MIX
        } else {
            abandonTransition()
            // No mix: the new track's buffers flow from here, so its staged format arms now.
            armStagedFor(currentId)
            mixSourceId = null
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
        // Through the stretcher in the incoming domain, then converted to the outgoing one the held
        // tail is in. Without conversion this is exactly the old path.
        val stretchedBuf = if (stretch != 0L) stretched(buffer) ?: return else buffer
        val src = if (converting) converted(stretchedBuf) ?: return else stretchedBuf
        val t = tail!!
        val frames = minOf(src.remaining(), tailLen - tailRead) / frameBytes
        if (frames > 0) {
            AutoMixMixer.process(mixer, t, tailRead, src, src.position(), t, tailRead, frames, encoding)
            val mixed = t.duplicate().order(ByteOrder.nativeOrder())
            mixed.limit(tailRead + frames * frameBytes).position(tailRead)
            val at = stamp(ptsUs, frames)
            enqueue(copyOf(mixed), at)
            mixedEndUs = at + frames * 1_000_000L / rate
            tailRead += frames * frameBytes
            src.position(src.position() + frames * frameBytes)
        }
        if (src.hasRemaining()) {
            val rest = src.remaining() / frameBytes
            val at = stamp(ptsUs, rest)
            enqueue(copyOf(src), at)
            mixedEndUs = at + rest * 1_000_000L / rate
        }
        if (tailRead >= tailLen) {
            phase = Phase.PASS
            finishConversion()
        }
    }

    /**
     * Incoming-domain audio into the pinned format the mix runs at. Null when nothing comes out
     * yet (the converter holds lookahead); the consumed input is inside it and arrives with the next
     * call, so nothing is lost. A conversion that cannot run drops the converter and lets the ending
     * play unmixed rather than wedging the sink on a buffer that will never convert.
     */
    private fun converted(input: ByteBuffer): ByteBuffer? {
        val wo = if (encoding == C.ENCODING_PCM_FLOAT) 4 else 2
        val inFrames = input.remaining() / inFrameBytes.coerceAtLeast(1)
        val need = ((inFrames.toLong() * rate / inRate.coerceAtLeast(1)) + 4) * channels * wo
        val b = resampleBuf?.takeIf { it.capacity() >= need } ?: ByteBuffer.allocateDirect(need.toInt().coerceAtLeast(16384))
            .order(ByteOrder.nativeOrder()).also { resampleBuf = it }
        b.clear()
        var r = AutoMixResample.process(resample, input, input.position(), input.remaining(), b, 0, b.capacity(), inEncoding, encoding)
        if (r < 0) {
            val bigger = ByteBuffer.allocateDirect((b.capacity() * 2 + 16384)).order(ByteOrder.nativeOrder())
            resampleBuf = bigger
            r = AutoMixResample.process(resample, input, input.position(), input.remaining(), bigger, 0, bigger.capacity(), inEncoding, encoding)
        }
        if (r < 0) {
            Log.w("flint", "conversion ${inRate} Hz x$inChannels -> $rate Hz x$channels failed, letting the ending play")
            dropConverter()
            abandonTransition()
            return null
        }
        val used = (r ushr 32).toInt()
        val made = (r and 0xffffffffL).toInt()
        input.position(input.position() + used)
        b.position(0).limit(made)
        return if (made == 0) null else b
    }

    /**
     * The mix is out: the mixed-in track's own format stays armed (its buffers keep flowing through
     * it), anything staged further ahead waits, and the mix stops counting as a mix.
     */
    private fun finishConversion() {
        mixSourceId = null
        armStagedFor(currentId)
    }

    /** Timestamps while mixing and stretching are the sink's own running clock, so a stretch never reads as a jump. */
    private fun stamp(ptsUs: Long, frames: Int): Long {
        if (stretch == 0L && syntheticPtsUs == C.TIME_UNSET) return ptsUs
        val at = if (syntheticPtsUs == C.TIME_UNSET) ptsUs else syntheticPtsUs
        syntheticPtsUs = at + frames * 1_000_000L / rate
        return at
    }

    private fun stretched(input: ByteBuffer): ByteBuffer? {
        val domBytes = stretchFrameBytes.coerceAtLeast(1)
        val domEnc = if (stretchEnc != 0) stretchEnc else encoding
        val need = input.remaining() * 3 + AutoMixStretch.latencyFrames(stretch) * domBytes + 8192
        val s = scratch?.takeIf { it.capacity() >= need } ?: ByteBuffer.allocateDirect(need).order(ByteOrder.nativeOrder()).also { scratch = it }
        s.clear()
        var produced = 0
        while (input.hasRemaining()) {
            val r = AutoMixStretch.process(stretch, input, input.position(), input.remaining(), s, produced, s.capacity() - produced, domEnc)
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
    /**
     * Native stream audio into the pinned format. Null when nothing comes out yet (the converter
     * holds lookahead); the consumed input is inside it and arrives with the next call, so nothing
     * is lost. Shares one buffer across calls: anything kept must be copied first.
     */
    private fun toPinned(input: ByteBuffer): ByteBuffer? = converted(input)

    private fun stretchOut(buffer: ByteBuffer) {
        val s = stretched(buffer) ?: return
        // The stretcher runs in the incoming domain; the pinned track below needs pinned audio.
        val o = if (converting) toPinned(s) else s
        if (o == null) return
        enqueue(copyOf(o), syntheticPtsUs.takeIf { it != C.TIME_UNSET } ?: 0L)
        syntheticPtsUs += (o.remaining() / frameBytes) * 1_000_000L / rate
    }

    private fun finishStretch(s: ByteBuffer, produced: Int): Int {
        val domEnc = if (stretchEnc != 0) stretchEnc else encoding
        val more = AutoMixStretch.drain(stretch, s, produced, s.capacity() - produced, domEnc)
        AutoMixStretch.destroy(stretch)
        stretch = 0L
        stretchRate = 0; stretchCh = 0; stretchEnc = 0; stretchFrameBytes = 0
        // Back on the track's own timestamps: tell the real sink to take the next one as a new reference.
        resyncNext = true
        syntheticPtsUs = C.TIME_UNSET
        return produced + more.coerceAtLeast(0)
    }

    /**
     * The held audio goes out unmixed: the next track never came, or the mix itself was cut short
     * (a new stream while mixing). A cut-short mix plays only what has not gone out yet - what is
     * already queued went out as the mix and is not repeated - so the ending is heard to its end
     * instead of stopping where the mix did. The converter is untouched: whatever still flows is
     * still in the format it was armed for.
     */
    private fun abandonTransition() {
        if (tailLen > 0 && (phase == Phase.HOLD || phase == Phase.MIX)) {
            val from = if (phase == Phase.MIX) tailRead.coerceIn(0, tailLen) else 0
            if (from < tailLen) {
                val t = tail!!.duplicate().order(ByteOrder.nativeOrder())
                t.position(from).limit(tailLen)
                // At the timestamp it was held at, not at nought: this audio is the ending of the track,
                // in the track's own timeline, and the sink downstream reads these to keep the clock.
                // Past the start of a mix the queue already holds the mix, so the rest follows it.
                val at = if (phase == Phase.MIX && mixedEndUs != C.TIME_UNSET) mixedEndUs
                    else heldFromUs.takeIf { it != C.TIME_UNSET } ?: 0L
                enqueue(copyOf(t), at)
            }
        }
        if (phase != Phase.PASS) Log.i("flint", "transition abandoned in $phase")
        phase = Phase.PASS
        tailLen = 0
        heldFromUs = C.TIME_UNSET
        heldUs = 0L
        mixSourceId = null
        // Given up on, not forgotten: the planned point is behind us now, and without this the next
        // buffer of the same track would start the hold over.
        plan = null
        planFor = currentId
    }

    // ---- analysis tap ----

    private fun onNewStream(id: String) {
        finishAnalysis()
        currentId = id
        analysisTainted = false
        analyzerFor = null
    }

    private fun feedAnalysis(buffer: ByteBuffer, from: Int, bytes: Int) =
        feedAnalysis(buffer, from, bytes, rate, channels, encoding)

    private fun feedAnalysis(buffer: ByteBuffer, from: Int, bytes: Int, fRate: Int, fCh: Int, fEnc: Int) {
        val id = currentId ?: return
        if (analysisTainted || bytes <= 0) return
        if (analyzerFor != id) {
            analyzerFor = id
            if (analyzer != 0L) { AutoMixAnalyzer.destroy(analyzer); analyzer = 0L }
            if (listener.wantsAnalysis(id)) { analyzer = AutoMixAnalyzer.create(fRate, fCh, 0); analyzerRate = fRate }
        }
        if (analyzer != 0L) AutoMixAnalyzer.feed(analyzer, buffer, from, bytes, fEnc)
    }

    private fun finishAnalysis() {
        val h = analyzer
        val id = analyzerFor
        analyzer = 0L
        if (h == 0L || id == null) return
        if (analysisTainted) { AutoMixAnalyzer.destroy(h); return }
        listener.analysed(id, h, AutoMixAnalyzer.frames(h), analyzerRate)
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

    /**
     * The renderer asks for the playhead every few milliseconds whether or not it has audio to give,
     * which makes this the one place that can notice the sound running out. While a track's ending is
     * held nothing goes downstream, and the held audio is only released when the next track's first
     * buffer arrives to be mixed into it. If that buffer is late - the next track still being fetched,
     * the outgoing one not decoded to its end yet - what is already in the sink plays out and the
     * listener gets a hole where the end of the song should be, as long as the crossfade itself.
     *
     * So the ending is let go unmixed before that can happen. No crossfade is a disappointment; silence
     * for the last twelve seconds of a song is a fault.
     */
    override fun getCurrentPositionUs(sourceEnded: Boolean): Long {
        val at = super.getCurrentPositionUs(sourceEnded)
        if (at == AudioSink.CURRENT_POSITION_NOT_SET) return at
        if (phase == Phase.HOLD && heldFromUs != C.TIME_UNSET && heldFromUs - at < DRY_US) {
            Log.i("flint", "transition: nothing to mix in yet with ${(heldFromUs - at) / 1000} ms of sound left, letting the ending play")
            abandonTransition()
            drain()
        }
        // Held audio has left the output but has not been heard, and this is the only thing the player
        // asks about how far the track has got - so it is counted as played. The player starts the next
        // track only once everything of this one has been, and the samples to mix into what is held are
        // the next track's: without this they arrived after the sink had run dry, and the crossfade
        // played after a hole as long as itself. That was "the last twelve seconds go silent".
        //
        // It is worth exactly the audio in hand, and it is given back: once the mix begins, what is
        // reported stands still until what is really being heard has caught up with it.
        reported = maxOf(reported, at + heldUs)
        return reported
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
        mixedEndUs = C.TIME_UNSET
        heldFromUs = C.TIME_UNSET; heldUs = 0L; reported = Long.MIN_VALUE
        plan = null; planFor = null
        mixSourceId = null
        resyncNext = false
        syntheticPtsUs = C.TIME_UNSET
        if (stretch != 0L) { AutoMixStretch.destroy(stretch); stretch = 0L }
        stretchRate = 0; stretchCh = 0; stretchEnc = 0; stretchFrameBytes = 0
        if (pendingStretch != 0L) { AutoMixStretch.destroy(pendingStretch); pendingStretch = 0L }
        // A seek: the latch and the formats stand (the track below is untouched), but the
        // converter's stream position starts over and anything staged is for another timeline.
        resetConverter()
        staged.clear()
        pendingConfig = null
        // A seek: the analyser has not heard this track continuously any more.
        if (analyzer != 0L) { AutoMixAnalyzer.destroy(analyzer); analyzer = 0L }
        analysisTainted = true
    }

    override fun flush() { clear(); super.flush() }

    override fun reset() {
        clear()
        // Stopped: the latch goes with the track below, and the next playback pins again.
        dropConverter()
        staged.clear()
        mixSourceId = null
        convId = null
        rate = 0; channels = 0; encoding = 0; frameBytes = 0
        if (mixer != 0L) { AutoMixMixer.destroy(mixer); mixer = 0L }
        pool.clear()
        tail = null
        scratch = null
        resampleBuf = null
        super.reset()
    }
}
