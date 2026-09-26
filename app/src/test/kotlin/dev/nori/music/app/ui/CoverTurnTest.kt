package dev.nori.music.app.ui

import dev.nori.music.app.ui.CoverTurn.Show
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class CoverTurnTest {
    private val grace = 180L

    @Test fun `a picture at hand shows with the change, no placeholder`() {
        val t = CoverTurn(grace)
        t.song("a", 0, ready = true)
        assertEquals(Show.PICTURE, t.show(0))
        assertEquals(0L, t.holdLeft(0))
    }

    @Test fun `a cover still loading holds what was there for the grace, then the placeholder`() {
        val t = CoverTurn(grace)
        t.song("a", 0, ready = true)
        t.song("b", 1_000, ready = false)
        assertEquals(Show.HOLD, t.show(1_000))
        assertEquals(grace, t.holdLeft(1_000))
        assertEquals(Show.HOLD, t.show(1_000 + grace - 1))
        assertEquals(1L, t.holdLeft(1_000 + grace - 1))
        assertEquals(Show.PLATE, t.show(1_000 + grace))
        assertEquals(0L, t.holdLeft(1_000 + grace))
    }

    @Test fun `a picture from the disk inside the grace never shows the placeholder`() {
        val t = CoverTurn(grace)
        t.song("b", 0, ready = false)
        assertTrue(t.arrived("b"))
        for (now in 0L..grace * 3 step 10) assertEquals(Show.PICTURE, t.show(now))
    }

    @Test fun `a late picture after the placeholder is still shown`() {
        val t = CoverTurn(grace)
        t.song("b", 0, ready = false)
        assertEquals(Show.PLATE, t.show(2_000))
        assertTrue(t.arrived("b"))
        assertEquals(Show.PICTURE, t.show(2_000))
    }

    @Test fun `a picture for a song skipped past is dropped`() {
        val t = CoverTurn(grace)
        t.song("a", 0, ready = false)
        t.song("b", 50, ready = false)
        assertFalse(t.arrived("a"))
        assertEquals(Show.HOLD, t.show(60))
        assertEquals(Show.PLATE, t.show(50 + grace))
    }

    @Test fun `fast skips take only the last song's picture, and the grace runs from the last skip`() {
        val t = CoverTurn(grace)
        t.song("a", 0, ready = true)
        t.song("b", 100, ready = false)
        t.song("c", 200, ready = false)
        t.song("d", 300, ready = false)
        assertFalse(t.arrived("b"))
        assertFalse(t.arrived("c"))
        assertEquals(Show.HOLD, t.show(300 + grace - 1))
        assertEquals(Show.PLATE, t.show(300 + grace))
        assertTrue(t.arrived("d"))
        assertEquals(Show.PICTURE, t.show(300 + grace))
        assertEquals("d", t.url)
    }

    @Test fun `a record that slid in without a picture leaves nothing to hold`() {
        val t = CoverTurn(grace)
        t.song("a", 0, ready = true)
        t.song("b", 1_000, ready = false, cleared = true)
        assertEquals(Show.PLATE, t.show(1_000))
        assertEquals(0L, t.holdLeft(1_000))
        assertTrue(t.arrived("b"))
        assertEquals(Show.PICTURE, t.show(1_001))
    }

    @Test fun `the same cover again keeps its picture and its time`() {
        val t = CoverTurn(grace)
        t.song("album", 0, ready = false)
        t.song("album", 150, ready = false)
        // The grace runs from the first song with this cover, not from the second.
        assertEquals(Show.PLATE, t.show(grace))
        t.song("album", 400, ready = true)
        assertEquals(Show.PICTURE, t.show(400))
    }

    @Test fun `a song without a cover goes to the placeholder, and nothing arrives for it`() {
        val t = CoverTurn(grace)
        t.song("a", 0, ready = true)
        t.song(null, 10, ready = false)
        assertEquals(Show.PLATE, t.show(10))
        assertFalse(t.arrived(null))
        assertFalse(t.arrived("a"))
    }

    @Test fun `nothing arrives before there is a song`() {
        val t = CoverTurn(grace)
        assertFalse(t.arrived("a"))
    }
}
