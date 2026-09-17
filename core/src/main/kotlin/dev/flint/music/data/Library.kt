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
    BY_NAME("alphabeticalByName"), BY_ARTIST("alphabeticalByArtist"), STARRED("starred"), BY_GENRE("byGenre"),
}

enum class StarKind(val param: String) { SONG("id"), ALBUM("albumId"), ARTIST("artistId") }

/**
 * Everything the app knows about the server. Reads that a screen opens with go
 * through [cached]: the stored response paints the screen at once, the network
 * answer replaces it only when it differs. The UI never sees bytes or URLs.
 */
class Library(private val core: Core, private val http: Http) {

    private fun params(vararg p: Pair<String, Any?>) = p.mapNotNull { (k, v) -> v?.let { Param(k, it.toString()) } }

    private suspend fun <T> call(endpoint: String, params: List<Param>, parse: (ByteArray) -> T): T =
        withContext(Dispatchers.IO) { parse(http.get(core.url(endpoint, params))) }

    private fun <T> cached(endpoint: String, params: List<Param>, parse: (ByteArray) -> T): Flow<T> = flow {
        val key = endpoint + params.joinToString("") { "&${it.key}=${it.value}" }
        val stored = core.cacheGet(key)
        if (stored != null) runCatching { parse(stored) }.onSuccess { emit(it) }
        try {
            val fresh = http.get(core.url(endpoint, params))
            if (stored == null || !fresh.contentEquals(stored)) {
                emit(parse(fresh))
                core.cachePut(key, fresh)
            }
        } catch (e: CancellationException) {
            throw e
        } catch (e: Exception) {
            if (stored == null) throw e
        }
    }.flowOn(Dispatchers.IO)

    // ---- session ----

    suspend fun ping(): ServerInfo = call("ping", emptyList(), core::parseStatus)

    // ---- search ----

    /**
     * Always asks the server, so provider results from octo-fiesta show up. One big
     * page: the proxy repeats its external results on every offset.
     */
    suspend fun search(query: String, songs: Int = 40, albums: Int = 20, artists: Int = 10): SearchResult =
        call("search3", params("query" to query, "songCount" to songs, "albumCount" to albums, "artistCount" to artists), core::parseSearch)

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
        return if (sort == AlbumSort.RANDOM) flow { emit(call("getAlbumList2", p, core::parseAlbumList)) } else cached("getAlbumList2", p, core::parseAlbumList)
    }

    fun artists(): Flow<List<Artist>> = cached("getArtists", emptyList(), core::parseArtists)
    fun album(id: String): Flow<AlbumDetail> = cached("getAlbum", params("id" to id), core::parseAlbum)
    fun artist(id: String): Flow<ArtistDetail> = cached("getArtist", params("id" to id), core::parseArtist)
    fun artistInfo(id: String): Flow<ArtistInfo> = cached("getArtistInfo2", params("id" to id, "count" to 10), core::parseArtistInfo)
    fun topSongs(artist: String): Flow<List<Song>> = cached("getTopSongs", params("artist" to artist, "count" to 20), core::parseSongs)
    fun playlists(): Flow<List<Playlist>> = cached("getPlaylists", emptyList(), core::parsePlaylists)
    fun playlist(id: String): Flow<PlaylistDetail> = cached("getPlaylist", params("id" to id), core::parsePlaylist)
    fun starred(): Flow<Starred> = cached("getStarred2", emptyList(), core::parseStarred)
    fun genres(): Flow<List<Genre>> = cached("getGenres", emptyList(), core::parseGenres)
    fun radio(): Flow<List<RadioStation>> = cached("getInternetRadioStations", emptyList(), core::parseRadio)
    fun lyrics(songId: String): Flow<Lyrics> = cached("getLyricsBySongId", params("id" to songId), core::parseLyrics)

    suspend fun randomSongs(size: Int = 100, genre: String? = null): List<Song> =
        call("getRandomSongs", params("size" to size, "genre" to genre), core::parseSongs)

    suspend fun songsByGenre(genre: String, count: Int = 200): List<Song> =
        call("getSongsByGenre", params("genre" to genre, "count" to count), core::parseSongs)

    suspend fun similarSongs(id: String, count: Int = 50): List<Song> =
        call("getSimilarSongs2", params("id" to id, "count" to count), core::parseSongs)

    suspend fun song(id: String): Song? = call("getSong", params("id" to id), core::parseSongs).firstOrNull()
    suspend fun albumSongs(id: String): List<Song> = call("getAlbum", params("id" to id), core::parseAlbum).songs
    suspend fun playlistSongs(id: String): List<Song> = call("getPlaylist", params("id" to id), core::parsePlaylist).songs

    // ---- writes; each drops the cached reads it makes stale ----

    private suspend fun write(endpoint: String, params: List<Param>, vararg stale: String) {
        call(endpoint, params, core::parseStatus)
        stale.forEach(core::cacheEvict)
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

    /** [submission] false marks "now playing"; true counts the play. */
    suspend fun scrobble(id: String, submission: Boolean, timeMs: Long? = null) =
        write("scrobble", params("id" to id, "submission" to submission, "time" to timeMs))

    // ---- queue hand-off between devices ----

    suspend fun pushQueue(ids: List<String>, current: String?, positionMs: Long) =
        write("savePlayQueue", ids.map { Param("id", it) } + params("current" to current, "position" to positionMs))

    suspend fun pullQueue(): PlayQueue = call("getPlayQueue", emptyList(), core::parsePlayQueue)

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
            val seen = core.ingestSearch(http.get(core.url("search3", p)))
            total = IngestStats(total.artists + seen.artists, total.albums + seen.albums, total.songs + seen.songs)
            emit(total)
            if (seen.songs == 0u && seen.albums == 0u && seen.artists == 0u) break
            offset += page
        }
    }.flowOn(Dispatchers.IO)

    suspend fun indexSize(): IngestStats = withContext(Dispatchers.IO) { core.indexSize() }

    fun coverUrl(id: String?, size: Int): String? = id?.let { core.coverUrl(it, size.toUInt()) }
}
