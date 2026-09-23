package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.ffi.SearchResult
import dev.nori.music.ffi.SearchScope
import dev.nori.music.ffi.SearchSession
import dev.nori.music.ffi.SearchView
import dev.nori.music.net.said
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

/** What the search screen shows: the core's view of the search (`search.rs`), and the recent searches. */
data class SearchUi(
    val query: String = "",
    /** The answer narrowed to the scope; null with no query, when the recent searches show instead. */
    val shown: SearchResult? = null,
    val searching: Boolean = false,
    val error: String? = null,
    val scope: SearchScope = SearchScope.EVERYTHING,
    val scopesOffered: Boolean = false,
    val nothingFound: Boolean = false,
    val history: List<String> = emptyList(),
) {
    internal fun with(v: SearchView) = copy(
        query = v.text, shown = v.shown, searching = v.searching, error = v.error, scope = v.scope,
        scopesOffered = v.scopesOffered, nothingFound = v.nothingFound,
    )
}

/**
 * Live search in two layers. Every keystroke is answered at once from the offline index; once typing
 * pauses the server is asked too, because only the server (octo-fiesta) knows about tracks that are not
 * in the library yet. A newer keystroke cancels the request in flight, socket included. Which answer is
 * still worth showing, and what the screen then says, is the core's [SearchSession].
 */
@OptIn(FlowPreview::class)
class SearchViewModel(app: Application) : NoriViewModel(app) {
    private val session = SearchSession()
    /** What is in the field, as typed; the session is told off the main thread. */
    private val text = MutableStateFlow("")
    private val query = MutableStateFlow("")
    private val _ui = MutableStateFlow(SearchUi())
    val ui: StateFlow<SearchUi> = _ui
    private val localLimit by lazy { dev.nori.music.ffi.librarySizes().localSearch }

    init {
        viewModelScope.launch { _ui.update { it.copy(history = nori.library.searchHistory()) } }
        viewModelScope.launch {
            text.collectLatest { t ->
                val v = withContext(Dispatchers.Default) { session.typed(t) }
                _ui.update { it.with(v) }
                query.value = v.query
            }
        }
        viewModelScope.launch {
            query.collectLatest { q ->
                if (q.isBlank()) return@collectLatest
                val v = runCatching { withContext(Dispatchers.IO) { session.local(nori.core, q, localLimit) } }.getOrNull() ?: return@collectLatest
                _ui.update { it.with(v) }
            }
        }
        viewModelScope.launch {
            query.debounce { if (it.isBlank()) 0L else nori.settings.value.liveSearchDelayMs.toLong() }.collectLatest { q ->
                if (q.isBlank()) return@collectLatest
                val v = try {
                    // The core asks and takes the answer itself: it never comes out here to be handed back.
                    nori.library.searchInto(session, q)
                } catch (e: CancellationException) {
                    throw e
                } catch (e: Exception) {
                    session.failed(q, e.said)
                }
                if (v != null) _ui.update { it.with(v) }
            }
        }
    }

    /** The field follows the finger at once; what the search makes of it follows from the core. */
    fun setQuery(text: String) {
        _ui.update { it.copy(query = text) }
        this.text.value = text
    }

    /** Called when the user acts on a result: that is a query worth remembering. */
    fun remember() = viewModelScope.launch {
        val q = query.value
        // Too short to be a query (the core says): nothing remembered, nothing to show.
        val history = withContext(Dispatchers.IO) { nori.core.searchRememberRecent(q) } ?: return@launch
        _ui.update { it.copy(history = history) }
    }

    fun setScope(s: SearchScope) { val v = session.scope(s); _ui.update { it.with(v) } }

    fun clearHistory() = viewModelScope.launch {
        nori.library.forgetSearches()
        _ui.update { it.copy(history = emptyList()) }
    }
}
