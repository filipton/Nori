package dev.nori.music.settings

import android.content.Context
import kotlinx.coroutines.flow.MutableStateFlow
import dev.nori.music.ffi.settings.EqLevel
import dev.nori.music.ffi.settings.SavedQuality
import dev.nori.music.ffi.settings.SavedServer
import dev.nori.music.ffi.settings.SettingChange
import dev.nori.music.ffi.settings.SoundTool
import dev.nori.music.ffi.settings.ServerList
import dev.nori.music.ffi.settings.SoundBand
import dev.nori.music.ffi.settings.SoundSettings
import dev.nori.music.ffi.settings.StoredPrefs
import dev.nori.music.ffi.settings.eqGraphic
import dev.nori.music.ffi.settings.serverLabel
import dev.nori.music.ffi.db.dbFileName
import dev.nori.music.ffi.settings.settingsOpen
import dev.nori.music.ffi.settings.settingsPut
import dev.nori.music.ffi.settings.settingsSoundTool
import dev.nori.music.ffi.settings.eqModelGet
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update

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
enum class AutoFillKind { SONGS, ALBUMS }

enum class AutoFillBasis { SIMILAR, ARTIST, GENRE, ERA }

/** The home page's shelves; the app names each (`Say.homeRow`). */
enum class HomeRow { PINNED, PLAYLISTS, RECENT, NEWEST, FREQUENT, TOP_SONGS, RANDOM, STARRED }

/** Whether each band kind has a gain and a slope, and the equalizer's ranges: the core's (`settings::eq_model`), asked once. */
val EQ by lazy { eqModelGet() }

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

/**
 * Mirrors `EqKind` in the core; the ordinals are the wire format, so the order must not change. Whether
 * each has a gain and whether its width is a slope are the core's; the app names each (`Say.bandKind`).
 */
enum class BandKind {
    PEAKING, LOW_SHELF, HIGH_SHELF, LOW_PASS, HIGH_PASS, BAND_PASS, NOTCH, ALL_PASS, LOW_SHELF_SLOPE, HIGH_SHELF_SLOPE;
    val usesGain: Boolean get() = EQ.bandKinds[ordinal].usesGain
    val slope: Boolean get() = EQ.bandKinds[ordinal].slope
}

/** Which side a band applies to; the app names each (`Say.bandChannel`). */
enum class BandChannel { BOTH, LEFT, RIGHT }

/** One equalizer filter. The ten default bands are peaking filters an octave apart. */
data class Band(val kind: BandKind, val freq: Float, val gainDb: Float, val q: Float, val channel: BandChannel = BandChannel.BOTH) {
    companion object {
        /** The ten graphic bands, from the core so the numbers live in one place. */
        val GRAPHIC: List<Band> by lazy { eqGraphic().map { it.band() } }
    }
}

/** One stream quality: [bitRate] 0 and empty [format] mean the original file. */
data class Quality(val bitRate: Int = 0, val format: String = "")

/**
 * Every setting, as the core checked and keeps them (`settings.rs`); the defaults are the core's own
 * (`StoredPrefs::default`), so a fresh install gets them through [StoredPrefs.prefs].
 */
