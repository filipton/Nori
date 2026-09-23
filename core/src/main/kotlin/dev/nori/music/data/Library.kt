package dev.nori.music.data

import dev.nori.music.ffi.Album
import dev.nori.music.ffi.AlbumDetail
import dev.nori.music.ffi.Artist
import dev.nori.music.ffi.ArtistDetail
import dev.nori.music.ffi.ArtistInfo
import dev.nori.music.ffi.Client
import dev.nori.music.ffi.Core
import dev.nori.music.ffi.Genre
import dev.nori.music.ffi.IngestStats
import dev.nori.music.ffi.Lyrics
import dev.nori.music.ffi.LyricsOrigin
import dev.nori.music.ffi.Page
import dev.nori.music.ffi.Playlist
import dev.nori.music.ffi.PlaylistDetail
import dev.nori.music.ffi.RadioStation
import dev.nori.music.ffi.Read
import dev.nori.music.ffi.SearchResult
import dev.nori.music.ffi.ServerInfo
import dev.nori.music.ffi.Song
import dev.nori.music.ffi.Starrable
import dev.nori.music.ffi.Starred
import dev.nori.music.ffi.StarMarks
import dev.nori.music.ffi.AlbumSort
import dev.nori.music.ffi.albumSortApi
import dev.nori.music.ffi.librarySizes
import dev.nori.music.ffi.starMarks
import dev.nori.music.ffi.Write
import dev.nori.music.ffi.starMark
import dev.nori.music.net.lifted
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.withContext

/** What can be starred. The marks are kept per kind by the core (`stars.rs`), keyed by id alone. */
enum class StarKind(internal val target: Starrable) {
    SONG(Starrable.SONG), ALBUM(Starrable.ALBUM), ARTIST(Starrable.ARTIST),
}

/** The mark of [id] of [kind], if this session changed it. */
fun StarMarks.of(kind: StarKind, id: String): Boolean? = when (kind) {
    StarKind.SONG -> songs[id]
    StarKind.ALBUM -> albums[id]
    StarKind.ARTIST -> artists[id]
}

/** How the app sizes, names and keeps artwork (the core's `cover_rules`), read once. */
object Covers {
    val rules: dev.nori.music.ffi.CoverRules by lazy { dev.nori.music.ffi.coverRules() }
}

/**
 * Everything the app knows about the server. The core's [Client] does the work - which address, what is
 * stored and for how long, what is queued while offline, how LRCLIB is asked - and this turns its answers
 * into flows for the screens: the stored answer first, the network's only when it differs. The UI never
 * sees bytes or URLs.
 */
