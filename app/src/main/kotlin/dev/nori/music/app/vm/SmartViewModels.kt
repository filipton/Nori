package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.ffi.HistoryEntry
import dev.nori.music.ffi.ListeningStats
import dev.nori.music.ffi.SmartEdit
import dev.nori.music.ffi.SmartPlaylist
import dev.nori.music.ffi.Song
import dev.nori.music.ffi.smartEditPrepare
import dev.nori.music.ffi.Note
import dev.nori.music.ffi.wordsNote
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

// The editor's form is the core's `SmartEdit`: what a new one holds, how a rule changes and how it is
// written and read back are all smart/draft.rs.

class SmartViewModel(app: Application) : NoriViewModel(app) {
    private val _saved = MutableStateFlow<List<SmartPlaylist>>(emptyList())
    val saved: StateFlow<List<SmartPlaylist>> = _saved
    val defaults: List<SmartPlaylist> = nori.library.smartDefaults()
    private val _songs = MutableStateFlow<Load<List<Song>>>(Load.Loading)
    val songs: StateFlow<Load<List<Song>>> = _songs

    init { refresh() }
    private fun refresh() = viewModelScope.launch { _saved.value = runCatching { nori.library.smartPlaylists() }.getOrDefault(emptyList()) }

    fun find(id: String): SmartPlaylist? = (_saved.value + defaults).firstOrNull { it.id == id }

    fun open(json: String) = viewModelScope.launch {
        _songs.value = Load.Loading
        _songs.value = runCatching { Load.Ready(nori.library.smartSongs(json)) }.getOrElse { Load.Failed(it.message ?: wordsNote(Note.COULD_NOT_EVALUATE)) }
    }

    /** Null when saved; otherwise what is wrong with the definition. */
    fun save(draft: SmartEdit, onSaved: (String) -> Unit): String? {
        val ready = smartEditPrepare(draft)
        ready.error?.let { return it }
        viewModelScope.launch { val id = nori.library.smartSave(ready.id, ready.name, ready.json); refresh(); onSaved(id) }
        return null
    }

    fun delete(id: String) = viewModelScope.launch { nori.library.smartDelete(id); refresh() }
}

class HistoryViewModel(app: Application) : NoriViewModel(app) {
    private val _entries = MutableStateFlow(Grown.empty<HistoryEntry>())
    val entries: StateFlow<List<HistoryEntry>> = _entries
    private val _stats = MutableStateFlow<ListeningStats?>(null)
    val stats: StateFlow<ListeningStats?> = _stats
    private var exhausted = false

    init { loadMore() }

    fun loadMore() {
        if (exhausted) return
        val offset = _entries.value.size.toUInt()
        viewModelScope.launch {
            // A page that could not be read ends the list, as an empty one would.
            val next = runCatching { withContext(Dispatchers.IO) { nori.core.historyPage(offset) } }.getOrNull()
            exhausted = next?.exhausted ?: true
            _entries.value = _entries.value.plus(next?.entries.orEmpty())
        }
    }

    /** [days] 0 means everything. */
    fun loadStats(days: Int) = viewModelScope.launch {
        _stats.value = runCatching { withContext(Dispatchers.IO) { nori.core.statsDays(days.toUInt()) } }.getOrNull()
    }

    fun clear() = viewModelScope.launch { nori.library.clearHistory(); _entries.value = Grown.empty(); exhausted = true }
}
