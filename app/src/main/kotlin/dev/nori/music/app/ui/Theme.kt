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
    // Everything dressed in the theme's own colours - the plates behind the buttons, the chrome, the
    // status bar - worked out once per scheme in Rust (nori_look::dress) and only looked up after.
    val look = remember(scheme) {
        FixedLook(
            dev.nori.music.look.CoverLook.plain(
                intArrayOf(
                    scheme.background.toArgb(), scheme.onSurface.toArgb(), scheme.onSurfaceVariant.toArgb(), scheme.primary.toArgb(),
                    scheme.onPrimary.toArgb(), scheme.surfaceVariant.toArgb(), scheme.surfaceContainer.toArgb(),
                    scheme.surfaceContainerHigh.toArgb(), scheme.secondaryContainer.toArgb(), scheme.outlineVariant.toArgb(),
                ),
            ),
        )
    }
    androidx.compose.runtime.CompositionLocalProvider(androidx.compose.ui.platform.LocalDensity provides density, LocalLook provides look) {
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
    val tones = dev.nori.music.look.CoverLook.tones(seed.toArgb(), dark)
    val t = List(tones.size) { Color(tones[it]) }
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

/** AMOLED black: the dark surfaces nori_look puts in their place (`dress::AMOLED`), so those pixels are off. */
private fun ColorScheme.black(): ColorScheme {
    val a = dev.nori.music.look.CoverLook.amoled()
    return copy(
        background = Color(a[0]), surface = Color(a[1]), surfaceDim = Color(a[2]),
        surfaceContainerLowest = Color(a[3]), surfaceContainerLow = Color(a[4]), surfaceContainer = Color(a[5]),
        surfaceContainerHigh = Color(a[6]), surfaceContainerHighest = Color(a[7]),
    )
}
