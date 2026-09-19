package dev.flint.music.settings

import android.content.Context
import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import org.json.JSONArray
import org.json.JSONObject

/** AUTO: album gain while the neighbours in the queue are from the same album, track gain otherwise. */
enum class ReplayGainMode { OFF, TRACK, ALBUM, AUTO }

enum class ThemeMode { SYSTEM, LIGHT, DARK }

/** What a tap on a song in a list does. */
enum class TapAction { PLAY_LIST, PLAY_ONE, QUEUE, PLAY_NEXT }

/** What dragging a song row sideways does. */
enum class SwipeAction { NONE, QUEUE, PLAY_NEXT, FAVOURITE, DOWNLOAD }

/**
 * What the queue is extended with when the last song starts, and where that comes from. Someone who
 * listens to records wants the next record, not fifteen loose songs, so the two are separate choices:
 * what is added, and what it is chosen by.
 */
enum class AutoFillKind(val label: String) { SONGS("Songs"), ALBUMS("Albums") }

enum class AutoFillBasis(val label: String) {
    SIMILAR("Similar music"), ARTIST("The same artist"), GENRE("The same genre"), ERA("The same era")
}

/**
 * Where the left swipe is stored. It used to default to Play next and be saved along with everything
 * else, so a new key is what gives existing installs the new default (the left swipe favourites).
 */
private const val SWIPE_LEFT = "swipeLeft3"

enum class HomeRow(val title: String) {
    PINNED("Favourite playlists"), PLAYLISTS("Playlists"), RECENT("Recently played"), NEWEST("Recently added"),
    FREQUENT("Most played albums"), TOP_SONGS("Most played songs"), RANDOM("Random"), STARRED("Favourite albums"),
}

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

/** Mirrors `EqKind` in the core; the ordinals are the wire format, so the order must not change. */
enum class BandKind(val label: String, val usesGain: Boolean = true) {
    PEAKING("Peak"), LOW_SHELF("Low shelf"), HIGH_SHELF("High shelf"),
    LOW_PASS("Low pass", false), HIGH_PASS("High pass", false), BAND_PASS("Band pass", false),
    NOTCH("Notch", false), ALL_PASS("All pass", false),
    LOW_SHELF_SLOPE("Low shelf (slope)"), HIGH_SHELF_SLOPE("High shelf (slope)"),
}

/** Which side a band applies to. */
enum class BandChannel(val label: String) { BOTH("Both"), LEFT("Left"), RIGHT("Right") }

