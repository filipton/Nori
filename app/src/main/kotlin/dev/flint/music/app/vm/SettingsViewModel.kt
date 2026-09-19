package dev.flint.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.flint.music.ffi.IngestStats
import dev.flint.music.playback.DacState
import dev.flint.music.settings.Prefs
import dev.flint.music.settings.ServerProfile
import dev.flint.music.net.describeConnectionError
import dev.flint.music.ffi.MusicFolder
import dev.flint.music.settings.Band
import dev.flint.music.settings.BandKind
import dev.flint.music.settings.BandChannel
import dev.flint.music.ffi.parseEqPreset
import dev.flint.music.ffi.eqPresets
import dev.flint.music.ffi.NamedPreset
import dev.flint.music.ffi.AutoEqEntry
import dev.flint.music.ffi.SoundProfile
import dev.flint.music.settings.Sound
import dev.flint.music.settings.withSound
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class LoginUi(val busy: Boolean = false, val error: String? = null, val done: Boolean = false)
data class AutoEqUi(val count: Int = 0, val query: String = "", val hits: List<AutoEqEntry> = emptyList(), val busy: Boolean = false, val applied: String? = null, val error: String? = null)

data class SyncUi(val running: Boolean = false, val indexed: IngestStats = IngestStats(0u, 0u, 0u), val error: String? = null)

class SettingsViewModel(app: Application) : FlintViewModel(app) {
    val prefs: StateFlow<Prefs> = flint.settings.prefs
    val dac: StateFlow<DacState> = flint.dac.state
    private val _login = MutableStateFlow(LoginUi())
    val login: StateFlow<LoginUi> = _login
    private val _sync = MutableStateFlow(SyncUi())
    val sync: StateFlow<SyncUi> = _sync
    private var syncJob: Job? = null

    init { viewModelScope.launch { runCatching { flint.library.indexSize() }.onSuccess { n -> _sync.update { it.copy(indexed = n) } } } }

    fun update(change: (Prefs) -> Prefs) = flint.settings.update(change)

    /** A blank profile for the "add server" form. */
    fun newProfile() = ServerProfile(id = java.util.UUID.randomUUID().toString().take(8))

    fun login(profile: ServerProfile) {
        if (_login.value.busy) return
        _login.value = LoginUi(busy = true)
        viewModelScope.launch {
            _login.value = try {
                flint.login(profile)
                LoginUi(done = true)
            } catch (e: Exception) {
                LoginUi(error = describeConnectionError(e))
            }
        }
    }

    fun clearLoginResult() { _login.value = LoginUi() }
    fun switchServer(profile: ServerProfile) = flint.activate(profile)
    fun updateServer(profile: ServerProfile) = flint.updateServer(profile)
    fun removeServer(id: String) = flint.removeServer(id)

    /** Copies a picked PKCS#12 file into the app and returns the profile that uses it. */
    fun importClientCert(profile: ServerProfile, uri: android.net.Uri, password: String): ServerProfile {
        val name = "${profile.id}.p12"
        val dir = java.io.File(getApplication<Application>().filesDir, "certs").apply { mkdirs() }
        getApplication<Application>().contentResolver.openInputStream(uri)?.use { input -> java.io.File(dir, name).outputStream().use { input.copyTo(it) } }
        return profile.copy(clientCert = name, clientCertPassword = password)
    }

    private val _folders = MutableStateFlow<List<MusicFolder>>(emptyList())
    val musicFolders: StateFlow<List<MusicFolder>> = _folders
    fun loadMusicFolders() = viewModelScope.launch { runCatching { flint.library.musicFolders() }.onSuccess { _folders.value = it } }

    fun logout() = flint.logout()

    // ---- equalizer ----

    /** The equalizer screen is open: the player answers a moved slider at once instead of seconds later. */
    /** Moves one home shelf up or down the page. */
    fun moveHomeRow(from: Int, to: Int) = update { p ->
        val rows = p.homeRows.toMutableList()
        if (from !in rows.indices || to !in rows.indices) return@update p
        rows.add(to, rows.removeAt(from))
        p.copy(homeRows = rows)
    }

