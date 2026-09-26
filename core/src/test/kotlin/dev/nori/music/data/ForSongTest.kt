package dev.nori.music.data

import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.flow.flow
import kotlinx.coroutines.flow.flowOf
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Test

class ForSongTest {
    private fun ForSong<String>.shownUnder(id: String?) = of(id, "loading")

    @Test
    fun an_answer_for_another_song_is_the_loader() {
        val a = ForSong("a", "words of a")
        assertEquals("words of a", a.shownUnder("a"))
        assertEquals("loading", a.shownUnder("b"))
        assertEquals("loading", a.shownUnder(null))
        assertEquals("nothing", ForSong<String>(null, "nothing").shownUnder(null))
    }

    @Test
    fun a_song_left_while_its_lyrics_load_never_hands_them_to_the_next() = runBlocking {
        val playing = MutableStateFlow<String?>("a")
        val aAnswers = CompletableDeferred<Unit>()
        val aAsked = CompletableDeferred<Unit>()
        val seen = mutableListOf<ForSong<String>>()
        val job = launch {
            playing.followSong({ it }, "loading", { "none" }, { "failed" }) { song ->
                flow {
                    if (song == "a") { aAsked.complete(Unit); aAnswers.await() }
                    emit("words of $song")
                }
            }.collect { seen += it }
        }
        aAsked.await()
        // Skipped while the first song's lyrics are still out; they arrive after the skip.
        playing.value = "b"
        yield()
        aAnswers.complete(Unit)
        while (seen.lastOrNull() != ForSong("b", "words of b")) yield()
        job.cancel()
        assertFalse("a's words are never tagged b: $seen", seen.any { it.songId == "b" && it.value == "words of a" })
        assertFalse("a's words never arrive at all once it was left: $seen", seen.any { it.value == "words of a" })
        assertEquals(ForSong("b", "loading"), seen[seen.indexOfFirst { it.songId == "b" }])
    }

    @Test
    fun a_panel_opened_again_after_the_song_changed_is_not_handed_the_old_words() = runBlocking {
        val playing = MutableStateFlow<String?>("a")
        val scope = kotlinx.coroutines.CoroutineScope(coroutineContext + kotlinx.coroutines.Job())
        val lyrics = playing.followSong({ it }, "loading", { "none" }, { "failed" }) { flowOf("words of $it") }
            .stateIn(scope, SharingStarted.WhileSubscribed(0, replayExpirationMillis = 0), ForSong(null, "loading"))
        // The panel watches song a, then closes.
        assertEquals(ForSong("a", "words of a"), lyrics.first { it.value != "loading" })
        yield()
        // The song changes while nobody watches; the panel opens again on b.
        playing.value = "b"
        // What the panel draws under b, from the first value it is handed until b's words.
        val shown = mutableListOf<String>()
        lyrics.first { shown += it.of("b", "loading"); shown.last() == "words of b" }
        assertEquals("the reopened panel shows the loader, then b's words: $shown", setOf("loading", "words of b"), shown.toSet())
        assertEquals("loading", shown.first())
        scope.cancel()
    }

    /** Follows [playing] with the answers kept, recording what arrives and how often each song is fetched. */
    private class Following(val answers: SongAnswers<String> = SongAnswers()) {
        val playing = MutableStateFlow<String?>("a")
        val fetched = mutableMapOf<String, Int>()
        val seen = mutableListOf<ForSong<String>>()
        /** Answers each fetch gives; a song not here gives "words of <song>". */
        val gives = mutableMapOf<String, kotlinx.coroutines.flow.Flow<String>>()
        fun flow() = playing.followSong({ it }, "loading", { "none" }, { "failed" }, answers, same = { a, b -> a.substringBefore('#') == b.substringBefore('#') }, keep = { it != "failed" }) { song ->
            fetched[song] = (fetched[song] ?: 0) + 1
            gives[song] ?: flowOf("words of $song")
        }
        /** What song [id] showed, in order. */
        fun of(id: String) = seen.filter { it.songId == id }.map { it.value }
    }

    @Test
    fun a_song_come_back_to_during_a_skip_starts_from_its_words_and_is_not_fetched_again() = runBlocking {
        val f = Following()
        val job = launch { f.flow().collect { f.seen += it } }
        while (f.seen.lastOrNull() != ForSong("a", "words of a")) yield()
        // Around a skip the page is on b for a moment, back on a, then on b for good.
        f.playing.value = "b"
        while (f.seen.lastOrNull()?.songId != "b") yield()
        f.playing.value = "a"
        while (f.seen.lastOrNull()?.songId != "a") yield()
        f.playing.value = "b"
        while (f.seen.lastOrNull() != ForSong("b", "words of b")) yield()
        job.cancel()
        assertEquals("a never goes back through the loader: ${f.seen}", listOf("loading", "words of a", "words of a"), f.of("a"))
        assertEquals("a is looked up once", 1, f.fetched["a"])
    }

    @Test
    fun the_same_words_read_again_are_not_handed_on_and_a_lookup_cut_short_carries_on() = runBlocking {
        val f = Following()
        val release = CompletableDeferred<Unit>()
        // The server's words at once; the service's, finer, only later.
        f.gives["a"] = flow { emit("plain#1"); release.await(); emit("timed") }
        val job = launch { f.flow().collect { f.seen += it } }
        while (f.of("a").lastOrNull() != "plain#1") yield()
        // Left before the service answered, and come back to.
        f.playing.value = "b"
        while (f.seen.lastOrNull() != ForSong("b", "words of b")) yield()
        f.gives["a"] = flow { emit("plain#2"); emit("timed") }
        f.playing.value = "a"
        while (f.of("a").lastOrNull() != "timed") yield()
        job.cancel()
        assertEquals("the kept words first, the same ones read again not handed on: ${f.seen}", listOf("loading", "plain#1", "plain#1", "timed"), f.of("a"))
        assertEquals("cut short, so looked up again", 2, f.fetched["a"])
    }

    @Test
    fun a_panel_opened_again_on_the_same_song_shows_its_words_at_once() = runBlocking {
        val playing = MutableStateFlow<String?>("a")
        val scope = kotlinx.coroutines.CoroutineScope(coroutineContext + kotlinx.coroutines.Job())
        val lyrics = playing.followSong({ it }, "loading", { "none" }, { "failed" }, SongAnswers()) { flowOf("words of $it") }
            .stateIn(scope, SharingStarted.WhileSubscribed(0), ForSong(null, "loading"))
        assertEquals(ForSong("a", "words of a"), lyrics.first { it.value != "loading" })
        yield()
        // Watched again with nothing changed: the first value handed over is already the words.
        assertEquals("words of a", lyrics.value.of("a", "loading"))
        scope.cancel()
    }

    private fun kotlinx.coroutines.CoroutineScope.cancel() = (coroutineContext[kotlinx.coroutines.Job])!!.cancel()

    @Test fun `a song's words follow only that song's playhead`() {
        var read = 0
        val at = { read++; 42_000L }
        assertEquals(42_000L, playheadFor("a", "a", at))
        assertEquals("skipped to b: a's words stand where they were", null, playheadFor("a", "b", at))
        assertEquals("nothing playing", null, playheadFor("a", null, at))
        assertEquals("words of no song", null, playheadFor(null, null, at))
        assertEquals("the player is asked only for its own song", 1, read)
    }
}
