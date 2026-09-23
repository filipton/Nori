package dev.nori.music.app.ui

import dev.nori.music.look.CoverLook
import androidx.compose.ui.draw.drawWithCache
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
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
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
import dev.nori.music.app.vm.PlayerViewModel
import dev.nori.music.app.vm.SettingsViewModel
import dev.nori.music.playback.PlayerState
import dev.nori.music.settings.ThemeMode

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
    /**
     * Keep the Shuffle + Play row even while [onPlay] / [onShuffle] are still null (hinted album /
     * artist / playlist pages waiting on the server). Without this the row jumps from heart-only to
     * transport when the detail lands - a one-frame pop.
     */
    awaitingPlay: Boolean = false,
    /**
     * Says whether the queue on the phone was started from this page. A page only has to say what
     * "this page's own" means - its songs, or the albums of an artist - and reads [PlayerState] for
     * the rest, so the two big buttons can answer for that queue rather than for the player in
     * general: Play becomes Pause while it sounds (and picks it up where it stopped rather than
     * starting the record again), Shuffle lights while it is shuffling, and a second press on Shuffle
     * turns shuffle off instead of drawing the same songs into a new queue.
     */
    playingHere: (PlayerState) -> Boolean = { false },
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
    // Shuffle stays labelled Shuffle (never Pause); it lights while this page's queue is shuffling.
    val player: PlayerViewModel = viewModel()
    val playerState by player.state.collectAsStateWithLifecycle()
    // The queue this page started is what is on - whichever song of it happens to be sounding.
    val here = playingHere(playerState)
    val shuffling = playerState.shuffle && here
    // Pause while that queue sounds; otherwise Play starts one, or picks this one back up.
    val pausing = here && (playerState.playing || playerState.buffering)

    TintedTheme(palette) {
        val scheme = MaterialTheme.colorScheme
        SystemBarIcons(LocalLook.current)
        PageTint(palette)
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
                                },
                        ) {
                            Cover(coverUrl, 0.dp, Modifier.fillMaxSize())
                            val look = LocalLook.current
                            Box(
                                Modifier.fillMaxSize().drawWithCache {
                                    // The whole dissolve happens inside the artwork, and finishes on the
                                    // page colour rather than on the cover's edge colour. It used to stop
                                    // on the edge colour and leave a second gradient below to carry on -
                                    // but the parallax slides the picture down over that gradient as the
                                    // page scrolls, squeezing it into a few dozen pixels, and a colour
                                    // ramp that steep across the full width is a line. The picture has
                                    // its own height to do this in, and ending on the page colour means
                                    // there is nothing left to hand over to. The stops are the look's
                                    // (nori_look::dress), made into brushes once per size.
                                    val dissolve = Brush.verticalGradient(
                                        0.60f to Color.Transparent,
                                        0.76f to look.color(CoverLook.HERO_EDGE),
                                        0.88f to look.color(CoverLook.HERO_MID),
                                        1f to look.color(CoverLook.BACKGROUND),
                                    )
                                    // Just enough shade under the status bar for white icons on a pale cover.
                                    val shade = Brush.verticalGradient(0f to Color.Black.copy(alpha = 0.30f), 0.16f to Color.Transparent)
                                    onDrawWithContent {
                                        drawContent()
                                        drawRect(dissolve)
                                        drawRect(shade)
                                    }
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
                        // side by side give the page two things to look at instead of one. The row is
                        // reserved while [awaitingPlay] so the layout does not jump when the taps land.
                        if (awaitingPlay || onPlay != null || onShuffle != null) Row(
                            Modifier.fillMaxWidth().padding(start = Space.gutter, end = Space.gutter, top = 16.dp),
                            Arrangement.spacedBy(12.dp), Alignment.CenterVertically,
                        ) {
                            CircleButton(
                                Icons.Filled.Shuffle, "Shuffle",
                                enabled = shuffling || onShuffle != null, lit = shuffling,
                                onClick = {
                                    // On the page that is what sounds, a second press turns shuffle off
                                    // for this queue rather than shuffling the same songs once more.
                                    if (shuffling) player.toggleShuffle() else onShuffle?.invoke()
                                },
                            )
                            PillButton(
                                if (pausing) "Pause" else "Play", if (pausing) Icons.Filled.Pause else Icons.Filled.PlayArrow,
                                // On its own queue the pill pauses and resumes; starting the record
                                // over is what it does only for a queue that is not this page's.
                                { if (here) player.toggle() else onPlay?.invoke() }, Modifier.weight(1f),
                                prominent = true, enabled = here || onPlay != null,
                            )
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
