package dev.nori.music.data

import dev.nori.music.ffi.model.Lyrics
import dev.nori.music.ffi.settings.LyricsOrigin
import dev.nori.music.ffi.lyrics.LyricsPick
import dev.nori.music.ffi.lyrics.lyricsSame

/**
 * Lyrics from outside the server (LRCLIB) are looked up, matched, ranked and cached by the core's client
 * (lrclib.rs); this is only the shape the screens show them in. Where they came from, and how the credit
 * line under them says it, are the core's (`LyricsOrigin`, `words_lyrics_credit`).
 */
data class FoundLyrics(val lyrics: Lyrics, val source: LyricsOrigin)

/**
 * The same words from the same place as [other], whatever key the core kept their timing under: the
 * same lyrics read again (the lookup run again for a song come back to) are not new lyrics to show.
 * The rule is the core's (`nori_lyrics::race::lyrics_same`); asked once per answer, never per frame.
 */
fun FoundLyrics.sameAs(other: FoundLyrics): Boolean =
    this === other || lyricsSame(LyricsPick(lyrics, source), LyricsPick(other.lyrics, other.source))