/** One equalizer filter. The ten default bands are peaking filters an octave apart. */
data class Band(val kind: BandKind, val freq: Float, val gainDb: Float, val q: Float, val channel: BandChannel = BandChannel.BOTH) {
    companion object {
        val GRAPHIC = listOf(31f, 62f, 125f, 250f, 500f, 1000f, 2000f, 4000f, 8000f, 16000f).map { Band(BandKind.PEAKING, it, 0f, 1.41f) }
        fun decode(s: String?): List<Band>? = s?.split(';')?.mapNotNull { b ->
            val p = b.split(':')
            if (p.size < 4) null else runCatching {
                Band(BandKind.entries[p[0].toInt()], p[1].toFloat(), p[2].toFloat(), p[3].toFloat(), BandChannel.entries[p.getOrNull(4)?.toInt() ?: 0])
            }.getOrNull()
        }?.takeIf { it.isNotEmpty() }
        fun encode(bands: List<Band>) = bands.joinToString(";") { "${it.kind.ordinal}:${it.freq}:${it.gainDb}:${it.q}:${it.channel.ordinal}" }
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
    /** Songs downloaded at the same time, 1 to 10; the rest wait their turn in the order they were asked for. */
    val parallelDownloads: Int = 5,
    /**
     * Covers of the songs coming up in the queue, fetched into the cache before they are shown, so a
     * skip or a swipe lands on a picture that is already there. 0 to 10; the one before is always kept.
     */
    val coversAhead: Int = 3,
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
    /** When the last queued song starts, keep the music going past the end of the queue. */
    val autoFill: Boolean = true,
    /** Songs, or one whole album at a time, queued in its own order. */
    val autoFillKind: AutoFillKind = AutoFillKind.SONGS,
    /** What the next songs are chosen by: what the server thinks is similar, or the artist, genre or decade. */
    val autoFillBasis: AutoFillBasis = AutoFillBasis.SIMILAR,
    val eqEnabled: Boolean = false,
    val eqBands: List<Band> = Band.GRAPHIC,
    /** Null: pulled down automatically by the largest boost, so the curve cannot clip. */
    val eqPreampDb: Float? = null,
    /** Headphone crossfeed level in dB; 0 is off. */
    val crossfeedDb: Float = 0f,
    /** −1 hard left, 0 centre, +1 hard right. */
    val balance: Float = 0f,
    val mono: Boolean = false,
    /**
     * Catches what the pre-amp, the equalizer and a positive ReplayGain would otherwise clip. Costs a few
     * milliseconds of delay, so it is opt-in; below its threshold the samples come through untouched.
     */
    val limiter: Boolean = false,
    val limiterThresholdDb: Float = -1f,
    val crossfadeSec: Int = 0,
    /**
     * AutoMix: transitions planned from each track's analysed tempo, beats and cue points, like Apple Music's.
     * Analysis runs on audio being played anyway, once per track; a transition costs a few percent of a core
     * for its own few seconds. Off by default.
     */
    val autoMix: Boolean = false,
    val autoMixMaxS: Int = 12,
    val autoMixBeatMatch: Boolean = true,
    val autoMixMaxTempoPct: Float = 6f,
    val autoMixBassSwap: Boolean = true,
    val autoMixFilters: Boolean = true,
    /** Off: tempo is matched by changing speed and pitch together (cheaper, and within 2 % inaudible). */
    val autoMixKeepPitch: Boolean = true,
    val speed: Float = 1f,
    val skipSilence: Boolean = false,
    /** A play counts once this much of the track was heard (or four minutes, whichever comes first). */
    val scrobblePercent: Int = 50,
    val liveSearchDelayMs: Int = 350,
    // ---- optional subsystems; one that is off is never initialised and costs nothing ----
    /** Keeps a local play history and a taste score per song; feeds mixes, smart playlists and the year in review. */
    val tasteModel: Boolean = true,
    /** Third-party lookups: lyrics from LRCLIB, the AutoEQ headphone list, update checks. */
    val thirdPartyLookups: Boolean = false,
    /** Apply the profile bound to an output device when that device becomes the active one. */
    val profilePerOutput: Boolean = true,
    /**
     * Headphones connected with nothing chosen for them and a matching AutoEQ curve: use that curve and
     * remember it for the device, instead of asking first. Off asks. Fetches one small preset per new device.
     */
    val autoEqAuto: Boolean = false,
    /** "Shuffle" spreads artists and albums apart instead of being purely random. */
    val weightedShuffle: Boolean = true,
    /** The sung part of the current lyric line fills in word by word. Redraws one line of text per frame, only while the lyrics are on screen. */
    val lyricsSweep: Boolean = true,
    val lyricsKeepScreenOn: Boolean = true,
    val lyricsTranslation: Boolean = true,
    /** 0 small, 1 medium, 2 large. */
    val lyricsSize: Int = 1,
    /**
     * Ask LRCLIB when the server has no synced lyrics. Needs [thirdPartyLookups]. Off means only the
     * server's lyrics (octo-fiesta already asks LRCLIB itself for tracks it serves from a provider).
     */
    val lyricsLrclib: Boolean = true,
    // ---- look ----
    val theme: ThemeMode = ThemeMode.SYSTEM,
    /** Pure black backgrounds in dark mode: OLED pixels are off, which saves power as well as looking right. */
    val amoled: Boolean = false,
    /**
     * With AMOLED black on, the full-screen player still wears the cover's colours, the way Apple Music's
     * does - it is one page about one record, and a sleeve dropping straight into black reads as cut
     * off. Off keeps that screen black as well.
     */
    val playerColours: Boolean = true,
    /** Android 12+ wallpaper colours; off uses [accent]. */
    val dynamicColor: Boolean = true,
    /** ARGB seed colour when dynamic colour is off or unavailable. */
    val accent: Long = 0xFF6750A4,
    /** Album, artist and playlist pages take their colour from the cover, which runs edge to edge at the top. */
    val coverColors: Boolean = true,
    /** Shorter, plainer movement everywhere; also follows the system when animations are off there. */
    val reduceMotion: Boolean = false,
    /**
     * Animate even though Android's own animations are switched off. That switch is as often a speed
     * habit as an accessibility need, and with it off every Compose animation is scaled to nothing -
     * the lyrics then jump from line to line whatever this app asks for. On, the app's own movement
     * runs at its real speed regardless; Reduce motion above still turns it off.
     */
    val ignoreSystemMotion: Boolean = false,
    /**
     * How big the interface is drawn. 0 is automatic: laid out as if the screen were at least as wide
     * as the one every size was measured against, so a phone set to a large display size does not
     * blow the layout up. Anything else is a fixed factor on top of the system's own size.
     */
    val uiScale: Float = 0f,
    val tapAction: TapAction = TapAction.PLAY_LIST,
    val swipeRight: SwipeAction = SwipeAction.QUEUE,
    val swipeLeft: SwipeAction = SwipeAction.FAVOURITE,
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

    /** Something in the sample domain is switched on. Anything here stops audio offload but not burst playback. */
    val dsp get() = eqEnabled || crossfeedDb > 0f || balance != 0f || mono || limiter

    /** What the pre-amp actually is, automatic headroom included. */
    val effectivePreampDb get() = if (!eqEnabled) 0f else eqPreampDb ?: -(eqBands.filter { it.kind.usesGain }.maxOfOrNull { it.gainDb } ?: 0f).coerceAtLeast(0f)
}

/**
 * SharedPreferences rather than DataStore: the playback service needs its
 * settings synchronously on start, and this is one small file read once.
 */
/** The part of [Prefs] a sound profile remembers. */
data class Sound(
    val eqEnabled: Boolean, val eqBands: List<Band>, val eqPreampDb: Float?, val crossfeedDb: Float,
    val balance: Float, val mono: Boolean, val limiter: Boolean, val limiterThresholdDb: Float,
    val replayGain: ReplayGainMode, val preampDb: Float, val crossfadeSec: Int, val hiRes: Boolean, val bitPerfect: Boolean,
) {
    fun toJson(): String = JSONObject().apply {
        put("eqEnabled", eqEnabled); put("eqBands", Band.encode(eqBands)); eqPreampDb?.let { put("eqPreampDb", it.toDouble()) }
        put("crossfeedDb", crossfeedDb.toDouble()); put("balance", balance.toDouble()); put("mono", mono)
        put("limiter", limiter); put("limiterThresholdDb", limiterThresholdDb.toDouble())
        put("replayGain", replayGain.ordinal); put("preampDb", preampDb.toDouble()); put("crossfadeSec", crossfadeSec)
        put("hiRes", hiRes); put("bitPerfect", bitPerfect)
    }.toString()

    companion object {
        fun of(p: Prefs) = Sound(p.eqEnabled, p.eqBands, p.eqPreampDb, p.crossfeedDb, p.balance, p.mono, p.limiter, p.limiterThresholdDb, p.replayGain, p.preampDb, p.crossfadeSec, p.hiRes, p.bitPerfect)

        fun fromJson(json: String): Sound? = runCatching {
            val o = JSONObject(json)
            val d = Prefs()
            Sound(
                o.optBoolean("eqEnabled"), Band.decode(o.optString("eqBands")) ?: d.eqBands,
                if (o.has("eqPreampDb")) o.getDouble("eqPreampDb").toFloat() else null,
                o.optDouble("crossfeedDb", 0.0).toFloat(), o.optDouble("balance", 0.0).toFloat(), o.optBoolean("mono"),
                o.optBoolean("limiter"), o.optDouble("limiterThresholdDb", -1.0).toFloat(),
                ReplayGainMode.entries[o.optInt("replayGain").coerceIn(0, 3)], o.optDouble("preampDb", 0.0).toFloat(),
                o.optInt("crossfadeSec"), o.optBoolean("hiRes"), o.optBoolean("bitPerfect"),
            )
        }.getOrNull()
    }
}

fun Prefs.withSound(s: Sound) = copy(
    eqEnabled = s.eqEnabled, eqBands = s.eqBands, eqPreampDb = s.eqPreampDb, crossfeedDb = s.crossfeedDb,
    balance = s.balance, mono = s.mono, limiter = s.limiter, limiterThresholdDb = s.limiterThresholdDb,
    replayGain = s.replayGain, preampDb = s.preampDb, crossfadeSec = s.crossfadeSec, hiRes = s.hiRes, bitPerfect = s.bitPerfect,
)

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
            parallelDownloads = sp.getInt("parallelDownloads", d.parallelDownloads).coerceIn(1, 10),
            coversAhead = sp.getInt("coversAhead", d.coversAhead).coerceIn(0, 10),
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
            autoFillKind = AutoFillKind.entries.getOrElse(sp.getInt("autoFillKind", 0)) { d.autoFillKind },
            autoFillBasis = AutoFillBasis.entries.getOrElse(sp.getInt("autoFillBasis", 0)) { d.autoFillBasis },
            eqEnabled = sp.getBoolean("eqEnabled", false),
            eqBands = Band.decode(sp.getString("eqBands", null)) ?: d.eqBands,
            eqPreampDb = if (sp.contains("eqPreampDb")) sp.getFloat("eqPreampDb", 0f) else null,
            crossfeedDb = sp.getFloat("crossfeedDb", 0f), balance = sp.getFloat("balance", 0f), mono = sp.getBoolean("mono", false),
            limiter = sp.getBoolean("limiter", false), limiterThresholdDb = sp.getFloat("limiterThresholdDb", -1f),
            crossfadeSec = sp.getInt("crossfadeSec", 0),
            autoMix = sp.getBoolean("autoMix", false), autoMixMaxS = sp.getInt("autoMixMaxS", 12), autoMixBeatMatch = sp.getBoolean("autoMixBeatMatch", true),
            autoMixMaxTempoPct = sp.getFloat("autoMixMaxTempoPct", 6f), autoMixBassSwap = sp.getBoolean("autoMixBassSwap", true),
            autoMixFilters = sp.getBoolean("autoMixFilters", true), autoMixKeepPitch = sp.getBoolean("autoMixKeepPitch", true),
            speed = sp.getFloat("speed", 1f),
            skipSilence = sp.getBoolean("skipSilence", false),
            scrobblePercent = sp.getInt("scrobblePercent", 50),
            liveSearchDelayMs = sp.getInt("liveSearchDelayMs", d.liveSearchDelayMs),
            profilePerOutput = sp.getBoolean("profilePerOutput", true), autoEqAuto = sp.getBoolean("autoEqAuto", false),
            tasteModel = sp.getBoolean("tasteModel", true), thirdPartyLookups = sp.getBoolean("thirdPartyLookups", false), weightedShuffle = sp.getBoolean("weightedShuffle", true),
            lyricsSweep = sp.getBoolean("lyricsSweep", true), lyricsKeepScreenOn = sp.getBoolean("lyricsKeepScreenOn", true), lyricsTranslation = sp.getBoolean("lyricsTranslation", true), lyricsSize = sp.getInt("lyricsSize", 1), lyricsLrclib = sp.getBoolean("lyricsLrclib", true),
            theme = ThemeMode.entries.getOrElse(sp.getInt("theme", 0)) { ThemeMode.SYSTEM }, amoled = sp.getBoolean("amoled", false),
            dynamicColor = sp.getBoolean("dynamicColor", true), accent = sp.getLong("accent", 0xFF6750A4), coverColors = sp.getBoolean("coverColors", true), reduceMotion = sp.getBoolean("reduceMotion", false), ignoreSystemMotion = sp.getBoolean("ignoreSystemMotion", false), uiScale = sp.getFloat("uiScale", 0f), playerColours = sp.getBoolean("playerColours", true),
            tapAction = TapAction.entries.getOrElse(sp.getInt("tapAction", 0)) { d.tapAction },
            swipeRight = SwipeAction.entries.getOrElse(sp.getInt("swipeRight", d.swipeRight.ordinal)) { d.swipeRight },
            swipeLeft = SwipeAction.entries.getOrElse(sp.getInt(SWIPE_LEFT, d.swipeLeft.ordinal)) { d.swipeLeft },
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
        putInt("cacheMb", p.cacheMb); putInt("parallelDownloads", p.parallelDownloads); putInt("coversAhead", p.coversAhead)
        putInt("replayGain", p.replayGain.ordinal); putFloat("preampDb", p.preampDb)
        putFloat("untaggedGainDb", p.untaggedGainDb); putInt("fadeMs", p.fadeMs); putFloat("pitch", p.pitch)
        putBoolean("previousAlwaysSkips", p.previousAlwaysSkips); putInt("precacheWifi", p.precacheWifi); putInt("precacheMobile", p.precacheMobile)
        putBoolean("skipOnError", p.skipOnError); putBoolean("crossfadeKeepAlbums", p.crossfadeKeepAlbums)
        putBoolean("offload", p.offload); putBoolean("bitPerfect", p.bitPerfect); putBoolean("scrobble", p.scrobble); putBoolean("hiRes", p.hiRes); putBoolean("autoFill", p.autoFill)
        putInt("autoFillKind", p.autoFillKind.ordinal); putInt("autoFillBasis", p.autoFillBasis.ordinal)
        putBoolean("eqEnabled", p.eqEnabled); putString("eqBands", Band.encode(p.eqBands))
        if (p.eqPreampDb == null) remove("eqPreampDb") else putFloat("eqPreampDb", p.eqPreampDb)
        putFloat("crossfeedDb", p.crossfeedDb); putFloat("balance", p.balance); putBoolean("mono", p.mono)
        putBoolean("limiter", p.limiter); putFloat("limiterThresholdDb", p.limiterThresholdDb)
        run { }; putInt("crossfadeSec", p.crossfadeSec)
        putBoolean("autoMix", p.autoMix); putInt("autoMixMaxS", p.autoMixMaxS); putBoolean("autoMixBeatMatch", p.autoMixBeatMatch)
        putFloat("autoMixMaxTempoPct", p.autoMixMaxTempoPct); putBoolean("autoMixBassSwap", p.autoMixBassSwap)
        putBoolean("autoMixFilters", p.autoMixFilters); putBoolean("autoMixKeepPitch", p.autoMixKeepPitch)
        putFloat("speed", p.speed); putBoolean("skipSilence", p.skipSilence); putInt("scrobblePercent", p.scrobblePercent)
        putInt("liveSearchDelayMs", p.liveSearchDelayMs)
        putBoolean("profilePerOutput", p.profilePerOutput); putBoolean("autoEqAuto", p.autoEqAuto); putBoolean("tasteModel", p.tasteModel); putBoolean("thirdPartyLookups", p.thirdPartyLookups); putBoolean("weightedShuffle", p.weightedShuffle)
        putBoolean("lyricsSweep", p.lyricsSweep); putBoolean("lyricsKeepScreenOn", p.lyricsKeepScreenOn); putBoolean("lyricsTranslation", p.lyricsTranslation); putInt("lyricsSize", p.lyricsSize); putBoolean("lyricsLrclib", p.lyricsLrclib)
        putInt("theme", p.theme.ordinal); putBoolean("amoled", p.amoled); putBoolean("dynamicColor", p.dynamicColor); putLong("accent", p.accent); putBoolean("coverColors", p.coverColors); putBoolean("reduceMotion", p.reduceMotion); putBoolean("ignoreSystemMotion", p.ignoreSystemMotion); putFloat("uiScale", p.uiScale); putBoolean("playerColours", p.playerColours)
        putInt("tapAction", p.tapAction.ordinal); putInt("swipeRight", p.swipeRight.ordinal); putInt(SWIPE_LEFT, p.swipeLeft.ordinal)
        putBoolean("skipExplicit", p.skipExplicit); putString("homeRows", p.homeRows.joinToString(",") { it.name })
        putString("pinnedPlaylists", p.pinnedPlaylists.joinToString("\n")); putString("listPrefs", JSONObject(p.listPrefs).toString())
    }.apply()
}
