package dev.nori.music.playback

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFormat
import android.media.AudioManager
import android.media.AudioMixerAttributes
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import dev.nori.music.ffi.DacStep
import dev.nori.music.ffi.dacDecide
import dev.nori.music.ffi.dacMock
import dev.nori.music.ffi.dacTrackLine
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * What the output path looks like right now; shown to the user so they can verify it, and so a DAC that
 * cannot be driven bit-perfect says why rather than silently doing nothing.
 */
data class DacState(
    val device: String? = null,
    val bitPerfect: Boolean = false,
    val sampleRate: Int = 0,
    val bits: Int = 0,
    /** The device offers at least one bit-perfect mode. */
    val supported: Boolean = false,
    /** Every bit-perfect mode the device offers, as "44.1 kHz / 24 bit". */
    val modes: List<String> = emptyList(),
    /** Set when a mode exists for the playing rate but not in a sample format this app can write. */
    val blockedBy: String? = null,
    /** The format currently being played, for the diagnostic line. */
    val playing: String? = null,
    /** What the AudioTrack was actually opened with, and whether it was offloaded: the honest answer. */
    val track: String? = null,
)

/** One USB output, with the bit-perfect modes the framework offers for it. */
class DacPort(
    val id: Int,
    val name: String,
    val modes: List<AudioFormat>,
    /** The framework handle. Null for a mock, which never reaches the audio system. */
    val device: AudioDeviceInfo?,
)

/**
 * Everything [BitPerfect] needs from the audio system, as four calls. Replacing it is the only way to
 * exercise this path without the hardware: a USB DAC cannot be attached to an emulator, and the decision
 * table here (which mode matches, what to say when none does) is where the bugs live.
 */
interface DacSource {
    fun find(): DacPort?
    fun prefer(port: DacPort, format: AudioFormat): Boolean
    fun release(port: DacPort)

    companion object {
        /**
         * A DAC that is not there, described as `name@44100/16,96000/24`: the rates and bit depths it offers
         * bit-perfect. `tools/app.sh do "dac <spec>"` points the app at one, which is the only way to check
         * this path on an emulator. It never reaches the audio system, so nothing is routed anywhere; what it
         * exercises is the decision - which mode is picked, and what the user is told when none can be.
         */
        fun mock(spec: String): DacSource {
            // The spec is read by the core (dac_mock); only the framework's format objects are made here.
            val m = dacMock(spec)
            val modes = m.rates.indices.map { i ->
                AudioFormat.Builder().setSampleRate(m.rates[i].toInt()).setEncoding(m.encodings[i])
                    .setChannelMask(AudioFormat.CHANNEL_OUT_STEREO).build()
            }
            val port = DacPort(-1, m.name, modes, null)
            return object : DacSource {
                override fun find() = port
                // A mock cannot ask the framework for anything, so it grants whatever it advertised.
                override fun prefer(port: DacPort, format: AudioFormat) = format in port.modes
                override fun release(port: DacPort) = Unit
            }
        }
    }
}

/**
 * Hands a USB DAC to the app: on Android 14+ the framework can route media to a
 * USB device with no mixer, no resampler and no volume scaling, at the track's
 * own sample rate. The mixer attributes have to match what the AudioTrack is
 * opened with, so they are applied from the sink's own `configure` - before the
 * AudioTrack exists, which is the only moment the framework reads them.
 *
 * Everything that touches samples (equalizer, ReplayGain volume) must be off
 * while [state].bitPerfect is true; [PlaybackService] does that.
 */
