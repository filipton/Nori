package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.ffi.IngestStats
import dev.nori.music.playback.DacState
import dev.nori.music.playback.DeviceSound
import dev.nori.music.playback.Outputs
import dev.nori.music.settings.Prefs
import dev.nori.music.settings.ServerProfile
import dev.nori.music.net.describeConnectionError
import dev.nori.music.ffi.MusicFolder
import dev.nori.music.settings.Band
import dev.nori.music.settings.BandKind
import dev.nori.music.settings.BandChannel
import dev.nori.music.ffi.parseEqPreset
import dev.nori.music.ffi.eqPresets
import dev.nori.music.ffi.NamedPreset
import dev.nori.music.ffi.AutoEqEntry
import dev.nori.music.ffi.SoundProfile
import dev.nori.music.settings.Sound
import dev.nori.music.settings.withSound
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.launch

data class LoginUi(val busy: Boolean = false, val error: String? = null, val done: Boolean = false)
data class AutoEqUi(val count: Int = 0, val query: String = "", val hits: List<AutoEqEntry> = emptyList(), val busy: Boolean = false, val applied: String? = null, val error: String? = null)

/**
 * One output device in the equalizer's device list: [name] is what it calls itself, [kind] where it is
 * plugged in, [sound] what it gets (a profile's name, "Flat", "Automatic", "Leave as is").
 */
data class DeviceRow(val output: String, val name: String, val kind: String?, val current: Boolean, val sound: String, val choice: DeviceSound.Choice)

/** A line for the snackbar about the device that just connected, with the one thing it offers to do. */
data class EqNotice(val message: String, val action: String, val source: DeviceSound.Notice)

data class SyncUi(val running: Boolean = false, val indexed: IngestStats = IngestStats(0u, 0u, 0u), val error: String? = null)

/** What lives on the phone: streamed music, covers, finished downloads and the library index. */
data class StorageUi(
    val streamBytes: Long = 0L,
    val coverBytes: Long = 0L,
    val downloadBytes: Long = 0L,
    val downloadSongs: Int = 0,
    val indexBytes: Long = 0L,
    val busy: Boolean = false,
)

class SettingsViewModel(app: Application) : NoriViewModel(app) {
    val prefs: StateFlow<Prefs> = nori.settings.prefs
    val dac: StateFlow<DacState> = nori.dac.state
    private val _login = MutableStateFlow(LoginUi())
    val login: StateFlow<LoginUi> = _login
    private val _sync = MutableStateFlow(SyncUi())
    val sync: StateFlow<SyncUi> = _sync
    private var syncJob: Job? = null

    init { viewModelScope.launch { runCatching { nori.library.indexSize() }.onSuccess { n -> _sync.update { it.copy(indexed = n) } } } }

    fun update(change: (Prefs) -> Prefs) = nori.settings.update(change)

    /** Applies "Space for streamed music" at once instead of at the next track. */
    fun applyCacheLimit() = viewModelScope.launch(Dispatchers.IO) { nori.applyCacheLimit() }

    // ---- storage ----

    private val _storage = MutableStateFlow(StorageUi())
    val storage: StateFlow<StorageUi> = _storage

    /** Measures what is on the phone; the caches answer from their index, the rest is weighed. */
    fun refreshStorage() = viewModelScope.launch(Dispatchers.IO) {
        val app = getApplication<Application>()
        _storage.value = StorageUi(
            streamBytes = nori.sources.streamBytes(),
            coverBytes = dirBytes(java.io.File(app.cacheDir, "covers")),
            downloadBytes = nori.sources.downloadBytes(),
            downloadSongs = nori.downloads.state.value.done.size,
            indexBytes = app.filesDir.listFiles()
                ?.filter { it.name.startsWith("nori") && (it.name.endsWith(".db") || it.name.endsWith("-wal") || it.name.endsWith("-shm")) }
                ?.sumOf { dirBytes(it) } ?: 0L,
        )
    }

    /** Empties the streamed-music cache; downloads, covers and the index stay. */
    fun clearStreamCache() = viewModelScope.launch(Dispatchers.IO) {
        _storage.update { it.copy(busy = true) }
        nori.sources.clearStream()
        refreshStorage()
    }

    /** Empties the cover cache; pictures are fetched again as they are shown. */
    fun clearCovers() = viewModelScope.launch(Dispatchers.IO) {
        _storage.update { it.copy(busy = true) }
        runCatching { coil3.SingletonImageLoader.get(getApplication()).diskCache?.clear() }
        refreshStorage()
    }

