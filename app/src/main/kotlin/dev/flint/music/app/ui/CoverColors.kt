package dev.flint.music.app.ui

import android.graphics.Bitmap
import android.util.LruCache
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.toArgb
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
    return PagePalette(edge, background, on, on.copy(alpha = 0.66f), accent)
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
