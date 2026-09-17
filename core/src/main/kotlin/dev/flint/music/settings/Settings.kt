package dev.flint.music.settings

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

enum class ReplayGainMode { OFF, TRACK, ALBUM }

/** One stream quality: [bitRate] 0 and empty [format] mean the original file. */
data class Quality(val bitRate: Int = 0, val format: String = "") {
    val key get() = "$bitRate$format"
}

data class Prefs(
    val serverUrl: String = "",
    val user: String = "",
    val password: String = "",
    val wifi: Quality = Quality(),
    val mobile: Quality = Quality(192, "opus"),
    val download: Quality = Quality(),
    val cacheMb: Int = 1024,
    val replayGain: ReplayGainMode = ReplayGainMode.OFF,
    val preampDb: Float = 0f,
    /** Decode on the audio DSP and let the CPU sleep. Off whenever the equalizer is on. */
    val offload: Boolean = true,
    /** Ask Android 14+ for an unmixed, unresampled path to a USB DAC. */
    val bitPerfect: Boolean = false,
    /** 32-bit float to the mixer so 24-bit files are not cut to 16. media3 skips audio processors in this mode, so no equalizer. Read when the service starts. */
    val hiRes: Boolean = false,
    val scrobble: Boolean = true,
    /** When the last queued song starts, queue songs similar to it. */
    val autoFill: Boolean = true,
    val eqEnabled: Boolean = false,
    /** Gains in dB for [dev.flint.music.playback.Equalizer.FREQUENCIES]. */
    val eqGains: List<Float> = List(10) { 0f },
    val liveSearchDelayMs: Int = 350,
) {
    val loggedIn get() = serverUrl.isNotEmpty() && user.isNotEmpty()
}

/**
 * SharedPreferences rather than DataStore: the playback service needs its
 * settings synchronously on start, and this is one small file read once.
 */
class Settings(context: Context) {
    private val sp: SharedPreferences = context.getSharedPreferences("flint", Context.MODE_PRIVATE)
    private val state = MutableStateFlow(load())
    val prefs: StateFlow<Prefs> = state
    val value get() = state.value

    fun update(change: (Prefs) -> Prefs) {
        val next = change(state.value)
        if (next == state.value) return
        state.value = next
        save(next)
    }

    private fun quality(name: String, def: Quality) =
        Quality(sp.getInt("${name}BitRate", def.bitRate), sp.getString("${name}Format", def.format)!!)

    private fun load(): Prefs {
        val d = Prefs()
        return Prefs(
            serverUrl = sp.getString("serverUrl", "")!!,
            user = sp.getString("user", "")!!,
            password = sp.getString("password", "")!!,
            wifi = quality("wifi", d.wifi),
            mobile = quality("mobile", d.mobile),
            download = quality("download", d.download),
            cacheMb = sp.getInt("cacheMb", d.cacheMb),
            replayGain = ReplayGainMode.entries[sp.getInt("replayGain", 0).coerceIn(0, 2)],
            preampDb = sp.getFloat("preampDb", 0f),
            offload = sp.getBoolean("offload", true),
            bitPerfect = sp.getBoolean("bitPerfect", false),
            hiRes = sp.getBoolean("hiRes", false),
            scrobble = sp.getBoolean("scrobble", true),
            autoFill = sp.getBoolean("autoFill", true),
            eqEnabled = sp.getBoolean("eqEnabled", false),
            eqGains = sp.getString("eqGains", null)?.split(',')?.mapNotNull { it.toFloatOrNull() }?.takeIf { it.size == 10 } ?: d.eqGains,
            liveSearchDelayMs = sp.getInt("liveSearchDelayMs", d.liveSearchDelayMs),
        )
    }

    private fun save(p: Prefs) = sp.edit().apply {
        putString("serverUrl", p.serverUrl); putString("user", p.user); putString("password", p.password)
        for ((n, q) in listOf("wifi" to p.wifi, "mobile" to p.mobile, "download" to p.download)) {
            putInt("${n}BitRate", q.bitRate); putString("${n}Format", q.format)
        }
        putInt("cacheMb", p.cacheMb)
        putInt("replayGain", p.replayGain.ordinal); putFloat("preampDb", p.preampDb)
        putBoolean("offload", p.offload); putBoolean("bitPerfect", p.bitPerfect); putBoolean("scrobble", p.scrobble); putBoolean("hiRes", p.hiRes); putBoolean("autoFill", p.autoFill)
        putBoolean("eqEnabled", p.eqEnabled); putString("eqGains", p.eqGains.joinToString(","))
        putInt("liveSearchDelayMs", p.liveSearchDelayMs)
    }.apply()
}
