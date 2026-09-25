package dev.nori.music.data

import dev.nori.music.ffi.model.LyricLine
import dev.nori.music.ffi.model.Lyrics
import dev.nori.music.ffi.words.LyricsOrigin
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FoundLyricsTest {
    private fun lyrics(text: String, key: ULong) =
        Lyrics(synced = true, wordTimed = false, lines = listOf(LyricLine(1000, 2000, text, emptyList(), null, false)), key = key)

    @Test
    fun the_same_words_read_again_under_another_key_are_the_same_lyrics() {
        val shown = FoundLyrics(lyrics("hold on", 7uL), LyricsOrigin.LRCLIB)
        assertTrue("the lookup run again keeps them under a new key", shown.sameAs(FoundLyrics(lyrics("hold on", 12uL), LyricsOrigin.LRCLIB)))
        assertFalse("other words", shown.sameAs(FoundLyrics(lyrics("let go", 7uL), LyricsOrigin.LRCLIB)))
        assertFalse("another source's", shown.sameAs(FoundLyrics(lyrics("hold on", 7uL), LyricsOrigin.UNISON)))
    }
}