    fun setTuning(on: Boolean) = flint.player.setTuning(on)

    /**
     * Flips one setting by name, for the debug test bridge. Only the switches a check needs; anything
     * else returns false so a typo in a script fails loudly instead of silently doing nothing.
     */
    fun setByName(name: String, value: String): Boolean {
        val on = value.equals("true", true) || value == "1"
        val change: (dev.flint.music.settings.Prefs) -> dev.flint.music.settings.Prefs? = {
            when (name) {
                "limiter" -> it.copy(limiter = on)
                "eq" -> it.copy(eqEnabled = on)
                "mono" -> it.copy(mono = on)
                "hiRes" -> it.copy(hiRes = on)
                "bitPerfect" -> it.copy(bitPerfect = on)
                "offload" -> it.copy(offload = on)
                "autoMix" -> it.copy(autoMix = on)
                "amoled" -> it.copy(amoled = on)
                "playerColours" -> it.copy(playerColours = on)
                "coverColors" -> it.copy(coverColors = on)
                "thirdPartyLookups" -> it.copy(thirdPartyLookups = on)
                "crossfadeKeepAlbums" -> it.copy(crossfadeKeepAlbums = on)
                "lyricsSweep" -> it.copy(lyricsSweep = on)
                "crossfadeSec" -> it.copy(crossfadeSec = value.toIntOrNull() ?: it.crossfadeSec)
                "crossfeedDb" -> it.copy(crossfeedDb = value.toFloatOrNull() ?: it.crossfeedDb)
                "limiterThresholdDb" -> it.copy(limiterThresholdDb = value.toFloatOrNull() ?: it.limiterThresholdDb)
                else -> null
            }
        }
        if (change(prefs.value) == null) return false
        update { change(it) ?: it }
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

    private val _profiles = MutableStateFlow<List<SoundProfile>>(emptyList())
    val profiles: StateFlow<List<SoundProfile>> = _profiles
    val outputs: StateFlow<List<String>> = flint.outputs.known
    val currentOutput: StateFlow<String> = flint.outputs.current

    init { refreshProfiles() }
    private fun refreshProfiles() = viewModelScope.launch { _profiles.value = runCatching { flint.core.profiles() }.getOrDefault(emptyList()) }

    /** Saves the sound settings as they are now under [name]. */
    fun saveProfile(name: String, outputs: List<String> = emptyList()) = viewModelScope.launch {
        runCatching { flint.core.profileSave(SoundProfile(name.trim(), Sound.of(prefs.value).toJson(), outputs)) }
        refreshProfiles()
    }

    fun applyProfile(p: SoundProfile) = Sound.fromJson(p.json)?.let { s -> update { it.withSound(s) } }
    fun deleteProfile(name: String) = viewModelScope.launch { runCatching { flint.core.profileDelete(name) }; refreshProfiles() }

    /** Binds or unbinds an output device to a profile; the service applies it when that device becomes active. */
    fun bindProfile(p: SoundProfile, output: String, bound: Boolean) = viewModelScope.launch {
        val outs = if (bound) (p.outputs + output).distinct() else p.outputs - output
        runCatching { flint.core.profileSave(p.copy(outputs = outs)) }
        refreshProfiles()
    }

    private val _autoEq = MutableStateFlow(AutoEqUi())
    val autoEq: StateFlow<AutoEqUi> = _autoEq

    init { viewModelScope.launch { _autoEq.update { it.copy(count = runCatching { flint.core.autoeqCount() }.getOrDefault(0u).toInt()) } } }

    /** Downloads the AutoEQ index once (850 kB) so searching is local afterwards. */
    fun downloadAutoEqIndex() = viewModelScope.launch {
        // Asked for by name, with a button: that is the consent. The lookups switch is for what the app
        // fetches on its own - missing lyrics, update checks - not for a download the user started.
        _autoEq.update { it.copy(busy = true, error = null) }
        _autoEq.value = try {
            val text = withContext(Dispatchers.IO) { flint.http.get(flint.core.autoeqIndexUrl()).decodeToString() }
            AutoEqUi(count = withContext(Dispatchers.IO) { flint.core.autoeqStore(text) }.toInt())
        } catch (e: Exception) {
            AutoEqUi(error = describeConnectionError(e))
        }
    }

    fun searchAutoEq(query: String) = viewModelScope.launch {
        _autoEq.update { it.copy(query = query) }
        if (query.length < 2) return@launch _autoEq.update { it.copy(hits = emptyList()) }
        val hits = withContext(Dispatchers.IO) { runCatching { flint.core.autoeqSearch(query, 40u) }.getOrDefault(emptyList()) }
        _autoEq.update { if (it.query == query) it.copy(hits = hits) else it }
    }

    /**
     * What an output device might be in the AutoEQ database: "Bluetooth: LE_WH-1000XM5" -> "WH-1000XM5".
     * Empty when the index is not downloaded or the name says nothing (the speaker, a generic "USB Audio").
     */
    suspend fun autoEqFor(output: String): List<AutoEqEntry> {
        val name = output.substringAfter(": ", "").replace(Regex("^(LE[_-]|BT[_-])", RegexOption.IGNORE_CASE), "").replace('_', ' ').trim()
        if (name.length < 3 || name.equals("DAC", true) || name.contains("USB Audio", true) || name.equals("device", true)) return emptyList()
        return withContext(Dispatchers.IO) { runCatching { flint.core.autoeqSearch(name, 5u) }.getOrDefault(emptyList()) }
    }

    /** Applies [entry]'s curve and binds it, as a profile named after it, to [output]: next time it loads by itself. */
    fun adoptAutoEq(entry: AutoEqEntry, output: String) = viewModelScope.launch {
        applyAutoEq(entry).join()
        if (_autoEq.value.applied == entry.name) {
            runCatching { flint.core.profileSave(SoundProfile(entry.name, Sound.of(prefs.value).toJson(), listOf(output))) }
            refreshProfiles()
        }
    }

    /** Fetches one headphone's parametric preset and makes it the current curve. */
    fun applyAutoEq(entry: AutoEqEntry) = viewModelScope.launch {
        _autoEq.update { it.copy(busy = true, error = null) }
        try {
            val text = withContext(Dispatchers.IO) { flint.http.get(flint.core.autoeqPresetUrl(entry)).decodeToString() }
            if (importPreset(text) == 0) throw IllegalStateException("that file had no filters in it")
            _autoEq.update { it.copy(busy = false, applied = entry.name) }
        } catch (e: Exception) {
            _autoEq.update { it.copy(busy = false, error = describeConnectionError(e)) }
        }
    }

    private val _analysed = MutableStateFlow(0)
    /** How many tracks AutoMix has measured. */
    val analysed: StateFlow<Int> = _analysed

    fun refreshAnalysed() = viewModelScope.launch { _analysed.value = runCatching { flint.core.analysisCount() }.getOrDefault(0u).toInt() }

    fun clearAnalyses() = viewModelScope.launch { runCatching { flint.core.analysisClear() }; refreshAnalysed() }

    // ---- downloads ----

    /** Queues every song of the offline index for download; run [syncLibrary] first so the index is complete. */
    fun downloadLibrary() = viewModelScope.launch {
        var offset = 0
        while (true) {
            val page = flint.library.indexedSongs(offset, 500)
            if (page.isEmpty()) break
            flint.downloads.download(page)
            offset += page.size
        }
    }

    /** Fills the offline search index with the whole library. Optional: the app works without it. */
    fun syncLibrary() {
        if (_sync.value.running) return
        _sync.update { it.copy(running = true, error = null) }
        syncJob = viewModelScope.launch {
            try {
                flint.library.sync().collect { }
                _sync.value = SyncUi(indexed = flint.library.indexSize())
            } catch (e: Exception) {
                _sync.update { it.copy(running = false, error = e.message) }
            }
        }
    }
}
