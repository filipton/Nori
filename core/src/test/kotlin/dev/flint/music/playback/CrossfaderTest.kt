package dev.flint.music.playback

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import java.nio.ByteBuffer
import java.nio.ByteOrder
import kotlin.math.abs

class CrossfaderTest {
    private fun pcm(frames: Int, value: Short): ByteBuffer {
        val b = ByteBuffer.allocate(frames * 4).order(ByteOrder.nativeOrder())
        repeat(frames * 2) { b.putShort(value) }
        b.flip()
        return b
    }

    @Test
    fun startsAsTheOldTrackEndsAsTheNewOneAndKeepsPower() {
        val fader = Crossfader(pcm(1000, 10000), 4)
        val out = ByteBuffer.allocate(4000).order(ByteOrder.nativeOrder())
        val next = pcm(1500, 10000)
        fader.mix(next, out)
        assertTrue(fader.done)
        assertEquals("input beyond the fade is left for the caller", 500 * 4, next.remaining())
        out.flip()
        val samples = ShortArray(2000) { out.short }
        // Two uncorrelated tracks keep constant power under cos/sin gains; identical ones peak at +3 dB mid-fade.
        assertTrue(abs(samples[0] - 10000) < 50)
        assertTrue(abs(samples[1999] - 10000) < 50)
        assertTrue(abs(samples[1000] - 14142) < 50)
    }

    @Test
    fun aShortNextTrackLetsTheEndingFadeOut() {
        val fader = Crossfader(pcm(1000, 10000), 4)
        val out = ByteBuffer.allocate(8000).order(ByteOrder.nativeOrder())
        fader.mix(pcm(100, 0), out)
        fader.rest(out)
        out.flip()
        assertEquals(1000 * 4, out.remaining())
        val samples = ShortArray(2000) { out.short }
        assertTrue(samples[1999] < 100)
    }
}
