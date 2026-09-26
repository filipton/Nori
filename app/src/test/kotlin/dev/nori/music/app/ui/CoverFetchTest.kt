package dev.nori.music.app.ui

import dev.nori.music.app.ui.CoverTurn.Show
import dev.nori.music.look.CoverPixels
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class CoverFetchTest {
    /** A loader whose answers the test gives, in any order, on a clock the test moves. */
    private class Fake : CoverFetch.Source<String> {
        class Load(val url: String, val done: (String?, Int) -> Unit) {
            var cancelled = false
            var answered = false
        }

        val loads = mutableListOf<Load>()
        val kept = mutableMapOf<String, String>()
        val failures = mutableListOf<Triple<String, Int, Boolean>>()
        private val timers = mutableListOf<Pair<Long, () -> Unit>>()
        var now = 0L

        override fun kept(url: String, width: Int, height: Int): Pair<String, Boolean>? = kept[url]?.let { it to true }

        override fun load(url: String, width: Int, height: Int, done: (String?, Int) -> Unit): CoverFetch.Handle {
            val l = Load(url, done)
            loads += l
            return CoverFetch.Handle { l.cancelled = true }
        }

        override fun later(ms: Long, run: () -> Unit): CoverFetch.Handle {
            val t = (now + ms) to run
            timers += t
            return CoverFetch.Handle { timers.remove(t) }
        }

        override fun failed(url: String, status: Int, again: Boolean) {
            failures += Triple(url, status, again)
        }

        val backs = mutableListOf<() -> Unit>()

        override fun whenBack(run: () -> Unit): CoverFetch.Handle {
            backs += run
            return CoverFetch.Handle { backs.remove(run) }
        }

        /** The network comes back: everyone waiting for it is run, once. */
        fun networkBack() {
            val due = backs.toList()
            backs.clear()
            due.forEach { it() }
        }

        /** The load of [url] (the latest one) answers, unless it was cancelled: then, like CoverLoader.Request, nothing is handed on. */
        fun answer(url: String, picture: String?, status: Int = if (picture != null) CoverPixels.OK else CoverPixels.NETWORK + 4) {
            val l = loads.last { it.url == url && !it.answered }
            l.answered = true
            if (!l.cancelled) l.done(picture, status)
        }

        fun pending(url: String): Boolean = loads.any { it.url == url && !it.answered && !it.cancelled }

        fun advance(ms: Long) {
            now += ms
            while (true) {
                val due = timers.firstOrNull { it.first <= now } ?: break
                timers.remove(due)
                due.second()
            }
        }
    }

    /**
     * The sleeve as PlayerScreen composes it, without Compose: one fetch per song's cover (remember(url)),
     * the last one's let go when the song changes (its DisposableEffect), and the turn told of each song
     * and of each picture that comes.
     */
    private class Sleeve(val fake: Fake) {
        val turn = CoverTurn(180)
        var fetch: CoverFetch<String>? = null
        var shown: String? = null
        var missing = false

        fun song(url: String) {
            fetch?.stop()
            missing = false
            val f = CoverFetch(url, fake, fake.kept[url], shown = { p -> if (turn.arrived(url)) shown = p }, missing = { missing = true })
            fetch = f
            turn.song(url, fake.now, ready = f.picture != null)
            if (f.picture != null) turn.arrived(url).also { shown = f.picture }
            f.want(600, 600)
        }
    }

    @Test fun `fast skips with loads answering out of order end on the last song's picture`() {
        val fake = Fake()
        val sleeve = Sleeve(fake)
        fake.kept["A"] = "picture A"
        sleeve.song("A")
        assertEquals("picture A", sleeve.shown)
        for (url in listOf("B", "C", "D", "E", "F")) {
            fake.advance(120)
            sleeve.song(url)
        }
        // Every song passed was asked for and let go; F is on its way.
        for (url in listOf("B", "C", "D", "E")) assertFalse("$url is still asked for", fake.pending(url))
        assertTrue(fake.pending("F"))
        // The answers come back out of order, the passed songs' among them: none of them shows.
        fake.answer("D", "picture D")
        fake.answer("B", null)
        fake.answer("E", "picture E")
        assertEquals(Show.PLATE, sleeve.turn.show(fake.now + 180))
        assertEquals("nothing of a song skipped past", "picture A", sleeve.shown)
        assertTrue("no failure is told for a song let go of", fake.failures.isEmpty())
        fake.answer("C", "picture C")
        fake.answer("F", "picture F")
        assertEquals("picture F", sleeve.shown)
        assertEquals(Show.PICTURE, sleeve.turn.show(fake.now))
    }

    @Test fun `a load of the last song that fails is asked again, and its picture shows`() {
        val fake = Fake()
        val sleeve = Sleeve(fake)
        for (url in listOf("B", "C", "F")) sleeve.song(url)
        // The loader answered F with nothing: the flight it joined ended with nobody waiting (skipped away
        // and back), or the network dropped it.
        fake.answer("F", null, CoverPixels.CLOSED)
        assertFalse("the view does not give up on a failure the next try can mend", sleeve.missing)
        assertEquals(listOf(Triple("F", CoverPixels.CLOSED, true)), fake.failures)
        assertTrue(sleeve.fetch!!.asking)
        assertFalse(fake.pending("F"))
        fake.advance(CoverFetch.RETRY_MS)
        assertTrue("asked again", fake.pending("F"))
        fake.answer("F", "picture F")
        assertEquals("picture F", sleeve.shown)
        assertEquals(Show.PICTURE, sleeve.turn.show(fake.now))
    }

    @Test fun `a load that fails twice gives up, and a format that is not decoded at once`() {
        val fake = Fake()
        val sleeve = Sleeve(fake)
        sleeve.song("F")
        fake.answer("F", null, CoverPixels.HTTP + 503)
        fake.advance(CoverFetch.RETRY_MS)
        fake.answer("F", null, CoverPixels.NETWORK + 4)
        assertTrue(sleeve.missing)
        assertFalse(sleeve.fetch!!.asking)
        assertEquals(listOf(true, false), fake.failures.map { it.third })
        sleeve.song("G")
        fake.answer("G", null, CoverPixels.UNKNOWN)
        assertTrue(sleeve.missing)
        assertEquals(Triple("G", CoverPixels.UNKNOWN, false), fake.failures.last())
    }

    @Test fun `a try again is let go of with its song`() {
        val fake = Fake()
        val sleeve = Sleeve(fake)
        sleeve.song("F")
        fake.answer("F", null)
        sleeve.song("G")
        fake.advance(CoverFetch.RETRY_MS)
        assertFalse("F is not asked for again once the player has left it", fake.pending("F"))
        assertTrue(fake.pending("G"))
    }

    @Test fun `a kept picture shows at once and asks for nothing`() {
        val fake = Fake()
        fake.kept["K"] = "picture K"
        val shown = mutableListOf<String>()
        val f = CoverFetch("K", fake, fake.kept["K"], shown = { shown += it }, missing = {})
        f.want(600, 600)
        assertEquals("picture K", f.picture)
        assertTrue("the picture it started with is not handed on again", shown.isEmpty())
        assertTrue(fake.loads.isEmpty())
        val none = CoverFetch<String>(null, fake, null, shown = {}, missing = {})
        none.want(600, 600)
        assertNull(none.picture)
        assertTrue(fake.loads.isEmpty())
    }

    /** How a row's cover says what it is doing, as [CoverImage] keeps it: loading, a picture, or the plate's note. */
    private class Row(val fake: Fake, url: String) {
        var state = "loading"
        var picture: String? = null
        val fetch = CoverFetch(url, fake, null, shown = { picture = it; state = "picture" }, missing = { state = "plate" })
    }

    @Test fun `offline a cover settles on the plate at once, and still comes if the try again brings it`() {
        val fake = Fake()
        val row = Row(fake, "P")
        row.fetch.want(300, 300)
        fake.answer("P", null, CoverPixels.NETWORK + 1)
        assertEquals("no shimmer through the try again: the note is shown now", "plate", row.state)
        assertTrue("the try again still runs", row.fetch.asking)
        fake.advance(CoverFetch.RETRY_MS)
        fake.answer("P", "picture P")
        assertEquals("picture", row.state)
        assertEquals("picture P", row.picture)
        assertTrue("nobody waits for the network once it came", fake.backs.isEmpty())
    }

    @Test fun `a view that gave up asks again when the network comes back, and a failure it cannot mend never does`() {
        val fake = Fake()
        val row = Row(fake, "P")
        row.fetch.want(300, 300)
        fake.answer("P", null, CoverPixels.NETWORK + 2)
        fake.advance(CoverFetch.RETRY_MS)
        fake.answer("P", null, CoverPixels.NETWORK + 2)
        assertEquals("plate", row.state)
        assertFalse("nothing is on its way, nothing ticks", row.fetch.asking)
        assertEquals(1, fake.backs.size)
        fake.networkBack()
        assertTrue("asked again with the network", fake.pending("P"))
        fake.answer("P", "picture P")
        assertEquals("picture", row.state)
        // It failed once more, and gets its try again as a fresh start would.
        val again = Row(fake, "Q")
        again.fetch.want(300, 300)
        fake.answer("Q", null, CoverPixels.HTTP + 503)
        fake.advance(CoverFetch.RETRY_MS)
        fake.answer("Q", null, CoverPixels.HTTP + 503)
        assertEquals("a server's failure settles after its try again", "plate", again.state)
        again.fetch.stop()
        assertTrue("a view that has gone waits for nothing", fake.backs.isEmpty())
        val heic = Row(fake, "H")
        heic.fetch.want(300, 300)
        fake.answer("H", null, CoverPixels.UNKNOWN)
        assertEquals("plate", heic.state)
        assertTrue(fake.backs.isEmpty())
        fake.networkBack()
        assertFalse(fake.pending("H"))
    }

    @Test fun `only the ways of not reaching the server at all count as offline`() {
        for (kind in 0..3) assertTrue(CoverFetch.unreachable(CoverPixels.NETWORK + kind))
        for (status in listOf(CoverPixels.NETWORK + 4, CoverPixels.CLOSED, CoverPixels.HTTP + 404, CoverPixels.UNKNOWN)) assertFalse(CoverFetch.unreachable(status))
    }
}
