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
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import dev.flint.music.settings.SwipeAction
import dev.flint.music.settings.TapAction
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.last
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

    // ---- selection mode: long-press a song anywhere, then act on the whole selection ----

    private val _selection = MutableStateFlow<List<Song>>(emptyList())
    val selection: StateFlow<List<Song>> = _selection
    fun toggleSelected(song: Song) = _selection.update { s -> if (s.any { it.id == song.id }) s.filterNot { it.id == song.id } else s + song }
    fun clearSelection() { _selection.value = emptyList() }

    /** What a plain tap on row [index] of [songs] does, as configured. */
    fun tap(songs: List<Song>, index: Int) {
        if (_selection.value.isNotEmpty()) return toggleSelected(songs[index])
        when (flint.settings.value.tapAction) {
            TapAction.PLAY_LIST -> flint.player.play(songs, index)
            TapAction.PLAY_ONE -> flint.player.play(listOf(songs[index]))
            TapAction.QUEUE -> enqueue(listOf(songs[index]))
            TapAction.PLAY_NEXT -> playNext(listOf(songs[index]))
        }
    }

    fun swipe(song: Song, right: Boolean) {
        when (if (right) flint.settings.value.swipeRight else flint.settings.value.swipeLeft) {
            SwipeAction.NONE -> {}
            SwipeAction.QUEUE -> enqueue(listOf(song))
            SwipeAction.PLAY_NEXT -> playNext(listOf(song))
            SwipeAction.FAVOURITE -> star(song, !song.starred)
            SwipeAction.DOWNLOAD -> download(listOf(song))
        }
    }

    val swipeEnabled: Boolean get() = flint.settings.value.let { it.swipeLeft != SwipeAction.NONE || it.swipeRight != SwipeAction.NONE }

    /** Every album of an artist, in order, as one list of songs. */
    private suspend fun artistSongs(albums: List<Album>): List<Song> = albums.filterNot { it.isExternal }.flatMap { runCatching { flint.library.albumSongs(it.id) }.getOrDefault(emptyList()) }
    fun playArtist(albums: List<Album>, shuffle: Boolean = false) = attempt(null) { flint.player.play(artistSongs(albums), shuffle = shuffle) }
    fun queueArtist(albums: List<Album>) = attempt(null) { enqueue(artistSongs(albums)) }
    fun downloadArtist(albums: List<Album>) = attempt(null) { download(artistSongs(albums)) }

    fun play(songs: List<Song>, index: Int = 0) = flint.player.play(songs, index)

    /**
     * For the debug test bridge: one-word actions a check needs to drive, so a script never has to find
     * a button on screen. "download <ref>", "star <ref>", "pause", "resume", "next", "previous".
     */
    fun testAction(what: String, player: dev.flint.music.app.vm.PlayerViewModel) = attempt(null) {
        val verb = what.substringBefore(' ')
        val ref = what.substringAfter(' ', "")
        val songs = if (ref.isEmpty()) emptyList() else when {
            ref.startsWith("album:") -> flint.library.album(ref.substringAfter(':')).first().songs
            ref.startsWith("song:") -> listOfNotNull(flint.library.song(ref.substringAfter(':')))
            ref.startsWith("search:") -> flint.library.search(ref.substringAfter(':')).songs.take(1)
            else -> emptyList()
        }
        when (verb) {
            // Lyrics load only while the lyrics panel is watching them, which a headless check is not:
            // this asks for them the same way the panel does and parks the answer for the state dump.
            "lyrics" -> {
                val song = songs.firstOrNull() ?: flint.player.state.value.current ?: return@attempt
                // The flow emits the server's answer first and the LRCLIB fallback second; the last one
                // is the one the screen would end up showing.
                lastLyrics = flint.library.lyricsFor(song, flint.settings.value.thirdPartyLookups).last()
            }
            "seek" -> flint.player.seekTo(ref.toLongOrNull() ?: 0L)
            "download" -> download(songs)
            "star" -> songs.firstOrNull()?.let { star(it, !it.starred) }
            "pause" -> player.toggle()
            "resume" -> player.toggle()
            "next" -> player.next()
            "previous" -> player.previous()
            "enqueue" -> enqueue(songs)
        }
    }

    /** The last lyrics the test bridge asked for, so the state dump can report what arrived. */
    @Volatile var lastLyrics: dev.flint.music.data.FoundLyrics? = null
        private set

    /** For the debug test bridge: "song:<id>", "album:<id>" or "search:<text>" (first song hit). */
    fun playByRef(ref: String) = attempt(null) {
        val arg = ref.substringAfter(':')
        val songs = when {
            ref.startsWith("album:") -> flint.library.album(arg).first().songs
            ref.startsWith("song:") -> listOfNotNull(flint.library.song(arg))
            ref.startsWith("search:") -> flint.library.search(arg).songs.take(1)
            // Straight from what is already on the device: the only way to start playback with the
            // network off, and therefore the only honest test of offline playback.
            ref.startsWith("downloaded:") -> flint.downloads.state.value.done.drop(arg.toIntOrNull() ?: 0).take(1)
            else -> emptyList()
        }
        if (songs.isNotEmpty()) flint.player.play(songs, 0)
    }
    /** Spreads artists and albums apart (in the core) unless the user prefers a plain random order. */
    fun shuffle(songs: List<Song>) {
        if (flint.settings.value.weightedShuffle && songs.size > 2) flint.player.play(flint.library.shuffled(songs, System.nanoTime()))
        else flint.player.play(songs, shuffle = true)
    }

    fun instantMix(song: Song) = attempt(null) {
        val mix = flint.library.mix(dev.flint.music.data.Mix.INSTANT, System.nanoTime(), song.id)
        if (mix.isEmpty()) startRadio(song) else flint.player.play(mix)
    }

    fun excludeFromMixes(song: Song) = attempt("Excluded from mixes") { flint.library.excludeFromMixes(song.id, true) }

    fun exportM3u(name: String, songs: List<Song>): String = flint.library.m3uExport(name, songs)

    /** Creates a server playlist from an M3U file; tracks that are not in the index are reported, not guessed. */
    fun importM3u(name: String, text: String) = attempt(null) {
        val matched = flint.library.m3uImport(text)
        val found = matched.filterNotNull()
        if (found.isEmpty()) _messages.send("None of the ${matched.size} entries are in the offline index. Sync it first.")
        else { flint.library.createPlaylist(name, found.map { it.id }); _messages.send("Imported ${found.size} of ${matched.size} tracks into $name") }
    }
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

    /**
     * octo-fiesta downloads a provider item into the library when it is starred: a song on its own, an album
     * or playlist in full, on the server, without streaming it to the phone.
     */
    fun addToLibrary(id: String, isAlbum: Boolean) = attempt("The server is downloading it into your library") {
        flint.library.star(if (isAlbum) StarKind.ALBUM else StarKind.SONG, id, true)
    }

    fun star(song: Song, on: Boolean) = attempt(if (on) "Added to favourites" else "Removed from favourites") { flint.library.star(StarKind.SONG, song.id, on) }
    fun starAlbum(id: String, on: Boolean) = attempt(if (on) "Added to favourites" else "Removed from favourites") { flint.library.star(StarKind.ALBUM, id, on) }
    fun starArtist(id: String, on: Boolean) = attempt(if (on) "Added to favourites" else "Removed from favourites") { flint.library.star(StarKind.ARTIST, id, on) }
    fun rate(song: Song, rating: Int) = attempt("Rated") { flint.library.rate(song.id, rating) }

    private val _shares = Channel<String>(Channel.BUFFERED)
    /** Links ready to hand to the system share sheet. */
    val shares = _shares.receiveAsFlow()
    fun share(id: String) = attempt(null) { _shares.send(flint.library.share(id)) }

    fun download(songs: List<Song>) { flint.downloads.download(songs); _messages.trySend("Downloading ${songs.size} song${if (songs.size == 1) "" else "s"}") }
    fun downloadAlbum(a: Album) = attempt(null) { download(flint.library.albumSongs(a.id)) }
    fun removeDownloads(ids: List<String>) = flint.downloads.remove(ids)

    suspend fun playlists(): List<Playlist> = runCatching { flint.library.playlists().first() }.getOrDefault(emptyList())
    fun addToPlaylist(p: Playlist, songs: List<Song>) = attempt("Added to ${p.name}") { flint.library.addToPlaylist(p.id, songs.map { it.id }) }
    fun addToNewPlaylist(name: String, songs: List<Song>) = attempt("Created $name") { flint.library.createPlaylist(name, songs.map { it.id }) }
}
