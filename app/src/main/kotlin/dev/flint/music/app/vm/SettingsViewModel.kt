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
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class LoginUi(val busy: Boolean = false, val error: String? = null, val done: Boolean = false)
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
    fun setTuning(on: Boolean) = flint.player.setTuning(on)

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
