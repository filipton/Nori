package dev.flint.music.data

import dev.flint.music.ffi.Album
import dev.flint.music.ffi.AlbumDetail
import dev.flint.music.ffi.Artist
import dev.flint.music.ffi.ArtistDetail
import dev.flint.music.ffi.ArtistInfo
import dev.flint.music.ffi.Core
import dev.flint.music.ffi.Genre
import dev.flint.music.ffi.IngestStats
import dev.flint.music.ffi.Lyrics
import dev.flint.music.ffi.Param
import dev.flint.music.ffi.PlayQueue
import dev.flint.music.ffi.Playlist
import dev.flint.music.ffi.PlaylistDetail
import dev.flint.music.ffi.RadioStation
import dev.flint.music.ffi.SearchResult
import dev.flint.music.ffi.ServerInfo
import dev.flint.music.ffi.Song
import dev.flint.music.ffi.Starred
import dev.flint.music.net.Http
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOn
import kotlinx.coroutines.withContext

enum class AlbumSort(val api: String) {
    NEWEST("newest"), RECENT("recent"), FREQUENT("frequent"), RANDOM("random"),
    BY_NAME("alphabeticalByName"), BY_ARTIST("alphabeticalByArtist"), STARRED("starred"), BY_GENRE("byGenre"), HIGHEST("highest"), BY_YEAR("byYear"),
}

enum class StarKind(val param: String) { SONG("id"), ALBUM("albumId"), ARTIST("artistId") }

/**
 * Everything the app knows about the server. Reads that a screen opens with go
 * through [cached]: the stored response paints the screen at once, the network
 * answer replaces it only when it differs. The UI never sees bytes or URLs.
 */
private const val HOUR = 3_600_000L
private const val DAY = 24 * HOUR

