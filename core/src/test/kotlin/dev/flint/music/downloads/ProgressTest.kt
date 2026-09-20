package dev.nori.music.downloads

import kotlinx.coroutines.flow.MutableStateFlow
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class ProgressTest {
    @Test
    fun gateLetsThroughAtMostFourUpdatesASecondOfWholePercents() {
        val gate = ProgressGate()
        assertTrue("the first figure always shows", gate.offer(0f, 1_000))
        assertFalse("too soon", gate.offer(0.05f, 1_100))
        assertTrue(gate.offer(0.05f, 1_250))
        assertFalse("under a percent", gate.offer(0.055f, 1_600))
        assertTrue(gate.offer(0.07f, 1_600))
        // A chunk every millisecond for two seconds: no more than eight get through.
        var passed = 0
        for (t in 0 until 2_000) if (gate.offer(0.07f + t * 0.0004f, 2_000L + t)) passed++
        assertTrue("$passed updates in two seconds", passed <= 8)
    }

    @Test
    fun gateAlwaysShowsTheFinishAndTheSwitchToUnknown() {
        val gate = ProgressGate()
        assertTrue(gate.offer(0.995f, 0))
        assertTrue("the finish is not held back by the interval", gate.offer(1f, 10))
        assertFalse("and only once", gate.offer(1f, 1_000))

        val unknown = ProgressGate()
        assertTrue(unknown.offer(-1f, 0))
        assertFalse("still unknown: nothing to redraw", unknown.offer(-1f, 5_000))
        assertTrue("the size arriving shows at once", unknown.offer(0.2f, 5_001))
        assertTrue(unknown.offer(-1f, 5_002))
    }

    @Test
    fun fractionUsesTheStatedLengthThenTheEstimate() {
        assertEquals(0.5f, downloadFraction(200, 100, 999), 1e-6f)
        assertEquals(0.25f, downloadFraction(-1, 100, 400), 1e-6f)
        assertTrue("an estimate that is too low never reads as finished", downloadFraction(-1, 900, 400) < 1f)
        assertEquals(-1f, downloadFraction(-1, 100, 0), 0f)
    }

    @Test
    fun expectedSizeFollowsTheDownloadQuality() {
        assertEquals(9_000_000L, expectedBytes(9_000_000, 240, 0))
        // Four minutes at 192 kbps.
        assertEquals(240L * 192 * 125, expectedBytes(9_000_000, 240, 192))
        assertEquals(9_000_000L, expectedBytes(9_000_000, 0, 192))
    }

    private fun mark(phase: DownloadPhase, at: Long = 0) = DownloadMark(phase, MutableStateFlow(0f), at)

    @Test
    fun sectionsRunInQueueOrder() {
        // The index lists pending songs newest first; the queue runs oldest first.
        val pending = listOf("e", "d", "c", "b", "a")
        val marks = mapOf(
            "a" to mark(DownloadPhase.DOWNLOADING),
            "c" to mark(DownloadPhase.DOWNLOADING),
            "b" to mark(DownloadPhase.FAILED),
            "x" to mark(DownloadPhase.DONE, at = 5),
            "y" to mark(DownloadPhase.DONE, at = 9),
        )
        val s = downloadSections(pending, listOf("x", "y", "old"), marks) { it }
        assertEquals(listOf("a", "c"), s.active)
        assertEquals("a failure does not hold up the songs behind it", listOf("d", "e"), s.queued)
        assertEquals(listOf("b"), s.failed)
        assertEquals("newest first, and only this session's", listOf("y", "x"), s.finished)
    }

    @Test
    fun aSongJustFinishedIsListedBeforeTheIndexCatchesUp() {
        // Its mark says done while the index still has it pending: it is finished, not waiting.
        val s = downloadSections(listOf("b", "a"), emptyList(), mapOf("a" to mark(DownloadPhase.DONE)), { it })
        assertEquals(listOf("b"), s.queued)
        assertEquals(listOf("a"), s.finished)
    }
}
