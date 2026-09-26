package dev.nori.music.playback

import androidx.media3.common.PlaybackException
import androidx.media3.datasource.DataSourceException
import java.io.IOException
import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/** Which failures to open a song say it ends before the place asked for (MediaSources.pastEnd). */
class PastEndTest {
    @Test fun anUnsatisfiableRangeNamesTheWholeLength() {
        assertEquals(6_406_842L, MediaSources.unsatisfiedRange("bytes */6406842"))
        assertEquals(10L, MediaSources.unsatisfiedRange(" bytes */10 "))
        assertNull(MediaSources.unsatisfiedRange("bytes */*"))
        assertNull(MediaSources.unsatisfiedRange("bytes 0-9/10"))
    }

    @Test fun aPositionOutOfRangeIsAnEndOfUnknownPlace() {
        val e = DataSourceException(PlaybackException.ERROR_CODE_IO_READ_POSITION_OUT_OF_RANGE)
        assertEquals(-1L, MediaSources.pastEnd(e))
        assertEquals(-1L, MediaSources.pastEnd(IOException("wrapped", e)))
    }

    @Test fun aNetworkThatDroppedIsNoEnd() {
        assertNull(MediaSources.pastEnd(IOException("unexpected end of stream")))
        assertNull(MediaSources.pastEnd(DataSourceException(PlaybackException.ERROR_CODE_IO_NETWORK_CONNECTION_FAILED)))
    }
}
