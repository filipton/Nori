package dev.nori.music.settings

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import dev.nori.music.ffi.PrefValue
import dev.nori.music.ffi.SavedQuality
import dev.nori.music.ffi.SavedServer
import dev.nori.music.ffi.SoundBand
import dev.nori.music.ffi.SoundSettings
import dev.nori.music.ffi.StoredPrefs
import dev.nori.music.ffi.eqGraphic
import dev.nori.music.ffi.serverLabel
import dev.nori.music.ffi.dbFileName
import dev.nori.music.ffi.settingsOpen
import dev.nori.music.ffi.settingsPut
import kotlinx.coroutines.flow.StateFlow

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
    /** Its name, or else the host of its address (see the core's `settings::label`). Worked out once, when first shown. */
    val label: String by lazy { serverLabel(name, url) }
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
        /** The ten graphic bands, from the core so the numbers live in one place. */
        val GRAPHIC: List<Band> by lazy { eqGraphic().map { it.band() } }
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
    /**
     * Server gone mid-evening and the next song is not downloaded: keep playing from full downloads
     * until the network is back, then resume the parked queue. Off by default; costs nothing until it
     * engages (no network listener until then).
     */
    val bridgeOffline: Boolean = false,
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
    val autoMixEchoOut: Boolean = true,
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
    /** The bottom of the player's cover goes blurred before it melts into the page. One blur pass per frame while the cover moves. */
    val softSleeve: Boolean = true,
    /** A short message when something is favourited or unfavourited. The heart itself always changes. */
    val favouriteNotice: Boolean = true,
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

    /**
     * Something in the sample domain is switched on. Anything here stops audio offload but not burst playback.
     * The rule is nori_player::sound::sound_on, worked out once per settings.
     */
    val dsp: Boolean by lazy { dev.nori.music.playback.Dsp.soundOn(eqEnabled, crossfeedDb, balance, mono, limiter) }

    /** The pre-amp in effect: the one set, or the automatic one (nori_player::dsp::auto_preamp_db), worked out once per settings. */
    val effectivePreampDb: Float by lazy {
        if (!eqEnabled) 0f
        else eqPreampDb ?: dev.nori.music.playback.Dsp.autoPreampDb(IntArray(eqBands.size) { eqBands[it].kind.ordinal }, FloatArray(eqBands.size) { eqBands[it].gainDb })
    }
}

/** The part of [Prefs] a sound profile remembers; its JSON is read and written by the core (`settings.rs`). */
typealias Sound = SoundSettings

fun Band.stored() = SoundBand(kind = kind.ordinal, freq = freq, gainDb = gainDb, q = q, channel = channel.ordinal)
fun SoundBand.band() = Band(BandKind.entries[kind], freq, gainDb, q, BandChannel.entries[channel])

fun Prefs.sound() = Sound(
    eqEnabled = eqEnabled, eqBands = eqBands.map { it.stored() }, eqPreampDb = eqPreampDb, crossfeedDb = crossfeedDb,
    balance = balance, mono = mono, limiter = limiter, limiterThresholdDb = limiterThresholdDb,
    replayGain = replayGain.ordinal, preampDb = preampDb, crossfadeSec = crossfadeSec, hiRes = hiRes, bitPerfect = bitPerfect,
)

fun Prefs.withSound(s: Sound) = copy(
    eqEnabled = s.eqEnabled, eqBands = s.eqBands.map { it.band() }, eqPreampDb = s.eqPreampDb, crossfeedDb = s.crossfeedDb,
    balance = s.balance, mono = s.mono, limiter = s.limiter, limiterThresholdDb = s.limiterThresholdDb,
    replayGain = ReplayGainMode.entries[s.replayGain], preampDb = s.preampDb, crossfadeSec = s.crossfadeSec, hiRes = s.hiRes, bitPerfect = s.bitPerfect,
)

private fun ServerProfile.stored() = SavedServer(
    id = id, name = name, url = url, altUrl = altUrl, user = user, password = password, apiKey = apiKey, legacyAuth = legacyAuth,
    headers = headers, allowSelfSigned = allowSelfSigned, clientCert = clientCert, clientCertPassword = clientCertPassword,
    wifiOnly = wifiOnly, musicFolderId = musicFolderId, altMaxBitRate = altMaxBitRate,
)

