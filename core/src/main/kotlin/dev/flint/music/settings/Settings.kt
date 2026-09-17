package dev.flint.music.settings

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.json.JSONArray
import org.json.JSONObject

/** AUTO: album gain while the neighbours in the queue are from the same album, track gain otherwise. */
enum class ReplayGainMode { OFF, TRACK, ALBUM, AUTO }

/** What a tap on a song in a list does. */
enum class TapAction { PLAY_LIST, PLAY_ONE, QUEUE, PLAY_NEXT }

/** What dragging a song row sideways does. */
enum class SwipeAction { NONE, QUEUE, PLAY_NEXT, FAVOURITE, DOWNLOAD }

enum class HomeRow(val title: String) { PINNED("Pinned playlists"), RECENT("Recently played"), NEWEST("Recently added"), FREQUENT("Most played"), RANDOM("Random"), STARRED("Favourite albums") }

/**
 * One saved server. Each profile has its own index database, so switching is instant and nothing is re-synced.
 */
data class ServerProfile(
    val id: String,
    val name: String = "",
    val url: String = "",
    /** A second address of the same server (typically the public one); tried when [url] does not answer. */
    val altUrl: String = "",
    val user: String = "",
    val password: String = "",
    /** OpenSubsonic API key; replaces user and password when set. */
    val apiKey: String = "",
    /** Plain `p=enc:` auth for servers without token auth. Found out automatically at login. */
    val legacyAuth: Boolean = false,
    /** Sent with every request: reverse-proxy auth, Cloudflare Access service tokens, basic auth. */
    val headers: Map<String, String> = emptyMap(),
    val allowSelfSigned: Boolean = false,
    /** File name (under files/certs) of an imported PKCS#12 client certificate, for mutual TLS. */
    val clientCert: String = "",
    val clientCertPassword: String = "",
    /** Never talk to this server over a metered network. */
    val wifiOnly: Boolean = false,
    /** Restrict browsing and search to one music folder; empty means all. */
    val musicFolderId: String = "",
    /** Bitrate ceiling while connected through [altUrl]; 0 means none. */
    val altMaxBitRate: Int = 0,
) {
    val label get() = name.ifBlank { url.substringAfter("://").substringBefore('/') }

    fun toJson(): JSONObject = JSONObject().apply {
        put("id", id); put("name", name); put("url", url); put("altUrl", altUrl); put("user", user); put("password", password)
        put("apiKey", apiKey); put("legacyAuth", legacyAuth); put("headers", JSONObject(headers)); put("allowSelfSigned", allowSelfSigned)
        put("clientCert", clientCert); put("clientCertPassword", clientCertPassword); put("wifiOnly", wifiOnly)
        put("musicFolderId", musicFolderId); put("altMaxBitRate", altMaxBitRate)
    }

    companion object {
        fun fromJson(o: JSONObject) = ServerProfile(
            id = o.getString("id"), name = o.optString("name"), url = o.optString("url"), altUrl = o.optString("altUrl"),
            user = o.optString("user"), password = o.optString("password"), apiKey = o.optString("apiKey"), legacyAuth = o.optBoolean("legacyAuth"),
            headers = o.optJSONObject("headers")?.let { h -> h.keys().asSequence().associateWith { h.getString(it) } } ?: emptyMap(),
            allowSelfSigned = o.optBoolean("allowSelfSigned"), clientCert = o.optString("clientCert"), clientCertPassword = o.optString("clientCertPassword"),
            wifiOnly = o.optBoolean("wifiOnly"), musicFolderId = o.optString("musicFolderId"), altMaxBitRate = o.optInt("altMaxBitRate"),
        )
    }
}

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
    val servers: List<ServerProfile> = emptyList(),
    val activeServerId: String = "",
    val wifi: Quality = Quality(),
    val mobile: Quality = Quality(192, "opus"),
    val download: Quality = Quality(),
    val cacheMb: Int = 1024,
    val replayGain: ReplayGainMode = ReplayGainMode.OFF,
    val preampDb: Float = 0f,
    /** Applied to files that carry no ReplayGain tags, so they do not jump out next to tagged ones. */
    val untaggedGainDb: Float = -6f,
    /** Volume ramp on play, pause, seek and manual skip, in milliseconds; 0 is off. Costs nothing between ramps. */
    val fadeMs: Int = 0,
    val pitch: Float = 1f,
    /** "Previous" always goes to the previous track instead of first rewinding the current one. */
    val previousAlwaysSkips: Boolean = false,
    /** Whole tracks fetched ahead into the stream cache, in one go while the radio is already awake. */
    val precacheWifi: Int = 2,
    val precacheMobile: Int = 1,
    /** A track that fails to load is skipped (up to three in a row) instead of stopping playback. */
    val skipOnError: Boolean = true,
    /** No crossfade between two tracks that follow each other on the same album. */
    val crossfadeKeepAlbums: Boolean = true,
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
    val tapAction: TapAction = TapAction.PLAY_LIST,
    val swipeRight: SwipeAction = SwipeAction.QUEUE,
    val swipeLeft: SwipeAction = SwipeAction.PLAY_NEXT,
    /** Songs the server marks explicit are skipped instead of played. */
    val skipExplicit: Boolean = false,
    /** Home shelves, in order; a row that is not listed is hidden. */
    val homeRows: List<HomeRow> = HomeRow.entries,
    val pinnedPlaylists: List<String> = emptyList(),
    /** Remembered per list: sort order, grid or list, filters. Keys are list names. */
    val listPrefs: Map<String, String> = emptyMap(),
) {
    val server: ServerProfile? get() = servers.firstOrNull { it.id == activeServerId }
    val loggedIn get() = server != null
    val serverUrl get() = server?.url.orEmpty()
    val user get() = server?.user.orEmpty()

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

    /** Before profiles existed there was one server in three keys; it becomes the profile "default". */
    private fun servers(): List<ServerProfile> {
        sp.getString("servers", null)?.let { json ->
            return runCatching { JSONArray(json).let { a -> (0 until a.length()).map { ServerProfile.fromJson(a.getJSONObject(it)) } } }.getOrDefault(emptyList())
        }
        val url = sp.getString("serverUrl", "").orEmpty()
        return if (url.isEmpty()) emptyList() else listOf(ServerProfile("default", url = url, user = sp.getString("user", "").orEmpty(), password = sp.getString("password", "").orEmpty()))
    }

    private fun quality(name: String, def: Quality) =
        Quality(sp.getInt("${name}BitRate", def.bitRate), sp.getString("${name}Format", def.format)!!)

    private fun load(): Prefs {
        val d = Prefs()
        return Prefs(
            servers = servers(),
            activeServerId = sp.getString("activeServerId", null) ?: if (sp.getString("serverUrl", "").isNullOrEmpty()) "" else "default",
            wifi = quality("wifi", d.wifi),
            mobile = quality("mobile", d.mobile),
            download = quality("download", d.download),
            cacheMb = sp.getInt("cacheMb", d.cacheMb),
            replayGain = ReplayGainMode.entries[sp.getInt("replayGain", 0).coerceIn(0, 3)],
            preampDb = sp.getFloat("preampDb", 0f),
            untaggedGainDb = sp.getFloat("untaggedGainDb", d.untaggedGainDb), fadeMs = sp.getInt("fadeMs", 0), pitch = sp.getFloat("pitch", 1f),
            previousAlwaysSkips = sp.getBoolean("previousAlwaysSkips", false), precacheWifi = sp.getInt("precacheWifi", d.precacheWifi),
            precacheMobile = sp.getInt("precacheMobile", d.precacheMobile), skipOnError = sp.getBoolean("skipOnError", true),
            crossfadeKeepAlbums = sp.getBoolean("crossfadeKeepAlbums", true),
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
            tapAction = TapAction.entries.getOrElse(sp.getInt("tapAction", 0)) { d.tapAction },
            swipeRight = SwipeAction.entries.getOrElse(sp.getInt("swipeRight", d.swipeRight.ordinal)) { d.swipeRight },
            swipeLeft = SwipeAction.entries.getOrElse(sp.getInt("swipeLeft", d.swipeLeft.ordinal)) { d.swipeLeft },
            skipExplicit = sp.getBoolean("skipExplicit", false),
            homeRows = sp.getString("homeRows", null)?.split(',')?.mapNotNull { n -> HomeRow.entries.firstOrNull { it.name == n } } ?: d.homeRows,
            pinnedPlaylists = sp.getString("pinnedPlaylists", null)?.split('\n')?.filter { it.isNotEmpty() } ?: emptyList(),
            listPrefs = sp.getString("listPrefs", null)?.let { j -> runCatching { JSONObject(j).let { o -> o.keys().asSequence().associateWith { o.getString(it) } } }.getOrNull() } ?: emptyMap(),
        )
    }

    private fun save(p: Prefs) = sp.edit().apply {
        putString("servers", JSONArray(p.servers.map { it.toJson() }).toString()); putString("activeServerId", p.activeServerId)
        remove("serverUrl"); remove("user"); remove("password")
        for ((n, q) in listOf("wifi" to p.wifi, "mobile" to p.mobile, "download" to p.download)) {
            putInt("${n}BitRate", q.bitRate); putString("${n}Format", q.format)
        }
        putInt("cacheMb", p.cacheMb)
        putInt("replayGain", p.replayGain.ordinal); putFloat("preampDb", p.preampDb)
        putFloat("untaggedGainDb", p.untaggedGainDb); putInt("fadeMs", p.fadeMs); putFloat("pitch", p.pitch)
        putBoolean("previousAlwaysSkips", p.previousAlwaysSkips); putInt("precacheWifi", p.precacheWifi); putInt("precacheMobile", p.precacheMobile)
        putBoolean("skipOnError", p.skipOnError); putBoolean("crossfadeKeepAlbums", p.crossfadeKeepAlbums)
        putBoolean("offload", p.offload); putBoolean("bitPerfect", p.bitPerfect); putBoolean("scrobble", p.scrobble); putBoolean("hiRes", p.hiRes); putBoolean("autoFill", p.autoFill)
        putBoolean("eqEnabled", p.eqEnabled); putString("eqBands", Band.encode(p.eqBands))
        if (p.eqPreampDb == null) remove("eqPreampDb") else putFloat("eqPreampDb", p.eqPreampDb)
        putFloat("crossfeedDb", p.crossfeedDb); putInt("crossfadeSec", p.crossfadeSec)
        putFloat("speed", p.speed); putBoolean("skipSilence", p.skipSilence); putInt("scrobblePercent", p.scrobblePercent)
        putInt("liveSearchDelayMs", p.liveSearchDelayMs)
        putInt("tapAction", p.tapAction.ordinal); putInt("swipeRight", p.swipeRight.ordinal); putInt("swipeLeft", p.swipeLeft.ordinal)
        putBoolean("skipExplicit", p.skipExplicit); putString("homeRows", p.homeRows.joinToString(",") { it.name })
        putString("pinnedPlaylists", p.pinnedPlaylists.joinToString("\n")); putString("listPrefs", JSONObject(p.listPrefs).toString())
    }.apply()
}
