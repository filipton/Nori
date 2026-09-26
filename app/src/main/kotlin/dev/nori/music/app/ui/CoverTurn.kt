package dev.nori.music.app.ui

/**
 * The playing song's cover from one song to the next, decided without Compose (CoverTurnTest): which
 * song's picture is wanted, whether a picture that comes back is still that song's, and when what the
 * last song left on screen has to give way to the placeholder.
 *
 * A song whose picture is at hand shows it with the change. One whose picture still has to be read or
 * fetched keeps what is on screen for [graceMs] - long enough for a cover on the disk to come without the
 * placeholder blinking in for a frame or two first - and then goes to the placeholder, which wears no
 * song's colour, rather than leave the last song's cover under the new song's title. A picture that comes
 * back for a song that has been skipped past is not this song's, and is dropped.
 *
 * The same rules serve the sleeve's picture and the page's colours (PlayerScreen); the times are
 * nori-core's (`stage`), in milliseconds of the caller's clock.
 */
internal class CoverTurn(private val graceMs: Long) {
    enum class Show {
        /** This song's own picture (or colours). */
        PICTURE,
        /** Still what was on screen before: the grace, in which a cover on the disk comes without a blink. */
        HOLD,
        /** The placeholder, no song's colour. */
        PLATE,
    }

    /** The cover the song on the page has; null for a song without one. */
    var url: String? = null
        private set
    private var started = false
    private var since = 0L
    private var here = false
    private var cleared = false

    /**
     * The song on the page has the cover at [url] from [now]. [ready]: its picture is at hand already.
     * [cleared]: nothing of the song before is on screen any more (a record that slid in without a
     * picture took its place), so there is nothing to hold. The same cover again - the next song of the
     * same album - is no change: its picture stays.
     */
    fun song(url: String?, now: Long, ready: Boolean, cleared: Boolean = false) {
        if (started && url == this.url) {
            if (ready) here = true
            if (cleared) this.cleared = true
            return
        }
        started = true
        this.url = url
        since = now
        here = ready && url != null
        this.cleared = cleared
    }

    /** A picture for [url] has come: true when it is this song's and is to be shown, false when it is stale. */
    fun arrived(url: String?): Boolean {
        if (!started || url == null || url != this.url) return false
        here = true
        return true
    }

    /** What the page shows for this song at [now]. */
    fun show(now: Long): Show = when {
        here -> Show.PICTURE
        url == null || cleared || now - since >= graceMs -> Show.PLATE
        else -> Show.HOLD
    }

    /** How long after [now] the hold runs out; 0 when it is not holding. */
    fun holdLeft(now: Long): Long = if (show(now) == Show.HOLD) since + graceMs - now else 0L
}
