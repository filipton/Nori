package dev.flint.music.downloads

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class BatchTest {
    @Test
    fun theTotalHoldsStillWhileSongsFinish() {
        val b = DownloadBatch()
        repeat(49) { b.queued("s$it") }
        assertEquals("1 of 49", "${b.position()} of ${b.total}")
        b.completed("s0"); b.completed("s1")
        assertEquals("3 of 49", "${b.position()} of ${b.total}")
        repeat(49) { b.completed("s$it") }
        assertEquals(49, b.done)
        assertEquals("49 of 49", "${b.position()} of ${b.total}")
        assertTrue(b.drained)
    }

    @Test
    fun aSongQueuedTwiceCountsOnce() {
        val b = DownloadBatch()
        assertTrue("the first song starts a batch", b.queued("a"))
        assertFalse(b.queued("a"))
        b.queued("b")
        assertEquals(2, b.total)
    }

    @Test
    fun aDrainedBatchIsReplacedByTheNextSong() {
        val b = DownloadBatch()
        b.queued("a"); b.completed("a")
        assertTrue(b.drained)
        assertEquals(1, b.done)
        assertTrue(b.queued("b"))
        assertEquals(1, b.total)
        assertEquals(0, b.done)
    }

    @Test
    fun aRetryInsideTheBatchIsTheSameSong() {
        val b = DownloadBatch()
        b.queued("a"); b.queued("b")
        b.failed("a")
        assertEquals(1, b.failed)
        b.queued("a")
        assertEquals(0, b.failed)
        assertEquals(2, b.total)
        b.completed("a"); b.completed("b")
        assertEquals(2, b.done)
    }

    @Test
    fun cancelledSongsLeaveTheCount() {
        val b = DownloadBatch()
        b.queued("a"); b.queued("b"); b.queued("c")
        b.failed("b")
        b.removed("c")
        b.removed("b")
        assertEquals(1, b.total)
        assertEquals(0, b.failed)
        b.removed("x") // never part of it
        assertEquals(1, b.total)
    }

    @Test
    fun progressCountsFinishedSongsAndTheRunningOnes() {
        val b = DownloadBatch()
        repeat(4) { b.queued("s$it") }
        assertEquals(0f, b.fraction(0.0), 0f)
        b.completed("s0")
        assertEquals(0.375f, b.fraction(0.5), 1e-6f)
        // A running fraction can never claim more than the songs still open.
        assertEquals(1f, b.fraction(99.0), 1e-6f)
    }

    @Test
    fun aBatchIsNamedAfterAnAlbumOnlyWhenEverySongIsFromIt() {
        val b = DownloadBatch()
        b.queued("a", "Blue"); b.queued("b", "Blue")
        assertEquals("Blue", b.label())
        b.queued("c", "Red")
        assertNull(b.label())
        b.removed("c")
        assertEquals("Blue", b.label())
        b.queued("d")
        assertNull("a song without an album name", b.label())
    }
}
