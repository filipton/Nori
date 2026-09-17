package dev.flint.music.playback

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
)

/**
 * Hands a USB DAC to the app: on Android 14+ the framework can route media to a
 * USB device with no mixer, no resampler and no volume scaling, at the track's
 * own sample rate. The mixer attributes have to match what the AudioTrack is
 * opened with, so they are re-applied whenever the playing format changes.
 *
 * Everything that touches samples (equalizer, ReplayGain volume) must be off
 * while [state].bitPerfect is true; [PlaybackService] does that.
 */
class BitPerfect(context: Context) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val media = AudioAttributes.Builder().setUsage(AudioAttributes.USAGE_MEDIA).build()
    private val _state = MutableStateFlow(DacState())
    val state: StateFlow<DacState> = _state

    private var enabled = false
    private var sampleRate = 0
    private var encoding = AudioFormat.ENCODING_PCM_16BIT
    private var applied: AudioDeviceInfo? = null
    var onChanged: () -> Unit = {}

    private val devices = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) = refresh()
        override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) = refresh()
    }

    fun start() = audio.registerAudioDeviceCallback(devices, Handler(Looper.getMainLooper()))

    fun stop() {
        audio.unregisterAudioDeviceCallback(devices)
        clear()
    }

    fun setEnabled(on: Boolean) {
        if (enabled == on) return
        enabled = on
        refresh()
    }

    /** Called with the format the sink is about to be opened with. */
    fun onFormat(rate: Int, pcmEncoding: Int) {
        if (rate <= 0 || (rate == sampleRate && pcmEncoding == encoding)) return
        sampleRate = rate
        encoding = pcmEncoding
        refresh()
    }

    private fun usbDac(): AudioDeviceInfo? = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
        .firstOrNull { it.type == AudioDeviceInfo.TYPE_USB_DEVICE || it.type == AudioDeviceInfo.TYPE_USB_HEADSET }

    private fun refresh() {
        val dac = usbDac()
        val before = _state.value
        if (Build.VERSION.SDK_INT < 34 || dac == null) {
            clear()
            _state.value = DacState(device = dac?.productName?.toString(), blockedBy = if (dac != null && Build.VERSION.SDK_INT < 34) "bit-perfect output needs Android 14 or newer; USB exclusive mode would be the way in" else null)
        } else {
            _state.value = apply34(dac)
        }
        if (before.bitPerfect != _state.value.bitPerfect) onChanged()
    }

    private fun label(f: AudioFormat) = "%.1f kHz / %d bit".format(f.sampleRate / 1000.0, bits(f.encoding))

    @androidx.annotation.RequiresApi(34)
    private fun apply34(dac: AudioDeviceInfo): DacState {
        val name = dac.productName?.toString()
        val modes = audio.getSupportedMixerAttributes(dac).filter { it.mixerBehavior == AudioMixerAttributes.MIXER_BEHAVIOR_BIT_PERFECT }
        val labels = modes.map { label(it.format) }
        val playing = if (sampleRate == 0) null else "%.1f kHz / %d bit".format(sampleRate / 1000.0, bits(encoding))
        if (!enabled || modes.isEmpty() || sampleRate == 0) {
            clear()
            return DacState(name, supported = modes.isNotEmpty(), modes = labels, playing = playing)
        }
        val match = modes.firstOrNull { it.format.sampleRate == sampleRate && it.format.encoding == encoding }
        if (match == null) {
            // Either the DAC cannot take this rate at all, or it can but only in a sample format this app
            // cannot write yet: media3's sink emits 16-bit or float, never 24- or 32-bit integer.
            val sameRate = modes.filter { it.format.sampleRate == sampleRate }
            val why = when {
                sameRate.isEmpty() -> "this DAC has no bit-perfect mode at ${playing?.substringBefore(" /")}"
                else -> "this DAC wants ${sameRate.joinToString(" or ") { "${bits(it.format.encoding)} bit" }} at ${playing?.substringBefore(" /")}, which needs the integer output path (not built yet)"
            }
            Log.w("BitPerfect", "no usable bit-perfect mode: $why; offered ${modes.map { it.format }}")
            clear()
            return DacState(name, supported = true, modes = labels, blockedBy = why, playing = playing)
        }
        val ok = runCatching { audio.setPreferredMixerAttributes(media, dac, match) }.getOrDefault(false)
        applied = dac.takeIf { ok }
        return DacState(name, ok, sampleRate, bits(encoding), true, labels, if (ok) null else "the system refused the preferred mixer attributes", playing)
    }

    private fun clear() {
        val dac = applied ?: return
        applied = null
        if (Build.VERSION.SDK_INT >= 34) runCatching { audio.clearPreferredMixerAttributes(media, dac) }
    }

    private fun bits(enc: Int) = when (enc) {
        AudioFormat.ENCODING_PCM_16BIT -> 16
        AudioFormat.ENCODING_PCM_24BIT_PACKED -> 24
        AudioFormat.ENCODING_PCM_32BIT, AudioFormat.ENCODING_PCM_FLOAT -> 32
        else -> 0
    }
}
