package dev.flint.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.flint.music.data.AlbumSort
import dev.flint.music.ffi.Album
import dev.flint.music.ffi.AlbumDetail
import dev.flint.music.ffi.Artist
import dev.flint.music.ffi.ArtistDetail
import dev.flint.music.ffi.ArtistInfo
import dev.flint.music.ffi.Genre
import dev.flint.music.ffi.Playlist
import dev.flint.music.ffi.PlaylistDetail
import dev.flint.music.ffi.RadioStation
import dev.flint.music.ffi.Song
import dev.flint.music.ffi.Starred
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.distinctUntilChanged
import dev.flint.music.settings.HomeRow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.ExperimentalCoroutinesApi

data class HomeUi(val rows: List<Pair<HomeRow, List<Album>>> = emptyList(), val pinned: List<Playlist> = emptyList())

class HomeViewModel(app: Application) : FlintViewModel(app) {
    // The app was opened: a good moment to replay whatever was starred, rated or played while offline.
    init { viewModelScope.launch { runCatching { flint.library.flushPending() } } }

    private fun row(sort: AlbumSort) = flint.library.albums(sort, size = 20).catch { emit(emptyList()) }.onStart { emit(emptyList()) }

    private fun source(r: HomeRow) = when (r) {
        HomeRow.RECENT -> row(AlbumSort.RECENT); HomeRow.NEWEST -> row(AlbumSort.NEWEST); HomeRow.FREQUENT -> row(AlbumSort.FREQUENT)
        HomeRow.RANDOM -> row(AlbumSort.RANDOM); HomeRow.STARRED -> row(AlbumSort.STARRED); HomeRow.PINNED -> flowOf(emptyList())
    }

    /** Only the rows the user kept are requested at all; a hidden shelf costs no request. */
    @OptIn(ExperimentalCoroutinesApi::class)
    val ui: StateFlow<Load<HomeUi>> = flint.settings.prefs.map { it.homeRows to it.pinnedPlaylists }.distinctUntilChanged().flatMapLatest { (rows, pins) ->
        val pinned = if (HomeRow.PINNED in rows && pins.isNotEmpty()) flint.library.playlists().map { all -> all.filter { it.id in pins } }.catch { emit(emptyList()) }.onStart { emit(emptyList()) } else flowOf(emptyList())
        combine(combine(rows.map { r -> source(r).map { r to it } }) { it.toList() }.onStart { emit(emptyList()) }, pinned) { shelves, p -> HomeUi(shelves, p) }
    }.asLoad()
}

/** The album grid: one sort order at a time, pages appended as the list nears its end. */
@OptIn(ExperimentalCoroutinesApi::class)
class AlbumsViewModel(app: Application) : FlintViewModel(app) {
    private val pageSize = 60
    private val _sort = MutableStateFlow(flint.settings.value.listPrefs["albums.sort"]?.let { n -> AlbumSort.entries.firstOrNull { it.name == n } } ?: AlbumSort.BY_NAME)
    val sort: StateFlow<AlbumSort> = _sort
    private val _albums = MutableStateFlow<List<Album>>(emptyList())
    val albums: StateFlow<List<Album>> = _albums
    private var loading = false
    private var exhausted = false

    init { loadMore() }

    fun setSort(s: AlbumSort) {
        if (s == _sort.value) return
        _sort.value = s
        flint.settings.update { it.copy(listPrefs = it.listPrefs + ("albums.sort" to s.name)) }
        _albums.value = emptyList()
        exhausted = false
        loading = false
        loadMore()
    }

    fun loadMore() {
        if (loading || exhausted) return
        loading = true
        val sort = _sort.value
        val offset = _albums.value.size
        viewModelScope.launch {
            flint.library.albums(sort, pageSize, offset).catch { }.collect { page ->
                if (sort != _sort.value) return@collect
                _albums.update { it.take(offset) + page }
                exhausted = page.size < pageSize
            }
            loading = false
        }
    }
}

class ArtistsViewModel(app: Application) : FlintViewModel(app) {
    val artists: StateFlow<Load<List<Artist>>> = flint.library.artists().asLoad()
}

class PlaylistsViewModel(app: Application) : FlintViewModel(app) {
    private val refresh = MutableStateFlow(0)
    @OptIn(ExperimentalCoroutinesApi::class)
    val playlists: StateFlow<Load<List<Playlist>>> = refresh.flatMapLatest { flint.library.playlists() }.asLoad()

    fun create(name: String) = viewModelScope.launch { runCatching { flint.library.createPlaylist(name, emptyList()) }; refresh.value++ }
    fun delete(id: String) = viewModelScope.launch { runCatching { flint.library.deletePlaylist(id) }; refresh.value++ }
}

class StarredViewModel(app: Application) : FlintViewModel(app) {
    // Re-queried on every star change: the one-shot read would otherwise keep a removed favourite
    // until the screen is reopened. The stored answer paints first, so there is no loading flash.
    @OptIn(ExperimentalCoroutinesApi::class)
    // This session's marks are applied on top, so an unstarred item leaves the list at once rather than
    // when the server's new answer arrives.
    val starred: StateFlow<Load<Starred>> = combine(flint.library.starsVersion.flatMapLatest { flint.library.starred() }, flint.library.starMarks) { s, marks ->
        s.copy(
            artists = s.artists.filter { marks["artistId:${it.id}"] != false },
            albums = s.albums.filter { marks["albumId:${it.id}"] != false },
            songs = s.songs.filter { marks["id:${it.id}"] != false },
        )
    }.asLoad()
}

