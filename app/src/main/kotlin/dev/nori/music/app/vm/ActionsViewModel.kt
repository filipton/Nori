package dev.nori.music.app.vm

import android.app.Application
import androidx.lifecycle.viewModelScope
import dev.nori.music.data.StarKind
import dev.nori.music.downloads.DownloadState
import dev.nori.music.downloads.DownloadMark
import dev.nori.music.ffi.transfers.DownloadSections
import dev.nori.music.net.said
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.conflate
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.stateIn
import dev.nori.music.ffi.model.Album
import dev.nori.music.ffi.model.Playlist
import dev.nori.music.ffi.model.Song
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.update
import dev.nori.music.settings.SwipeAction
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.last
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import dev.nori.music.ffi.words.Said
import dev.nori.music.ffi.queue.ShufflePlan
import dev.nori.music.ffi.queue.TapPlan
import dev.nori.music.ffi.queue.TestRef
import dev.nori.music.ffi.coverWants
import dev.nori.music.ffi.queue.shufflePlan
import dev.nori.music.ffi.queue.tapPlan
import dev.nori.music.ffi.queue.testRef
import dev.nori.music.ffi.words.words
import dev.nori.music.ffi.words.wordsDownloading
import dev.nori.music.ffi.words.wordsDownloadsRemoved
import dev.nori.music.ffi.wordsFavourite

/** Everything that can be done to a song, album or playlist from any screen. One instance per activity. */
class ActionsViewModel(app: Application) : NoriViewModel(app) {
    private val _messages = Channel<String>(Channel.CONFLATED)
    /** One-line confirmations and failures, for a snackbar or whatever the UI uses. */
    val messages = _messages.receiveAsFlow()
    /** This session's star changes, so every heart on screen can prefer them over its snapshot. */
    val starMarks: StateFlow<dev.nori.music.ffi.library.StarMarks> = nori.library.starMarks
    val downloads: StateFlow<DownloadState> = nori.downloads.state

    // A process started in the background could not restart the download service; with a screen up it can.
    init { nori.downloads.resume() }

    private fun attempt(done: String?, block: suspend () -> Unit) = viewModelScope.launch {
        try { block(); done?.let { _messages.send(it) } } catch (e: Exception) { _messages.send(e.said ?: words(Said.FAILED, "")) }
    }

    // ---- selection mode: long-press a song anywhere, then act on the whole selection ----

    private val _selection = MutableStateFlow<List<Song>>(emptyList())
    val selection: StateFlow<List<Song>> = _selection
    fun toggleSelected(song: Song) = _selection.update { s -> if (s.any { it.id == song.id }) s.filterNot { it.id == song.id } else s + song }
    fun clearSelection() { _selection.value = emptyList() }

    /** What a plain tap on row [index] of [songs] does: the core's answer from the settings (`tap_plan`). */
    fun tap(songs: List<Song>, index: Int) {
        when (tapPlan(_selection.value.isNotEmpty())) {
            TapPlan.SELECT -> toggleSelected(songs[index])
            TapPlan.PLAY_LIST -> nori.player.play(songs, index)
            TapPlan.PLAY_ONE -> nori.player.play(listOf(songs[index]))
            TapPlan.QUEUE -> enqueue(listOf(songs[index]))
            TapPlan.PLAY_NEXT -> playNext(listOf(songs[index]))
        }
    }

    /** What swiping a song row right and left does, as set in Settings. */
    val swipes: Pair<SwipeAction, SwipeAction> get() = nori.settings.value.let { it.swipeRight to it.swipeLeft }

    // By id: the core has the artist's albums from reading the page, so they need not be handed back.
    fun playArtist(artistId: String, shuffle: Boolean = false) = attempt(null) { nori.player.play(nori.library.artistSongs(artistId), shuffle = shuffle) }
    fun queueArtist(artistId: String) = attempt(null) { enqueue(nori.library.artistSongs(artistId)) }
    fun downloadArtist(artistId: String) = attempt(null) { download(nori.library.artistSongs(artistId)) }

    fun play(songs: List<Song>, index: Int = 0) = nori.player.play(songs, index)

