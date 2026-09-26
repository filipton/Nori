package dev.nori.music.app.vm

import org.junit.Assert.assertEquals
import org.junit.Test

/** Search lands on a row by its key: the row's title slugged, as the search result's title is. */
class SettingKeyTest {
    @Test
    fun keysAreTheTitlesSlugged() {
        assertEquals("fill-in-words-as-they-re-sung", settingKey("Fill in words as they're sung"))
        assertEquals("quality-on-wi-fi", settingKey("Quality on Wi-Fi"))
        assertEquals("autoeq-for-headphones", settingKey("  AutoEQ for headphones!"))
        assertEquals("skip-songs-that-won-t-play", settingKey("Skip songs that won't play"))
        assertEquals("paxsenix-musixmatch", settingKey("PaxSenix: Musixmatch"))
    }
}
