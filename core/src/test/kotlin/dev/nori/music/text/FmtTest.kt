package dev.nori.music.text

import org.junit.After
import org.junit.Assert.assertEquals
import org.junit.Before
import org.junit.Test
import java.util.Locale

/**
 * The app's numbers read as they did when the core wrote them (crates/words and crates/text, whose test
 * vectors these are): Java's rounding on the shortest decimal form, the locale's decimal separator, and
 * the clock's times.
 */
class FmtTest {
    private lateinit var was: Locale

    @Before fun root() { was = Locale.getDefault(); Locale.setDefault(Locale.ROOT) }
    @After fun back() { Locale.setDefault(was) }

    private class Fixed(val bits: ULong, val places: Int, val plus: Boolean, val want: String)

    /** Printed by Java's `String.format` (OpenJDK 21, root locale): value bits, places, plus, result. */
    private val fixed = listOf(
        Fixed(0x4016187300000000uL, 1, true, "+5.5"),
        Fixed(0x401e7d2900000000uL, 1, true, "+7.6"),
        Fixed(0xc0022c4f00000000uL, 0, true, "-2"),
        Fixed(0xbff72e8e00000000uL, 0, false, "-1"),
        Fixed(0xc01d7c2820000000uL, 0, false, "-7"),
        Fixed(0x40085eb800000000uL, 0, true, "+3"),
        Fixed(0xbfe16ac400000000uL, 1, false, "-0.5"),
        Fixed(0x40145c4400000000uL, 1, true, "+5.1"),
        Fixed(0xc017efbde0000000uL, 1, true, "-6.0"),
        Fixed(0xc0154d29e0000000uL, 1, true, "-5.3"),
        Fixed(0xc0205cf940000000uL, 2, false, "-8.18"),
        Fixed(0x40207ef3c0000000uL, 1, true, "+8.2"),
        Fixed(0x4019460700000000uL, 1, false, "6.3"),
        Fixed(0x3fef623c00000000uL, 3, true, "+0.981"),
        Fixed(0xc006892800000000uL, 1, false, "-2.8"),
        Fixed(0x401b092d00000000uL, 1, false, "6.8"),
        Fixed(0xc021068d40000000uL, 0, false, "-9"),
        Fixed(0xc02061f160000000uL, 2, false, "-8.19"),
        Fixed(0xc02282e780000000uL, 2, true, "-9.26"),
        Fixed(0xc02447d400000000uL, 2, false, "-10.14"),
        Fixed(0x40056e9400000000uL, 2, true, "+2.68"),
        Fixed(0x402181e000000000uL, 3, false, "8.754"),
        Fixed(0xc022b47560000000uL, 0, true, "-9"),
        Fixed(0xc00cc57400000000uL, 2, false, "-3.60"),
        Fixed(0xc0118c7580000000uL, 2, false, "-4.39"),
        Fixed(0xc01ff52d00000000uL, 3, false, "-7.989"),
        Fixed(0xc011aea1e0000000uL, 2, false, "-4.42"),
        Fixed(0xc016394f40000000uL, 0, false, "-6"),
        Fixed(0x4022ca9540000000uL, 1, false, "9.4"),
        Fixed(0xc002771480000000uL, 2, false, "-2.31"),
        Fixed(0xc0015fca00000000uL, 3, true, "-2.172"),
        Fixed(0x3ff495b400000000uL, 1, false, "1.3"),
        Fixed(0x4021a20b40000000uL, 0, false, "9"),
        Fixed(0x3ff9d20c00000000uL, 2, false, "1.61"),
        Fixed(0x40279fb400000000uL, 0, true, "+12"),
        Fixed(0x3f83580000000000uL, 2, true, "+0.01"),
        Fixed(0x3fbbfa5000000000uL, 0, true, "+0"),
        Fixed(0xc003965d80000000uL, 0, true, "-2"),
        Fixed(0x402426d640000000uL, 1, false, "10.1"),
        Fixed(0x3fdd039400000000uL, 0, false, "0"),
        Fixed(0x3fc7a4e000000000uL, 0, false, "0"),
        Fixed(0xc018258de0000000uL, 2, true, "-6.04"),
        Fixed(0xc014f72520000000uL, 1, true, "-5.2"),
        Fixed(0xc01d4843c0000000uL, 2, false, "-7.32"),
        Fixed(0xc02156bfc0000000uL, 3, true, "-8.669"),
        Fixed(0xc01b450cc0000000uL, 2, true, "-6.82"),
        Fixed(0x3fe0dd8c00000000uL, 1, true, "+0.5"),
        Fixed(0xc008d40f00000000uL, 0, true, "-3"),
        Fixed(0xc01a79ee80000000uL, 2, false, "-6.62"),
        Fixed(0xc025d73180000000uL, 0, true, "-11"),
    )