class Library(
    private val coreOf: () -> Core,
    private val clientOf: () -> Client,
) {
    // Resolved on every use, which is always on an IO thread: the core belongs to the active server profile
    // and building it costs ~100 ms the UI thread should not pay.
    private val core get() = coreOf()
    private val client get() = clientOf()
    /** How much each read asks for (the core's `library_sizes`). */
    private val sizes by lazy { librarySizes() }

    /**
     * Star changes made this session, one map per kind, as the core keeps them (`stars.rs`, which also
     * lays them over the lists it is asked to). Reads that paint once (a cached list, a queue snapshot)
     * would otherwise sit stale until the screen is reopened; the UI prefers these over the snapshot.
     * Bumped alongside [starsVersion].
     */
    private val _starMarks = MutableStateFlow(StarMarks(emptyMap(), emptyMap(), emptyMap()))
    val starMarks: StateFlow<StarMarks> = _starMarks.asStateFlow()
    /** Star state as it should be shown: this session's change wins over the [snapshot] a list or queue item was built with. */
    fun isStarred(kind: StarKind, id: String, snapshot: Boolean): Boolean = _starMarks.value.of(kind, id) ?: snapshot
    /** Bumped on every star change, so one-shot reads (the favourites list) can re-query. */
    private val _starsVersion = MutableStateFlow(0)
    val starsVersion: StateFlow<Int> = _starsVersion.asStateFlow()

    /** A read that always asks the server. */
    private suspend fun <T> call(read: Read, pick: (Page) -> T): T = withContext(Dispatchers.IO) { pick(lifted { client.readNow(read) }) }

    /**
     * The stored answer paints the screen at once; the server is asked unless that answer is fresh, and
     * its answer is emitted only when it differs. When a failure is an error is the core's (`read_refresh`:
     * offline with something stored is not).
     */
    private inline fun <T> cached(read: Read, crossinline pick: (Page) -> T): Flow<T> = flow {
        val c = client
        val stored = lifted { c.readStored(read) }
        stored.page?.let { emit(pick(it)) }
        if (stored.fresh) return@flow
        lifted { c.readRefresh(read, stored.digest) }?.let { emit(pick(it)) }
    }.flowOn(Dispatchers.IO)

    /**
     * Throws away the stored answers whose key starts with one of [prefixes], so the next read of them
     * has to ask the server (see the core's `drop_cached`).
     */
    suspend fun dropCached(vararg prefixes: String) = withContext(Dispatchers.IO) { lifted { client.dropCached(prefixes.toList()) } }

    // ---- session ----

    suspend fun musicFolders(): List<dev.nori.music.ffi.MusicFolder> = call(Read.MusicFolders) { (it as Page.Folders).v }


    // ---- search ----

    /**
     * Always asks the server, so provider results from octo-fiesta show up. One big
     * page: the proxy repeats its external results on every offset.
     */
    suspend fun search(query: String): SearchResult =
        call(Read.Search(query, sizes.searchSongs, sizes.searchAlbums, sizes.searchArtists)) { (it as Page.Found).v }

    suspend fun searchHistory(): List<String> = withContext(Dispatchers.IO) { core.searchHistory() }
    suspend fun forgetSearches() = withContext(Dispatchers.IO) { core.searchForget() }

    // ---- browse ----

    /** "By year" is this year's albums; "random" is never stored (the core decides both). */
    fun albums(sort: AlbumSort, size: Int, offset: Int = 0, genre: String? = null): Flow<List<Album>> =
        cached(Read.AlbumList(albumSortApi(sort), size, offset, genre)) { (it as Page.Albums).v }

    fun artists(): Flow<List<Artist>> = cached(Read.ArtistIndex) { (it as Page.Artists).v }
    fun album(id: String): Flow<AlbumDetail> = cached(Read.AlbumById(id)) { (it as Page.AlbumPage).v }
    fun artist(id: String): Flow<ArtistDetail> = cached(Read.ArtistById(id)) { (it as Page.ArtistPage).v }
    fun artistInfo(id: String): Flow<ArtistInfo> = cached(Read.ArtistAbout(id)) { (it as Page.About).v }
    fun topSongs(artist: String): Flow<List<Song>> = cached(Read.TopSongs(artist)) { (it as Page.Songs).v }
    fun playlists(): Flow<List<Playlist>> = cached(Read.PlaylistList) { (it as Page.Playlists).v }
    fun playlist(id: String): Flow<PlaylistDetail> = cached(Read.PlaylistById(id)) { (it as Page.PlaylistPage).v }
    fun starred(): Flow<Starred> = cached(Read.StarredItems) { (it as Page.StarredPage).v }
    fun genres(): Flow<List<Genre>> = cached(Read.GenreList) { (it as Page.Genres).v }
    fun radio(): Flow<List<RadioStation>> = cached(Read.RadioList) { (it as Page.Stations).v }
    fun lyrics(songId: String): Flow<Lyrics> = cached(Read.LyricsBySong(songId)) { (it as Page.LyricsPage).v }

    /**
     * The server's lyrics, or when it has no synced ones and the settings allow it, LRCLIB's. What follows
     * the server's answer is the core's decision (`lyrics_after_server`).
     */
    fun lyricsFor(song: Song): Flow<FoundLyrics> = flow {
        var fromServer: Lyrics? = null
        // An empty answer from the server is not shown here while LRCLIB may still have the song; the core
        // hands it back below if nothing better follows.
        lyrics(song.id).catch { }.collect { fromServer = it; if (it.lines.isNotEmpty()) emit(FoundLyrics(it, LyricsOrigin.SERVER)) }
        val server = fromServer
        val hasLines = server != null && server.lines.isNotEmpty()
        lifted { client.lyricsAfterServer(song, hasLines, server?.synced == true) }
            ?.let { emit(FoundLyrics(it.lyrics, if (it.lrclib) LyricsOrigin.LRCLIB else LyricsOrigin.SERVER)) }
    }.flowOn(Dispatchers.IO)

    // ---- local only: history, mixes, smart playlists (all computed in the Rust core from the index) ----

    suspend fun clearHistory() = withContext(Dispatchers.IO) { core.historyClear() }

    suspend fun excludeFromMixes(songId: String, excluded: Boolean) = withContext(Dispatchers.IO) { core.mixExcludedSet(songId, excluded) }

    // ---- what to play: each list is made in the core (actions.rs), requests included ----

    /** A radio from one song: it, then what the server finds similar, or random songs of its genre. */
    suspend fun radio(song: Song): List<Song> = withContext(Dispatchers.IO) { lifted { client.radio(song) } }

    /** An instant mix from the index, or the radio when the index has nothing near the song. */
    suspend fun instantMix(song: Song): List<Song> = withContext(Dispatchers.IO) { lifted { client.instantMix(song) } }

    /** Every album of an artist (a provider's left out), in order, as one list of songs. */
    suspend fun artistSongs(albums: List<Album>): List<Song> = withContext(Dispatchers.IO) { client.artistSongs(albums) }

    suspend fun shuffleAll(): List<Song> = withContext(Dispatchers.IO) { lifted { client.shuffleAll() } }

    suspend fun smartPlaylists() = withContext(Dispatchers.IO) { core.smartList() }
    fun smartDefaults() = dev.nori.music.ffi.smartDefaults()
    suspend fun smartSave(id: String, name: String, json: String): String = withContext(Dispatchers.IO) { core.smartSave(id, name, json) }
    suspend fun smartDelete(id: String) = withContext(Dispatchers.IO) { core.smartDelete(id) }
    suspend fun smartSongs(json: String): List<Song> =
        withContext(Dispatchers.IO) { core.smartEvaluate(json, 0u, sizes.smartSongs) }

    fun m3uExport(name: String, songs: List<Song>): String = dev.nori.music.ffi.m3uExport(name, songs)

    /** The server's folder tree, for libraries organised by directory rather than by tags. */
    fun folders(): Flow<List<Artist>> = cached(Read.FolderIndex) { (it as Page.Artists).v }
    fun folder(id: String): Flow<dev.nori.music.ffi.Directory> = cached(Read.FolderById(id)) { (it as Page.DirectoryPage).v }

    /** Sorted pages of the offline index: the "all songs" and "by decade" lists. */
    suspend fun browseSongs(sort: String, descending: Boolean, starredOnly: Boolean, years: IntRange?, offset: Int, limit: Int): List<Song> = withContext(Dispatchers.IO) {
        core.browseSongs(sort, descending, starredOnly, (years?.first ?: 0).toUInt(), (years?.last ?: 0).toUInt(), offset.toUInt(), limit.toUInt())
    }

    suspend fun decades(): List<Genre> = withContext(Dispatchers.IO) { core.browseDecades() }


    suspend fun randomSongs(): List<Song> = call(Read.RandomSongs(sizes.randomSongs, null)) { (it as Page.Songs).v }

    suspend fun songsByGenre(genre: String): List<Song> = call(Read.SongsByGenre(genre, sizes.genreSongs)) { (it as Page.Songs).v }


    /** What carries the queue on past the song playing (see the core's autofill.rs); nothing on any failure. */
    suspend fun autofill(): List<Song> = withContext(Dispatchers.IO) { client.autofill() }

    /**
     * Draws mix [id] unless this period's draw is there already ([again]: a different one); the core
     * fetches the server's random songs when the index has nothing to draw from. True when it changed.
     */
    suspend fun mixEnsure(id: String, again: Boolean): Boolean = withContext(Dispatchers.IO) { client.mixEnsure(id, again) }

    /** Draws whichever mixes are missing or from the last period. True when any tile changed. */
    suspend fun mixWarm(): Boolean = withContext(Dispatchers.IO) { client.mixWarmAll() }

    /** What picking the server's saved queue back up plays (the core's `resume_from_server`). */
    suspend fun resumeFromServer(): dev.nori.music.ffi.ResumePlan = withContext(Dispatchers.IO) { lifted { client.resumeFromServer() } }

    suspend fun song(id: String): Song? = call(Read.SongById(id)) { (it as Page.OneSong).v }
    suspend fun albumSongs(id: String): List<Song> = call(Read.AlbumSongs(id)) { (it as Page.Songs).v }
    suspend fun playlistSongs(id: String): List<Song> = call(Read.PlaylistSongs(id)) { (it as Page.Songs).v }

    // ---- writes: the core sends them, keeps them while offline and drops the reads they make stale ----

    private suspend fun write(w: Write) = withContext(Dispatchers.IO) { lifted { client.write(w) } }

    /** Replays queued writes. Stops at the first network failure; a write the server rejects is dropped. */
    suspend fun flushPending() = withContext(Dispatchers.IO) { lifted { client.flushPending() } }

    suspend fun star(kind: StarKind, id: String, on: Boolean) {
        // The mark goes up first, so the heart fills under the finger. Doing it after the request meant
        // waiting a round trip to the server - which is what "favourites do not refresh" was.
        val marked = starMark(kind.target, id, on)
        _starMarks.value = marked.marks
        _starsVersion.update { it + 1 }
        try {
            withContext(Dispatchers.IO) { lifted { client.starSend(kind.target, id, on, marked.previous) } }
        } catch (e: Exception) {
            // Refused (offline is not: the core keeps those and replays them). The core has put the mark
            // from before back, so the screen stops showing a favourite the server never took.
            _starMarks.value = starMarks()
            _starsVersion.update { it + 1 }
            throw e
        }
        // Now the server has it, so the lists that come from it can be asked again.
        _starsVersion.update { it + 1 }
    }

    suspend fun createPlaylist(name: String, songIds: List<String>) = write(Write.CreatePlaylist(name, songIds))

    suspend fun addToPlaylist(id: String, songIds: List<String>) = write(Write.AddToPlaylist(id, songIds))

    suspend fun removeFromPlaylist(id: String, index: Int) = write(Write.RemoveFromPlaylist(id, index))

    suspend fun deletePlaylist(id: String) = write(Write.DeletePlaylist(id))

    /** Not worth queueing: by the time it could be replayed it is no longer true. */
    suspend fun nowPlaying(id: String) { call(Read.NowPlaying(id)) { it } }

    suspend fun createRadio(name: String, streamUrl: String) = write(Write.CreateRadio(name, streamUrl))
    suspend fun deleteRadio(id: String) = write(Write.DeleteRadio(id))

    /** A public link to a song or album; the server must have sharing enabled. */
    suspend fun share(id: String): String = call(Read.ShareLink(id)) { (it as Page.ShareUrl).v }


    /** [submission] false marks "now playing"; true counts the play. */
    suspend fun scrobble(id: String, submission: Boolean, timeMs: Long? = null) = write(Write.Scrobble(id, submission, timeMs))

    // ---- queue hand-off between devices ----

    suspend fun pushQueue(ids: List<String>, current: String?, positionMs: Long) = write(Write.SaveQueue(ids, current, positionMs))


    // ---- offline index ----

    /**
     * Walks the whole library into the local index. The pages go from the socket
     * into SQLite inside Rust; only three counters come back per page.
     */
    fun sync(page: Int = sizes.syncPage.toInt()): Flow<IngestStats> = flow {
        var offset = 0u
        var total = IngestStats(0u, 0u, 0u)
        while (true) {
            val step = lifted { client.syncPage(offset, page.toUInt(), total) }
            total = step.total
            emit(total)
            offset = step.nextOffset ?: break
        }
    }.flowOn(Dispatchers.IO)

    suspend fun indexSize(): IngestStats = withContext(Dispatchers.IO) { core.indexSize() }

    @Volatile private var coverPrefix: String? = null

    /**
     * Called for every row a list draws, on the UI thread: plain string work, no FFI. The core signs the
     * prefix once (per server and address) and says how the rest is put together (`cover_rules`); a row
     * only appends its id and size, which is cheaper than any crossing into the core would be.
     */
    fun coverUrl(id: String?, size: Int): String? {
        if (id == null) return null
        val prefix = coverPrefix ?: (core.urlPrefix("getCoverArt") + Covers.rules.idParam).also { coverPrefix = it }
        return prefix + android.net.Uri.encode(id) + Covers.rules.sizeParam + size
    }

    /** The signed prefix changes with the server, the credentials or the address in use. */
    fun onServerChanged() { coverPrefix = null }
}
