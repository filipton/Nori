package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.data.StarKind
import dev.nori.music.downloads.DownloadState
import dev.nori.music.downloads.DownloadMark
import dev.nori.music.ffi.DownloadSections
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.stateIn
import dev.nori.music.ffi.Album
import dev.nori.music.ffi.Playlist
import dev.nori.music.ffi.Song
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import dev.nori.music.settings.SwipeAction
import dev.nori.music.settings.TapAction
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.last
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch

/** Everything that can be done to a song, album or playlist from any screen. One instance per activity. */
class ActionsViewModel(app: Application) : NoriViewModel(app) {
    private val _messages = Channel<String>(Channel.CONFLATED)
    /** One-line confirmations and failures, for a snackbar or whatever the UI uses. */
    val messages = _messages.receiveAsFlow()
    /** This session's star changes, so every heart on screen can prefer them over its snapshot. */
    val starMarks: StateFlow<Map<String, Boolean>> = nori.library.starMarks
    val downloads: StateFlow<DownloadState> = nori.downloads.state

    // A process started in the background could not restart the download service; with a screen up it can.
    init { nori.downloads.resume() }

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
        when (nori.settings.value.tapAction) {
            TapAction.PLAY_LIST -> nori.player.play(songs, index)
            TapAction.PLAY_ONE -> nori.player.play(listOf(songs[index]))
            TapAction.QUEUE -> enqueue(listOf(songs[index]))
            TapAction.PLAY_NEXT -> playNext(listOf(songs[index]))
        }
    }

    /** What swiping a song row right and left does, as set in Settings. */
    val swipes: Pair<SwipeAction, SwipeAction> get() = nori.settings.value.let { it.swipeRight to it.swipeLeft }

    /** Every album of an artist, in order, as one list of songs. */
    private suspend fun artistSongs(albums: List<Album>): List<Song> = albums.filterNot { it.isExternal }.flatMap { runCatching { nori.library.albumSongs(it.id) }.getOrDefault(emptyList()) }
    fun playArtist(albums: List<Album>, shuffle: Boolean = false) = attempt(null) { nori.player.play(artistSongs(albums), shuffle = shuffle) }
    fun queueArtist(albums: List<Album>) = attempt(null) { enqueue(artistSongs(albums)) }
    fun downloadArtist(albums: List<Album>) = attempt(null) { download(artistSongs(albums)) }

    fun play(songs: List<Song>, index: Int = 0) = nori.player.play(songs, index)

    /**
     * For the debug test bridge: one-word actions a check needs to drive, so a script never has to find
     * a button on screen. "download <ref>", "star <ref>", "pause", "resume", "next", "previous".
     */
    fun testAction(what: String, player: dev.nori.music.app.vm.PlayerViewModel) = attempt(null) {
        val verb = what.substringBefore(' ')
        val ref = what.substringAfter(' ', "")
        val songs = if (ref.isEmpty()) emptyList() else when {
            ref.startsWith("album:") -> nori.library.album(ref.substringAfter(':')).first().songs
            ref.startsWith("song:") -> listOfNotNull(nori.library.song(ref.substringAfter(':')))
            ref.startsWith("search:") -> nori.library.search(ref.substringAfter(':')).songs.take(1)
            else -> emptyList()
        }
        when (verb) {
            // Lyrics load only while the lyrics panel is watching them, which a headless check is not:
            // this asks for them the same way the panel does and parks the answer for the state dump.
            "lyrics" -> {
                val song = songs.firstOrNull() ?: nori.player.state.value.current ?: return@attempt
                // The flow emits the server's answer first and the LRCLIB fallback second; the last one
                // is the one the screen would end up showing.
                lastLyrics = nori.library.lyricsFor(song, nori.settings.value.thirdPartyLookups).last()
            }
            "seek" -> nori.player.seekTo(ref.toLongOrNull() ?: 0L)
            // "dac <name>@44100/16,96000/24" pretends a USB DAC with those bit-perfect modes is attached;
            // "dac off" hands the app back to the real audio system. See DacSource.mock.
            "dac" -> {
                val off = ref.isEmpty() || ref == "off"
                nori.dac.testSource(if (off) null else dev.nori.music.playback.DacSource.mock(ref))
                nori.outputs.testUsb(if (off) null else ref.substringBefore('@').ifEmpty { "Mock DAC" })
            }
            "download" -> download(songs)
            // Everything not yet downloaded is dropped; finished downloads stay.
            "canceldownloads" -> cancelAllDownloads()
            "star" -> songs.firstOrNull()?.let { star(it, !it.starred) }
            // "notification favourite" / "notification shuffle": the session command the notification's button sends.
            "notification" -> nori.player.pressSessionButton(if (ref == "shuffle") dev.nori.music.playback.PlaybackService.CMD_SHUFFLE else dev.nori.music.playback.PlaybackService.CMD_FAVOURITE)
            "pause" -> player.toggle()
            "resume" -> player.toggle()
            "next" -> player.next()
            "previous" -> player.previous()
            // What the equalizer screen sends while it is open: the shallow buffer for live
            // tweaking, then back. For the checks that the deep buffer returns afterwards.
            "tuning" -> nori.player.setTuning(ref == "on")
            "enqueue" -> enqueue(songs)
            "playnext" -> playNext(songs)
            "shuffle" -> player.toggleShuffle()
            // "newplaylist <name>|<ref>": the checks create one, look for it on the server, then delete it.
            "newplaylist" -> {
                val name = ref.substringBefore('|')
                val pick = ref.substringAfter('|', "")
                val tracks = if (pick.startsWith("search:")) nori.library.search(pick.substringAfter(':')).songs.take(1) else emptyList()
                nori.library.createPlaylist(name, tracks.map { it.id })
            }
        }
    }

    /** The last lyrics the test bridge asked for, so the state dump can report what arrived. */
    @Volatile var lastLyrics: dev.nori.music.data.FoundLyrics? = null
        private set

    /** For the debug test bridge: "song:<id>", "album:<id>" or "search:<text>" (first song hit). */
    fun playByRef(ref: String) = attempt(null) {
        val arg = ref.substringAfter(':')
        val songs = when {
            ref.startsWith("album:") -> nori.library.album(arg).first().songs
            ref.startsWith("song:") -> listOfNotNull(nori.library.song(arg))
            ref.startsWith("search:") -> nori.library.search(arg).songs.take(1)
            // Straight from what is already on the device: the only way to start playback with the
            // network off, and therefore the only honest test of offline playback.
            ref.startsWith("downloaded:") -> nori.downloads.state.value.done.drop(arg.toIntOrNull() ?: 0).take(1)
            else -> emptyList()
        }
        if (songs.isNotEmpty()) nori.player.play(songs, 0)
    }
    /** Spreads artists and albums apart (in the core) unless the user prefers a plain random order. */
    fun shuffle(songs: List<Song>) {
        if (songs.isEmpty()) return
        if (nori.settings.value.weightedShuffle && songs.size > 2) {
            nori.player.playShuffledOrder(nori.library.shuffled(songs, System.nanoTime()))
        } else {
            nori.player.play(songs, shuffle = true)
        }
    }

    fun instantMix(song: Song) = attempt(null) {
        val mix = nori.library.mix(dev.nori.music.data.Mix.INSTANT, System.nanoTime(), song.id)
        if (mix.isEmpty()) startRadio(song) else nori.player.play(mix)
    }

    fun excludeFromMixes(song: Song) = attempt("Excluded from mixes") { nori.library.excludeFromMixes(song.id, true) }

    fun exportM3u(name: String, songs: List<Song>): String = nori.library.m3uExport(name, songs)

    /** Creates a server playlist from an M3U file; tracks that are not in the index are reported, not guessed. */
    fun importM3u(name: String, text: String) = attempt(null) {
        val matched = nori.library.m3uImport(text)
        val found = matched.filterNotNull()
        if (found.isEmpty()) _messages.send("None of the ${matched.size} entries are on this phone yet. Update the offline search first.")
        else { nori.library.createPlaylist(name, found.map { it.id }); _messages.send("Imported ${found.size} of ${matched.size} tracks into $name") }
    }
    fun playNext(songs: List<Song>) { nori.player.playNext(songs); _messages.trySend("Playing next") }
    fun enqueue(songs: List<Song>) { nori.player.enqueue(songs); _messages.trySend("Added to queue") }

    fun playAlbum(a: Album, shuffle: Boolean = false) = attempt(null) { nori.player.play(nori.library.albumSongs(a.id), shuffle = shuffle) }
    fun playPlaylist(p: Playlist) = attempt(null) { nori.player.play(nori.library.playlistSongs(p.id)) }
    fun shuffleAll() = attempt(null) { nori.player.play(nori.library.randomSongs(200)) }

    /** An endless-ish mix seeded from one song. */
    fun startRadio(song: Song) = attempt(null) {
        val similar = nori.library.similarSongs(song.id).filter { it.id != song.id }
        nori.player.play(listOf(song) + similar.ifEmpty { nori.library.randomSongs(50, song.genre) })
    }

    /** Picks up the queue another device (or the web player) left on the server. */
    fun resumeFromServer() = attempt(null) {
        val q = nori.library.pullQueue()
        if (q.songs.isEmpty()) _messages.send("No queue saved on the server") else { nori.player.play(q.songs, q.index.toInt()); nori.player.seekTo(q.positionMs.toLong()) }
    }

    /**
     * octo-fiesta downloads a provider item into the library when it is starred: a song on its own, an album
     * or playlist in full, on the server, without streaming it to the phone.
     */
    fun addToLibrary(id: String, isAlbum: Boolean) = attempt("The server is downloading it into your library") {
        nori.library.star(if (isAlbum) StarKind.ALBUM else StarKind.SONG, id, true)
    }

    fun star(song: Song, on: Boolean) = favourite(on) { nori.library.star(StarKind.SONG, song.id, on) }
    fun starAlbum(id: String, on: Boolean) = favourite(on) { nori.library.star(StarKind.ALBUM, id, on) }
    fun starArtist(id: String, on: Boolean) = favourite(on) { nori.library.star(StarKind.ARTIST, id, on) }

    /**
     * The heart changes the moment it is pressed (starMarks), so its message does too: it used to wait
     * for the server, and a quick favourite-unfavourite showed "Added" long after the heart was empty
     * again. Only a failure comes back afterwards. The message can be switched off in Settings.
     */
    private fun favourite(on: Boolean, block: suspend () -> Unit) {
        if (nori.settings.value.favouriteNotice) _messages.trySend(if (on) "Added to favourites" else "Removed from favourites")
        attempt(null, block)
    }

    private val _shares = Channel<String>(Channel.BUFFERED)
    /** Links ready to hand to the system share sheet. */
    val shares = _shares.receiveAsFlow()
    fun share(id: String) = attempt(null) { _shares.send(nori.library.share(id)) }

    fun download(songs: List<Song>) {
        nori.downloads.download(songs)
        warmCovers(songs)
        _messages.trySend("Downloading ${songs.size} song${if (songs.size == 1) "" else "s"}")
    }

    /**
     * Fetches the artwork of songs being downloaded into the image cache, at the two sizes the app
     * asks for. Covers are only kept once something has drawn them, so a song downloaded from a menu -
     * without its cover ever being on screen - arrives on the device with no picture, and shows a blank
     * plate for the rest of its life offline.
     */
    private fun warmCovers(songs: List<Song>) = viewModelScope.launch(kotlinx.coroutines.Dispatchers.IO) {
        val context = getApplication<Application>()
        val loader = coil3.SingletonImageLoader.get(context)
        songs.asSequence().mapNotNull { it.coverArt }.filterNot { it.startsWith("ext-") || it.startsWith("pl-") }
            .distinct().take(500)
            .forEach { art ->
                for (size in intArrayOf(320, 800)) {
                    loader.enqueue(
                        coil3.request.ImageRequest.Builder(context)
                            .data(nori.library.coverUrl(art, size)).size(size).build(),
                    )
                }
            }
    }

    /** Gives the downloads back: the same menu entry that offered them should be able to take them away. */
    fun undownload(songs: List<Song>) {
        nori.downloads.remove(songs.map { it.id })
        _messages.trySend("Removed ${songs.size} download${if (songs.size == 1) "" else "s"}")
    }
    fun downloadAlbum(a: Album) = attempt(null) { download(nori.library.albumSongs(a.id)) }
    fun removeDownloads(ids: List<String>) = nori.downloads.remove(ids)

    /** What each download this session touched is doing; see [dev.nori.music.downloads.Downloads.marks]. */
    val downloadMarks: StateFlow<Map<String, DownloadMark>> = nori.downloads.marks

    /**
     * The downloads screen's lists: downloading, waiting (in the order they will run), failed, and
     * finished this session - split in the core, asked again when the index or a phase changes, and
     * only while the screen is watching. Null until the first answer, which is not the same as empty.
     */
    val downloadSections: StateFlow<DownloadSections?> =
        combine(nori.downloads.state, nori.downloads.marks) { _, _ -> runCatching { nori.core.downloadSections() }.getOrNull() }
            .flowOn(kotlinx.coroutines.Dispatchers.IO)
            .stateIn<DownloadSections?>(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    fun retryDownloads(songs: List<Song>) = nori.downloads.retry(songs)
    fun cancelDownloads(songs: List<Song>) = nori.downloads.cancel(songs.map { it.id })
    fun cancelAllDownloads() = nori.downloads.cancelAll()

    suspend fun playlists(): List<Playlist> = runCatching { nori.library.playlists().first() }.getOrDefault(emptyList())
    fun addToPlaylist(p: Playlist, songs: List<Song>) = attempt("Added to ${p.name}") { nori.library.addToPlaylist(p.id, songs.map { it.id }) }
    fun addToNewPlaylist(name: String, songs: List<Song>) = attempt("Created $name") { nori.library.createPlaylist(name, songs.map { it.id }) }
}
