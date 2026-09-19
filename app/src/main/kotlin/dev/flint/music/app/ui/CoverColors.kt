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
    val body = p.dominantSwatch?.rgb ?: p.mutedSwatch?.rgb ?: edge.toArgb()
    val accentSeed = (p.vibrantSwatch ?: p.lightVibrantSwatch ?: p.lightMutedSwatch ?: p.dominantSwatch)?.rgb ?: body
    // The page colour keeps the cover's hue but goes where text can live: deep in dark mode, pale in light.
    val hsl = FloatArray(3).also { ColorUtils.colorToHSL(mix(body, edge.toArgb()), it) }
    val background = when {
        dark && amoled -> Color.Black
        dark -> Color(ColorUtils.HSLToColor(floatArrayOf(hsl[0], (hsl[1] * 0.75f).coerceAtMost(0.5f), hsl[2].coerceIn(0.07f, 0.14f))))
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
    repeat(2) { blur(px) }

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
        hsl[2] = (pageHsl[2] + (hsl[2] - meanL) * spread * 2.5f).coerceIn(pageHsl[2] - spread, pageHsl[2] + spread)
        px[i] = ColorUtils.HSLToColor(hsl)
    }
    return Bitmap.createBitmap(px, WASH, WASH, Bitmap.Config.ARGB_8888).asImageBitmap()
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

private fun mix(a: Int, b: Int) = ColorUtils.blendARGB(a, b, 0.5f)

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