class GenresViewModel(app: Application) : FlintViewModel(app) {
    val genres: StateFlow<Load<List<Genre>>> = flint.library.genres().asLoad()
}

class RadioViewModel(app: Application) : FlintViewModel(app) {
    private val refresh = MutableStateFlow(0)
    @OptIn(ExperimentalCoroutinesApi::class)
    val stations: StateFlow<Load<List<RadioStation>>> = refresh.flatMapLatest { flint.library.radio() }.asLoad()
    fun play(s: RadioStation) = flint.player.playRadio(s)
    fun add(name: String, url: String) = viewModelScope.launch { runCatching { flint.library.createRadio(name, url) }; refresh.value++ }
    fun delete(id: String) = viewModelScope.launch { runCatching { flint.library.deleteRadio(id) }; refresh.value++ }
}

// ---- detail screens; ids arrive through [open] because the UI layer owns navigation ----

@OptIn(ExperimentalCoroutinesApi::class)
abstract class DetailViewModel<T>(app: Application) : FlintViewModel(app) {
    protected val id = MutableStateFlow<String?>(null)
    fun open(id: String) { this.id.value = id }
    protected abstract fun load(id: String): kotlinx.coroutines.flow.Flow<T>
    val ui: StateFlow<Load<T>> by lazy { id.flatMapLatest { if (it == null) flow { } else load(it) }.asLoad() }
}

class AlbumViewModel(app: Application) : DetailViewModel<AlbumDetail>(app) {
    override fun load(id: String) = flint.library.album(id)
}

data class ArtistUi(val detail: ArtistDetail, val info: ArtistInfo?, val top: List<Song>)

class ArtistViewModel(app: Application) : DetailViewModel<ArtistUi>(app) {
    @OptIn(ExperimentalCoroutinesApi::class)
    override fun load(id: String) = flint.library.artist(id).flatMapLatest { d ->
        combine(
            flint.library.artistInfo(id).map<ArtistInfo, ArtistInfo?> { it }.catch { emit(null) }.onStart { emit(null) },
            flint.library.topSongs(d.artist.name).catch { emit(emptyList()) }.onStart { emit(emptyList()) },
        ) { info, top -> ArtistUi(d, info, top) }
    }
}

class PlaylistViewModel(app: Application) : DetailViewModel<PlaylistDetail>(app) {
    private val version = MutableStateFlow(0)
    @OptIn(ExperimentalCoroutinesApi::class)
    override fun load(id: String) = version.flatMapLatest { flint.library.playlist(id) }

    fun removeAt(index: Int) = viewModelScope.launch {
        val pid = id.value ?: return@launch
        runCatching { flint.library.removeFromPlaylist(pid, index) }
        version.value++
    }
}

class GenreViewModel(app: Application) : DetailViewModel<List<Song>>(app) {
    override fun load(id: String) = flow { emit(flint.library.songsByGenre(id)) }
}

enum class SongSort(val key: String, val label: String, val descending: Boolean = false) {
    TITLE("title", "Title"), ARTIST("artist", "Artist"), ALBUM("album", "Album"), YEAR("year", "Year", true),
    ADDED("created", "Added", true), PLAYS("playCount", "Most played", true), LONGEST("duration", "Longest", true),
}

/** Every song of the offline index, a page at a time. Nothing here touches the network. */
class SongsViewModel(app: Application) : FlintViewModel(app) {
    private val page = 200
    private val _sort = MutableStateFlow(flint.settings.value.listPrefs["songs.sort"]?.let { n -> SongSort.entries.firstOrNull { it.name == n } } ?: SongSort.TITLE)
    val sort: StateFlow<SongSort> = _sort
    private val _starred = MutableStateFlow(false)
    val starredOnly: StateFlow<Boolean> = _starred
    private val _songs = MutableStateFlow<List<Song>>(emptyList())
    val songs: StateFlow<List<Song>> = _songs
    private var years: IntRange? = null
    private var loading = false
    private var exhausted = false

    init { loadMore() }

    fun setYears(range: IntRange?) { if (range != years) { years = range; reset() } }
    fun setSort(s: SongSort) { _sort.value = s; flint.settings.update { it.copy(listPrefs = it.listPrefs + ("songs.sort" to s.name)) }; reset() }
    fun setStarredOnly(on: Boolean) { _starred.value = on; reset() }
    private fun reset() { _songs.value = emptyList(); exhausted = false; loading = false; loadMore() }

    fun loadMore() {
        if (loading || exhausted) return
        loading = true
        val (s, st, y, offset) = listOf(_sort.value, _starred.value, years, _songs.value.size)
        viewModelScope.launch {
            val next = runCatching { flint.library.browseSongs(_sort.value.key, _sort.value.descending, _starred.value, years, offset as Int, page) }.getOrDefault(emptyList())
            if (s == _sort.value && st == _starred.value && y == years) { _songs.update { it + next }; exhausted = next.size < page }
            loading = false
        }
    }
}

class DecadesViewModel(app: Application) : FlintViewModel(app) {
    val decades: StateFlow<Load<List<Genre>>> = flow { emit(flint.library.decades()) }.asLoad()
}

class FoldersViewModel(app: Application) : FlintViewModel(app) {
    val roots: StateFlow<Load<List<Artist>>> = flint.library.folders().asLoad()
}

class FolderViewModel(app: Application) : DetailViewModel<dev.flint.music.ffi.Directory>(app) {
    override fun load(id: String) = flint.library.folder(id)
}
