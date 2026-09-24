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

    private fun kotlinx.coroutines.CoroutineScope.cancel() = (coroutineContext[kotlinx.coroutines.Job])!!.cancel()
}
