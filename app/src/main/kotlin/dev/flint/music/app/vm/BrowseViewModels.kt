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
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.onStart
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.ExperimentalCoroutinesApi

data class HomeUi(val recent: List<Album> = emptyList(), val newest: List<Album> = emptyList(), val frequent: List<Album> = emptyList(), val random: List<Album> = emptyList())

class HomeViewModel(app: Application) : FlintViewModel(app) {
    private fun row(sort: AlbumSort) = flint.library.albums(sort, size = 20).catch { emit(emptyList()) }.onStart { emit(emptyList()) }

    val ui: StateFlow<Load<HomeUi>> =
        combine(row(AlbumSort.RECENT), row(AlbumSort.NEWEST), row(AlbumSort.FREQUENT), row(AlbumSort.RANDOM), ::HomeUi).asLoad()
}

/** The album grid: one sort order at a time, pages appended as the list nears its end. */
@OptIn(ExperimentalCoroutinesApi::class)
class AlbumsViewModel(app: Application) : FlintViewModel(app) {
    private val pageSize = 60
    private val _sort = MutableStateFlow(AlbumSort.BY_NAME)
    val sort: StateFlow<AlbumSort> = _sort
    private val _albums = MutableStateFlow<List<Album>>(emptyList())
    val albums: StateFlow<List<Album>> = _albums
    private var loading = false
    private var exhausted = false

    init { loadMore() }

    fun setSort(s: AlbumSort) {
        if (s == _sort.value) return
        _sort.value = s
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
    val starred: StateFlow<Load<Starred>> = flint.library.starred().asLoad()
}

class GenresViewModel(app: Application) : FlintViewModel(app) {
    val genres: StateFlow<Load<List<Genre>>> = flint.library.genres().asLoad()
}

class RadioViewModel(app: Application) : FlintViewModel(app) {
    val stations: StateFlow<Load<List<RadioStation>>> = flint.library.radio().asLoad()
    fun play(s: RadioStation) = flint.player.playRadio(s)
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
