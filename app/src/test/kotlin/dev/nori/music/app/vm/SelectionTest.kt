package dev.nori.music.app.vm

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The selection bar's state: picked rows end with back and with any change of page. */
class SelectionTest {
    private fun picked() = Selection<String> { it }

    @Test fun `a long press picks and a second one puts back`() {
        val s = picked()
        s.toggle("a"); s.toggle("b")
        assertEquals(listOf("a", "b"), s.items.value)
        s.toggle("a")
        assertEquals(listOf("b"), s.items.value)
    }

    @Test fun `back lets go of the selection and stays on the page`() {
        val s = picked()
        s.onPage("album/1")
        s.toggle("a")
        assertTrue("back is taken by the selection", s.back())
        assertEquals(emptyList<String>(), s.items.value)
        // The next back is the page's own: nothing is picked any more.
        assertFalse(s.back())
    }

    @Test fun `back with nothing picked goes on to the page`() {
        assertFalse(picked().back())
    }

    @Test fun `leaving the page ends the selection`() {
        val s = picked()
        s.onPage("album/1")
        s.toggle("a")
        s.onPage("home")
        assertEquals(emptyList<String>(), s.items.value)
    }

    @Test fun `the same page again keeps it`() {
        val s = picked()
        s.onPage("album/1")
        s.toggle("a")
        s.onPage("album/1")
        assertEquals(listOf("a"), s.items.value)
    }

    @Test fun `the first page seen ends nothing`() {
        val s = picked()
        s.toggle("a")
        s.onPage("home")
        assertEquals(listOf("a"), s.items.value)
    }
}
