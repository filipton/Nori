package dev.flint.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.flint.music.ffi.IngestStats
import dev.flint.music.playback.DacState
import dev.flint.music.settings.Prefs
import kotlinx.coroutines.Job
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

data class LoginUi(val busy: Boolean = false, val error: String? = null)
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

    fun login(url: String, user: String, password: String) {
        if (_login.value.busy) return
        _login.value = LoginUi(busy = true)
        viewModelScope.launch {
            _login.value = try {
                flint.login(url, user, password)
                LoginUi()
            } catch (e: Exception) {
                LoginUi(error = e.message ?: "Could not reach the server")
            }
        }
    }

    fun logout() = flint.logout()

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