data class Prefs(
    val servers: List<ServerProfile>,
    val activeServerId: String,
    val wifi: Quality,
    val mobile: Quality,
    val download: Quality,
    /** Songs downloaded at the same time, 1 to 10; the rest wait their turn in the order they were asked for. */
    val parallelDownloads: Int,
    /**
     * Covers of the songs coming up in the queue, fetched into the cache before they are shown, so a
     * skip or a swipe lands on a picture that is already there. 0 to 10; the one before is always kept.
     */
    val coversAhead: Int,
    val cacheMb: Int,
    val replayGain: ReplayGainMode,
    val preampDb: Float,
    /** Applied to files that carry no ReplayGain tags, so they do not jump out next to tagged ones. */
    val untaggedGainDb: Float,
    /** Volume ramp on play, pause, seek and manual skip, in milliseconds; 0 is off. Costs nothing between ramps. */
    val fadeMs: Int,
    val pitch: Float,
    /** "Previous" always goes to the previous track instead of first rewinding the current one. */
    val previousAlwaysSkips: Boolean,
    /** Whole tracks fetched ahead into the stream cache, in one go while the radio is already awake. */
    val precacheWifi: Int,
    val precacheMobile: Int,
    /** A track that fails to load is skipped (up to three in a row) instead of stopping playback. */
    val skipOnError: Boolean,
    /** No crossfade between two tracks that follow each other on the same album. */
    val crossfadeKeepAlbums: Boolean,
    /** Decode on the audio DSP and let the CPU sleep. Only possible while nothing has to touch samples. */
    val offload: Boolean,
    /** Ask Android 14+ for an unmixed, unresampled path to a USB DAC. */
    val bitPerfect: Boolean,
    /** 32-bit float to the mixer so 24-bit files are not cut to 16. media3 skips audio processors in this mode, so no equalizer. Read when the service starts. */
    val hiRes: Boolean,
    val scrobble: Boolean,
    /** When the last queued song starts, keep the music going past the end of the queue. */
    val autoFill: Boolean,
    /**
     * Server gone mid-evening and the next song is not downloaded: keep playing from full downloads
     * until the network is back, then resume the parked queue. Off by default; costs nothing until it
     * engages (no network listener until then).
     */
    val bridgeOffline: Boolean,
    /** Songs, or one whole album at a time, queued in its own order. */
    val autoFillKind: AutoFillKind,
    /** What the next songs are chosen by: what the server thinks is similar, or the artist, genre or decade. */
    val autoFillBasis: AutoFillBasis,
    val eqEnabled: Boolean,
    val eqBands: List<Band>,
    /** Null: pulled down automatically by the largest boost, so the curve cannot clip. */
    val eqPreampDb: Float?,
    /** Headphone crossfeed level in dB; 0 is off. */
    val crossfeedDb: Float,
    /** −1 hard left, 0 centre, +1 hard right. */
    val balance: Float,
    val mono: Boolean,
    /**
     * Catches what the pre-amp, the equalizer and a positive ReplayGain would otherwise clip. Costs a few
     * milliseconds of delay, so it is opt-in; below its threshold the samples come through untouched.
     */
    val limiter: Boolean,
    val limiterThresholdDb: Float,
    val crossfadeSec: Int,
    /**
     * AutoMix: transitions planned from each track's analysed tempo, beats and cue points, like Apple Music's.
     * Analysis runs on audio being played anyway, once per track; a transition costs a few percent of a core
     * for its own few seconds. Off by default.
     */
    val autoMix: Boolean,
    val autoMixMaxS: Int,
    val autoMixBeatMatch: Boolean,
    val autoMixMaxTempoPct: Float,
    val autoMixBassSwap: Boolean,
    val autoMixFilters: Boolean,
    val autoMixEchoOut: Boolean,
    /** Off: tempo is matched by changing speed and pitch together (cheaper, and within 2 % inaudible). */
    val autoMixKeepPitch: Boolean,
    /** "Better beat detection": the core's measurer runs Beat This! over the songs coming up (only in a build with it). */
    val autoMixBetterBeats: Boolean,
    /** The beat model may be downloaded over mobile data; otherwise it waits for Wi-Fi. */
    val autoMixBeatsMobileData: Boolean,
    val speed: Float,
    val skipSilence: Boolean,
    /** A play counts once this much of the track was heard (or four minutes, whichever comes first). */
    val scrobblePercent: Int,
    val liveSearchDelayMs: Int,
    // ---- optional subsystems; one that is off is never initialised and costs nothing ----
    /** Keeps a local play history and a taste score per song; feeds mixes, smart playlists and the year in review. */
    val tasteModel: Boolean,
    /**
     * "Look things up online": over everything asked of a third party by itself - lyrics from the lyrics
     * services, the AutoEQ headphone list, moving covers - each with its own switch too. On for a new install.
     */
    val thirdPartyLookups: Boolean,
    /** Apply the profile bound to an output device when that device becomes the active one. */
    val profilePerOutput: Boolean,
    /**
     * Headphones connected with nothing chosen for them and a matching AutoEQ curve: use that curve and
     * remember it for the device, instead of asking first. Off asks. Fetches one small preset per new device.
     */
    val autoEqAuto: Boolean,
    /**
     * Keep the AutoEQ headphone list: the core fetches it on an unmetered network when it is missing or a
     * month old (`Client.autoeqUpdate`). Needs [thirdPartyLookups]. On by default.
     */
    val autoEqDownload: Boolean,
    /** The sung part of the current lyric line fills in word by word. Redraws one line of text per frame, only while the lyrics are on screen. */
    val lyricsSweep: Boolean,
    /** The bottom of the player's cover goes blurred before it melts into the page. One blur pass per frame while the cover moves. */
    val softSleeve: Boolean,
    /**
     * Moving covers: an album's motion artwork from Apple Music plays in the player's sleeve, where it has
     * one. Needs [thirdPartyLookups]. Off by default; off, nothing of it is built.
     */
    val motionArtwork: Boolean,
    /** Moving covers only on unmetered networks: each is a few megabytes. */
    val motionArtworkWifiOnly: Boolean,
    /** A short message when something is favourited or unfavourited. The heart itself always changes. */
    val favouriteNotice: Boolean,
    val lyricsKeepScreenOn: Boolean,
    val lyricsTranslation: Boolean,
    /** 0 small, 1 medium, 2 large. */
    val lyricsSize: Int,
    /**
     * Look lyrics up online when the server has no timed ones. Needs [thirdPartyLookups]. Off means only
     * the server's lyrics (octo-fiesta already asks LRCLIB itself for tracks it serves from a provider).
     */
    val lyricsOnline: Boolean,
    /** Every lyrics service by name, in the order they rank; which to ask is the core's (`lyrics_sources`). */
    val lyricsOrder: List<String>,
    /** The lyrics services switched on, by name: the open ones out of the box. */
    val lyricsOn: List<String>,
    /** Keep asking past lyrics timed line by line for lyrics timed word by word. */
    val lyricsPreferWords: Boolean,
    /** The user's own PaxSenix key; empty for none. */
    val paxsenixKey: String,
    /** A BetterLyrics key; empty for none. */
    val betterLyricsKey: String,
    // ---- look ----
    val theme: ThemeMode,
    /** Pure black backgrounds in dark mode: OLED pixels are off, which saves power as well as looking right. */
    val amoled: Boolean,
    /**
     * With AMOLED black on, the full-screen player still wears the cover's colours, the way Apple Music's
     * does - it is one page about one record, and a sleeve dropping straight into black reads as cut
     * off. Off keeps that screen black as well.
     */
    val playerColours: Boolean,
    /** Android 12+ wallpaper colours; off uses [accent]. */
    val dynamicColor: Boolean,
    /** ARGB seed colour when dynamic colour is off or unavailable. */
    val accent: Long,
    /** Album, artist and playlist pages take their colour from the cover, which runs edge to edge at the top. */
    val coverColors: Boolean,
    /** Shorter, plainer movement everywhere; also follows the system when animations are off there. */
    val reduceMotion: Boolean,
    /**
     * Animate even though Android's own animations are switched off. That switch is as often a speed
     * habit as an accessibility need, and with it off every Compose animation is scaled to nothing -
     * the lyrics then jump from line to line whatever this app asks for. On, the app's own movement
     * runs at its real speed regardless; Reduce motion above still turns it off.
     */
    val ignoreSystemMotion: Boolean,
    /**
     * How big the interface is drawn. 0 is automatic: laid out as if the screen were at least as wide
     * as the one every size was measured against, so a phone set to a large display size does not
     * blow the layout up. Anything else is a fixed factor on top of the system's own size.
     */
    val uiScale: Float,
    val tapAction: TapAction,
    val swipeRight: SwipeAction,
    val swipeLeft: SwipeAction,
    /** Songs the server marks explicit are skipped instead of played. */
    val skipExplicit: Boolean,
    /** Home shelves, in order; a row that is not listed is hidden. */
    val homeRows: List<HomeRow>,
    val pinnedPlaylists: List<String>,
    /** Remembered per list: sort order, grid or list, filters. Keys are list names. */
    val listPrefs: Map<String, String>,
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

    /** The pre-amp in effect: the one set, or the automatic one (the core's `SoundSettings::effective_preamp_db`), worked out once per settings. */
    val effectivePreampDb: Float by lazy {
        // Over plain JNI: a band's drag makes new settings on every step, and the screen shows this for each.
        dev.nori.music.playback.Dsp.effectivePreampDb(
            eqEnabled, eqPreampDb ?: 0f, eqPreampDb == null,
            IntArray(eqBands.size) { eqBands[it].kind.ordinal }, FloatArray(eqBands.size) { eqBands[it].gainDb },
        )
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

fun ServerProfile.stored() = SavedServer(
    id = id, name = name, url = url, altUrl = altUrl, user = user, password = password, apiKey = apiKey, legacyAuth = legacyAuth,
    headers = headers, allowSelfSigned = allowSelfSigned, clientCert = clientCert, clientCertPassword = clientCertPassword,
    wifiOnly = wifiOnly, musicFolderId = musicFolderId, altMaxBitRate = altMaxBitRate,
)

fun SavedServer.profile() = ServerProfile(
    id = id, name = name, url = url, altUrl = altUrl, user = user, password = password, apiKey = apiKey, legacyAuth = legacyAuth,
    headers = headers, allowSelfSigned = allowSelfSigned, clientCert = clientCert, clientCertPassword = clientCertPassword,
    wifiOnly = wifiOnly, musicFolderId = musicFolderId, altMaxBitRate = altMaxBitRate,
)

/** The saved servers and the one in use, as the core edits them (`settings::servers_*`). */
fun Prefs.serverList() = ServerList(servers.map { it.stored() }, activeServerId)

fun Prefs.withServers(list: ServerList) = copy(servers = list.servers.map { it.profile() }, activeServerId = list.activeServerId)

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
    autoMixBetterBeats = autoMixBetterBeats, autoMixBeatsMobileData = autoMixBeatsMobileData,
    speed = speed, skipSilence = skipSilence, scrobblePercent = scrobblePercent, liveSearchDelayMs = liveSearchDelayMs, tasteModel = tasteModel,
    thirdPartyLookups = thirdPartyLookups, profilePerOutput = profilePerOutput, autoEqAuto = autoEqAuto, autoEqDownload = autoEqDownload,
    lyricsSweep = lyricsSweep, softSleeve = softSleeve, motionArtwork = motionArtwork, motionArtworkWifiOnly = motionArtworkWifiOnly,
    favouriteNotice = favouriteNotice, lyricsKeepScreenOn = lyricsKeepScreenOn,
    lyricsTranslation = lyricsTranslation, lyricsSize = lyricsSize, lyricsOnline = lyricsOnline, lyricsOrder = lyricsOrder, lyricsOn = lyricsOn,
    lyricsPreferWords = lyricsPreferWords, paxsenixKey = paxsenixKey, betterLyricsKey = betterLyricsKey, theme = theme.ordinal, amoled = amoled,
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
    autoMixBetterBeats = autoMixBetterBeats, autoMixBeatsMobileData = autoMixBeatsMobileData,
    speed = speed, skipSilence = skipSilence, scrobblePercent = scrobblePercent, liveSearchDelayMs = liveSearchDelayMs, tasteModel = tasteModel,
    thirdPartyLookups = thirdPartyLookups, profilePerOutput = profilePerOutput, autoEqAuto = autoEqAuto, autoEqDownload = autoEqDownload,
    lyricsSweep = lyricsSweep, softSleeve = softSleeve, motionArtwork = motionArtwork, motionArtworkWifiOnly = motionArtworkWifiOnly,
    favouriteNotice = favouriteNotice, lyricsKeepScreenOn = lyricsKeepScreenOn,
    lyricsTranslation = lyricsTranslation, lyricsSize = lyricsSize, lyricsOnline = lyricsOnline, lyricsOrder = lyricsOrder, lyricsOn = lyricsOn,
    lyricsPreferWords = lyricsPreferWords, paxsenixKey = paxsenixKey, betterLyricsKey = betterLyricsKey, theme = ThemeMode.entries[theme], amoled = amoled,
    playerColours = playerColours, dynamicColor = dynamicColor, accent = accent, coverColors = coverColors, reduceMotion = reduceMotion,
    ignoreSystemMotion = ignoreSystemMotion, uiScale = uiScale, tapAction = TapAction.entries[tapAction], swipeRight = SwipeAction.entries[swipeRight],
    swipeLeft = SwipeAction.entries[swipeLeft], skipExplicit = skipExplicit, homeRows = homeRows.map { HomeRow.entries[it] }, pinnedPlaylists = pinnedPlaylists,
    listPrefs = listPrefs,
)

/**
 * The settings are the core's (`settings_store.rs`): it reads them once, keeps them and writes them to
 * the app's database whenever they change. The playback service needs them synchronously on start,
 * and this is a few rows read once.
 */
class Settings(private val context: Context) {
    private val state = MutableStateFlow(load())
    val prefs: StateFlow<Prefs> = state
    val value get() = state.value

    /**
     * What each change asks of the player, as the core says (settings_store.rs: APPLY_AUDIO 1,
     * APPLY_GAIN 2, REPLAN 4, SOUND 8); nothing for a change only screens care about.
     */
    private val _effects = kotlinx.coroutines.flow.MutableSharedFlow<Int>(extraBufferCapacity = 16)
    val effects: kotlinx.coroutines.flow.SharedFlow<Int> = _effects

    fun update(change: (Prefs) -> Prefs) {
        val next = change(state.value)
        if (next == state.value) return
        put(next.stored(), next)
    }

    /** Where a band crosses to the core and back; one for the settings, taken in turn. */
    private val band = FloatArray(5)

    /**
     * One equalizer band changed, on every step of a slider: edited in the core where the settings are
     * kept, which holds it in range, and only that band replaced here. Building and comparing the whole
     * settings record for each step was most of what a drag cost.
     */
    fun setBand(index: Int, b: Band) {
        val effect: Int
        val kept: Band
        synchronized(band) {
            band[0] = b.kind.ordinal.toFloat(); band[1] = b.freq; band[2] = b.gainDb; band[3] = b.q; band[4] = b.channel.ordinal.toFloat()
            effect = SoundEdit.setBand(index, band)
            if (effect < 0) return
            kept = Band(BandKind.entries[band[0].toInt()], band[1], band[2], band[3], BandChannel.entries[band[4].toInt()])
        }
        state.update { p -> if (index in p.eqBands.indices) p.copy(eqBands = p.eqBands.toMutableList().also { it[index] = kept }) else p }
        if (effect != 0) _effects.tryEmit(effect)
    }

    /** Pre-amp, balance, limiter ceiling or crossfeed moved; edited in the core like a band, which holds and snaps it. */
    fun setLevel(level: EqLevel, value: Float) {
        val r = SoundEdit.setLevel(level.ordinal, value)
        if (r == -1L) return
        val kept = java.lang.Float.intBitsToFloat((r ushr 32).toInt())
        state.update { p ->
            when (level) {
                EqLevel.PREAMP -> p.copy(eqPreampDb = kept)
                EqLevel.BALANCE -> p.copy(balance = kept)
                EqLevel.LIMITER -> p.copy(limiterThresholdDb = kept)
                EqLevel.CROSSFEED -> p.copy(crossfeedDb = kept)
                EqLevel.REPLAY_GAIN_PREAMP -> p.copy(preampDb = kept)
            }
        }
        val effect = r.toInt()
        if (effect != 0) _effects.tryEmit(effect)
    }

    /**
     * A change by name the core has already kept (`setting_set`): taken in here, and nothing sent back.
     * A settings row's every step used to send the whole record back to be compared and kept again.
     */
    fun took(change: SettingChange) {
        state.value = change.prefs.prefs()
        if (change.effect != 0u) _effects.tryEmit(change.effect.toInt())
    }

    /**
     * One of the equalizer screen's tools, used where the core keeps the settings; only the sound part
     * comes back. Returns how many bands there are now. Throws, saying why, for an import with no filters.
     */
    fun soundTool(tool: SoundTool): Int {
        val c = settingsSoundTool(tool) ?: return state.value.eqBands.size
        state.update { it.withSound(c.sound) }
        if (c.effect != 0u) _effects.tryEmit(c.effect.toInt())
        return c.sound.eqBands.size
    }

    /** Settings the core already worked out (a device's sound, a server's list), kept as they are. */
    fun put(stored: StoredPrefs) {
        val next = stored.prefs()
        if (next != state.value) put(stored, next)
    }

    private fun put(stored: StoredPrefs, next: Prefs) {
        // The core first: whatever reacts to the new value (on any thread) reads it from there.
        val effect = settingsPut(stored)
        state.value = next
        if (effect != 0u) _effects.tryEmit(effect.toInt())
    }

    private fun load(): Prefs = settingsOpen(java.io.File(context.filesDir, dbFileName()).path).prefs()
}