class Library(
    private val coreOf: () -> Core,
    private val httpOf: () -> Http,
    /** The music folder browsing is restricted to, or empty. */
    private val folder: () -> String = { "" },
    /** Called when the server did not answer: may switch to the profile's other address. True when it did. */
    private val onUnreachable: suspend () -> Boolean = { false },
) {
    // Resolved on every use, which is always on an IO thread: the core belongs to the active server profile
    // and building it costs ~100 ms the UI thread should not pay.
    private val core get() = coreOf()
    private val http get() = httpOf()

    /** The endpoints that take `musicFolderId`. */
    private val foldered = setOf("getAlbumList2", "getArtists", "search3", "getRandomSongs", "getStarred2", "getSongsByGenre", "getIndexes")

    private fun scoped(endpoint: String, params: List<Param>): List<Param> =
        folder().let { f -> if (f.isEmpty() || endpoint !in foldered) params else params + Param("musicFolderId", f) }

    /** One request; if the server is unreachable and the profile has a second address, that is tried once. */
    private suspend fun fetch(endpoint: String, params: List<Param>): ByteArray {
        val p = scoped(endpoint, params)
        return try {
            http.get(core.url(endpoint, p))
        } catch (e: java.io.IOException) {
            if (e is dev.flint.music.net.MeteredNetworkException || !onUnreachable()) throw e
            http.get(core.url(endpoint, p))
        }
    }

    private fun params(vararg p: Pair<String, Any?>) = p.mapNotNull { (k, v) -> v?.let { Param(k, it.toString()) } }

    private suspend fun <T> call(endpoint: String, params: List<Param>, parse: (ByteArray) -> T): T =
        withContext(Dispatchers.IO) { parse(fetch(endpoint, params)) }

    /**
     * [freshMs]: an answer younger than this is not re-asked. Opening the same screens again within
     * a couple of minutes then costs no request at all, which on mobile data means no radio wake-up.
     * Writes evict what they change, so the user's own actions are never hidden by this.
     */
    private fun <T> cached(endpoint: String, params: List<Param>, freshMs: Long = 2 * 60_000L, parse: (ByteArray) -> T): Flow<T> = flow {
        val key = endpoint + scoped(endpoint, params).joinToString("") { "&${it.key}=${it.value}" }
        val stored = core.cacheGet(key)
        if (stored != null) {
            val shown = runCatching { parse(stored) }.onSuccess { emit(it) }.isSuccess
            if (shown && core.cacheFresh(key, freshMs)) return@flow
        }
        try {
            val fresh = fetch(endpoint, params)
            if (stored == null || !fresh.contentEquals(stored)) emit(parse(fresh))
            core.cachePut(key, fresh) // also when unchanged: it restarts the freshness window
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (stored == null) throw e
        }
    }.flowOn(Dispatchers.IO)

    // ---- session ----

    suspend fun musicFolders(): List<dev.flint.music.ffi.MusicFolder> = call("getMusicFolders", emptyList()) { core.parseMusicFolders(it) }

    suspend fun ping(): ServerInfo = call("ping", emptyList(), { core.parseStatus(it) })

    // ---- search ----

    /**
     * Always asks the server, so provider results from octo-fiesta show up. One big
     * page: the proxy repeats its external results on every offset.
     */
    suspend fun search(query: String, songs: Int = 40, albums: Int = 20, artists: Int = 10): SearchResult =
        call("search3", params("query" to query, "songCount" to songs, "albumCount" to albums, "artistCount" to artists), { core.parseSearch(it) })

    /** Offline and instant: the FTS index of everything seen so far. */
    suspend fun localSearch(query: String, limit: Int = 30): SearchResult =
        withContext(Dispatchers.IO) { core.localSearch(query, limit.toUInt()) }

    suspend fun searchHistory(): List<String> = withContext(Dispatchers.IO) { core.searchHistory() }
    suspend fun rememberSearch(query: String) = withContext(Dispatchers.IO) { core.searchRemember(query) }
    suspend fun forgetSearches() = withContext(Dispatchers.IO) { core.searchForget() }

    // ---- browse ----

    fun albums(sort: AlbumSort, size: Int = 50, offset: Int = 0, genre: String? = null): Flow<List<Album>> {
        val p = params("type" to sort.api, "size" to size, "offset" to offset, "genre" to genre)
        // A cached "random" would be the same shuffle every time.
        if (sort == AlbumSort.BY_YEAR) return albumsByYear(java.time.Year.now().value, 0, size, offset)
        return if (sort == AlbumSort.RANDOM) flow { emit(call("getAlbumList2", p, { core.parseAlbumList(it) })) } else cached("getAlbumList2", p, parse = { core.parseAlbumList(it) })
    }

    fun artists(): Flow<List<Artist>> = cached("getArtists", emptyList(), parse = { core.parseArtists(it) })
    fun album(id: String): Flow<AlbumDetail> = cached("getAlbum", params("id" to id), parse = { core.parseAlbum(it) })
    fun artist(id: String): Flow<ArtistDetail> = cached("getArtist", params("id" to id), parse = { core.parseArtist(it) })
    fun artistInfo(id: String): Flow<ArtistInfo> = cached("getArtistInfo2", params("id" to id, "count" to 10), DAY, { core.parseArtistInfo(it) })
    fun topSongs(artist: String): Flow<List<Song>> = cached("getTopSongs", params("artist" to artist, "count" to 20), DAY, { core.parseSongs(it) })
    fun playlists(): Flow<List<Playlist>> = cached("getPlaylists", emptyList(), parse = { core.parsePlaylists(it) })
    fun playlist(id: String): Flow<PlaylistDetail> = cached("getPlaylist", params("id" to id), parse = { core.parsePlaylist(it) })
    fun starred(): Flow<Starred> = cached("getStarred2", emptyList(), parse = { core.parseStarred(it) })
    fun genres(): Flow<List<Genre>> = cached("getGenres", emptyList(), HOUR, { core.parseGenres(it) })
    fun radio(): Flow<List<RadioStation>> = cached("getInternetRadioStations", emptyList(), HOUR, { core.parseRadio(it) })
    fun lyrics(songId: String): Flow<Lyrics> = cached("getLyricsBySongId", params("id" to songId), HOUR, { core.parseLyrics(it) })

    /** The server's folder tree, for libraries organised by directory rather than by tags. */
    fun folders(): Flow<List<Artist>> = cached("getIndexes", emptyList(), HOUR) { core.parseIndexes(it) }
    fun folder(id: String): Flow<dev.flint.music.ffi.Directory> = cached("getMusicDirectory", params("id" to id)) { core.parseDirectory(it) }

    /** Sorted pages of the offline index: the "all songs" and "by decade" lists. */
    suspend fun browseSongs(sort: String, descending: Boolean, starredOnly: Boolean, years: IntRange?, offset: Int, limit: Int): List<Song> = withContext(Dispatchers.IO) {
        core.browseSongs(sort, descending, starredOnly, (years?.first ?: 0).toUInt(), (years?.last ?: 0).toUInt(), offset.toUInt(), limit.toUInt())
    }

    suspend fun decades(): List<Genre> = withContext(Dispatchers.IO) { core.browseDecades() }

    fun albumsByYear(from: Int, to: Int, size: Int = 50, offset: Int = 0): Flow<List<Album>> =
        cached("getAlbumList2", params("type" to "byYear", "fromYear" to from, "toYear" to to, "size" to size, "offset" to offset)) { core.parseAlbumList(it) }

    suspend fun randomSongs(size: Int = 100, genre: String? = null): List<Song> =
        call("getRandomSongs", params("size" to size, "genre" to genre), { core.parseSongs(it) })

    suspend fun songsByGenre(genre: String, count: Int = 200): List<Song> =
        call("getSongsByGenre", params("genre" to genre, "count" to count), { core.parseSongs(it) })

    suspend fun similarSongs(id: String, count: Int = 50): List<Song> =
        call("getSimilarSongs2", params("id" to id, "count" to count), { core.parseSongs(it) })

    suspend fun song(id: String): Song? = call("getSong", params("id" to id), { core.parseSongs(it) }).firstOrNull()
    suspend fun albumSongs(id: String): List<Song> = call("getAlbum", params("id" to id), { core.parseAlbum(it) }).songs
    suspend fun playlistSongs(id: String): List<Song> = call("getPlaylist", params("id" to id), { core.parsePlaylist(it) }).songs

    // ---- writes; each drops the cached reads it makes stale ----

    /**
     * A write that cannot reach the server is kept and replayed later, in order, so stars, ratings,
     * playlist edits and plays made offline are not lost. The UI is told it worked either way.
     */
    private suspend fun write(endpoint: String, params: List<Param>, vararg stale: String) = withContext(Dispatchers.IO) {
        try {
            core.parseStatus(fetch(endpoint, params))
            flushPending()
        } catch (e: java.io.IOException) {
            core.pendingAdd(endpoint, params)
        }
        stale.forEach(core::cacheEvict)
    }

    /** Replays queued writes. Stops at the first network failure; a write the server rejects is dropped. */
    suspend fun flushPending() = withContext(Dispatchers.IO) {
        for (p in core.pendingList()) {
            try {
                core.parseStatus(http.get(core.url(p.endpoint, p.params)))
            } catch (e: java.io.IOException) {
                return@withContext
            } catch (e: Exception) {
                // rejected: replaying it again would not help
            }
            core.pendingDone(p.rowId)
        }
    }

    suspend fun star(kind: StarKind, id: String, on: Boolean) =
        write(if (on) "star" else "unstar", params(kind.param to id), "getStarred2", "getAlbum", "getArtist", "getPlaylist")

    suspend fun rate(id: String, rating: Int) = write("setRating", params("id" to id, "rating" to rating), "getAlbum", "getPlaylist")

    suspend fun createPlaylist(name: String, songIds: List<String>) =
        write("createPlaylist", params("name" to name) + songIds.map { Param("songId", it) }, "getPlaylist")

    suspend fun addToPlaylist(id: String, songIds: List<String>) =
        write("updatePlaylist", params("playlistId" to id) + songIds.map { Param("songIdToAdd", it) }, "getPlaylist")

    suspend fun removeFromPlaylist(id: String, index: Int) =
        write("updatePlaylist", params("playlistId" to id, "songIndexToRemove" to index), "getPlaylist")

    suspend fun deletePlaylist(id: String) = write("deletePlaylist", params("id" to id), "getPlaylist")

    /** Not worth queueing: by the time it could be replayed it is no longer true. */
    suspend fun nowPlaying(id: String) { call("scrobble", params("id" to id, "submission" to false)) { core.parseStatus(it) } }

    suspend fun createRadio(name: String, streamUrl: String) = write("createInternetRadioStation", params("name" to name, "streamUrl" to streamUrl), "getInternetRadioStations")
    suspend fun deleteRadio(id: String) = write("deleteInternetRadioStation", params("id" to id), "getInternetRadioStations")

    /** A public link to a song or album; the server must have sharing enabled. */
    suspend fun share(id: String): String = call("createShare", params("id" to id)) { core.parseShare(it) }

    /** Every song in the offline index, a page at a time. */
    suspend fun indexedSongs(offset: Int, limit: Int): List<Song> = withContext(Dispatchers.IO) { core.indexedSongs(offset.toUInt(), limit.toUInt()) }

    /** [submission] false marks "now playing"; true counts the play. */
    suspend fun scrobble(id: String, submission: Boolean, timeMs: Long? = null) =
        write("scrobble", params("id" to id, "submission" to submission, "time" to timeMs))

    // ---- queue hand-off between devices ----

    suspend fun pushQueue(ids: List<String>, current: String?, positionMs: Long) =
        write("savePlayQueue", ids.map { Param("id", it) } + params("current" to current, "position" to positionMs))

    suspend fun pullQueue(): PlayQueue = call("getPlayQueue", emptyList(), { core.parsePlayQueue(it) })

    // ---- offline index ----

    /**
     * Walks the whole library into the local index. The pages go from the socket
     * into SQLite inside Rust; only three counters come back per page.
     */
    fun sync(page: Int = 500): Flow<IngestStats> = flow {
        var offset = 0
        var total = IngestStats(0u, 0u, 0u)
        while (true) {
            val p = params("query" to "", "songCount" to page, "songOffset" to offset, "albumCount" to page, "albumOffset" to offset, "artistCount" to page, "artistOffset" to offset)
            val seen = core.ingestSearch(fetch("search3", p))
            total = IngestStats(total.artists + seen.artists, total.albums + seen.albums, total.songs + seen.songs)
            emit(total)
            if (seen.songs == 0u && seen.albums == 0u && seen.artists == 0u) break
            offset += page
        }
    }.flowOn(Dispatchers.IO)

    suspend fun indexSize(): IngestStats = withContext(Dispatchers.IO) { core.indexSize() }

    @Volatile private var coverPrefix: String? = null

    /** Called for every row a list draws, on the UI thread: plain string work, no FFI. */
    fun coverUrl(id: String?, size: Int): String? {
        if (id == null) return null
        val prefix = coverPrefix ?: core.urlPrefix("getCoverArt").also { coverPrefix = it }
        return "$prefix&id=${android.net.Uri.encode(id)}&size=$size"
    }

    /** The signed prefix changes with the server or the credentials. */
    fun onServerChanged() { coverPrefix = null }
}
