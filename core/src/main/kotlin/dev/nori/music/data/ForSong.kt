package dev.nori.music.data

import kotlinx.coroutines.ExperimentalCoroutinesApi
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.catch
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.flatMapLatest
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.map

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
 * What [followSong] last showed for each of the last few songs, and whether that song's fetch had
 * finished. Kept by whoever follows the songs (a ViewModel), so it outlives the watching: a song come
 * back to - the panel opened again, the song heard for a moment during a skip and then again - starts
 * from what it showed, not from the loader and a lookup all over again.
 */
class SongAnswers<T>(private val songs: Int = 4) {
    internal class Kept<T>(val value: T, val done: Boolean)

    private val kept = LinkedHashMap<String, Kept<T>>()

    @Synchronized internal fun get(id: String): Kept<T>? = kept[id]

    @Synchronized internal fun put(id: String, value: T, done: Boolean) {
        kept.remove(id)
        kept[id] = Kept(value, done)
        while (kept.size > songs) kept.remove(kept.keys.first())
    }
}

/**
 * What [fetch] finds for each song these name, as the song changes: [loading] as each starts, then its
 * answers, every one tagged with the song it is for; [none] (asked for only then) when nothing plays,
 * [failed] when the fetch fails. A song left before its answer came cancels the fetch, so a late answer
 * is never tagged with the next song.
 *
 * With [answers], a song shown before starts from what it showed instead of [loading], and one whose
 * fetch had finished is not fetched again. An answer [same] as the one shown is not handed on: the same
 * lyrics read again were handed over as new ones, and the page faded them out and in again and started
 * their clock from the top. Only answers that [keep] allows are remembered (not a failure).
 */
@OptIn(ExperimentalCoroutinesApi::class)
fun <S : Any, T> Flow<S?>.followSong(
    id: (S) -> String,
    loading: T,
    none: () -> T,
    failed: (Throwable) -> T,
    answers: SongAnswers<T>? = null,
    same: (T, T) -> Boolean = { a, b -> a == b },
    keep: (T) -> Boolean = { true },
    fetch: (S) -> Flow<T>,
): Flow<ForSong<T>> =
    distinctUntilChanged { a, b -> a?.let(id) == b?.let(id) }.flatMapLatest { song ->
        if (song == null) return@flatMapLatest flowOf(ForSong(null, none()))
        val songId = id(song)
        flow {
            val had = answers?.get(songId)
            // What is on screen for this song, and whether anything is.
            var shown: T = loading
            var showing = false
            if (had != null) { shown = had.value; showing = true; emit(had.value) } else emit(loading)
            if (had?.done == true) return@flow
            fetch(song).collect { v ->
                if (!showing || !same(shown, v)) {
                    shown = v
                    showing = true
                    emit(v)
                }
                if (keep(v)) answers?.put(songId, shown, done = false)
            }
            if (showing && keep(shown)) answers?.put(songId, shown, done = true)
        }.catch { emit(failed(it)) }.map { ForSong(songId, it) }
    }

/**
 * The playhead read by [position] when the song playing ([playing]) is still [songId], else null. What
 * is drawn for one song must not follow the next one's playhead: a skip moves the player before the page
 * that shows the old song's words has changed, and read then the new song's first second put those words
 * back before their first line, which scrolled them to the top in one frame as they began to fade.
 */
inline fun playheadFor(songId: String?, playing: String?, position: () -> Long): Long? =
    if (songId != null && songId == playing) position() else null
