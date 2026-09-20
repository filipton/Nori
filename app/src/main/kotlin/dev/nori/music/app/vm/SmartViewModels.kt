package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.ffi.HistoryEntry
import dev.nori.music.ffi.ListeningStats
import dev.nori.music.ffi.SmartPlaylist
import dev.nori.music.ffi.Song
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import org.json.JSONArray
import org.json.JSONObject

/** One condition of a smart playlist as the editor sees it. The core's JSON schema is the storage format. */
data class SmartRule(val field: String = "genre", val op: String = "contains", val value: String = "")

data class SmartDraft(
    val id: String = "", val name: String = "", val all: Boolean = true, val rules: List<SmartRule> = listOf(SmartRule()),
    val sortField: String = "random", val descending: Boolean = false, val limit: Int = 100,
) {
    fun toJson(): String = JSONObject().apply {
        put("match", JSONObject().put("all", all).put("rules", JSONArray(rules.filter { it.value.isNotBlank() || it.op in FLAG_OPS }.map { r ->
            JSONObject().put("field", r.field).put("op", r.op).apply {
                if (r.op !in FLAG_OPS) put("value", if (r.op == "between") JSONArray(r.value.split(Regex("[,\\s-]+")).filter(String::isNotBlank).map { it.toLongOrNull() ?: it }) else r.value.toLongOrNull()?.takeIf { r.field in NUMBERS || r.op.endsWith("Days") } ?: r.value)
            }
        })))
        put("sort", JSONObject().put("field", sortField).put("descending", descending).put("seed", 1))
        if (limit > 0) put("limit", limit)
    }.toString()

    companion object {
        val TEXTS = listOf("title", "album", "artist", "genre", "suffix")
        val NUMBERS = listOf("year", "duration", "track", "discNumber", "bitRate", "sampleRate", "bitDepth", "size", "userRating", "playCount", "skipCount", "serverPlayCount")
        val DATES = listOf("lastPlayed", "added")
        val FLAGS = listOf("starred", "isDownloaded", "excludedFromMixes")
        val FLAG_OPS = listOf("isTrue", "isFalse")
        fun ops(field: String) = when (field) {
            in TEXTS -> listOf("contains", "is", "isNot", "notContains", "startsWith", "endsWith")
            in NUMBERS -> listOf("is", "isNot", "greater", "less", "between")
            in DATES -> listOf("withinDays", "notWithinDays", "greater", "less")
            else -> FLAG_OPS
        }
        val SORTS = listOf("random") + TEXTS + NUMBERS + DATES

        /** Reads back what this editor wrote; definitions with nested groups are kept as raw JSON by the caller. */
        fun from(p: SmartPlaylist): SmartDraft? = runCatching {
            val o = JSONObject(p.json)
            val m = o.optJSONObject("match")
            val rules = m?.optJSONArray("rules")?.let { a ->
                (0 until a.length()).map { i ->
                    val r = a.getJSONObject(i)
                    if (r.has("rules")) return null
                    SmartRule(r.getString("field"), r.getString("op"), r.opt("value")?.let { v -> if (v is JSONArray) (0 until v.length()).joinToString(" ") { v.get(it).toString() } else v.toString() }.orEmpty())
                }
            } ?: emptyList()
            val s = o.optJSONObject("sort")
            SmartDraft(p.id, p.name, m?.optBoolean("all", true) ?: true, rules.ifEmpty { listOf(SmartRule()) }, s?.optString("field", "random") ?: "random", s?.optBoolean("descending") ?: false, o.optInt("limit", 0))
        }.getOrNull()
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
        _songs.value = runCatching { Load.Ready(nori.library.smartSongs(json, nori.downloads.state.value.doneIds)) }.getOrElse { Load.Failed(it.message ?: "Could not evaluate") }
    }

    /** Null when saved; otherwise what is wrong with the definition. */
    fun save(draft: SmartDraft, onSaved: (String) -> Unit): String? {
        val json = draft.toJson()
        nori.library.smartCheck(json)?.let { return it }
        viewModelScope.launch { val id = nori.library.smartSave(draft.id.takeUnless { it.startsWith("default-") }.orEmpty(), draft.name.ifBlank { "Smart playlist" }, json); refresh(); onSaved(id) }
        return null
    }

    fun delete(id: String) = viewModelScope.launch { nori.library.smartDelete(id); refresh() }
}

class HistoryViewModel(app: Application) : NoriViewModel(app) {
    private val _entries = MutableStateFlow<List<HistoryEntry>>(emptyList())
    val entries: StateFlow<List<HistoryEntry>> = _entries
    private val _stats = MutableStateFlow<ListeningStats?>(null)
    val stats: StateFlow<ListeningStats?> = _stats
    private var exhausted = false

    init { loadMore() }

    fun loadMore() {
        if (exhausted) return
        viewModelScope.launch {
            val next = runCatching { nori.library.history(100, _entries.value.size) }.getOrDefault(emptyList())
            exhausted = next.size < 100
            _entries.update { it + next }
        }
    }

    /** [days] 0 means everything. */
    fun loadStats(days: Int) = viewModelScope.launch {
        val now = System.currentTimeMillis()
        _stats.value = runCatching { nori.library.stats(if (days == 0) 0 else now - days * 86_400_000L, now) }.getOrNull()
    }

    fun clear() = viewModelScope.launch { nori.library.clearHistory(); _entries.value = emptyList(); exhausted = true }
}
