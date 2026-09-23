package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.ffi.SearchResult
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.FlowPreview
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.debounce
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import dev.nori.music.ffi.SearchSplit
import dev.nori.music.ffi.searchSplit

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
    /** [result] split by the core (search.rs): only the library's items, null when that is all of it. */
    val library: SearchResult? = null,
    /** Only the providers' items (octo-fiesta marks them; Navidrome's are the rest), null when there are none. */
    val providers: SearchResult? = null,
    val hasProviders: Boolean = false,
) {
    /** [result] narrowed to what [scope] asks for. */
    val shown: SearchResult? get() = result?.let { r ->
        when (scope) {
            SearchScope.EVERYTHING -> r
            SearchScope.LIBRARY -> library ?: r
            SearchScope.PROVIDERS -> providers ?: NOTHING
        }
    }

    internal fun with(split: SearchSplit) = copy(result = split.everything, library = split.library, providers = split.providers, hasProviders = split.hasProviders)

    private companion object {
        val NOTHING = SearchResult(emptyList(), emptyList(), emptyList())
    }
}

/**
 * Live search in two layers. Every keystroke is answered at once from the
 * offline index; once typing pauses the server is asked too, because only the
 * server (octo-fiesta) knows about tracks that are not in the library yet. A
 * newer keystroke cancels the request in flight, socket included.
 */
@OptIn(FlowPreview::class)
class SearchViewModel(app: Application) : NoriViewModel(app) {
    private val query = MutableStateFlow("")
    private val _ui = MutableStateFlow(SearchUi())
    val ui: StateFlow<SearchUi> = _ui

    init {
        viewModelScope.launch { _ui.update { it.copy(history = nori.library.searchHistory()) } }
        viewModelScope.launch {
            query.collectLatest { q ->
                if (q.isBlank()) return@collectLatest
                val local = runCatching { withContext(Dispatchers.IO) { nori.core.localSearchSplit(q, 30u) } }.getOrNull() ?: return@collectLatest
                _ui.update { if (it.query.trim() == q && !it.fromServer) it.with(local) else it }
            }
        }
        viewModelScope.launch {
            query.debounce { if (it.isBlank()) 0L else nori.settings.value.liveSearchDelayMs.toLong() }.collectLatest { q ->
                if (q.isBlank()) return@collectLatest
                try {
                    val found = nori.library.search(q)
                    val remote = withContext(Dispatchers.Default) { searchSplit(found) }
                    _ui.update { if (it.query.trim() == q) it.with(remote).copy(fromServer = true, searching = false, error = null) else it }
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
            if (q.isEmpty()) it.copy(query = text, result = null, library = null, providers = null, hasProviders = false, fromServer = false, searching = false, error = null)
            else it.copy(query = text, fromServer = false, searching = true, error = null)
        }
        query.value = q
    }

    /** Called when the user acts on a result: that is a query worth remembering. */
    fun remember() = viewModelScope.launch {
        val q = query.value
        // Too short to be a query (the core says): nothing remembered, nothing to show.
        val history = withContext(Dispatchers.IO) { nori.core.searchRememberRecent(q) } ?: return@launch
        _ui.update { it.copy(history = history) }
    }

    fun setScope(s: SearchScope) = _ui.update { it.copy(scope = s) }

    fun clearHistory() = viewModelScope.launch {
        nori.library.forgetSearches()
        _ui.update { it.copy(history = emptyList()) }
    }
}
