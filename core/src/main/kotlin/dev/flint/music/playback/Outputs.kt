package dev.flint.music.playback

import android.content.Context
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioManager
import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

/**
 * Which output the music is going to, as a stable key a sound profile can be bound to: the speaker,
 * wired headphones, each Bluetooth device by name, each USB DAC by name. The callback only fires when
 * something is plugged in or paired, so this costs nothing while music plays.
 */
class Outputs(context: Context) {
    private val audio = context.getSystemService(AudioManager::class.java)
    private val _current = MutableStateFlow(SPEAKER)
    val current: StateFlow<String> = _current
    /** Every output seen so far, so settings can offer them even when unplugged. */
    private val _known = MutableStateFlow(listOf(SPEAKER))
    val known: StateFlow<List<String>> = _known

    /**
     * A USB audio device is attached. Audio offload targets the phone's own DSP: with the stream handed
     * to the chip, a track routed to USB opens without complaint and then plays nothing, which is the
     * "silent DAC" this flag exists to prevent. [PlaybackService] decodes on the CPU while it is true.
     */
    private val _usb = MutableStateFlow(false)
    val usb: StateFlow<Boolean> = _usb

    private val callback = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) = refresh()
        override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) = refresh()
    }

    fun start() {
        // registerAudioDeviceCallback reports every device already attached, so this also fills the
        // initial state: a DAC plugged in before the service started would otherwise go unnoticed.
        audio.registerAudioDeviceCallback(callback, Handler(Looper.getMainLooper()))
    }

    /**
     * Pretend a USB device of this name is attached, so everything that hangs off it - offload standing
     * down, the player's output button, a sound profile bound to that output - can be checked without
     * the hardware. Null hands it back to the audio system.
     */
    fun testUsb(name: String?) {
        override = name
        refresh()
    }

    private var override: String? = null

    fun stop() = audio.unregisterAudioDeviceCallback(callback)

    private fun refresh() {
        val devices = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
        // Android routes media to the most recently attached of these, in this order of precedence.
        val fake = override?.let { "USB: $it" }
        val active = fake ?: devices.minByOrNull { rank(it.type) }?.let(::key) ?: SPEAKER
        _current.value = active
        _known.value = (_known.value + devices.map(::key) + SPEAKER + listOfNotNull(fake)).distinct().sorted()
        _usb.value = fake != null ||
            devices.any { it.type == AudioDeviceInfo.TYPE_USB_DEVICE || it.type == AudioDeviceInfo.TYPE_USB_HEADSET || it.type == AudioDeviceInfo.TYPE_USB_ACCESSORY }
    }

    // Lowest wins. Everything the framework lists that is not one of these - telephony, HDMI, a
    // virtual sink - ranks *below* the built-in speaker rather than above it. It used to rank above,
    // so a phone with a telephony output (which is every phone) reported that as where the music was
    // going, and anything keyed on the current output believed it.
    private fun rank(type: Int) = when (type) {
        AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET -> 0
        AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET -> 1
        AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, AudioDeviceInfo.TYPE_BLE_HEADSET, AudioDeviceInfo.TYPE_BLE_SPEAKER -> 2
        AudioDeviceInfo.TYPE_DOCK, AudioDeviceInfo.TYPE_HDMI, AudioDeviceInfo.TYPE_AUX_LINE -> 3
        AudioDeviceInfo.TYPE_BUILTIN_SPEAKER -> 8
        else -> 9
    }

    private fun key(d: AudioDeviceInfo): String = when (d.type) {
        AudioDeviceInfo.TYPE_BUILTIN_SPEAKER -> SPEAKER
        AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET -> "Wired headphones"
        AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET -> "USB: " + (d.productName?.toString()?.trim().orEmpty().ifEmpty { "DAC" })
        AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, AudioDeviceInfo.TYPE_BLE_HEADSET, AudioDeviceInfo.TYPE_BLE_SPEAKER -> "Bluetooth: " + (d.productName?.toString()?.trim().orEmpty().ifEmpty { "device" })
        else -> d.productName?.toString()?.trim().orEmpty().ifEmpty { "Other output" }
    }

    companion object { const val SPEAKER = "Phone speaker" }
}
