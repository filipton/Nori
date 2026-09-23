package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.ffi.HistoryEntry
import dev.nori.music.ffi.ListeningStats
import dev.nori.music.ffi.SmartEdit
import dev.nori.music.ffi.SmartEditRule
import dev.nori.music.ffi.SmartPlaylist
import dev.nori.music.ffi.Song
import dev.nori.music.ffi.smartEditJson
import dev.nori.music.ffi.smartEditPrepare
import dev.nori.music.ffi.smartEditRead
import dev.nori.music.ffi.smartEditSchema
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext

/** One condition of a smart playlist as the editor sees it. The core's JSON schema is the storage format. */
data class SmartRule(val field: String = "genre", val op: String = "contains", val value: String = "")

/** The editor's form. Writing it as a definition, reading one back and the tables it offers are the core's (smart/draft.rs). */
data class SmartDraft(
    val id: String = "", val name: String = "", val all: Boolean = true, val rules: List<SmartRule> = listOf(SmartRule()),
    val sortField: String = "random", val descending: Boolean = false, val limit: Int = 100,
) {
    internal fun edit() = SmartEdit(id, name, all, rules.map { SmartEditRule(it.field, it.op, it.value) }, sortField, descending, limit)

    fun toJson(): String = smartEditJson(edit())

    companion object {
        private val schema by lazy { smartEditSchema() }
        val TEXTS: List<String> get() = schema.texts
        val NUMBERS: List<String> get() = schema.numbers
        val DATES: List<String> get() = schema.dates
        val FLAGS: List<String> get() = schema.flags
        val FLAG_OPS: List<String> get() = schema.flagOps
        fun ops(field: String): List<String> = schema.ops[field] ?: schema.flagOps
        val SORTS: List<String> get() = schema.sorts

        /** Reads back what this editor wrote; definitions with nested groups are kept as raw JSON by the caller. */
        fun from(p: SmartPlaylist): SmartDraft? = smartEditRead(p)?.let { e ->
            SmartDraft(e.id, e.name, e.all, e.rules.map { SmartRule(it.field, it.op, it.value) }, e.sortField, e.descending, e.limit)
        }
    }
}

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
        _songs.value = runCatching { Load.Ready(nori.library.smartSongs(json)) }.getOrElse { Load.Failed(it.message ?: "Could not evaluate") }
    }

    /** Null when saved; otherwise what is wrong with the definition. */
    fun save(draft: SmartDraft, onSaved: (String) -> Unit): String? {
        val ready = smartEditPrepare(draft.edit())
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
