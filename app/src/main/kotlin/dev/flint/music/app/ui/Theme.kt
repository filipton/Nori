package dev.flint.music.app.ui

import android.os.Build
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.material3.ColorScheme
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.darkColorScheme
import androidx.compose.material3.dynamicDarkColorScheme
import androidx.compose.material3.dynamicLightColorScheme
import androidx.compose.material3.lightColorScheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.platform.LocalContext
import androidx.core.graphics.ColorUtils
import dev.flint.music.settings.Prefs
import dev.flint.music.settings.ThemeMode

/**
 * Material You: the wallpaper's colours on Android 12+, otherwise a scheme grown from one accent colour.
 * AMOLED replaces every dark surface with true black, so those pixels are simply off.
 */
@Composable
fun FlintTheme(prefs: Prefs, content: @Composable () -> Unit) {
    val context = LocalContext.current
    val dark = when (prefs.theme) { ThemeMode.SYSTEM -> isSystemInDarkTheme(); ThemeMode.DARK -> true; ThemeMode.LIGHT -> false }
    val dynamic = prefs.dynamicColor && Build.VERSION.SDK_INT >= 31
    val scheme = remember(dark, dynamic, prefs.accent, prefs.amoled) {
        val base = when {
            dynamic && dark -> dynamicDarkColorScheme(context)
            dynamic -> dynamicLightColorScheme(context)
            else -> seeded(Color(prefs.accent), dark)
        }
        if (dark && prefs.amoled) base.black() else base
    }
    MaterialTheme(colorScheme = scheme, content = content)
}

/** A light or dark scheme from one colour: tones of the same hue, like Material's own generator but tiny. */
private fun seeded(seed: Color, dark: Boolean): ColorScheme {
    val hsl = FloatArray(3).also { ColorUtils.colorToHSL(seed.toArgb(), it) }
    fun tone(l: Float, s: Float = hsl[1]) = Color(ColorUtils.HSLToColor(floatArrayOf(hsl[0], s.coerceIn(0f, 1f), l)))
    return if (dark) darkColorScheme(
        primary = tone(0.80f), onPrimary = tone(0.20f), primaryContainer = tone(0.30f), onPrimaryContainer = tone(0.90f),
        secondary = tone(0.78f, hsl[1] * 0.4f), secondaryContainer = tone(0.28f, hsl[1] * 0.4f), onSecondaryContainer = tone(0.90f, hsl[1] * 0.4f),
        surface = tone(0.07f, hsl[1] * 0.12f), background = tone(0.07f, hsl[1] * 0.12f),
        surfaceVariant = tone(0.22f, hsl[1] * 0.15f), onSurfaceVariant = tone(0.80f, hsl[1] * 0.15f),
    ) else lightColorScheme(
        primary = tone(0.40f), onPrimary = Color.White, primaryContainer = tone(0.90f), onPrimaryContainer = tone(0.12f),
        secondary = tone(0.40f, hsl[1] * 0.4f), secondaryContainer = tone(0.90f, hsl[1] * 0.4f), onSecondaryContainer = tone(0.12f, hsl[1] * 0.4f),
        surface = tone(0.98f, hsl[1] * 0.2f), background = tone(0.98f, hsl[1] * 0.2f),
        surfaceVariant = tone(0.90f, hsl[1] * 0.15f), onSurfaceVariant = tone(0.30f, hsl[1] * 0.15f),
    )
}

private fun ColorScheme.black() = copy(
    background = Color.Black, surface = Color.Black, surfaceDim = Color.Black,
    surfaceContainerLowest = Color.Black, surfaceContainerLow = Color(0xFF0A0A0A), surfaceContainer = Color(0xFF111111),
    surfaceContainerHigh = Color(0xFF181818), surfaceContainerHighest = Color(0xFF202020),
)
