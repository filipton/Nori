package dev.nori.music.text

/**
 * How the app writes numbers on screen: times, sizes, speeds, decibels and frequencies. Plain Kotlin, no
 * Android in it, so it is tested on the JVM (core/src/test). The words around the numbers are string
 * resources ([Words], the app's `Say`); what is here are the numbers and their unit symbols.
 *
 * Fractions go through `String.format` in the default locale, which rounds half up on the shortest decimal
 * form of the number (62.5 Hz is "63", 0.15 is "0.2") and writes the phone's own decimal separator ("12,4
 * MB" on a Polish phone). Clock times are always ASCII digits.
 */
object Fmt {
    /** Times up to two hours are made once per second and kept; a longer mix is formatted as it goes. */
    const val CACHED_SECONDS = 7200

    private val made = arrayOfNulls<String>(CACHED_SECONDS)
    private val left = arrayOfNulls<String>(CACHED_SECONDS)

    /**
     * "3:07", or "1:02:03" from an hour. Each second's text is made once for the life of the process: the
     * seek bar asks for two of these every second a song plays, and every list row for its song's length,
     * so after the first time through they cost an array read and allocate nothing.
     */
    fun duration(seconds: Long): String {
        if (seconds < 0 || seconds >= CACHED_SECONDS) return clock(seconds, false)
        val i = seconds.toInt()
        return made[i] ?: clock(seconds, false).also { made[i] = it }
    }

    /** The time left in a song, under the seek bar: "-3:07". Kept the same way as [duration]. */
    fun durationLeft(seconds: Long): String {
        if (seconds < 0 || seconds >= CACHED_SECONDS) return clock(seconds, true)
        val i = seconds.toInt()
        return left[i] ?: clock(seconds, true).also { left[i] = it }
    }

    /** [duration] (or, [minus], [durationLeft]) onto the end of [out]. */
    fun appendClock(out: StringBuilder, seconds: Long, minus: Boolean) {
        val s = seconds
        if (minus) out.append('-')
        if (s >= 3600) {
            out.append(s / 3600).append(':')
            pad2(out, s / 60 % 60)
        } else {
            out.append(s / 60)
        }
        out.append(':')
        pad2(out, s % 60)
    }

    /** [appendClock] as a string of its own, its digits written into a char array: no builder, no boxing. */
    fun clock(seconds: Long, minus: Boolean): String {
        if (seconds < 0) return StringBuilder(12).also { appendClock(it, seconds, minus) }.toString()
        val c = CharArray(24)
        var i = c.size
        fun two(n: Long) { c[--i] = '0' + (n % 10).toInt(); c[--i] = '0' + (n / 10).toInt() }
        two(seconds % 60)
        c[--i] = ':'
        var lead = if (seconds >= 3600) { two(seconds / 60 % 60); c[--i] = ':'; seconds / 3600 } else seconds / 60
        do { c[--i] = '0' + (lead % 10).toInt(); lead /= 10 } while (lead > 0)
        if (minus) c[--i] = '-'
        return String(c, i, c.size - i)
    }

    /** Rust's and C's `{:02}`: a minus counts toward the width, so -5 stays "-5". */
    private fun pad2(out: StringBuilder, n: Long) {
        if (n in 0..9) out.append('0')
        out.append(n)
    }

    private val FIXED = Array(4) { "%.${it}f" }
    private val FIXED_PLUS = Array(4) { "%+.${it}f" }

    /** Java's `"%.{places}f"` in the default locale, with a '+' in front of a non-negative number when [plus]. */
    fun fixed(v: Double, places: Int, plus: Boolean = false): String =
        String.format((if (plus) FIXED_PLUS else FIXED)[places], v)

    /** A decibel figure with its sign, one decimal: "+3.5", "-1.0", and "+0.0" for nothing at all, negative zero included. */
    fun signedDb(db: Float): String = fixed(if (db == 0f) 0.0 else db.toDouble(), 1, true)

    /** How far the lyrics are nudged, "+0.5" (the unit is the caller's). */
    fun nudge(ms: Long): String = fixed((ms.toFloat() / 1000f).toDouble(), 1, true)

    /**
     * A band's frequency as its label says it: "63", "1k", "2.500k", "12.50k", "16k". The trailing zeros
     * are cut with a point in the pattern, so with a decimal comma they stay ("1,000k"), as they always did.
     */
    fun hz(f: Float): String =
        if (f >= 1000f) (String.format("%.4g", (f / 1000f).toDouble()) + "k").replace(".000k", "k").replace(".00k", "k")
        else fixed(f.toDouble(), 0)

    /** "850 B", "38 KB", "2.1 MB", "38 MB", "2.1 GB". */
    fun bytes(bytes: Long): String = when {
        bytes < 1024 -> "$bytes B"
        bytes < 1_048_576 -> fixed(bytes / 1024.0, 0) + " KB"
        bytes < 10_485_760 -> fixed(bytes / 1_048_576.0, 1) + " MB"
        bytes < 1_073_741_824 -> fixed(bytes / 1_048_576.0, 0) + " MB"
        else -> fixed(bytes / 1_073_741_824.0, 1) + " GB"
    }

    /** "12.4 MB". */
    fun megabytes(bytes: Long): String = fixed(bytes / 1_048_576.0, 1) + " MB"

    /** "850 KB/s", "3.2 MB/s" onto [out]; nothing when nothing is measurable. */
    fun appendSpeed(out: StringBuilder, bps: Long) {
        when {
            bps <= 0 -> Unit
            bps < 1_000 -> out.append(bps).append(" B/s")
            bps < 1_000_000 -> out.append(fixed(bps / 1_000.0, 0)).append(" KB/s")
            bps < 10_000_000 -> out.append(fixed(bps / 1_000_000.0, 1)).append(" MB/s")
            else -> out.append(fixed(bps / 1_000_000.0, 0)).append(" MB/s")
        }
    }

    /** A sample rate as a DAC's mode is written, "44.1 kHz", in the phone's number style ("44,1 kHz"). */
    fun kiloHertz(rate: Int): String = fixed(rate / 1000.0, 1) + " kHz"

    /** A sample rate in kHz as the records write it, "44.1", "96.0": never localised (it never was). */
    fun khz(rate: Int): String = (rate / 1000.0).toString()
}
