package dev.flint.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.flint.music.data.StarKind
import dev.flint.music.downloads.DownloadState
import dev.flint.music.ffi.Album
import dev.flint.music.ffi.Playlist
import dev.flint.music.ffi.Song
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch

/** Everything that can be done to a song, album or playlist from any screen. One instance per activity. */
class ActionsViewModel(app: Application) : FlintViewModel(app) {
    private val _messages = Channel<String>(Channel.BUFFERED)
    /** One-line confirmations and failures, for a snackbar or whatever the UI uses. */
    val messages = _messages.receiveAsFlow()
    val downloads: StateFlow<DownloadState> = flint.downloads.state

    private fun attempt(done: String?, block: suspend () -> Unit) = viewModelScope.launch {
        try { block(); done?.let { _messages.send(it) } } catch (e: Exception) { _messages.send(e.message ?: "Failed") }
    }

    fun play(songs: List<Song>, index: Int = 0) = flint.player.play(songs, index)
    fun shuffle(songs: List<Song>) = flint.player.play(songs, shuffle = true)
    fun playNext(songs: List<Song>) { flint.player.playNext(songs); _messages.trySend("Playing next") }
    fun enqueue(songs: List<Song>) { flint.player.enqueue(songs); _messages.trySend("Added to queue") }

    fun playAlbum(a: Album, shuffle: Boolean = false) = attempt(null) { flint.player.play(flint.library.albumSongs(a.id), shuffle = shuffle) }
    fun playPlaylist(p: Playlist) = attempt(null) { flint.player.play(flint.library.playlistSongs(p.id)) }
    fun shuffleAll() = attempt(null) { flint.player.play(flint.library.randomSongs(200)) }

    /** An endless-ish mix seeded from one song. */
    fun startRadio(song: Song) = attempt(null) {
        val similar = flint.library.similarSongs(song.id).filter { it.id != song.id }
        flint.player.play(listOf(song) + similar.ifEmpty { flint.library.randomSongs(50, song.genre) })
    }

    /** Picks up the queue another device (or the web player) left on the server. */
    fun resumeFromServer() = attempt(null) {
        val q = flint.library.pullQueue()
        if (q.songs.isEmpty()) _messages.send("No queue saved on the server") else { flint.player.play(q.songs, q.index.toInt()); flint.player.seekTo(q.positionMs.toLong()) }
    }

    fun star(song: Song, on: Boolean) = attempt(if (on) "Added to favourites" else "Removed from favourites") { flint.library.star(StarKind.SONG, song.id, on) }
    fun starAlbum(id: String, on: Boolean) = attempt(if (on) "Added to favourites" else "Removed from favourites") { flint.library.star(StarKind.ALBUM, id, on) }
    fun starArtist(id: String, on: Boolean) = attempt(if (on) "Added to favourites" else "Removed from favourites") { flint.library.star(StarKind.ARTIST, id, on) }
    fun rate(song: Song, rating: Int) = attempt("Rated") { flint.library.rate(song.id, rating) }

    fun download(songs: List<Song>) { flint.downloads.download(songs); _messages.trySend("Downloading ${songs.size} song${if (songs.size == 1) "" else "s"}") }
    fun downloadAlbum(a: Album) = attempt(null) { download(flint.library.albumSongs(a.id)) }
    fun removeDownloads(ids: List<String>) = flint.downloads.remove(ids)

    suspend fun playlists(): List<Playlist> = runCatching { flint.library.playlists().first() }.getOrDefault(emptyList())
    fun addToPlaylist(p: Playlist, songs: List<Song>) = attempt("Added to ${p.name}") { flint.library.addToPlaylist(p.id, songs.map { it.id }) }
    fun addToNewPlaylist(name: String, songs: List<Song>) = attempt("Created $name") { flint.library.createPlaylist(name, songs.map { it.id }) }
}
