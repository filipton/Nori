package dev.nori.music.data

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.onStart

/**
 * Something loaded for one song, carrying which song that was ([songId], null for none playing), so a
 * screen can tell an answer for the song it shows from one left over from the song before.
 *
 * The lyrics are loaded while the lyrics panel watches them, and the last answer outlives the watching:
 * the panel opened again after the song had changed was handed the old song's lyrics first, under the
 * new song's title and with its playhead, so nothing lit and nothing scrolled until the new ones came -
 * on a slow phone, long enough to see.
 */
data class ForSong<out T>(val songId: String?, val value: T) {
    /** [value] if it is for the song [id] shown now; otherwise [waiting], as it has not arrived yet. */
    fun of(id: String?, waiting: @UnsafeVariance T): T = if (songId == id) value else waiting
}

/**
 * What [fetch] finds for each song these name, as the song changes: [loading] as each starts, then its
 * answers, every one tagged with the song it is for; [none] (asked for only then) when nothing plays,
 * [failed] when the fetch fails. A song left before its answer came cancels the fetch, so a late answer
 * is never tagged with the next song.
 */
@OptIn(ExperimentalCoroutinesApi::class)
fun <S : Any, T> Flow<S?>.followSong(id: (S) -> String, loading: T, none: () -> T, failed: (Throwable) -> T, fetch: (S) -> Flow<T>): Flow<ForSong<T>> =
    distinctUntilChanged { a, b -> a?.let(id) == b?.let(id) }.flatMapLatest { song ->
        val songId = song?.let(id)
        val found = if (song == null) flowOf(none()) else fetch(song)
        found.onStart { emit(loading) }.catch { emit(failed(it)) }.map { ForSong(songId, it) }
    }