    private fun dirBytes(f: java.io.File): Long {
        if (f.isFile) return f.length()
        return f.listFiles()?.sumOf(::dirBytes) ?: 0L
    }

    /** A blank profile for the "add server" form. */
    fun newProfile() = ServerProfile(id = java.util.UUID.randomUUID().toString().take(8))

    fun login(profile: ServerProfile) {
        if (_login.value.busy) return
        _login.value = LoginUi(busy = true)
        viewModelScope.launch {
            _login.value = try {
                nori.login(profile)
                LoginUi(done = true)
            } catch (e: Exception) {
                LoginUi(error = describeConnectionError(e))
            }
        }
    }

    fun clearLoginResult() { _login.value = LoginUi() }
    fun switchServer(profile: ServerProfile) = nori.activate(profile)
    fun updateServer(profile: ServerProfile) = nori.updateServer(profile)
    fun removeServer(id: String) = nori.removeServer(id)

    /** Copies a picked PKCS#12 file into the app and returns the profile that uses it. */
    fun importClientCert(profile: ServerProfile, uri: android.net.Uri, password: String): ServerProfile {
        val name = "${profile.id}.p12"
        val dir = java.io.File(getApplication<Application>().filesDir, "certs").apply { mkdirs() }
        getApplication<Application>().contentResolver.openInputStream(uri)?.use { input -> java.io.File(dir, name).outputStream().use { input.copyTo(it) } }
        return profile.copy(clientCert = name, clientCertPassword = password)
    }

    private val _folders = MutableStateFlow<List<MusicFolder>>(emptyList())
    val musicFolders: StateFlow<List<MusicFolder>> = _folders
    fun loadMusicFolders() = viewModelScope.launch { runCatching { nori.library.musicFolders() }.onSuccess { _folders.value = it } }

    fun logout() = nori.logout()

    // ---- equalizer ----

    /** The equalizer screen is open: the player answers a moved slider at once instead of seconds later. */
    /** Moves one home shelf up or down the page. */
    fun moveHomeRow(from: Int, to: Int) = update { p ->
        val rows = p.homeRows.toMutableList()
        if (from !in rows.indices || to !in rows.indices) return@update p
        rows.add(to, rows.removeAt(from))
        p.copy(homeRows = rows)
    }

    fun setTuning(on: Boolean) = nori.player.setTuning(on)

    /**
     * Flips one setting by name, for the debug test bridge. Only the switches a check needs; anything
     * else returns false so a typo in a script fails loudly instead of silently doing nothing.
     */
    fun setByName(name: String, value: String): Boolean {
        if (testDevice(name, value)) return true
        // Not a setting: the one button on that screen a check needs, so a run can start from a phone
        // that has measured nothing and see the measuring happen.
        if (name == "clearAnalyses") { clearAnalyses(); return true }
        val on = value.equals("true", true) || value == "1"
        val change: (dev.nori.music.settings.Prefs) -> dev.nori.music.settings.Prefs? = {
            when (name) {
                "limiter" -> it.copy(limiter = on)
                "eq" -> it.copy(eqEnabled = on)
                "mono" -> it.copy(mono = on)
                "hiRes" -> it.copy(hiRes = on)
                "bitPerfect" -> it.copy(bitPerfect = on)
                "offload" -> it.copy(offload = on)
                "autoMix" -> it.copy(autoMix = on)
                "amoled" -> it.copy(amoled = on)
                "ignoreSystemMotion" -> it.copy(ignoreSystemMotion = on)
                "reduceMotion" -> it.copy(reduceMotion = on)
                "playerColours" -> it.copy(playerColours = on)
                "coverColors" -> it.copy(coverColors = on)
                "thirdPartyLookups" -> it.copy(thirdPartyLookups = on)
                "crossfadeKeepAlbums" -> it.copy(crossfadeKeepAlbums = on)
                "lyricsSweep" -> it.copy(lyricsSweep = on)
                "crossfadeSec" -> it.copy(crossfadeSec = value.toIntOrNull() ?: it.crossfadeSec)
                "coversAhead" -> it.copy(coversAhead = value.toIntOrNull()?.coerceIn(0, 10) ?: it.coversAhead)
                "cacheMb" -> it.copy(cacheMb = value.toIntOrNull()?.coerceIn(256, 16384) ?: it.cacheMb).also { viewModelScope.launch(Dispatchers.IO) { nori.applyCacheLimit() } }
                "parallelDownloads" -> it.copy(parallelDownloads = value.toIntOrNull()?.coerceIn(1, 10) ?: it.parallelDownloads)
                "crossfeedDb" -> it.copy(crossfeedDb = value.toFloatOrNull() ?: it.crossfeedDb)
                "limiterThresholdDb" -> it.copy(limiterThresholdDb = value.toFloatOrNull() ?: it.limiterThresholdDb)
                "autoFill" -> it.copy(autoFill = on)
                "autoFillKind" -> dev.nori.music.settings.AutoFillKind.entries.firstOrNull { k -> k.name.equals(value, true) }?.let { k -> it.copy(autoFillKind = k) }
                "autoFillBasis" -> dev.nori.music.settings.AutoFillBasis.entries.firstOrNull { b -> b.name.equals(value, true) }?.let { b -> it.copy(autoFillBasis = b) }
                "autoEqAuto" -> it.copy(autoEqAuto = on)
                "profilePerOutput" -> it.copy(profilePerOutput = on)
                else -> null
            }
        }
        if (change(prefs.value) == null) return false
        update { change(it) ?: it }
        return true
    }

