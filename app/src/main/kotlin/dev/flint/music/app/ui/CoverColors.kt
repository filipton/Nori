package dev.flint.music.app.ui

import android.graphics.Bitmap
import android.util.LruCache
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.toArgb
import androidx.core.graphics.scale
import androidx.compose.ui.platform.LocalContext
import androidx.core.graphics.ColorUtils
import androidx.palette.graphics.Palette
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext

/**
 * Picked once per cover (a 160 px copy, which the list screens have usually loaded already) and kept
 * in memory, so reopening a page costs nothing. Palette runs off the main thread on ~25k pixels.
 */
private object CoverPalette {
    val cache = LruCache<String, PagePalette>(128)
}

@Composable
fun rememberCoverPalette(url: String?, dark: Boolean, amoled: Boolean): PagePalette? {
    val context = LocalContext.current
    val key = url?.let { "$it|$dark|$amoled" }
    val palette by produceState(key?.let(CoverPalette.cache::get), key) {
        // Drop the previous cover's colours the moment the track changes: holding them while the new
        // artwork loads leaves the mini player wearing the last song's tint for a second.
        value = key?.let(CoverPalette.cache::get)
        if (key == null || value != null) return@produceState
        value = withContext(Dispatchers.Default) {
            val result = SingletonImageLoader.get(context).execute(
                ImageRequest.Builder(context).data(url).size(CoverSize.ROW).allowHardware(false).build(),
            ) as? SuccessResult ?: return@withContext null
            derive(result.image.toBitmap(), dark, amoled)
        }?.also { CoverPalette.cache.put(key, it) }
    }
    return palette
}

/**
 * The seam is the whole trick. A page tinted with the cover's *dominant* colour still shows a line
 * where the picture ends, because the bottom of a picture is rarely its dominant colour. So the wash
 * starts from the average of the cover's bottom rows - the exact colour the last pixel row is - and
 * travels from there into a page colour deep or pale enough to carry text.
 */
private fun derive(bitmap: Bitmap, dark: Boolean, amoled: Boolean): PagePalette {
    val edge = Color(bottomAverage(bitmap))
    val p = Palette.from(bitmap).maximumColorCount(16).generate()
    val body = dominant(bitmap, edge.toArgb())
    val accentSeed = (p.vibrantSwatch ?: p.lightVibrantSwatch ?: p.lightMutedSwatch ?: p.dominantSwatch)?.rgb ?: body
    // The page colour keeps the cover's hue but goes where text can live: deep in dark mode, pale in
    // light. It is [body] alone, with none of [edge] mixed in any more. Both branches below throw the
    // seed's lightness away and clamp it into a narrow band, so all a dark bottom row ever contributed
    // was its hue and its greyness - which is exactly what turned a sleeve of pale dusty pink into a
    // brown page, because that sleeve has dark hair along its bottom edge. Carrying the seam is not
    // this colour's job: [edge] is handed out separately and the wash still starts from it.
    val hsl = FloatArray(3).also { ColorUtils.colorToHSL(body, it) }
    val background = when {
        dark && amoled -> Color.Black
        // Clamping to 0.14 and taking a quarter off the saturation left pale sleeves with no colour a
        // viewer would name - a dusty pink arrived as a neutral brown. There was room to spare: white
        // text has about 14:1 on that pink at 0.20, and better than 8:1 on the worst case the band
        // allows (a yellow at the top of it), which still holds after the wash's own +0.05 of lightness.
        dark -> Color(ColorUtils.HSLToColor(floatArrayOf(hsl[0], (hsl[1] * 0.85f).coerceAtMost(0.55f), hsl[2].coerceAtMost(0.20f))))
        else -> Color(ColorUtils.HSLToColor(floatArrayOf(hsl[0], (hsl[1] * 0.55f).coerceAtMost(0.4f), hsl[2].coerceIn(0.90f, 0.96f))))
    }
    val on = if (background.luminance() < 0.4f) Color.White else Color(0xFF0D0D0D)
    // An accent that disappears into the page is no accent: lighten or darken it until it reads.
    val accent = readable(Color(accentSeed), background, on)
    // AMOLED black is a promise that those pixels are switched off; a wash would light them up again.
    val wash = if (amoled && dark) null else runCatching { washOf(bitmap, background, dark) }.getOrNull()
    return PagePalette(edge, background, on, on.copy(alpha = 0.66f), accent, wash = wash)
}