    /**
     * For the debug test bridge: one-word actions a check needs to drive, so a script never has to find
     * a button on screen. "download <ref>", "star <ref>", "pause", "resume", "next", "previous".
     */
    fun testAction(what: String, player: dev.nori.music.app.vm.PlayerViewModel) = attempt(null) {
        val verb = what.substringBefore(' ')
        val ref = what.substringAfter(' ', "")
        val songs = songsOf(testRef(ref))
        when (verb) {
            // Lyrics load only while the lyrics panel is watching them, which a headless check is not:
            // this asks for them the same way the panel does and parks the answer for the state dump.
            "lyrics" -> {
                val song = songs.firstOrNull() ?: nori.player.state.value.current ?: return@attempt
                // The flow emits the server's answer first and the LRCLIB fallback second; the last one
                // is the one the screen would end up showing.
                lastLyrics = nori.library.lyricsFor(song).last()
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
                val tracks = (testRef(pick) as? TestRef.Search)?.let { songsOf(it) }.orEmpty()
                nori.library.createPlaylist(name, tracks.map { it.id })
            }
        }
    }

    /** The last lyrics the test bridge asked for, so the state dump can report what arrived. */
    @Volatile var lastLyrics: dev.nori.music.data.FoundLyrics? = null
        private set

    /** The songs a test bridge reference names (see the core's `test_ref`). */
    /**
     * What a test reference plays. Never a provider song: asking the server for an `ext-` item makes
     * octo-fiesta fetch it, so the test tools pass over them, even inside an album that mixes them in.
     */
    private suspend fun songsOf(ref: TestRef): List<Song> = when (ref) {
        is TestRef.Album -> nori.library.album(ref.id).first().songs.filterNot { it.id.startsWith("ext-") }
        is TestRef.Song -> listOfNotNull(nori.library.song(ref.id)).filterNot { it.id.startsWith("ext-") }
        is TestRef.Search -> nori.library.search(ref.text).songs.filterNot { it.id.startsWith("ext-") }.take(1)
        // Straight from what is already on the device: the only way to start playback with the
        // network off, and therefore the only honest test of offline playback.
        is TestRef.Downloaded -> withContext(Dispatchers.IO) { nori.downloads.state.value.done }.drop(ref.index.toInt()).take(1)
        TestRef.Nothing -> emptyList()
    }

    /** For the debug test bridge: "song:<id>", "album:<id>", "search:<text>" (first song hit) or "downloaded:<n>". */
    fun playByRef(ref: String) = attempt(null) {
        val songs = songsOf(testRef(ref))
        if (songs.isNotEmpty()) nori.player.play(songs, 0)
    }

    /** Spreads artists and albums apart (in the core) unless the user prefers a plain random order. */
    fun shuffle(songs: List<Song>) {
        when (val plan = shufflePlan(songs)) {
            ShufflePlan.Empty -> {}
            ShufflePlan.PlayerShuffle -> nori.player.play(songs, shuffle = true)
            is ShufflePlan.Order -> nori.player.playShuffledOrder(songs, plan.order)
        }
    }

    fun instantMix(song: Song) = attempt(null) { nori.player.play(nori.library.instantMix(song)) }

    fun excludeFromMixes(song: Song) = attempt(words(Said.EXCLUDED_FROM_MIXES, "")) { nori.library.excludeFromMixes(song.id, true) }

    fun exportM3u(name: String, songs: List<Song>): String = nori.library.m3uExport(name, songs)

    /** Creates a server playlist from an M3U file; tracks that are not in the index are reported, not guessed. */
    fun importM3u(name: String, text: String) = attempt(null) {
        val imported = withContext(Dispatchers.IO) { nori.core.m3uImport(name, text) }
        if (imported.songIds.isNotEmpty()) nori.library.createPlaylist(name, imported.songIds)
        _messages.send(imported.message)
    }
    fun playNext(songs: List<Song>) { nori.player.playNext(songs); _messages.trySend(words(Said.PLAYING_NEXT, "")) }
    fun enqueue(songs: List<Song>) { nori.player.enqueue(songs); _messages.trySend(words(Said.ADDED_TO_QUEUE, "")) }

    fun playAlbum(a: Album, shuffle: Boolean = false) = attempt(null) { nori.player.play(nori.library.albumSongs(a.id), shuffle = shuffle) }
    fun playPlaylist(p: Playlist) = attempt(null) { nori.player.play(nori.library.playlistSongs(p.id)) }
    fun shuffleAll() = attempt(null) { nori.player.play(nori.library.shuffleAll()) }

    /** An endless-ish mix seeded from one song. */
    fun startRadio(song: Song) = attempt(null) { nori.player.play(nori.library.radio(song)) }

    /** Picks up the queue another device (or the web player) left on the server. */
    fun resumeFromServer() = attempt(null) {
        // What to do with what the server kept is the core's (`resume_from_server`).
        when (val plan = nori.library.resumeFromServer()) {
            is dev.nori.music.ffi.library.ResumePlan.Nothing -> _messages.send(plan.message)
            is dev.nori.music.ffi.library.ResumePlan.Play -> { nori.player.play(plan.songs, plan.index.toInt()); nori.player.seekTo(plan.positionMs.toLong()) }
        }
    }