class BitPerfect(context: Context) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val media = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build()
    private val _state = MutableStateFlow(DacState())
    val state: StateFlow<DacState> = _state

    // Touched from two threads: the audio device callback on the main looper, and the audio track
    // provider on the playback thread. Everything that reads or writes them goes through refresh(),
    // which is synchronized; `applied` is also read on its own by the track provider.
    private var enabled = false
    private var sampleRate = 0
    private var encoding = AudioFormat.ENCODING_PCM_16BIT
    /** The port the framework is currently holding preferred mixer attributes for. */
    @Volatile private var applied: DacPort? = null
    var onChanged: () -> Unit = {}

    /**
     * Stands in for the audio system. A test run points this at a fake DAC (`app.sh do "dac <spec>"`) so the
     * whole path can be driven on an emulator; left alone it is the framework, and costs one field.
     */
    @Volatile private var source: DacSource = Framework()

    /** Swap in a mock DAC, or null for the real audio system. Only the test bridge calls this. */
    @Synchronized fun testSource(mock: DacSource?) {
        clear()
        source = mock ?: Framework()
        refresh()
    }

    private val devices = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) = refresh()
        override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) = refresh()
    }

    fun start() = audio.registerAudioDeviceCallback(devices, Handler(Looper.getMainLooper()))

    @Synchronized fun stop() {
        audio.unregisterAudioDeviceCallback(devices)
        clear()
    }

    @Synchronized fun setEnabled(on: Boolean) {
        if (enabled == on) return
        enabled = on
        refresh()
    }

    /**
     * The format the AudioTrack is about to be opened with, taken from the track configuration itself. It is
     * not a guess: an earlier version read the decoder's *input* format and assumed 16-bit unless hi-res was
     * on, while the sink actually writes whatever its processor chain ends in - so no mode ever matched, and
     * the toggle sat there doing nothing. Called on the playback thread, from the audio track provider,
     * which is the last moment the framework still reads preferred mixer attributes.
     */
    @Synchronized fun onFormat(rate: Int, pcmEncoding: Int) {
        if (rate <= 0 || (rate == sampleRate && pcmEncoding == encoding)) return
        sampleRate = rate
        encoding = pcmEncoding
        refresh()
    }

    /** What the AudioTrack was opened with, recorded after the fact so the user can check it. */
    fun onTrack(rate: Int, pcmEncoding: Int, offloaded: Boolean) {
        val line = dacTrackLine(rate.coerceAtLeast(0).toUInt(), pcmEncoding, offloaded)
        if (_state.value.track != line) _state.value = _state.value.copy(track = line)
    }

    /** The device the AudioTrack should be pinned to, or null to let Android route it. */
    fun preferredDevice(): AudioDeviceInfo? = applied?.device

    @Synchronized private fun refresh() {
        val port = runCatching { source.find() }.getOrNull()
        val before = _state.value.bitPerfect
        _state.value = decide(port)
        if (before != _state.value.bitPerfect) onChanged()
    }

    /**
     * Asks the core (nori_player::dac::decide) which mode fits, whether the one held already does, and what
     * to show, then does what it says. Every exit but keeping the held mode clears the preferred mixer
     * attributes first: attributes left pointing at a format the AudioTrack will not be opened with are how
     * this ends up routed somewhere silent.
     */
    private fun decide(port: DacPort?): DacState {
        if (port == null) { clear(); return DacState() }
        val track = _state.value.track
        val held = applied?.takeIf { it.id == port.id }
        val d = dacDecide(
            enabled, Build.VERSION.SDK_INT >= 34, port.name,
            port.modes.map { it.sampleRate.toUInt() }, port.modes.map { it.encoding }, sampleRate.toUInt(), encoding,
            held?.modes?.map { it.sampleRate.toUInt() }, held?.modes?.map { it.encoding }, _state.value.bitPerfect,
        )
        return when (val step = d.step) {
            DacStep.Keep -> _state.value.copy(modes = d.modes, playing = d.playing, track = track)
            DacStep.Release -> {
                d.blockedBy?.let { Log.w("BitPerfect", "no usable bit-perfect mode: $it") }
                clear()
                DacState(d.device, supported = d.supported, modes = d.modes, blockedBy = d.blockedBy, playing = d.playing, track = track)
            }
            is DacStep.Prefer -> {
                clear()
                val ok = runCatching { source.prefer(port, port.modes[step.index.toInt()]) }.getOrElse { Log.w("BitPerfect", "prefer refused", it); false }
                applied = port.takeIf { ok }
                DacState(d.device, ok, sampleRate, d.bits.toInt(), true, d.modes, if (ok) null else d.refused, d.playing, track)
            }
        }
    }

    private fun clear() {
        val port = applied ?: return
        applied = null
        runCatching { source.release(port) }.onFailure { Log.w("BitPerfect", "release failed", it) }
    }

    /** The real audio system. */
    private inner class Framework : DacSource {
        override fun find(): DacPort? {
            val d = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
                .firstOrNull { it.type == AudioDeviceInfo.TYPE_USB_DEVICE || it.type == AudioDeviceInfo.TYPE_USB_HEADSET }
                ?: return null
            val modes = if (Build.VERSION.SDK_INT >= 34) {
                audio.getSupportedMixerAttributes(d)
                    .filter { it.mixerBehavior == AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT }
                    .map { it.format }
            } else emptyList()
            // The name as the device gives it; the core trims it and names a nameless one.
            return DacPort(d.id, d.productName?.toString().orEmpty(), modes, d)
        }

        override fun prefer(port: DacPort, format: AudioFormat): Boolean {
            val d = port.device ?: return false
            if (Build.VERSION.SDK_INT < 34) return false
            val attrs = AudioMixerAttributes.Builder(format).setMixerBehavior(AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT).build()
            return audio.setPreferredMixerAttributes(media, d, attrs)
        }

        override fun release(port: DacPort) {
            val d = port.device ?: return
            if (Build.VERSION.SDK_INT >= 34) audio.clearPreferredMixerAttributes(media, d)
        }
    }
}
