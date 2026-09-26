package dev.nori.music.playback

import androidx.media3.common.Player
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

/** The long pause's rule, which both players follow (PlaybackService.idleRelease). */
class LongPauseTest {
    private val live = listOf(Player.STATE_READY, Player.STATE_BUFFERING, Player.STATE_ENDED)

    @Test fun aPauseArmsTheReleaseInEveryStateButIdle() {
        for (s in live) assertTrue("state $s", LongPause.arms(isPlaying = false, playWhenReady = false, state = s))
        assertFalse(LongPause.arms(isPlaying = false, playWhenReady = false, state = Player.STATE_IDLE))
    }

    @Test fun musicWantedOrPlayingArmsNothing() {
        for (s in live + Player.STATE_IDLE) {
            assertFalse(LongPause.arms(isPlaying = true, playWhenReady = true, state = s))
            // Waiting for the network or for the focus after a call: wanted, so not a pause.
            assertFalse(LongPause.arms(isPlaying = false, playWhenReady = true, state = s))
        }
    }

    @Test fun theReleaseStopsOnlyAPlayerStillPaused() {
        assertTrue(LongPause.releases(playWhenReady = false, state = Player.STATE_READY))
        // Played again before it fired, or already let go: nothing to do.
        assertFalse(LongPause.releases(playWhenReady = true, state = Player.STATE_READY))
        assertFalse(LongPause.releases(playWhenReady = false, state = Player.STATE_IDLE))
    }
}
