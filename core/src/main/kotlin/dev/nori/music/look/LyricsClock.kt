package dev.nori.music.look

import dev.nori.music.ffi.Lyrics

/**
 * How a page of lyrics moves with the song - which line is lit, how long its change takes, how far the
 * singing is into it, and when to look again - worked out in Rust (crates/look/src/lyrics.rs) so every
 * app built on it keeps the words in step the same way. See crates/core/src/look.rs.
 *
 * Made once per set of lyrics, then asked with the playhead every frame it matters: one JNI call with
 * primitives in and one `Long` out, the fields of which are read with the functions in the companion.
 * [close] frees it; a closed clock answers "nothing lit, never ask again".
 */
class LyricsClock(lyrics: Lyrics, positionMs: Long) : AutoCloseable {
    private var h = dev.nori.music.ffi.lyricsClock(lyrics, positionMs)

    /** Whether the lyrics carry per-word times, so the active line can fill in as it is sung. */
    val sweeps: Boolean = LyricsJni.sweeps(h)

    /** Where the lyrics are with the player at [positionMs]; [force] draws that moment whatever changed. */
    fun at(positionMs: Long, sweep: Boolean, force: Boolean): Long = LyricsJni.at(h, positionMs, sweep, force)

    /** What is on screen now, as a frame. */
    fun shown(): Long = LyricsJni.shown(h)

    /** Shows [line] at once and returns where to seek the player to. */
    fun tap(line: Int): Long = LyricsJni.tap(h, line)

    /** Sooner (> 0), later (< 0) or back to none (0); returns the nudge in ms. */
    fun nudge(dir: Int): Long = LyricsJni.nudge(h, dir)

    override fun close() { val was = h; h = 0; LyricsJni.destroy(was) }

    /** The fields of an answer, as `Step::pack` and `Frame::pack` lay them out. */
    companion object {
        private const val SUNG_BITS = 30
        private const val SUNG_ONE = (1 shl 18).toFloat()
        private const val ACTIVE_AT = SUNG_BITS
        private const val GLIDE_AT = ACTIVE_AT + 13
        private const val WAIT_AT = GLIDE_AT + 10
        private const val REDRAW_AT = WAIT_AT + 9

        /** The part of an answer that is what to draw: store this, so equal frames are equal values. */
        fun frame(step: Long): Long = step and ((1L shl WAIT_AT) - 1)
        /** The line lit and scrolled to, or -1. */
        fun active(frame: Long): Int = ((frame ushr ACTIVE_AT) and 0x1FFF).toInt() - 1
        /** How long the change into the active line takes, scroll and fades together. */
        fun glideMs(frame: Long): Int = ((frame ushr GLIDE_AT) and 0x3FF).toInt()
        /** How far into the active line the singing is, in UTF-16 units: 7.5 is half of the character at 7. */
        fun sung(frame: Long): Float = (frame and ((1L shl SUNG_BITS) - 1)).toFloat() / SUNG_ONE
        /** When to ask again: display frames while sweeping, ms otherwise; 0 for never. */
        fun wait(step: Long): Int = ((step ushr WAIT_AT) and 0x1FF).toInt()
        /** Whether anything on screen changed. */
        fun redraw(step: Long): Boolean = (step ushr REDRAW_AT) and 1L == 1L
    }
}

/** See crates/core/src/look.rs. */
internal object LyricsJni {
    init { System.loadLibrary("norimusic") }

    @JvmStatic external fun destroy(h: Long)
    @JvmStatic external fun sweeps(h: Long): Boolean
    /** `Step::pack`: sung (30 bits, 18 of them fraction), active + 1 (13), glide ms (10), wait (9), redraw (1). */
    @JvmStatic external fun at(h: Long, positionMs: Long, sweep: Boolean, force: Boolean): Long
    @JvmStatic external fun shown(h: Long): Long
    @JvmStatic external fun tap(h: Long, line: Int): Long
    @JvmStatic external fun nudge(h: Long, dir: Int): Long
}
