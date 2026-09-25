package dev.nori.music.app.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class FrameFadesTest {
    private val times = FadeTimes(inMs = 380f, outMs = 200f, delayMs = 80f, riseMs = 420f)

    private fun FrameFades<String>.levels() = pieces.map { it.value to it.level }

    @Test fun `the first piece is simply there`() {
        val f = FrameFades("a", { it }, times)
        assertEquals(listOf("a" to 1f), f.levels())
        assertFalse(f.moving)
    }

    @Test fun `a slow frame slows the fade rather than ending it`() {
        val f = FrameFades("old", { it }, times)
        f.show("new")
        // The frame the song changes on took a quarter of a second: it counts as two frames' worth.
        f.step(250f.coerceAtMost(FADE_MAX_STEP_MS))
        val old = f.pieces.first { it.value == "old" }
        assertTrue("the old words are still mostly there: ${old.level}", old.level > 0.8f)
        repeat(12) { f.step(16.7f) }
        assertTrue(f.pieces.none { it.value == "old" })
        assertEquals("new", f.current?.value)
    }

    @Test fun `the new piece waits, then fades in and rises`() {
        val f = FrameFades("a", { it }, times)
        f.show("b")
        f.step(80f)
        val b = f.current!!
        assertEquals(0f, b.level)
        f.step(190f)
        assertEquals(0.5f, b.level, 0.001f)
        repeat(20) { f.step(16.7f) }
        assertEquals(1f, b.level)
        assertEquals(1f, b.rise)
        assertFalse(f.moving)
    }

    @Test fun `a piece asked for again while leaving turns round where it is`() {
        val f = FrameFades("a", { it }, times)
        f.show("loading")
        f.step(100f)
        val a = f.pieces.first { it.value == "a" }
        assertEquals(0.5f, a.level, 0.001f)
        f.show("a")
        assertTrue("the same piece, not a new copy", f.current === a)
        f.step(19f)
        assertEquals(0.55f, a.level, 0.001f)
        assertTrue("the loader, barely in, has gone out again", f.pieces.none { it.value == "loading" })
    }

    @Test fun `the same key only updates the piece`() {
        val f = FrameFades("a1", { it.take(1) }, times)
        f.show("a2")
        assertEquals(listOf("a2" to 1f), f.levels())
        assertFalse(f.moving)
    }

    @Test fun `finish puts everything where it is going`() {
        val f = FrameFades("a", { it }, times)
        f.show("b")
        f.finish()
        assertEquals(listOf("b" to 1f), f.levels())
        assertFalse(f.moving)
    }

    @Test fun `the ease is the same both ways`() {
        for (i in 0..10) assertEquals(1f - fadeEase(1f - i / 10f), fadeEase(i / 10f), 1e-6f)
    }
}