private fun SavedServer.profile() = ServerProfile(
    id = id, name = name, url = url, altUrl = altUrl, user = user, password = password, apiKey = apiKey, legacyAuth = legacyAuth,
    headers = headers, allowSelfSigned = allowSelfSigned, clientCert = clientCert, clientCertPassword = clientCertPassword,
    wifiOnly = wifiOnly, musicFolderId = musicFolderId, altMaxBitRate = altMaxBitRate,
)

private fun Quality.stored() = SavedQuality(bitRate, format)
private fun SavedQuality.quality() = Quality(bitRate, format)

/** The settings as the core checks and stores them. */
fun Prefs.stored() = StoredPrefs(
    servers = servers.map { it.stored() }, activeServerId = activeServerId, wifi = wifi.stored(), mobile = mobile.stored(), download = download.stored(),
    parallelDownloads = parallelDownloads, coversAhead = coversAhead, cacheMb = cacheMb, replayGain = replayGain.ordinal, preampDb = preampDb,
    untaggedGainDb = untaggedGainDb, fadeMs = fadeMs, pitch = pitch, previousAlwaysSkips = previousAlwaysSkips, precacheWifi = precacheWifi,
    precacheMobile = precacheMobile, skipOnError = skipOnError, crossfadeKeepAlbums = crossfadeKeepAlbums, offload = offload, bitPerfect = bitPerfect,
    hiRes = hiRes, scrobble = scrobble, autoFill = autoFill, bridgeOffline = bridgeOffline, autoFillKind = autoFillKind.ordinal,
    autoFillBasis = autoFillBasis.ordinal, eqEnabled = eqEnabled, eqBands = eqBands.map { it.stored() }, eqPreampDb = eqPreampDb,
    crossfeedDb = crossfeedDb, balance = balance, mono = mono, limiter = limiter, limiterThresholdDb = limiterThresholdDb, crossfadeSec = crossfadeSec,
    autoMix = autoMix, autoMixMaxS = autoMixMaxS, autoMixBeatMatch = autoMixBeatMatch, autoMixMaxTempoPct = autoMixMaxTempoPct,
    autoMixBassSwap = autoMixBassSwap, autoMixFilters = autoMixFilters, autoMixEchoOut = autoMixEchoOut, autoMixKeepPitch = autoMixKeepPitch,
    speed = speed, skipSilence = skipSilence, scrobblePercent = scrobblePercent, liveSearchDelayMs = liveSearchDelayMs, tasteModel = tasteModel,
    thirdPartyLookups = thirdPartyLookups, profilePerOutput = profilePerOutput, autoEqAuto = autoEqAuto, weightedShuffle = weightedShuffle,
    lyricsSweep = lyricsSweep, softSleeve = softSleeve, favouriteNotice = favouriteNotice, lyricsKeepScreenOn = lyricsKeepScreenOn,
    lyricsTranslation = lyricsTranslation, lyricsSize = lyricsSize, lyricsLrclib = lyricsLrclib, theme = theme.ordinal, amoled = amoled,
    playerColours = playerColours, dynamicColor = dynamicColor, accent = accent, coverColors = coverColors, reduceMotion = reduceMotion,
    ignoreSystemMotion = ignoreSystemMotion, uiScale = uiScale, tapAction = tapAction.ordinal, swipeRight = swipeRight.ordinal,
    swipeLeft = swipeLeft.ordinal, skipExplicit = skipExplicit, homeRows = homeRows.map { it.ordinal }, pinnedPlaylists = pinnedPlaylists,
    listPrefs = listPrefs,
)