    /** Printed by the app's own hz() before it moved to the core: the float's bits and the label. */
    private val hz = listOf(
        0x45426b37.toInt() to "3.111k",
        0x43200000.toInt() to "160",
        0x44c80000.toInt() to "1.600k",
        0x41bdea63.toInt() to "24",
        0x42480000.toInt() to "50",
        0x43c80000.toInt() to "400",
        0x457493c9.toInt() to "3.913k",
        0x461c3e66.toInt() to "10k",
        0x42c80000.toInt() to "100",
        0x429344a3.toInt() to "74",
        0x461c4000.toInt() to "10k",
        0x41fc0000.toInt() to "32",
        0x45a57b63.toInt() to "5.295k",
        0x451c4000.toInt() to "2.500k",
        0x4479e000.toInt() to "1000",
        0x41e43bc0.toInt() to "29",
        0x441d8000.toInt() to "630",
        0x45c4e000.toInt() to "6.300k",
        0x446b9868.toInt() to "942",
        0x425e943c.toInt() to "56",
        0x45ac33d9.toInt() to "5.510k",
        0x41ed95b8.toInt() to "30",
        0x45f5c8fa.toInt() to "7.865k",
        0x43c93620.toInt() to "402",
        0x45bfb894.toInt() to "6.135k",
        0x4323b0cb.toInt() to "164",
    )

    @Test fun fixedRoundsAsJavaDoes() {
        for (f in fixed) assertEquals("${Double.fromBits(f.bits.toLong())} to ${f.places}", f.want, Fmt.fixed(Double.fromBits(f.bits.toLong()), f.places, f.plus))
        assertEquals("0.2", Fmt.fixed(0.15, 1))
        assertEquals("63", Fmt.fixed(62.5, 0))
        assertEquals("-0.0", Fmt.fixed(-0.04, 1))
        assertEquals("10.0", Fmt.fixed(9.96, 1))
        assertEquals("0.001", Fmt.fixed(0.0005, 3))
    }

    @Test fun frequenciesReadAsTheyDid() {
        for ((bits, want) in hz) assertEquals(want, Fmt.hz(Float.fromBits(bits)))
        assertEquals("12.50k", Fmt.hz(12_500f))
        assertEquals("2.500k", Fmt.hz(2_500f))
        assertEquals("16k", Fmt.hz(16_000f))
        assertEquals("63", Fmt.hz(62.5f))
    }

    @Test fun aCommaWhereThePointWasAndNowhereElse() {
        Locale.setDefault(Locale.forLanguageTag("pl-PL"))
        assertEquals("12,4", Fmt.fixed(12.4, 1))
        assertEquals("+3,0", Fmt.fixed(3.0, 1, true))
        assertEquals("1234,5", Fmt.fixed(1234.5, 1))
        // The point-only trim misses a comma, as it always did.
        assertEquals("1,000k", Fmt.hz(1_000f))
        assertEquals("12,50k", Fmt.hz(12_500f))
        assertEquals("12,4 MB", Fmt.megabytes(12 * 1_048_576L + 419_431))
    }

    @Test fun timesDecibelsAndSizes() {
        assertEquals("0:00", Fmt.duration(0))
        assertEquals("3:07", Fmt.duration(187))
        assertEquals("1:02:03", Fmt.duration(3723))
        assertEquals("-3:07", Fmt.durationLeft(187))
        assertEquals("-0:00", Fmt.durationLeft(0))
        assertEquals("-1:02:03", Fmt.durationLeft(3723))
        assertEquals("2:00:00", Fmt.duration(7200))
        for (t in longArrayOf(0, 9, 59, 60, 61, 599, 3599, 3600, 3723, 36_000, 360_000)) for (m in listOf(false, true)) {
            val b = StringBuilder(); Fmt.appendClock(b, t, m)
            assertEquals(b.toString(), Fmt.clock(t, m))
        }
        assertEquals("+0.0", Fmt.signedDb(-0.0f))
        assertEquals("+3.3", Fmt.signedDb(3.25f))
        assertEquals("-1.0", Fmt.signedDb(-1.0f))
        assertEquals("-0.3", Fmt.nudge(-250))
        assertEquals("850 B", Fmt.bytes(850))
        assertEquals("38 MB", Fmt.bytes(38 * 1_048_576L))
        assertEquals("2.1 GB", Fmt.bytes(2_254_857_830))
        assertEquals("10.1 MB", Fmt.megabytes(10 * 1_048_576L + 104_858))
        assertEquals("44.1", Fmt.khz(44_100))
        assertEquals("96.0", Fmt.khz(96_000))
        val b = StringBuilder()
        Fmt.appendSpeed(b, 0); assertEquals("", b.toString())
        Fmt.appendSpeed(b, 850_000); assertEquals("850 KB/s", b.toString()); b.setLength(0)
        Fmt.appendSpeed(b, 3_200_000); assertEquals("3.2 MB/s", b.toString())
    }
}