/**
 * How many pixels a side the wash is kept at. Sixteen held the colour but no shape at all, so the page
 * read as a plain field and the sleeve looked like it stopped dead; Apple's blur still has the record's
 * forms in it, which is what makes the picture seem to carry on behind the words. Thirty-two costs
 * 4 kB and exactly the same one quad per frame.
 */
private const val WASH = 32

/** How far each pixel of the wash is pulled back towards the flat page colour. */
private const val MUTE = 0.62f

/**
 * Apple's player is not painted one flat colour. Measure across their screenshot and the page varies
 * both ways: the background *is* the artwork, enormously enlarged and blurred, which is why it matches
 * the cover so exactly. One average of the cover's bottom rows cannot do that - a sleeve that is black
 * at the bottom and vivid everywhere else turns the whole page to mud.
 *
 * This is the same thing done cheaply: the cover shrunk to [WASH] pixels a side and smoothed once,
 * here, off the main thread, and then drawn stretched over the page, where the GPU's own bilinear
 * filter does the enlarging for free. It costs one 1 kB texture per cover and one quad per frame -
 * no blur shader, no `RenderEffect`, nothing that changes between frames.
 *
 * Every pixel is then pulled to within a hair of the page colour's own lightness and its saturation
 * held back, so the hues vary across the page but the contrast the text needs does not.
 */
private fun washOf(bitmap: Bitmap, background: Color, dark: Boolean): ImageBitmap {
    val small = bitmap.scale(WASH, WASH)
    val px = IntArray(WASH * WASH)
    small.getPixels(px, 0, WASH, 0, 0, WASH, WASH)
    if (small !== bitmap) small.recycle()
    repeat(3) { blur(px) }

    val pageHsl = FloatArray(3).also { ColorUtils.colorToHSL(background.toArgb(), it) }
    val hsl = FloatArray(3)
    var meanL = 0f
    for (p in px) { ColorUtils.colorToHSL(p, hsl); meanL += hsl[2] }
    meanL /= px.size
    // How far from the page colour a pixel may stray. Wider and text starts to sit on a light patch.
    val spread = if (dark) 0.05f else 0.035f
    val pull = if (dark) 0.9f else 0.7f
    val maxSat = if (dark) 0.5f else 0.32f
    for (i in px.indices) {
        ColorUtils.colorToHSL(px[i], hsl)
        hsl[1] = (hsl[1] * pull).coerceAtMost(maxSat)
        hsl[2] = (pageHsl[2] + (hsl[2] - meanL) * spread * 2.5f).coerceIn(pageHsl[2] - spread, pageHsl[2] + spread).coerceIn(0f, 1f)
        // Then most of the way back to the flat page colour. Clamping the lightness alone was not
        // enough on a record that is many colours at once: one that is teal down one side and warm
        // down the other gave the page teal and warm patches, and a patch reads as a fault where a
        // glow does not. Apple's pages look calm because their covers are mostly a single hue, not
        // because their wash does less - measured, their colour actually varies rather more than
        // ours did. Pulling each pixel back towards the page colour keeps the drift and takes the
        // shouting out of it.
        px[i] = ColorUtils.blendARGB(ColorUtils.HSLToColor(hsl), background.toArgb(), MUTE)
    }
    return Bitmap.createBitmap(smooth(px), WASH_OUT, WASH_OUT, Bitmap.Config.ARGB_8888).asImageBitmap()
}

/**
 * The size the wash is handed to the GPU at. The colours and shapes are worked out at [WASH], which is
 * plenty for what the page shows; but stretched straight from there over a whole screen, each of its
 * pixels became a visible step - the owner's friend called the blur "stairs" - and the melt at the
 * bottom of the sleeve, which reads the texture a row at a time, stepped as well. Four times finer,
 * filled in smoothly here once per cover, the steps are a few screen pixels tall and gone. It costs a
 * 64 kB texture instead of 4 kB, and still one quad per frame.
 */
private const val WASH_OUT = WASH * 4

/**
 * [WASH] pixels a side up to [WASH_OUT]: bilinear, then two light box passes so the corners the
 * bilinear leaves between samples round off, then a dither of one level either way per channel.
 * The dither is what hides the banding that eight bits give a slow dark gradient - a page that goes
 * from one deep colour to another over a whole screen has only a handful of levels to do it in, and
 * without noise each level is a band with an edge.
 */