/** The core's checked settings back as [Prefs]; every ordinal in them is in range. */
fun StoredPrefs.prefs() = Prefs(
    servers = servers.map { it.profile() }, activeServerId = activeServerId, wifi = wifi.quality(), mobile = mobile.quality(), download = download.quality(),
    parallelDownloads = parallelDownloads, coversAhead = coversAhead, cacheMb = cacheMb, replayGain = ReplayGainMode.entries[replayGain], preampDb = preampDb,
    untaggedGainDb = untaggedGainDb, fadeMs = fadeMs, pitch = pitch, previousAlwaysSkips = previousAlwaysSkips, precacheWifi = precacheWifi,
    precacheMobile = precacheMobile, skipOnError = skipOnError, crossfadeKeepAlbums = crossfadeKeepAlbums, offload = offload, bitPerfect = bitPerfect,
    hiRes = hiRes, scrobble = scrobble, autoFill = autoFill, bridgeOffline = bridgeOffline, autoFillKind = AutoFillKind.entries[autoFillKind],
    autoFillBasis = AutoFillBasis.entries[autoFillBasis], eqEnabled = eqEnabled, eqBands = eqBands.map { it.band() }, eqPreampDb = eqPreampDb,
    crossfeedDb = crossfeedDb, balance = balance, mono = mono, limiter = limiter, limiterThresholdDb = limiterThresholdDb, crossfadeSec = crossfadeSec,
    autoMix = autoMix, autoMixMaxS = autoMixMaxS, autoMixBeatMatch = autoMixBeatMatch, autoMixMaxTempoPct = autoMixMaxTempoPct,
    autoMixBassSwap = autoMixBassSwap, autoMixFilters = autoMixFilters, autoMixEchoOut = autoMixEchoOut, autoMixKeepPitch = autoMixKeepPitch,
    speed = speed, skipSilence = skipSilence, scrobblePercent = scrobblePercent, liveSearchDelayMs = liveSearchDelayMs, tasteModel = tasteModel,
    thirdPartyLookups = thirdPartyLookups, profilePerOutput = profilePerOutput, autoEqAuto = autoEqAuto, weightedShuffle = weightedShuffle,
    lyricsSweep = lyricsSweep, softSleeve = softSleeve, favouriteNotice = favouriteNotice, lyricsKeepScreenOn = lyricsKeepScreenOn,
    lyricsTranslation = lyricsTranslation, lyricsSize = lyricsSize, lyricsLrclib = lyricsLrclib, theme = ThemeMode.entries[theme], amoled = amoled,
    playerColours = playerColours, dynamicColor = dynamicColor, accent = accent, coverColors = coverColors, reduceMotion = reduceMotion,
    ignoreSystemMotion = ignoreSystemMotion, uiScale = uiScale, tapAction = TapAction.entries[tapAction], swipeRight = SwipeAction.entries[swipeRight],
    swipeLeft = SwipeAction.entries[swipeLeft], skipExplicit = skipExplicit, homeRows = homeRows.map { HomeRow.entries[it] }, pinnedPlaylists = pinnedPlaylists,
    listPrefs = listPrefs,
)

/**
 * The settings are the core's (`settings_store.rs`): it reads them once, keeps them and writes them to
 * the app's database whenever they change. The playback service needs them synchronously on start,
 * and this is a few rows read once. SharedPreferences is only read, the first time, to carry over what
 * was kept there before.
 */
class Settings(private val context: Context) {
    private val state = MutableStateFlow(load())
    val prefs: StateFlow<Prefs> = state
    val value get() = state.value

    fun update(change: (Prefs) -> Prefs) {
        val next = change(state.value)
        if (next == state.value) return
        // The core first: whatever reacts to the new value (on any thread) reads it from there.
        settingsPut(next.stored())
        state.value = next
    }

    private fun load(): Prefs {
        val raw = HashMap<String, PrefValue>()
        for ((k, v) in context.getSharedPreferences("nori", Context.MODE_PRIVATE).all) {
            raw[k] = when (v) {
                is Boolean -> PrefValue.Flag(v)
                is Int -> PrefValue.Number(v)
                is Long -> PrefValue.Big(v)
                is Float -> PrefValue.Decimal(v)
                is String -> PrefValue.Text(v)
                is Set<*> -> PrefValue.Texts(v.filterIsInstance<String>())
                else -> continue
            }
        }
        return settingsOpen(java.io.File(context.filesDir, dbFileName()).path, raw).prefs()
    }
}
