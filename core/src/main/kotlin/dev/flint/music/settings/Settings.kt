package dev.flint.music.settings

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow

enum class ReplayGainMode { OFF, TRACK, ALBUM }

enum class BandKind { PEAKING, LOW_SHELF, HIGH_SHELF }

/** One equalizer filter. The ten default bands are peaking filters an octave apart. */
data class Band(val kind: BandKind, val freq: Float, val gainDb: Float, val q: Float) {
    companion object {
        val GRAPHIC = listOf(31f, 62f, 125f, 250f, 500f, 1000f, 2000f, 4000f, 8000f, 16000f).map { Band(BandKind.PEAKING, it, 0f, 1.41f) }
        fun decode(s: String?): List<Band>? = s?.split(';')?.mapNotNull { b ->
            val p = b.split(':')
            if (p.size != 4) null else runCatching { Band(BandKind.entries[p[0].toInt()], p[1].toFloat(), p[2].toFloat(), p[3].toFloat()) }.getOrNull()
        }?.takeIf { it.isNotEmpty() }
        fun encode(bands: List<Band>) = bands.joinToString(";") { "${it.kind.ordinal}:${it.freq}:${it.gainDb}:${it.q}" }
    }
}

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
    /** Decode on the audio DSP and let the CPU sleep. Only possible while nothing has to touch samples. */
    val offload: Boolean = true,
    /** Ask Android 14+ for an unmixed, unresampled path to a USB DAC. */
    val bitPerfect: Boolean = false,
    /** 32-bit float to the mixer so 24-bit files are not cut to 16. media3 skips audio processors in this mode, so no equalizer. Read when the service starts. */
    val hiRes: Boolean = false,
    val scrobble: Boolean = true,
    /** When the last queued song starts, queue songs similar to it. */
    val autoFill: Boolean = true,
    val eqEnabled: Boolean = false,
    val eqBands: List<Band> = Band.GRAPHIC,
    /** Null: pulled down automatically by the largest boost, so the curve cannot clip. */
    val eqPreampDb: Float? = null,
    /** Headphone crossfeed level in dB; 0 is off. */
    val crossfeedDb: Float = 0f,
    val crossfadeSec: Int = 0,
    val speed: Float = 1f,
    val skipSilence: Boolean = false,
    /** A play counts once this much of the track was heard (or four minutes, whichever comes first). */
    val scrobblePercent: Int = 50,
    val liveSearchDelayMs: Int = 350,
) {
    val loggedIn get() = serverUrl.isNotEmpty() && user.isNotEmpty()

    /** Something in the sample domain is switched on: equalizer or crossfeed. */
    val dsp get() = eqEnabled || crossfeedDb > 0f

    /** What the pre-amp actually is, automatic headroom included. */
    val effectivePreampDb get() = if (!eqEnabled) 0f else eqPreampDb ?: -(eqBands.maxOfOrNull { it.gainDb } ?: 0f).coerceAtLeast(0f)
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
            eqBands = Band.decode(sp.getString("eqBands", null)) ?: d.eqBands,
            eqPreampDb = if (sp.contains("eqPreampDb")) sp.getFloat("eqPreampDb", 0f) else null,
            crossfeedDb = sp.getFloat("crossfeedDb", 0f),
            crossfadeSec = sp.getInt("crossfadeSec", 0),
            speed = sp.getFloat("speed", 1f),
            skipSilence = sp.getBoolean("skipSilence", false),
            scrobblePercent = sp.getInt("scrobblePercent", 50),
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
        putBoolean("eqEnabled", p.eqEnabled); putString("eqBands", Band.encode(p.eqBands))
        if (p.eqPreampDb == null) remove("eqPreampDb") else putFloat("eqPreampDb", p.eqPreampDb)
        putFloat("crossfeedDb", p.crossfeedDb); putInt("crossfadeSec", p.crossfadeSec)
        putFloat("speed", p.speed); putBoolean("skipSilence", p.skipSilence); putInt("scrobblePercent", p.scrobblePercent)
        putInt("liveSearchDelayMs", p.liveSearchDelayMs)
    }.apply()
}