private fun smooth(px: IntArray): IntArray {
    val n = WASH_OUT
    val r = FloatArray(n * n); val g = FloatArray(n * n); val b = FloatArray(n * n)
    val scale = (WASH - 1).toFloat() / (n - 1)
    for (y in 0 until n) {
        val fy = y * scale; val y0 = fy.toInt().coerceAtMost(WASH - 2); val ty = fy - y0
        for (x in 0 until n) {
            val fx = x * scale; val x0 = fx.toInt().coerceAtMost(WASH - 2); val tx = fx - x0
            val a = px[y0 * WASH + x0]; val bb = px[y0 * WASH + x0 + 1]
            val c = px[(y0 + 1) * WASH + x0]; val d = px[(y0 + 1) * WASH + x0 + 1]
            fun ch(v: Int, s: Int) = ((v shr s) and 0xFF).toFloat()
            fun lerp(s: Int) = (ch(a, s) * (1 - tx) + ch(bb, s) * tx) * (1 - ty) + (ch(c, s) * (1 - tx) + ch(d, s) * tx) * ty
            val i = y * n + x
            r[i] = lerp(16); g[i] = lerp(8); b[i] = lerp(0)
        }
    }
    repeat(2) { boxBlur(r, n); boxBlur(g, n); boxBlur(b, n) }
    // Fixed seed: the same cover gives the same texture every time, so reopening a page shows exactly
    // what it showed before.
    val noise = java.util.Random(0x5EED)
    return IntArray(n * n) { i ->
        fun q(v: Float) = (v + noise.nextFloat() - 0.5f).toInt().coerceIn(0, 255)
        (0xFF shl 24) or (q(r[i]) shl 16) or (q(g[i]) shl 8) or q(b[i])
    }
}

/** A separable box blur of radius 2 over an n by n channel, in place. */
private fun boxBlur(c: FloatArray, n: Int) {
    val tmp = FloatArray(c.size)
    for (y in 0 until n) for (x in 0 until n) {
        var s = 0f
        for (d in -2..2) s += c[y * n + (x + d).coerceIn(0, n - 1)]
        tmp[y * n + x] = s / 5f
    }
    for (y in 0 until n) for (x in 0 until n) {
        var s = 0f
        for (d in -2..2) s += tmp[(y + d).coerceIn(0, n - 1) * n + x]
        c[y * n + x] = s / 5f
    }
}

/** One separable 3-tap box pass over the tiny wash, so bilinear enlargement has no creases to show. */
private fun blur(px: IntArray) {
    val out = IntArray(px.size)
    for (pass in 0..1) {
        val src = if (pass == 0) px else out
        val dst = if (pass == 0) out else px
        for (y in 0 until WASH) for (x in 0 until WASH) {
            var r = 0; var g = 0; var b = 0
            for (d in -1..1) {
                val i = if (pass == 0) y * WASH + (x + d).coerceIn(0, WASH - 1)
                else (y + d).coerceIn(0, WASH - 1) * WASH + x
                val p = src[i]
                r += (p shr 16) and 0xFF; g += (p shr 8) and 0xFF; b += p and 0xFF
            }
            dst[y * WASH + x] = (0xFF shl 24) or ((r / 3) shl 16) or ((g / 3) shl 8) or (b / 3)
        }
    }
}

/**
 * Hue buckets the histogram counts into, then one for pale greys and whites, then one for black and
 * near-black. Black gets a bucket of its own at full weight: a sleeve that is mostly black is a black
 * record, and its page should be black too. White and pale grey stay discounted - a page that pale
 * would take the controls on it with it.
 */
private const val HUES = 18
private const val NEUTRAL = HUES
private const val DARK = HUES + 1

/**
 * The colour there is most of, which is not the question `Palette.dominantSwatch` answers. Palette
 * quantises to sixteen clusters and discards whole families of colour on the way in: its default
 * filter drops anything within a few points of white or black, and anything in the 10-37 degree band
 * unless it is strongly saturated. On a sleeve that is a field of pale dusty pink with a dark head of
 * hair down one side, the pink is thinned across several clusters and the hair comes through whole, so
 * the "dominant" swatch was the hair and the page took a hue the sleeve barely contains.
 *
 * So count pixels instead. Each sampled pixel votes for its hue in twenty-degree buckets - wide enough
 * that a field of one colour does not split across two of them - the heaviest bucket wins, and the
 * answer is that bucket's own weighted mean, so the page keeps the character of the field and not only
 * its hue. A pixel with almost no saturation votes at a quarter weight, and one so dark or so pale that
 * its hue is guesswork at less again, both into a bucket of their own: a sleeve that really is mostly
 * ink or mostly paper still gets a neutral page, but a white paper flower does not outvote the pink it
 * lies on. Nothing here rewards a colour for standing out - a small vivid mark loses to a large dull
 * field, which is the whole point.
 *
 * Every second row and column of a 160 px copy is ~6k pixels, on the same background pass as Palette
 * and cached with it, so this is paid once per cover.
 */