    /**
     * octo-fiesta downloads a provider item into the library when it is starred: a song on its own, an album
     * or playlist in full, on the server, without streaming it to the phone.
     */
    fun addToLibrary(id: String, isAlbum: Boolean) = attempt(words(Said.SERVER_DOWNLOADING, "")) {
        nori.library.star(if (isAlbum) StarKind.ALBUM else StarKind.SONG, id, true)
    }

    fun star(song: Song, on: Boolean) = favourite(on) { nori.library.star(StarKind.SONG, song.id, on) }
    fun starAlbum(id: String, on: Boolean) = favourite(on) { nori.library.star(StarKind.ALBUM, id, on) }
    fun starArtist(id: String, on: Boolean) = favourite(on) { nori.library.star(StarKind.ARTIST, id, on) }

    /**
     * The heart changes the moment it is pressed (starMarks), so its message does too: it used to wait
     * for the server, and a quick favourite-unfavourite showed "Added" long after the heart was empty
     * again. Only a failure comes back afterwards. The message can be switched off in Settings (the core
     * says nothing then).
     */
    private fun favourite(on: Boolean, block: suspend () -> Unit) {
        wordsFavourite(on)?.let { _messages.trySend(it) }
        attempt(null, block)
    }

    private val _shares = Channel<String>(Channel.BUFFERED)
    /** Links ready to hand to the system share sheet. */
    val shares = _shares.receiveAsFlow()
    fun share(id: String) = attempt(null) { _shares.send(nori.library.share(id)) }

    fun download(songs: List<Song>) {
        nori.downloads.download(songs)
        warmCovers(songs)
        _messages.trySend(wordsDownloading(songs.size.toUInt()))
    }

    /**
     * Fetches the artwork of songs being downloaded onto the disk, at the two sizes the app asks for.
     * Covers are only kept once something has drawn them, so a song downloaded from a menu - without its
     * cover ever being on screen - arrives on the device with no picture, and shows a blank plate for the
     * rest of its life offline. Not decoded: nothing is drawing them now, and hundreds of them would push
     * the covers on screen out of memory.
     */
    private fun warmCovers(songs: List<Song>) = viewModelScope.launch(kotlinx.coroutines.Dispatchers.IO) {
        val loader = dev.nori.music.data.CoverLoader.get(getApplication<Application>())
        for (want in coverWants(songs.mapNotNull { it.coverArt }, 500u)) {
            nori.library.coverUrl(want.id, want.size.toInt())?.let(loader::warm)
        }
    }

    /** Gives the downloads back: the same menu entry that offered them should be able to take them away. */
    fun undownload(songs: List<Song>) {
        nori.downloads.remove(songs.map { it.id })
        _messages.trySend(wordsDownloadsRemoved(songs.size.toUInt()))
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
        // A song finishing changes the table and its mark together: the changes are only the signal, and
        // the ones that arrive while the lists are being worked out are asked for once, not once each.
        combine(nori.downloads.state, nori.downloads.marks) { _, _ -> }
            .conflate()
            .map { runCatching { nori.core.downloadSections() }.getOrNull() }
            .flowOn(kotlinx.coroutines.Dispatchers.IO)
            .stateIn<DownloadSections?>(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    /**
     * Every downloaded song, newest first, for the library's downloads page: read again when the table
     * changes, and only while the page is watching. Null until the first answer.
     */
    val downloadedSongs: StateFlow<List<Song>?> =
        nori.downloads.state.map { it.done }
            .flowOn(kotlinx.coroutines.Dispatchers.IO)
            .stateIn<List<Song>?>(viewModelScope, SharingStarted.WhileSubscribed(5_000), null)

    fun retryDownloads(songs: List<Song>) = nori.downloads.retry(songs)
    fun cancelDownloads(songs: List<Song>) = nori.downloads.cancel(songs.map { it.id })
    fun cancelAllDownloads() = nori.downloads.cancelAll()

    suspend fun playlists(): List<Playlist> = runCatching { nori.library.playlists().first() }.getOrDefault(emptyList())
    fun addToPlaylist(p: Playlist, songs: List<Song>) = attempt(words(Said.ADDED_TO_PLAYLIST, p.name)) { nori.library.addToPlaylist(p.id, songs.map { it.id }) }
    fun addToNewPlaylist(name: String, songs: List<Song>) = attempt(words(Said.PLAYLIST_CREATED, name)) { nori.library.createPlaylist(name, songs.map { it.id }) }
}
