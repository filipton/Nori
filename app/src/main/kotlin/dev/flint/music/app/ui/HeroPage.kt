package dev.flint.music.app.ui

import android.app.Activity
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.core.view.WindowCompat
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.SettingsViewModel
import dev.flint.music.settings.ThemeMode

/**
 * An album, artist or playlist page in the way Apple Music does it and Material permits: the cover runs
 * edge to edge under the status bar, dissolves into a colour taken from it, and the whole page carries that
 * colour. Everything drawn inside sees a colour scheme built on it, so rows and buttons stay readable.
 * The tint is static: no blur and no animation, so an open page costs nothing between frames.
 */
@Composable
fun HeroPage(
    coverUrl: String?,
    title: String,
    subtitle: String,
    actions: @Composable RowScope.() -> Unit = {},
    content: LazyListScope.() -> Unit,
) {
    val settings: SettingsViewModel = viewModel()
    val prefs by settings.prefs.collectAsStateWithLifecycle()
    val dark = when (prefs.theme) { ThemeMode.SYSTEM -> isSystemInDarkTheme(); ThemeMode.DARK -> true; ThemeMode.LIGHT -> false }
    val colors = if (prefs.coverColors) rememberCoverColors(coverUrl?.takeUnless(::isProviderCover), dark, prefs.amoled) else null
    val base = MaterialTheme.colorScheme
    val scheme = remember(colors, base) {
        colors?.let {
            base.copy(
                background = it.background, surface = it.background, onBackground = it.onBackground, onSurface = it.onBackground,
                onSurfaceVariant = it.onBackgroundVariant, primary = it.accent,
                onPrimary = if (it.accent.luminance() < 0.5f) Color.White else Color.Black,
                secondaryContainer = it.onBackground.copy(alpha = 0.14f).compositeOver(it.background), onSecondaryContainer = it.onBackground,
            )
        } ?: base
    }
    val background = scheme.background
    // Light or dark status bar icons follow the page's own colour, not the app theme.
    val view = LocalView.current
    DisposableEffect(background) {
        val window = (view.context as? Activity)?.window
        val controller = window?.let { WindowCompat.getInsetsController(it, view) }
        val before = controller?.isAppearanceLightStatusBars
        controller?.isAppearanceLightStatusBars = background.luminance() > 0.5f
        onDispose { if (before != null) controller.isAppearanceLightStatusBars = before }
    }
    var fullscreen by remember { mutableStateOf(false) }
    if (fullscreen && coverUrl != null) androidx.compose.ui.window.Dialog({ fullscreen = false }) { Cover(coverUrl, 0.dp, Modifier.fillMaxWidth().clickable { fullscreen = false }) }

    MaterialTheme(colorScheme = scheme) {
        CompositionLocalProvider(LocalContentColor provides scheme.onBackground) {
            Box(Modifier.fillMaxSize().background(background)) {
                val list = rememberLazyListState()
                LazyColumn(state = list) {
                    item(key = "hero", contentType = "hero") {
                        if (coverUrl != null) Box(
                            Modifier.fillMaxWidth().aspectRatio(1f)
                                // Parallax and fade, read in the draw phase: scrolling never recomposes the hero.
                                .graphicsLayer {
                                    val scrolled = if (list.firstVisibleItemIndex == 0) list.firstVisibleItemScrollOffset.toFloat() else size.height
                                    translationY = scrolled * 0.45f
                                    alpha = 1f - (scrolled / size.height).coerceIn(0f, 1f) * 0.6f
                                }
                                .clickable { fullscreen = true },
                        ) {
                            Cover(coverUrl, 0.dp, Modifier.fillMaxSize())
                            // The bottom third of the cover melts into the page colour.
                            Box(
                                Modifier.fillMaxSize().drawWithContent {
                                    drawContent()
                                    drawRect(Brush.verticalGradient(0.55f to Color.Transparent, 1f to background))
                                    drawRect(Brush.verticalGradient(0f to Color.Black.copy(alpha = 0.35f), 0.18f to Color.Transparent))
                                },
                            )
                        } else Box(Modifier.statusBarsPadding().padding(top = 56.dp))
                    }
                    item(key = "title", contentType = "title") {
                        Row(Modifier.fillMaxWidth().padding(start = 20.dp, end = 8.dp, top = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                            Column(Modifier.weight(1f)) {
                                Text(title, style = MaterialTheme.typography.headlineSmall.copy(fontWeight = FontWeight.Bold), maxLines = 2, overflow = TextOverflow.Ellipsis)
                                if (subtitle.isNotEmpty()) Text(subtitle, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
                            }
                            actions()
                        }
                    }
                    content()
                }
                // Back sits over the picture, on a soft disc so it reads on any cover.
                IconButton(
                    LocalNav.current::back,
                    Modifier.statusBarsPadding().padding(8.dp).background(Color.Black.copy(alpha = 0.28f), CircleShape),
                ) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back", tint = Color.White) }
            }
        }
    }
}

private fun Color.compositeOver(background: Color): Color {
    val a = alpha
    return Color(red * a + background.red * (1 - a), green * a + background.green * (1 - a), blue * a + background.blue * (1 - a), 1f)
}
