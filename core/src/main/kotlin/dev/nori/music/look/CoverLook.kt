package dev.nori.music.look

/**
 * The colours a page takes from its cover, and a theme's tones from one colour, worked out in Rust
 * (crates/look) so every app built on it dresses the same record the same way. See crates/core/src/look.rs.
 */
object CoverLook {
    init { System.loadLibrary("norimusic") }

    /** Pixels a side of the wash handed back. */
    const val WASH = 128

    /** A page's colours (ARGB). [wash] is [WASH] x [WASH] pixels, or null on AMOLED black. */
    class Colours(val edge: Int, val background: Int, val on: Int, val accent: Int, val washEdge: Int, val wash: IntArray?)

    /** The page for a cover's [pixels] (ARGB, as `Bitmap.getPixels` gives them). Off the main thread. */
    fun derive(pixels: IntArray, width: Int, height: Int, dark: Boolean, amoled: Boolean): Colours? {
        val out = IntArray(5 + WASH * WASH)
        val washed = derive(pixels, width, height, dark, amoled, out)
        if (out[1] == 0) return null
        return Colours(out[0], out[1], out[2], out[3], out[4], if (washed) out.copyOfRange(5, out.size) else null)
    }

    /** Primary, on primary, primary container, on primary container, secondary, secondary container,
     *  on secondary container, surface, background, surface variant, on surface variant. */
    fun tones(seed: Int, dark: Boolean): IntArray = IntArray(11).also { tones(seed, dark, it) }

    @JvmStatic private external fun derive(pixels: IntArray, width: Int, height: Int, dark: Boolean, amoled: Boolean, out: IntArray): Boolean
    @JvmStatic private external fun tones(seed: Int, dark: Boolean, out: IntArray)
}
