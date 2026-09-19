package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.SettingsViewModel
import dev.flint.music.settings.ThemeMode

/**
 * An album, artist or playlist page, built the way Apple Music builds one: the artwork runs edge to
 * edge under the status bar and dissolves into the page, the page wears the colour the artwork ends
 * on, and the title, the artist and the two big buttons sit centred underneath it.
 *
 * The dissolve is the point. The wash starts from the average colour of the cover's own bottom rows,
 * so there is no line where the picture stops - it simply runs out of picture. Everything is static:
 * one gradient, no blur, no animation, so an open page costs nothing between frames. Scrolling moves
 * the artwork in the draw phase only, so it never recomposes.
 */
@Composable
fun HeroPage(
    coverUrl: String?,
    title: String,
    /** The artist line under the title, in the cover's accent colour; tapping it opens [onSubtitle]. */
    subtitle: String? = null,
    /** A quiet line under that, in sentence case: year, song count, length, quality. */
    caption: String = "",
    onSubtitle: (() -> Unit)? = null,
    onPlay: (() -> Unit)? = null,
    onShuffle: (() -> Unit)? = null,
    /** Icon buttons on the line with the pills: favourite, queue, download. */
    actions: @Composable RowScope.() -> Unit = {},
    /**
     * Artwork of the page's own making, for a page with no cover to bleed (a mix): drawn as a tile in the
     * middle, below the status bar, the way Apple shows a made-for-you mix. Used only when [coverUrl] is null.
     */
    art: (@Composable () -> Unit)? = null,
    content: LazyListScope.() -> Unit,
) {
    val settings: SettingsViewModel = viewModel()
    val prefs by settings.prefs.collectAsStateWithLifecycle()
    val dark = when (prefs.theme) { ThemeMode.SYSTEM -> isSystemInDarkTheme(); ThemeMode.DARK -> true; ThemeMode.LIGHT -> false }
    val palette = if (prefs.coverColors) rememberCoverPalette(coverUrl?.takeUnless(::isProviderCover), dark, prefs.amoled) else null

    TintedTheme(palette) {
        val scheme = MaterialTheme.colorScheme
        SystemBarIcons(scheme.background)
        PageTint(palette)
        var fullscreen by remember { mutableStateOf(false) }
        if (fullscreen && coverUrl != null) androidx.compose.ui.window.Dialog({ fullscreen = false }) {
            Cover(coverUrl, 0.dp, Modifier.fillMaxWidth().clickable { fullscreen = false }, radius = Radius.card)
        }
        Box(Modifier.fillMaxSize().drawBehind { drawRect(scheme.background) }) {
            val list = rememberLazyListState()
            LazyColumn(state = list) {
                item(key = "hero", contentType = "hero") {
                    // One block: artwork, then the wash it melts into, carrying the title and the buttons.
                    //
                    Column(Modifier.fillMaxWidth()) {
                        if (coverUrl != null) Box(
                            Modifier.fillMaxWidth().aspectRatio(1f)
                                // Parallax and fade, read in the draw phase: scrolling never recomposes the hero.
                                .graphicsLayer {
                                    val scrolled = if (list.firstVisibleItemIndex == 0) list.firstVisibleItemScrollOffset.toFloat() else size.height
                                    translationY = scrolled * 0.4f
                                    alpha = 1f - (scrolled / size.height).coerceIn(0f, 1f) * 0.5f
                                }
                                .clickable { fullscreen = true },
                        ) {
                            Cover(coverUrl, 0.dp, Modifier.fillMaxSize())
                            val edge = palette?.edge ?: scheme.background
                            val page = palette?.background ?: scheme.background
                            Box(
                                Modifier.fillMaxSize().drawWithContent {
                                    drawContent()
                                    // The whole dissolve happens inside the artwork, and finishes on the
                                    // page colour rather than on the cover's edge colour. It used to stop
                                    // on the edge colour and leave a second gradient below to carry on -
                                    // but the parallax slides the picture down over that gradient as the
                                    // page scrolls, squeezing it into a few dozen pixels, and a colour
                                    // ramp that steep across the full width is a line. The picture has
                                    // its own height to do this in, and ending on the page colour means
                                    // there is nothing left to hand over to.
                                    drawRect(
                                        Brush.verticalGradient(
                                            0.60f to Color.Transparent,
                                            0.76f to edge.copy(alpha = 0.40f),
                                            0.88f to blend(edge, page, 0.55f).copy(alpha = 0.86f),
                                            1f to page,
                                        ),
                                    )
                                    // Just enough shade under the status bar for white icons on a pale cover.
                                    drawRect(Brush.verticalGradient(0f to Color.Black.copy(alpha = 0.30f), 0.16f to Color.Transparent))
                                },
                            )
                        } else if (art != null) Box(
                            Modifier.fillMaxWidth().statusBarsPadding().padding(top = 64.dp, bottom = 18.dp),
                            Alignment.Center,
                        ) { art() } else Spacer(Modifier.statusBarsPadding().height(72.dp))

                        // Nothing is painted here: the artwork above has already dissolved onto the page
                        // colour, and the page colour is what the root is painted with.
                        Column(Modifier.fillMaxWidth()) {
                        Column(
                            Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 4.dp),
                            horizontalAlignment = Alignment.CenterHorizontally,
                        ) {
                            Text(
                                title, style = MaterialTheme.typography.headlineSmall, textAlign = TextAlign.Center,
                                maxLines = 2, overflow = TextOverflow.Ellipsis,
                            )
                            if (!subtitle.isNullOrEmpty()) Text(
                                subtitle,
                                Modifier.padding(top = 2.dp).then(if (onSubtitle != null) Modifier.clickable(onClick = onSubtitle) else Modifier),
                                style = MaterialTheme.typography.titleMedium, color = scheme.primary,
                                textAlign = TextAlign.Center, maxLines = 1, overflow = TextOverflow.Ellipsis,
                            )
                            // Sentence case, as Apple writes it ("25 songs, 1 hour 42 minutes"). Small
                            // capitals here made the line shout louder than the artist above it.
                            Caption(caption, Modifier.padding(top = 6.dp), align = TextAlign.Center, caps = false)
                        }

                        // Apple's arrangement: shuffle in a circle on the left, one wide Play pill in the
                        // middle, and the page's other action in a circle on the right. Two equal pills
                        // side by side give the page two things to look at instead of one.
                        if (onPlay != null || onShuffle != null) Row(
                            Modifier.fillMaxWidth().padding(start = Space.gutter, end = Space.gutter, top = 16.dp),
                            Arrangement.spacedBy(12.dp), Alignment.CenterVertically,
                        ) {
                            if (onShuffle != null) CircleButton(Icons.Filled.Shuffle, "Shuffle", onClick = onShuffle)
                            if (onPlay != null) PillButton("Play", Icons.Filled.PlayArrow, onPlay, Modifier.weight(1f), prominent = true)
                            actions()
                        } else Row(
                            Modifier.fillMaxWidth().padding(start = Space.tight, end = Space.tight, top = 2.dp),
                            Arrangement.Center, Alignment.CenterVertically,
                        ) { actions() }
                        Spacer(Modifier.height(10.dp))
                        }
                    }
                }
                content()
                item(key = "tail") { Spacer(Modifier.height(Space.section + LocalChromeInset.current)) }
            }
            // Back floats over the artwork on a soft disc, so it reads on any cover.
            Box(Modifier.statusBarsPadding().padding(start = 12.dp, top = 10.dp)) {
                ScrimIconButton(Icons.AutoMirrored.Filled.ArrowBack, "Back", LocalNav.current::back)
            }
        }
    }
}