    /**
     * The device-sound half of the test bridge: `set deviceSound "<output>=flat|auto|quiet|profile:<name>|curve:<search>"`,
     * `set saveProfile <name>`, `set deleteProfile <name>`, `set forgetDevice <output>`, `set autoEqIndex 1`,
     * `set eqNotice apply|undo` (presses the snackbar's button, whichever notice is up).
     */
    private fun testDevice(name: String, value: String): Boolean {
        when (name) {
            "deviceSound" -> viewModelScope.launch {
                val output = value.substringBeforeLast('=')
                val spec = value.substringAfterLast('=')
                val choice = when {
                    spec == "flat" -> DeviceSound.Choice.Flat
                    spec == "quiet" -> DeviceSound.Choice.Quiet
                    spec.startsWith("profile:") -> DeviceSound.Choice.Profile(spec.removePrefix("profile:"))
                    spec.startsWith("curve:") -> withContext(Dispatchers.IO) { nori.core.autoeqSearch(spec.removePrefix("curve:"), 1u) }.firstOrNull()?.let { DeviceSound.Choice.Curve(it) } ?: return@launch
                    else -> DeviceSound.Choice.Automatic
                }
                assignDevice(output, choice)
            }
            "saveProfile" -> saveProfile(value)
            "deleteProfile" -> deleteProfile(value)
            "forgetDevice" -> forgetDevice(value)
            "autoEqIndex" -> downloadAutoEqIndex()
            "eqNotice" -> devices.lastNotice?.let { n ->
                viewModelScope.launch {
                    when (n) {
                        is DeviceSound.Offer -> if (value == "apply") devices.accept(n)
                        is DeviceSound.Applied -> if (value == "undo") devices.undo(n)
                    }
                    devices.consume(n)
                }
            }
            else -> return false
        }
        return true
    }

    /** The built-in curves, straight from the core so the numbers live in one place. */
    val presets: List<NamedPreset> by lazy { eqPresets() }

    fun applyPreset(p: NamedPreset) = update { prefs ->
        prefs.copy(
            eqEnabled = true, eqPreampDb = p.preampDb.takeIf { it != 0f },
            eqBands = p.bands.map { Band(BandKind.entries[it.kind.ordinal], it.freq, it.gainDb, it.q) }.ifEmpty { Band.GRAPHIC },
        )
    }

    fun setBand(index: Int, band: Band) = update { it.copy(eqBands = it.eqBands.toMutableList().also { l -> l[index] = band }) }
    fun addBand() = update { it.copy(eqBands = it.eqBands + Band(BandKind.PEAKING, 1000f, 0f, 1f)) }
    fun removeBand(index: Int) = update { it.copy(eqBands = it.eqBands.filterIndexed { i, _ -> i != index }.ifEmpty { Band.GRAPHIC }) }
    fun resetBands() = update { it.copy(eqBands = Band.GRAPHIC, eqPreampDb = null) }

    /** AutoEQ "ParametricEQ.txt" / Equalizer APO text. Returns how many filters were found. */
    fun importPreset(text: String): Int {
        val preset = parseEqPreset(text)
        if (preset.bands.isEmpty()) return 0
        update { p ->
            p.copy(
                eqEnabled = true, eqPreampDb = preset.preampDb,
                eqBands = preset.bands.map { Band(BandKind.entries[it.kind.ordinal], it.freq, it.gainDb, it.q) },
            )
        }
        return preset.bands.size
    }

