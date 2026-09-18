package dev.flint.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.flint.music.ffi.SearchResult
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch

enum class SearchScope { EVERYTHING, LIBRARY, PROVIDERS }

data class SearchUi(
    val query: String = "",
    val result: SearchResult? = null,
    /** True once [result] is the server's answer rather than the offline index's. */
    val fromServer: Boolean = false,
    val searching: Boolean = false,
    val error: String? = null,
    val history: List<String> = emptyList(),
    val scope: SearchScope = SearchScope.EVERYTHING,
) {
    /** [result] narrowed to what [scope] asks for; octo-fiesta marks provider items, Navidrome's are the rest. */
    val shown: SearchResult? get() = result?.let { r ->
        when (scope) {
            SearchScope.EVERYTHING -> r
            SearchScope.LIBRARY -> SearchResult(r.artists.filterNot { it.isExternal }, r.albums.filterNot { it.isExternal }, r.songs.filterNot { it.isExternal })
            SearchScope.PROVIDERS -> SearchResult(r.artists.filter { it.isExternal }, r.albums.filter { it.isExternal }, r.songs.filter { it.isExternal })
        }
    }
    val hasProviders: Boolean get() = result?.let { r -> r.songs.any { it.isExternal } || r.albums.any { it.isExternal } || r.artists.any { it.isExternal } } == true
}

/**
 * Live search in two layers. Every keystroke is answered at once from the
 * offline index; once typing pauses the server is asked too, because only the
 * server (octo-fiesta) knows about tracks that are not in the library yet. A
 * newer keystroke cancels the request in flight, socket included.
 */
@OptIn(FlowPreview::class)
class SearchViewModel(app: Application) : FlintViewModel(app) {
    private val query = MutableStateFlow("")
    private val _ui = MutableStateFlow(SearchUi())
    val ui: StateFlow<SearchUi> = _ui

    init {
        viewModelScope.launch { _ui.update { it.copy(history = flint.library.searchHistory()) } }
        viewModelScope.launch {
            query.collectLatest { q ->
                if (q.isBlank()) return@collectLatest
                val local = runCatching { flint.library.localSearch(q) }.getOrNull() ?: return@collectLatest
                _ui.update { if (it.query.trim() == q && !it.fromServer) it.copy(result = local) else it }
            }
        }
        viewModelScope.launch {
            query.debounce { if (it.isBlank()) 0L else flint.settings.value.liveSearchDelayMs.toLong() }.collectLatest { q ->
                if (q.isBlank()) return@collectLatest
                try {
                    // A merged provider result may repeat an id, and lists are keyed by id.
                    val remote = flint.library.search(q).let { r -> r.copy(artists = r.artists.distinctBy { it.id }, albums = r.albums.distinctBy { it.id }, songs = r.songs.distinctBy { it.id }) }
                    _ui.update { if (it.query.trim() == q) it.copy(result = remote, fromServer = true, searching = false, error = null) else it }
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    _ui.update { if (it.query.trim() == q) it.copy(searching = false, error = e.message) else it }
                }
            }
        }
    }

    fun setQuery(text: String) {
        val q = text.trim()
        _ui.update {
            if (q.isEmpty()) it.copy(query = text, result = null, fromServer = false, searching = false, error = null)
            else it.copy(query = text, fromServer = false, searching = true, error = null)
        }
        query.value = q
    }

    /** Called when the user acts on a result: that is a query worth remembering. */
    fun remember() = viewModelScope.launch {
        val q = query.value
        if (q.length < 2) return@launch
        flint.library.rememberSearch(q)
        _ui.update { it.copy(history = flint.library.searchHistory()) }
    }

    fun setScope(s: SearchScope) = _ui.update { it.copy(scope = s) }

    fun clearHistory() = viewModelScope.launch {
        flint.library.forgetSearches()
        _ui.update { it.copy(history = emptyList()) }
    }
}
