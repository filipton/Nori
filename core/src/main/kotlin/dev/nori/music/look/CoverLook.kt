package dev.nori.music.look

/**
 * How a page looks, worked out in Rust (crates/look) so every app built on it dresses the same record
 * the same way: a cover's colours and everything dressed in them - the theme's roles, the plates behind
 * the buttons, the chrome, the status bar, the gradients' stops - as one table of ints, looked up and
 * never recomputed. See crates/core/src/look.rs and crates/look/src/dress.rs, whose indices these are.
 */
object CoverLook {
    init { System.loadLibrary("norimusic") }

    /** Pixels a side of the wash handed back. */
    const val WASH = 128

    // A look's entries (nori_look::dress). Colours are ARGB; PAPER, BAND_TINT and the BAND_K* are Float
    // bits, and STATUS_LIGHT is 0 or 1.
    const val EDGE = 0
    const val BACKGROUND = 1
    const val ON = 2
    const val ON_VARIANT = 3
    const val ACCENT = 4
    const val MELT = 5
    const val ON_PRIMARY = 6
    const val SURFACE_VARIANT = 7
    const val SURFACE_CONTAINER = 8
    const val SURFACE_CONTAINER_HIGH = 9
    const val SECONDARY_CONTAINER = 10
    const val OUTLINE_VARIANT = 11
    const val PAPER = 12
    const val PILL = 13
    const val PILL_INK = 14
    const val PILL_PLATE = 15
    const val TINT_INK = 16
    const val CIRCLE_SELECTED = 17
    const val CIRCLE_PLATE = 18
    const val DISC = 19
    const val DISC_INK_SELECTED = 20
    const val FIELD = 21
    const val FORM = 22
    const val SWITCH_OFF = 23
    const val VEIL_13 = 24
    const val VEIL_6 = 25
    const val VEIL_10 = 26
    const val ON_60 = 27
    const val ON_45 = 28
    const val ON_55 = 29
    const val ON_22 = 30
    const val ON_85 = 31
    const val ON_35 = 32
    const val ON_80 = 33
    const val ON_VARIANT_70 = 34
    const val CHROME_SLAB = 35
    const val CHROME_CONTENT = 36
    const val CHROME_PAGE = 37
    const val CHROME_EDGE = 38
    const val CHROME_CONTENT_65 = 39
    const val CHROME_CONTENT_75 = 40
    const val CHROME_FADE = 41
    const val STATUS_LIGHT = 42
    const val BAND_TINT = 43
    const val BAND_KR = 44
    const val BAND_KG = 45
    const val BAND_KB = 46
    const val HERO_EDGE = 47
    const val HERO_MID = 48
    const val FLOOR_0 = 49
    const val FLOOR_22 = 50
    const val FLOOR_75 = 51
    const val LEN = 52

    /**
     * Looks [from] and [to] mixed at [t] into [out], all [LEN] entries (`nori_look::dress::mix`): one frame
     * of a page cross-fading. Primitives only, so a frame crosses once and allocates nothing.
     */
    @JvmStatic external fun mix(from: IntArray, to: IntArray, t: Float, out: IntArray)

    /** A page's look ([LEN] entries) and its wash, [WASH] x [WASH] pixels or null on AMOLED black. */
    class Colours(val look: IntArray, val wash: IntArray?)

    /** The page for a cover's [pixels] (ARGB, as `Bitmap.getPixels` gives them). Off the main thread. */
    fun derive(pixels: IntArray, width: Int, height: Int, dark: Boolean, amoled: Boolean): Colours? {
        val out = IntArray(LEN + WASH * WASH)
        val washed = derive(pixels, width, height, dark, amoled, out)
        if (out[BACKGROUND] == 0) return null
        return Colours(out.copyOf(LEN), if (washed) out.copyOfRange(LEN, out.size) else null)
    }

    /**
     * The look of a page in the theme's own colours, from its roles: background, on surface, on surface
     * variant, primary, on primary, surface variant, surface container, surface container high,
     * secondary container, outline variant. Once per theme.
     */
    fun plain(roles: IntArray): IntArray = IntArray(LEN).also { plain(roles, it) }

    /** Primary, on primary, primary container, on primary container, secondary, secondary container,
     *  on secondary container, surface, background, surface variant, on surface variant. */
    fun tones(seed: Int, dark: Boolean): IntArray = IntArray(11).also { tones(seed, dark, it) }

    /** What AMOLED black puts in a dark scheme: background, surface, surface dim, container lowest,
     *  low, container, high, highest. */
    fun amoled(): IntArray = IntArray(8).also { amoled(it) }

    @JvmStatic private external fun derive(pixels: IntArray, width: Int, height: Int, dark: Boolean, amoled: Boolean, out: IntArray): Boolean
    @JvmStatic private external fun plain(roles: IntArray, out: IntArray)
    @JvmStatic private external fun tones(seed: Int, dark: Boolean, out: IntArray)
    @JvmStatic private external fun amoled(out: IntArray)
}