    // ---- saved profiles and the AutoEQ database ----

    private val devices = nori.deviceSound
    val profiles: StateFlow<List<SoundProfile>> = devices.profiles
    val currentOutput: StateFlow<String> = nori.outputs.current

    init { refreshProfiles() }
    private fun refreshProfiles() = viewModelScope.launch { devices.refresh() }

    /** Every output seen, the one playing now first, each with the sound it gets. */
    val deviceRows: StateFlow<List<DeviceRow>> = combine(nori.outputs.known, currentOutput, profiles, devices.quiet, ::deviceRowsOf)
        // Filled from the start, so the list is there on the screen's first frame rather than popping in.
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), deviceRowsOf(nori.outputs.known.value, currentOutput.value, profiles.value, devices.quiet.value))

    private fun deviceRowsOf(known: List<String>, current: String, profiles: List<SoundProfile>, quiet: Set<String>): List<DeviceRow> =
        (known + current).distinct().map { o ->
            val bound = profiles.firstOrNull { o in it.outputs }?.name
            val choice = when {
                bound == DeviceSound.FLAT -> DeviceSound.Choice.Flat
                bound != null -> DeviceSound.Choice.Profile(bound)
                o in quiet -> DeviceSound.Choice.Quiet
                else -> DeviceSound.Choice.Automatic
            }
            val sound = when (choice) {
                DeviceSound.Choice.Automatic -> "Automatic"
                DeviceSound.Choice.Quiet -> "Leave as is"
                DeviceSound.Choice.Flat -> "Flat"
                else -> bound.orEmpty()
            }
            val kind = o.substringBefore(": ", "").ifEmpty { null }
            DeviceRow(o, o.substringAfter(": "), kind, o == current, sound, choice)
        }.sortedWith(compareBy({ it.output != Outputs.SPEAKER }, { it.name.lowercase() }))

    private val _assigning = MutableStateFlow<String?>(null)
    /** The device whose sound is being fetched and saved right now (an AutoEQ curve is a download). */
    val assigning: StateFlow<String?> = _assigning
    private val _assignError = MutableStateFlow<String?>(null)
    val assignError: StateFlow<String?> = _assignError

    /** Gives [output] its own sound; [onDone] runs once it is saved (and loaded, if that device is playing). */
    fun assignDevice(output: String, choice: DeviceSound.Choice, onDone: () -> Unit = {}) = viewModelScope.launch {
        _assigning.value = output
        _assignError.value = null
        try {
            devices.assign(output, choice, currentOutput.value)
            onDone()
        } catch (e: Exception) {
            _assignError.value = describeConnectionError(e)
        } finally {
            _assigning.value = null
        }
    }

    fun clearAssignError() { _assignError.value = null }

    /** Takes a device out of the list, with whatever was chosen for it. */
    fun forgetDevice(output: String) = viewModelScope.launch {
        devices.forget(output)
        nori.outputs.forget(output)
    }

    /** What to say about the device that just connected; only while it is still the one playing. */
    val eqNotice: StateFlow<EqNotice?> = combine(devices.notice, currentOutput) { n, current ->
        when {
            n == null || n.output != current -> null
            n is DeviceSound.Offer -> EqNotice("${n.entry.name} connected. Use its AutoEQ curve?", "Apply", n)
            n is DeviceSound.Applied -> EqNotice("Using AutoEQ for ${n.curve}", "Undo", n)
            else -> null
        }
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    /** The notice is on screen now, so it is not shown again. */
    fun eqNoticeShown(n: EqNotice) = devices.consume(n.source)

    /** "Apply" on an offer, "Undo" on a curve applied without asking. */
    fun eqNoticeAction(n: EqNotice) = viewModelScope.launch {
        when (val src = n.source) {
            is DeviceSound.Offer -> try {
                devices.accept(src)
            } catch (e: Exception) {
                _autoEq.update { it.copy(error = describeConnectionError(e)) }
            }
            is DeviceSound.Applied -> devices.undo(src)
        }
    }

    /** Saves the sound settings as they are now under [name]. */
    fun saveProfile(name: String) = viewModelScope.launch {
        val sound = Sound.of(prefs.value).toJson()
        // Saving over a profile keeps the devices it is chosen for.
        withContext(Dispatchers.IO) {
            runCatching {
                val kept = nori.core.profiles().firstOrNull { it.name == name.trim() }?.outputs.orEmpty()
                nori.core.profileSave(SoundProfile(name.trim(), sound, kept))
            }
        }
        refreshProfiles()
    }

    fun applyProfile(p: SoundProfile) = Sound.fromJson(p.json)?.let { s -> update { it.withSound(s) } }
    fun deleteProfile(name: String) = viewModelScope.launch { runCatching { nori.core.profileDelete(name) }; refreshProfiles() }

    private val _autoEq = MutableStateFlow(AutoEqUi())
    val autoEq: StateFlow<AutoEqUi> = _autoEq

    init { viewModelScope.launch { _autoEq.update { it.copy(count = runCatching { nori.core.autoeqCount() }.getOrDefault(0u).toInt()) } } }

    /** Downloads the AutoEQ index once (850 kB) so searching is local afterwards. */
    fun downloadAutoEqIndex() = viewModelScope.launch {
        // Asked for by name, with a button: that is the consent. The lookups switch is for what the app
        // fetches on its own - missing lyrics, update checks - not for a download the user started.
        _autoEq.update { it.copy(busy = true, error = null) }
        _autoEq.value = try {
            val text = withContext(Dispatchers.IO) { nori.http.get(nori.core.autoeqIndexUrl()).decodeToString() }
            AutoEqUi(count = withContext(Dispatchers.IO) { nori.core.autoeqStore(text) }.toInt())
        } catch (e: Exception) {
            AutoEqUi(error = describeConnectionError(e))
        }
    }

    fun searchAutoEq(query: String) = viewModelScope.launch {
        _autoEq.update { it.copy(query = query) }
        if (query.length < 2) return@launch _autoEq.update { it.copy(hits = emptyList()) }
        val hits = withContext(Dispatchers.IO) { runCatching { nori.core.autoeqSearch(query, 40u) }.getOrDefault(emptyList()) }
        _autoEq.update { if (it.query == query) it.copy(hits = hits) else it }
    }

    /**
     * What an output device might be in the AutoEQ database, best first: "Bluetooth: LE_WH-1000XM5" finds
     * "Sony WH-1000XM5". Empty when the index is not downloaded or the name says nothing (the speaker, a
     * generic "USB Audio").
     */
    suspend fun autoEqFor(output: String): List<AutoEqEntry> = devices.curvesFor(output)

    /** Fetches one headphone's parametric preset and makes it the current curve. */
    fun applyAutoEq(entry: AutoEqEntry) = viewModelScope.launch {
        _autoEq.update { it.copy(busy = true, error = null) }
        try {
            val text = withContext(Dispatchers.IO) { nori.http.get(nori.core.autoeqPresetUrl(entry)).decodeToString() }
            if (importPreset(text) == 0) throw IllegalStateException("that file had no filters in it")
            _autoEq.update { it.copy(busy = false, applied = entry.name) }
        } catch (e: Exception) {
            _autoEq.update { it.copy(busy = false, error = describeConnectionError(e)) }
        }
    }

    private val _analysed = MutableStateFlow(0)
    /** How many tracks AutoMix has measured. */
    val analysed: StateFlow<Int> = _analysed

    fun refreshAnalysed() = viewModelScope.launch { _analysed.value = runCatching { nori.core.analysisCount() }.getOrDefault(0u).toInt() }

    fun clearAnalyses() = viewModelScope.launch { runCatching { nori.core.analysisClear() }; refreshAnalysed() }

    // ---- downloads ----

    /** Queues every song of the offline index for download; run [syncLibrary] first so the index is complete. */
    fun downloadLibrary() = viewModelScope.launch {
        var offset = 0
        while (true) {
            val page = nori.library.indexedSongs(offset, 500)
            if (page.isEmpty()) break
            nori.downloads.download(page)
            offset += page.size
        }
    }

    /** Fills the offline search index with the whole library. Optional: the app works without it. */
    fun syncLibrary() {
        if (_sync.value.running) return
        _sync.update { it.copy(running = true, error = null) }
        syncJob = viewModelScope.launch {
            try {
                nori.library.sync().collect { }
                _sync.value = SyncUi(indexed = nori.library.indexSize())
            } catch (e: Exception) {
                _sync.update { it.copy(running = false, error = e.message) }
            }
        }
    }
}
