package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.Nori
import dev.nori.music.ffi.MixDraw
import dev.nori.music.ffi.MixLookup
import dev.nori.music.ffi.MixSheet
import dev.nori.music.ffi.MixTile
import dev.nori.music.ffi.Song
import dev.nori.music.ffi.mixTiles
import kotlinx.coroutines.Dispatchers
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
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.onEach
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.transform
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/** The id of the favourites tile and page; every other id names one of the core's mixes (`mix_catalogue`). */
const val FAVOURITES_MIX = "favourites"

/** A "For you" tile: what it is called and up to four covers of what is in it. */
data class MixCard(val id: String, val title: String, val covers: List<String>, val favourites: Boolean)

/** A mix page: exactly the songs that play, in the order they play. */
data class MixPage(
    val id: String, val title: String, val songs: List<Song>, val covers: List<String>,
    /** False for favourites (they follow the hearts) and for top songs (there is only one draw of those). */
    val refreshable: Boolean, val favourites: Boolean,
)

/** The core names the covers; the list rendition of each, so they are cache hits. */
private fun NoriViewModel.card(t: MixTile) = MixCard(t.id, t.title, t.covers.mapNotNull { cover(it, 320) }, t.favourites)
private fun NoriViewModel.page(s: MixSheet) = MixPage(s.id, s.title, s.songs, s.covers.mapNotNull { cover(it, 320) }, s.refreshable, s.favourites)

/**
 * Today's (or this week's) draw of each mix lives in the core (see mixes/board.rs), shared by the Home
 * tiles and the mix pages. This only tells the core what day it is, fetches the server's random songs
 * when a draw comes out empty (the one step that needs the network), and says when to read again.
 */
internal object MixStore {
    /** Bumped whenever a draw changed, so the tiles and pages read the core again. */
    val version = MutableStateFlow(0)
    private val lock = Mutex()

    /** The last row shown and for which server and taste setting, so a Home page opened again has its covers from the first frame. */
    @Volatile var lastCards: Triple<String, Boolean, List<MixCard>>? = null

    private fun today() = java.time.LocalDate.now().toEpochDay()

    /** With no listening history yet the personal mixes are empty: what the server thinks is random stands in. */
    private suspend fun fallback(nori: Nori, id: String, day: Long, again: Boolean) {
        val random = runCatching { nori.library.randomSongs(50) }.getOrDefault(emptyList())
        nori.core.mixDraw(id, day, again, random)
    }

    /** Draws mix [id] unless this period's draw is already there; [again] asks for a different one. */
    suspend fun ensure(nori: Nori, id: String, again: Boolean = false) = lock.withLock {
        withContext(Dispatchers.IO) {
            val day = today()
            when (nori.core.mixDraw(id, day, again, null)) {
                MixDraw.DRAWN -> version.update { it + 1 }
                MixDraw.NEEDS_FALLBACK -> { fallback(nori, id, day, again); version.update { it + 1 } }
                MixDraw.KEPT, MixDraw.UNKNOWN -> {}
            }
        }
    }

    /** Draws whichever mixes are missing or from the last period; a few milliseconds each, off the main thread. */
    suspend fun warm(nori: Nori) = lock.withLock {
        withContext(Dispatchers.IO) {
            val day = today()
            val warm = nori.core.mixWarm(day)
            warm.needsFallback.forEach { fallback(nori, it, day, false) }
            if (warm.changed || warm.needsFallback.isNotEmpty()) version.update { it + 1 }
        }
    }
}

/**
 * The starred songs handed to the core, re-asked on every star change, with this session's marks: an
 * unstarred song leaves the list under the finger instead of after the server's answer. Emits once the
 * core has them.
 */
@OptIn(ExperimentalCoroutinesApi::class)
private fun Nori.favouritesHanded(): Flow<Unit> =
    combine(library.starsVersion.flatMapLatest { library.starred() }.map { it.songs }, library.starMarks) { songs, _ ->
        withContext(Dispatchers.IO) { core.mixFavourites(songs) }
        Unit
    }

/** The "For you" row: favourites first, then the mixes the core draws from the index and the listening history. */
class MixesViewModel(app: Application) : NoriViewModel(app) {
    private fun warm() = viewModelScope.launch { MixStore.warm(nori) }

    private fun initial(taste: Boolean): List<MixCard> =
        MixStore.lastCards?.takeIf { it.first == nori.settings.value.activeServerId && it.second == taste }?.third
            ?: mixTiles(taste).map { card(it) }

    // Starts with the tiles it can name right away, so the row is there from the first frame and only
    // the covers arrive later (the tile cross-fades to them).
    val cards: StateFlow<List<MixCard>> = combine(
        nori.favouritesHanded()
            .catch { withContext(Dispatchers.IO) { nori.core.mixFavourites(emptyList()) }; emit(Unit) }
            .onStart { emit(Unit) },
        // The mixes are the taste model's: switched off, it draws nothing and offers only favourites.
        nori.settings.prefs.map { it.tasteModel }.distinctUntilChanged().onEach { if (it) warm() },
        MixStore.version,
    ) { _, taste, _ ->
        val server = nori.settings.value.activeServerId
        withContext(Dispatchers.IO) { nori.core.mixCards(taste) }.map { card(it) }
            .also { MixStore.lastCards = Triple(server, taste, it) }
    }.stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), initial(nori.settings.value.tasteModel))
}

/** One mix page. Its list does not change while it is open unless asked to (favourites follow the hearts). */
@OptIn(ExperimentalCoroutinesApi::class)
class MixViewModel(app: Application) : NoriViewModel(app) {
    private val id = MutableStateFlow<String?>(null)

    fun open(id: String) {
        if (this.id.value == id) return
        this.id.value = id
        if (id != FAVOURITES_MIX) viewModelScope.launch { MixStore.ensure(nori, id) }
    }

    /** Another draw of the same mix. */
    fun refresh() { id.value?.let { viewModelScope.launch { MixStore.ensure(nori, it, again = true) } } }

    val ui: StateFlow<Load<MixPage>> = id.filterNotNull().flatMapLatest { id ->
        val changes: Flow<Any> = if (id == FAVOURITES_MIX) nori.favouritesHanded() else MixStore.version
        changes.map { withContext(Dispatchers.IO) { nori.core.mixPage(id) } }
            .transform { found ->
                when (found) {
                    is MixLookup.Unknown -> throw IllegalArgumentException(found.message)
                    is MixLookup.Ready -> emit(page(found.sheet))
                    MixLookup.NotDrawn -> {}
                }
            }.distinctUntilChanged()
    }.asLoad()
}
