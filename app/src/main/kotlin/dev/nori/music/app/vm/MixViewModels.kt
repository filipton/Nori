package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.Nori
import dev.nori.music.data.Mix
import dev.nori.music.ffi.Song
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

/**
 * One "For you" mix: [id] is the route, [kind] is what the core draws, [title] is what the tile says.
 * Discover Daily and Discover Weekly both call the same taste-based draw; only the seed period differs.
 */
private data class MixSpec(val id: String, val kind: Mix, val title: String, val weekly: Boolean = false)

/** The mixes "For you" offers, in the order it offers them. The id is what the `mix/{id}` route carries. */
private val MIXES = listOf(
    MixSpec("quick-picks", Mix.QUICK_PICKS, "Quick picks"),
    MixSpec("discover", Mix.DISCOVER, "Discover"),
    MixSpec("discover-weekly", Mix.DISCOVER, "Discover Weekly", weekly = true),
    MixSpec("listen-again", Mix.LISTEN_AGAIN, "Listen again"),
    MixSpec("top", Mix.TOP, "Your top songs"),
)

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
private fun NoriViewModel.coversOf(songs: List<Song>?): List<String> =
    songs.orEmpty().asSequence().mapNotNull { it.coverArt }.distinct().take(4).mapNotNull { cover(it, 320) }.toList()

/**
 * Today's (or this week's) draw of each mix, shared by the Home tiles and the mix pages, so the covers
 * on a tile, the list on its page and what plays are one list. Drawn once per period (day, or ISO week
 * for Discover Weekly) or when the page asks for another; never written to the server. Held in memory:
 * after a restart the same seed over the same index draws the same mix again.
 *
 * The algorithm is on-device: play counts, skips, stars and genres in the local index (see mixes.rs).
 * Last.fm / ListenBrainz stay on the server — Navidrome already scrobbles there.
 */
internal object MixStore {
    data class Drawn(val songs: List<Song>, val period: Long, val generation: Int)

    private val _drawn = MutableStateFlow<Map<String, Drawn>>(emptyMap())
    val drawn: StateFlow<Map<String, Drawn>> = _drawn
    private val lock = Mutex()

    fun key(nori: Nori, id: String) = "${nori.settings.value.activeServerId}|$id"

    /** Draws mix [id] unless this period's draw is already here; [again] asks for a different one. */
    suspend fun ensure(nori: Nori, id: String, again: Boolean = false) = lock.withLock {
        val spec = MIXES.firstOrNull { it.id == id } ?: return@withLock
        val key = key(nori, id)
        val day = java.time.LocalDate.now().toEpochDay()
        // Weekly mixes share one seed for seven days so the tile does not churn every midnight.
        val period = if (spec.weekly) day / 7 else day
        val have = _drawn.value[key]
        if (have != null && have.period == period && !again) return@withLock
        val generation = if (have != null && have.period == period) have.generation + 1 else 0
        // Offset weekly seeds so they never collide with the same day's Discover draw.
        val seed = period * 1_000L + generation + if (spec.weekly) 7_000_000L else 0L
        val songs = runCatching { nori.library.mix(spec.kind, seed) }.getOrDefault(emptyList()).filter(::playable)
        // With no listening history yet the personal mixes are empty: what the server thinks is random stands in.
        val list = songs.ifEmpty { runCatching { nori.library.randomSongs(50) }.getOrDefault(emptyList()).filter(::playable) }
        _drawn.update { it + (key to Drawn(list.distinctBy { s -> s.id }, period, generation)) }
    }
}

/**
 * The starred songs, re-asked on every star change, with this session's marks applied at once: an
 * unstarred song leaves the list under the finger instead of after the server's answer.
 */
@OptIn(ExperimentalCoroutinesApi::class)
private fun Nori.favouriteSongs(): Flow<List<Song>> =
    combine(library.starsVersion.flatMapLatest { library.starred() }.map { it.songs }, library.starMarks) { songs, marks ->
        songs.filter { playable(it) && marks["id:${it.id}"] != false }.distinctBy { it.id }
    }

/** The "For you" row: favourites first, then the mixes the core draws from the index and the listening history. */
class MixesViewModel(app: Application) : NoriViewModel(app) {
    private fun cards(favourites: List<Song>, taste: Boolean, drawn: Map<String, MixStore.Drawn>): List<MixCard> =
        listOf(MixCard(FAVOURITES_MIX, "Favourites", coversOf(favourites), favourites = true)) +
            if (!taste) emptyList() else MIXES.map { spec ->
                MixCard(spec.id, spec.title, coversOf(drawn[MixStore.key(nori, spec.id)]?.songs), favourites = false)
            }

    /** Draws whichever mixes are missing or from the last period; a few milliseconds each, off the main thread. */
    private fun warm() = viewModelScope.launch { MIXES.forEach { MixStore.ensure(nori, it.id) } }

    // Starts with the tiles it can name right away, so the row is there from the first frame and only
    // the covers arrive later (the tile cross-fades to them).
    val cards: StateFlow<List<MixCard>> = combine(
        nori.favouriteSongs().catch { emit(emptyList()) }.onStart { emit(emptyList()) },
        // The mixes are the taste model's: switched off, it draws nothing and offers only favourites.
        nori.settings.prefs.map { it.tasteModel }.distinctUntilChanged().onEach { if (it) warm() },
        MixStore.drawn,
    ) { favourites, taste, drawn -> cards(favourites, taste, drawn) }
        .stateIn(viewModelScope, SharingStarted.WhileSubscribed(5_000), cards(emptyList(), nori.settings.value.tasteModel, MixStore.drawn.value))
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
        val spec = MIXES.firstOrNull { it.id == id }
        when {
            id == FAVOURITES_MIX -> nori.favouriteSongs().map { MixPage(id, "Favourites", it, coversOf(it), refreshable = false, favourites = true) }
            spec == null -> flow { throw IllegalArgumentException("There is no mix called $id") }
            else -> MixStore.drawn.mapNotNull { it[MixStore.key(nori, id)] }.distinctUntilChanged()
                .map { MixPage(id, spec.title, it.songs, coversOf(it.songs), refreshable = spec.kind != Mix.TOP, favourites = false) }
        }
    }.asLoad()
}
