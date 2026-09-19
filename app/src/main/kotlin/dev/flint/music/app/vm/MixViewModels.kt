package dev.flint.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.flint.music.Flint
import dev.flint.music.data.Mix
import dev.flint.music.ffi.Song
import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.mapNotNull
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock

/** The id of the favourites tile and page; every other id names one of [MIXES]. */
const val FAVOURITES_MIX = "favourites"

/** The mixes "For you" offers, in the order it offers them. The id is what the `mix/{id}` route carries. */
private val MIXES = listOf("quick-picks" to Mix.QUICK_PICKS, "discover" to Mix.DISCOVER, "listen-again" to Mix.LISTEN_AGAIN, "top" to Mix.TOP)

/** A "For you" tile: what it is called and up to four covers of what is in it. */
data class MixCard(val id: String, val title: String, val covers: List<String>, val favourites: Boolean)

/** A mix page: exactly the songs that play, in the order they play. */
data class MixPage(
    val id: String, val title: String, val songs: List<Song>, val covers: List<String>,
    /** False for favourites (they follow the hearts) and for top songs (there is only one draw of those). */
    val refreshable: Boolean, val favourites: Boolean,
)

/** Provider tracks are never queued unasked: a stream request makes octo-fiesta download them. */
private fun playable(s: Song) = !s.isExternal && !s.id.startsWith("ext-") && !s.id.startsWith("pl-")

/** Four different covers, for a tile's collage; the list rendition, so they are cache hits. */
private fun FlintViewModel.coversOf(songs: List<Song>?): List<String> =
    songs.orEmpty().asSequence().mapNotNull { it.coverArt }.distinct().take(4).mapNotNull { cover(it, 320) }.toList()

/**
 * Today's draw of each mix, shared by the Home tiles and the mix pages, so the covers on a tile, the
 * list on its page and what plays are one list. Drawn once a day (the seed is the date) or when the
 * page asks for another; never written to the server. Held in memory: after a restart the same seed
 * over the same index draws the same mix again.
 */
internal object MixStore {
    data class Drawn(val songs: List<Song>, val day: Long, val generation: Int)

    private val _drawn = MutableStateFlow<Map<String, Drawn>>(emptyMap())
    val drawn: StateFlow<Map<String, Drawn>> = _drawn
    private val lock = Mutex()

    fun key(flint: Flint, id: String) = "${flint.settings.value.activeServerId}|$id"

    /** Draws mix [id] unless today's draw is already here; [again] asks for a different one. */
    suspend fun ensure(flint: Flint, id: String, again: Boolean = false) = lock.withLock {
        val kind = MIXES.firstOrNull { it.first == id }?.second ?: return@withLock
        val key = key(flint, id)
        val day = java.time.LocalDate.now().toEpochDay()
        val have = _drawn.value[key]
        if (have != null && have.day == day && !again) return@withLock
        val generation = if (have != null && have.day == day) have.generation + 1 else 0
        val songs = runCatching { flint.library.mix(kind, day * 1_000 + generation) }.getOrDefault(emptyList()).filter(::playable)
        // With no listening history yet the personal mixes are empty: what the server thinks is random stands in.
        val list = songs.ifEmpty { runCatching { flint.library.randomSongs(50) }.getOrDefault(emptyList()).filter(::playable) }
        _drawn.update { it + (key to Drawn(list.distinctBy { s -> s.id }, day, generation)) }
    }
}

/**
 * The starred songs, re-asked on every star change, with this session's marks applied at once: an
 * unstarred song leaves the list under the finger instead of after the server's answer.
 */
@OptIn(ExperimentalCoroutinesApi::class)
private fun Flint.favouriteSongs(): Flow<List<Song>> =
    combine(library.starsVersion.flatMapLatest { library.starred() }.map { it.songs }, library.starMarks) { songs, marks ->
        songs.filter { playable(it) && marks["id:${it.id}"] != false }.distinctBy { it.id }
    }

/** The "For you" row: favourites first, then the mixes the core draws from the index and the listening history. */
class MixesViewModel(app: Application) : FlintViewModel(app) {
    private fun cards(favourites: List<Song>, taste: Boolean, drawn: Map<String, MixStore.Drawn>): List<MixCard> =
        listOf(MixCard(FAVOURITES_MIX, "Favourites", coversOf(favourites), favourites = true)) +
            if (!taste) emptyList() else MIXES.map { (id, kind) -> MixCard(id, kind.title, coversOf(drawn[MixStore.key(flint, id)]?.songs), favourites = false) }

    /** Draws whichever mixes are missing or from yesterday; a few milliseconds each, off the main thread. */
    private fun warm() = viewModelScope.launch { MIXES.forEach { MixStore.ensure(flint, it.first) } }

    // Starts with the tiles it can name right away, so the row is there from the first frame and only
    // the covers arrive later (the tile cross-fades to them).
    val cards: StateFlow<List<MixCard>> = combine(
        flint.favouriteSongs().catch { emit(emptyList()) }.onStart { emit(emptyList()) },
        // The mixes are the taste model's: switched off, it draws nothing and offers only favourites.
        flint.settings.prefs.map { it.tasteModel }.distinctUntilChanged().onEach { if (it) warm() },
        MixStore.drawn,
    ) { favourites, taste, drawn -> cards(favourites, taste, drawn) }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), cards(emptyList(), flint.settings.value.tasteModel, MixStore.drawn.value))
}

/** One mix page. Its list does not change while it is open unless asked to (favourites follow the hearts). */
@OptIn(ExperimentalCoroutinesApi::class)
class MixViewModel(app: Application) : FlintViewModel(app) {
    private val id = MutableStateFlow<String?>(null)

    fun open(id: String) {
        if (this.id.value == id) return
        this.id.value = id
        if (id != FAVOURITES_MIX) viewModelScope.launch { MixStore.ensure(flint, id) }
    }

    /** Another draw of the same mix. */
    fun refresh() { id.value?.let { viewModelScope.launch { MixStore.ensure(flint, it, again = true) } } }

    val ui: StateFlow<Load<MixPage>> = id.filterNotNull().flatMapLatest { id ->
        val kind = MIXES.firstOrNull { it.first == id }?.second
        when {
            id == FAVOURITES_MIX -> flint.favouriteSongs().map { MixPage(id, "Favourites", it, coversOf(it), refreshable = false, favourites = true) }
            kind == null -> flow { throw IllegalArgumentException("There is no mix called $id") }
            else -> MixStore.drawn.mapNotNull { it[MixStore.key(flint, id)] }.distinctUntilChanged()
                .map { MixPage(id, kind.title, it.songs, coversOf(it.songs), refreshable = kind != Mix.TOP, favourites = false) }
        }
    }.asLoad()
}
