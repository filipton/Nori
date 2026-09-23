package dev.nori.music.data

import dev.nori.music.ffi.Lyrics

/**
 * Lyrics from outside the server (LRCLIB) are looked up, matched, ranked and cached by the core's client
 * (lrclib.rs); these are only the shapes the screens show them in.
 */

/** Where a set of lyrics came from, for the credit line under them. */
enum class LyricsSource(val label: String) { SERVER("your server"), LRCLIB("LRCLIB") }

data class FoundLyrics(val lyrics: Lyrics, val source: LyricsSource)