private fun dominant(bitmap: Bitmap, fallback: Int): Int {
    val w = bitmap.width
    val h = bitmap.height
    if (w <= 0 || h <= 0) return fallback
    val rowStep = (h / 64).coerceAtLeast(1)
    val colStep = (w / 64).coerceAtLeast(1)
    val row = IntArray(w)
    val hsl = FloatArray(3)
    val weight = FloatArray(HUES + 2)
    val sumR = FloatArray(HUES + 2)
    val sumG = FloatArray(HUES + 2)
    val sumB = FloatArray(HUES + 2)
    var y = 0
    while (y < h) {
        bitmap.getPixels(row, 0, w, 0, y, w, 1)
        var x = 0
        while (x < w) {
            val px = row[x]
            ColorUtils.colorToHSL(px, hsl)
            // Dark and colourless - or so dark that any hue it has is noise - is black.
            val black = hsl[2] < 0.06f || (hsl[2] < 0.18f && hsl[1] < 0.25f)
            val bucket = when {
                black -> DARK
                hsl[2] > 0.97f || hsl[1] < 0.10f -> NEUTRAL
                else -> (hsl[0] / (360f / HUES)).toInt().coerceIn(0, HUES - 1)
            }
            val wt = when (bucket) {
                DARK -> 1f
                NEUTRAL -> 0.25f * (if (hsl[2] > 0.97f) 0.4f else 1f)
                else -> 0.25f + 0.75f * (hsl[1] / 0.25f).coerceAtMost(1f)
            }
            weight[bucket] += wt
            sumR[bucket] += wt * ((px shr 16) and 0xFF)
            sumG[bucket] += wt * ((px shr 8) and 0xFF)
            sumB[bucket] += wt * (px and 0xFF)
            x += colStep
        }
        y += rowStep
    }
    var best = 0
    for (i in weight.indices) if (weight[i] > weight[best]) best = i
    val n = weight[best]
    if (n <= 0f) return fallback
    return (0xFF shl 24) or
        ((sumR[best] / n).toInt().coerceIn(0, 255) shl 16) or
        ((sumG[best] / n).toInt().coerceIn(0, 255) shl 8) or
        (sumB[best] / n).toInt().coerceIn(0, 255)
}

/** The colour of the cover's last rows: what the page has to start from for the picture to melt into it. */
private fun bottomAverage(bitmap: Bitmap): Int {
    val h = bitmap.height
    val w = bitmap.width
    val rows = (h / 12).coerceIn(1, 12)
    val row = IntArray(w)
    var r = 0L; var g = 0L; var b = 0L
    for (y in h - rows until h) {
        bitmap.getPixels(row, 0, w, 0, y, w, 1)
        for (px in row) { r += (px shr 16) and 0xFF; g += (px shr 8) and 0xFF; b += px and 0xFF }
    }
    val n = (w * rows).coerceAtLeast(1)
    return (0xFF shl 24) or ((r / n).toInt() shl 16) or ((g / n).toInt() shl 8) or (b / n).toInt()
}

/** Pushes a colour lighter or darker in its own hue until it has contrast against the page. */
private fun readable(color: Color, background: Color, fallback: Color): Color {
    if (ColorUtils.calculateContrast(color.toArgb(), background.toArgb()) >= 3.2) return color
    val hsl = FloatArray(3).also { ColorUtils.colorToHSL(color.toArgb(), it) }
    val towardsLight = background.luminance() < 0.4f
    for (step in 1..8) {
        hsl[2] = if (towardsLight) (hsl[2] + 0.07f).coerceAtMost(0.92f) else (hsl[2] - 0.07f).coerceAtLeast(0.15f)
        hsl[1] = (hsl[1] * 1.05f).coerceAtMost(1f)
        val candidate = Color(ColorUtils.HSLToColor(hsl))
        if (ColorUtils.calculateContrast(candidate.toArgb(), background.toArgb()) >= 3.2) return candidate
    }
    return fallback
}
