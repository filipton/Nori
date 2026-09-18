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

/** The colours a page takes from its cover: what the cover dissolves into, and what reads on top of it. */
data class CoverColors(val background: Color, val onBackground: Color, val onBackgroundVariant: Color, val accent: Color)

/**
 * Picked once per cover (a 128 px copy, which the list screens have usually loaded already) and kept in
 * memory, so reopening a page costs nothing. Palette runs off the main thread on ~16k pixels.
 */
private object CoverPalette {
    val cache = LruCache<String, CoverColors>(128)
}

@Composable
fun rememberCoverColors(url: String?, dark: Boolean, amoled: Boolean): CoverColors? {
    val context = LocalContext.current
    val key = url?.let { "$it|$dark|$amoled" }
    val colors by produceState(key?.let(CoverPalette.cache::get), key) {
        if (key == null || value != null) return@produceState
        value = withContext(Dispatchers.Default) {
            val result = SingletonImageLoader.get(context).execute(
                ImageRequest.Builder(context).data(url).size(CoverSize.ROW).allowHardware(false).build(),
            ) as? SuccessResult ?: return@withContext null
            derive(result.image.toBitmap(), dark, amoled)
        }?.also { CoverPalette.cache.put(key, it) }
    }
    return colors
}

private fun derive(bitmap: Bitmap, dark: Boolean, amoled: Boolean): CoverColors {
    val p = Palette.from(bitmap).maximumColorCount(16).generate()
    // The colour of the picture as a whole, not its loudest detail: dominant first, vibrant as the accent.
    val base = p.dominantSwatch?.rgb ?: p.mutedSwatch?.rgb ?: 0xFF303030.toInt()
    val accent = (p.vibrantSwatch ?: p.lightVibrantSwatch ?: p.dominantSwatch)?.rgb ?: base
    val hsl = FloatArray(3).also { ColorUtils.colorToHSL(base, it) }
    // Deep enough for white text in dark mode, pale enough for dark text in light mode; AMOLED goes all the way to black.
    val background = when {
        dark && amoled -> Color.Black
        dark -> Color(ColorUtils.HSLToColor(floatArrayOf(hsl[0], (hsl[1] * 0.85f).coerceAtMost(0.6f), hsl[2].coerceIn(0.10f, 0.20f))))
        else -> Color(ColorUtils.HSLToColor(floatArrayOf(hsl[0], (hsl[1] * 0.6f).coerceAtMost(0.45f), hsl[2].coerceIn(0.86f, 0.93f))))
    }
    val on = if (background.luminance() < 0.4f) Color.White else Color(0xFF111111)
    val accentColor = Color(accent).let { a ->
        // An accent that disappears into the background is no accent: push it towards the text colour until it reads.
        if (ColorUtils.calculateContrast(a.toArgb(), background.toArgb()) >= 3.0) a else on
    }
    return CoverColors(background, on, on.copy(alpha = 0.72f), accentColor)
}
