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

    private val callback = object : AudioDeviceCallback() {
        override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>) = refresh()
        override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>) = refresh()
    }

    fun start() {
        audio.registerAudioDeviceCallback(callback, Handler(Looper.getMainLooper()))
    }

    fun stop() = audio.unregisterAudioDeviceCallback(callback)

    private fun refresh() {
        val devices = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS)
        // Android routes media to the most recently attached of these, in this order of precedence.
        val active = devices.minByOrNull { rank(it.type) }?.let(::key) ?: SPEAKER
        _current.value = active
        _known.value = (_known.value + devices.map(::key) + SPEAKER).distinct().sorted()
    }

    private fun rank(type: Int) = when (type) {
        AudioDeviceInfo.TYPE_USB_DEVICE, AudioDeviceInfo.TYPE_USB_HEADSET -> 0
        AudioDeviceInfo.TYPE_WIRED_HEADPHONES, AudioDeviceInfo.TYPE_WIRED_HEADSET -> 1
        AudioDeviceInfo.TYPE_BLUETOOTH_A2DP, AudioDeviceInfo.TYPE_BLE_HEADSET, AudioDeviceInfo.TYPE_BLE_SPEAKER -> 2
        AudioDeviceInfo.TYPE_BUILTIN_SPEAKER -> 9
        else -> 5
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
