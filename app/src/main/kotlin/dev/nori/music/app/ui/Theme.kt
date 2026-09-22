package dev.nori.music.app.ui

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
import dev.nori.music.settings.Prefs
import dev.nori.music.settings.ThemeMode

/**
 * Material You: the wallpaper's colours on Android 12+, otherwise a scheme grown from one accent colour.
 * AMOLED replaces every dark surface with true black, so those pixels are simply off.
 */
@Composable
fun NoriTheme(prefs: Prefs, content: @Composable () -> Unit) {
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
    val system = androidx.compose.ui.platform.LocalDensity.current
    val widthDp = androidx.compose.ui.platform.LocalConfiguration.current.screenWidthDp
    val scale = uiScale(prefs.uiScale, widthDp)
    // The whole app's density, scaled once here: dp and sp both follow it, so every size keeps its
    // proportion to the screen. The system's font scale is left as it is - that one is the reader's.
    val density = remember(system, scale) {
        if (scale == 1f) system else androidx.compose.ui.unit.Density(system.density * scale, system.fontScale)
    }
    androidx.compose.runtime.CompositionLocalProvider(androidx.compose.ui.platform.LocalDensity provides density) {
        MaterialTheme(colorScheme = scheme, typography = NoriTypography, content = content)
    }
}

/**
 * Every size in this app was measured against Apple's own screens as a share of the screen's width,
 * on a phone 411 dp wide. A phone whose display size makes it narrower in dp - one measured at 358 dp
 * - draws every one of those sizes a seventh larger, and the whole thing looks zoomed in. Automatic
 * lays the app out as if the screen were at least [REFERENCE_WIDTH_DP] wide, so the proportions hold;
 * it only ever shrinks, never enlarges past what the system asked for.
 */
fun uiScale(setting: Float, screenWidthDp: Int): Float = when {
    setting > 0f -> setting
    screenWidthDp <= 0 -> 1f
    else -> (screenWidthDp / REFERENCE_WIDTH_DP).coerceIn(0.75f, 1f)
}

private const val REFERENCE_WIDTH_DP = 411f

/** A light or dark scheme from one colour: tones of the same hue, worked out in Rust (`nori_look::theme::seeded`). */
private fun seeded(seed: Color, dark: Boolean): ColorScheme {
    val t = dev.nori.music.look.CoverLook.tones(seed.toArgb(), dark).map { Color(it) }
    return if (dark) darkColorScheme(
        primary = t[0], onPrimary = t[1], primaryContainer = t[2], onPrimaryContainer = t[3],
        secondary = t[4], secondaryContainer = t[5], onSecondaryContainer = t[6],
        surface = t[7], background = t[8], surfaceVariant = t[9], onSurfaceVariant = t[10],
    ) else lightColorScheme(
        primary = t[0], onPrimary = t[1], primaryContainer = t[2], onPrimaryContainer = t[3],
        secondary = t[4], secondaryContainer = t[5], onSecondaryContainer = t[6],
        surface = t[7], background = t[8], surfaceVariant = t[9], onSurfaceVariant = t[10],
    )
}

private fun ColorScheme.black() = copy(
    background = Color.Black, surface = Color.Black, surfaceDim = Color.Black,
    surfaceContainerLowest = Color.Black, surfaceContainerLow = Color(0xFF0A0A0A), surfaceContainer = Color(0xFF111111),
    surfaceContainerHigh = Color(0xFF181818), surfaceContainerHighest = Color(0xFF202020),
)
