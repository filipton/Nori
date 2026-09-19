package dev.flint.music.app.ui

import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.background
import androidx.compose.ui.draw.drawBehind
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.ui.composed
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.PointerInputChange
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInRoot
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.graphics.vector.ImageVector
import kotlinx.coroutines.cancelAndJoin
import kotlinx.coroutines.launch
import kotlinx.coroutines.flow.first
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.foundation.layout.Spacer
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.FastForward
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.PlayerViewModel

/**
 * The chrome that never leaves: what is playing, and where to go. Apple Music stacks them into one
 * floating slab with a rounded top and a hairline between the two halves, and the page scrolls
 * underneath it. That is what this is - one surface, two rows, no boxes and no Material indicator pill.
 *
 * It is drawn in two layers. This one, under the player sheet, is the mini player (the sheet grows out
 * of it, so it is simply covered as the sheet rises) with room left below it for the tab bar. The tab
 * bar is [TabBar], over the sheet, so that it can slide down out of the way as the player opens rather
 * than vanish under it in one frame. The two are split because only the tab bar can move: the mini
 * player holds the drag that opens the player, and moving the element a drag started on corrupts it.
 */
@Composable
fun BottomChrome(player: PlayerViewModel, actions: ActionsViewModel, onOpenPlayer: () -> Unit, tabsHeight: androidx.compose.ui.unit.Dp) {
    val (slab, content, page) = chromeColours()
    // A soft wash under the chrome so the list fades out as it passes behind it. Apple gets this from
    // blurring what is behind the bars; one vertical gradient costs nothing and reads much the same.
    Column(
        Modifier.drawBehind {
            drawRect(
                androidx.compose.ui.graphics.Brush.verticalGradient(
                    0f to Color.Transparent, 0.45f to page.copy(alpha = 0.75f), 1f to page,
                ),
            )
        },
    ) {
        SelectionBar(actions)
        Box(Modifier.padding(horizontal = 10.dp)) { MiniPlayer(player, onOpenPlayer, slab, content) }
        Spacer(Modifier.height(tabsHeight))
        Spacer(Modifier.navigationBarsPadding())
    }
}

/**
 * The tabs, over the player sheet. As the sheet rises they slide down off the screen - gone by the
 * time it is 70 % open - and come back up as it closes, read in the draw phase from the sheet's
 * progress so nothing recomposes while it moves. Slid away, they are out of reach as well as sight.
 */
@Composable
fun TabBar(route: String?, tabs: List<Tab>, onTab: (String) -> Unit, onHeight: (androidx.compose.ui.unit.Dp) -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val (slab, content, _) = chromeColours()
    val search = tabs.firstOrNull { it.route == "search" }
    val rest = tabs.filter { it.route != "search" }
    val sheet = LocalPlayerSheet.current
    val density = androidx.compose.ui.platform.LocalDensity.current
    Column(
        Modifier.graphicsLayer {
            val t = (sheet.progress.value / 0.7f).coerceIn(0f, 1f)
            translationY = t * (size.height + 12.dp.toPx())
        },
    ) {
        Row(
            Modifier.onGloballyPositioned { onHeight(with(density) { it.size.height.toDp() }) }
                .padding(start = 10.dp, end = 10.dp, top = 8.dp, bottom = 4.dp),
            Arrangement.spacedBy(8.dp), Alignment.CenterVertically,
        ) {
            Surface(shape = PillShape, color = slab, contentColor = content, shadowElevation = 8.dp, modifier = Modifier.weight(1f)) {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 4.dp, vertical = 5.dp),
                    Arrangement.SpaceEvenly, Alignment.CenterVertically,
                ) { rest.forEach { t -> TabButton(t, selected = route == t.route, content = content) { onTab(t.route) } } }
            }
            if (search != null) Surface(
                onClick = { onTab(search.route) }, shape = CircleShape, color = slab, shadowElevation = 8.dp,
                modifier = Modifier.size(58.dp).semantics { contentDescription = search.label },
            ) {
                Box(Modifier.fillMaxSize(), Alignment.Center) {
                    Icon(search.icon, null, Modifier.size(25.dp), tint = if (route == search.route) scheme.primary else content)
                }
            }
        }
        Spacer(Modifier.navigationBarsPadding())
    }
}

/**
 * The chrome's colours: the slab, what is written on it, and the page it fades into. Neutral, like
 * Apple's. Only a page that is *about* one cover - an album, an artist, the player - wears that cover's
 * colour; a bar tinted by whatever happens to be playing turns the whole app red on screens that have
 * nothing to do with the record.
 */
@Composable
private fun chromeColours(): Triple<Color, Color, Color> {
    val scheme = MaterialTheme.colorScheme
    val tint = currentPageTint()
    val slab = tint?.let { blend(it.background, it.edge, 0.12f) } ?: scheme.onSurface.copy(alpha = 0.09f).over(scheme.background)
    return Triple(slab, tint?.onBackground ?: scheme.onSurface, tint?.background ?: scheme.background)
}

data class Tab(val route: String, val label: String, val icon: ImageVector)

@Composable
private fun TabButton(tab: Tab, selected: Boolean, content: Color, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    // Apple marks the current tab twice over: the accent colour on the glyph, and a plain lighter patch
    // behind it - light grey on their white bar, so the equivalent here is a little of the bar's own
    // text colour. Tinting that patch with the accent is what made it read as a Material pill.
    val colour = if (selected) scheme.primary else content
    val press = remember { androidx.compose.foundation.interaction.MutableInteractionSource() }
    // No patch behind anything. Where you are is the accent colour, a bold label and a slightly larger
    // glyph - every shape drawn behind the current tab, circle or rectangle, ended up reading as
    // Material's active indicator no matter how faint it was made.
    Column(
        Modifier.clip(RoundedCornerShape(14.dp))
            .clickable(interactionSource = press, indication = null, onClick = onClick)
            .padding(horizontal = 18.dp, vertical = 7.dp)
            .semantics { contentDescription = tab.label },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(tab.icon, null, Modifier.size(if (selected) 26.dp else 23.dp), tint = colour)
        Text(
            tab.label, Modifier.padding(top = 2.dp),
            style = MaterialTheme.typography.labelSmall.copy(fontSize = 10.5f.sp, letterSpacing = 0.sp),
            color = if (selected) colour else colour.copy(alpha = 0.7f),
            fontWeight = if (selected) FontWeight.Bold else FontWeight.Medium,
        )
    }
}

/**
 * The bar above the tabs: artwork, what is playing, and the two controls a thumb wants. Sideways
 * flings skip, an upward fling opens the player - the same gestures the full screen answers to.
 * No progress bar on purpose: it would tick for as long as the app is open.
 */
@Composable
fun MiniPlayer(vm: PlayerViewModel, onOpen: () -> Unit, slab: Color, content: Color) {
    val state by vm.state.collectAsStateWithLifecycle()
    val title = state.current?.title ?: state.radio ?: return
    val settings: dev.flint.music.app.vm.SettingsViewModel = androidx.lifecycle.viewmodel.compose.viewModel()
    val prefs by settings.prefs.collectAsStateWithLifecycle()
    val dark = when (prefs.theme) {
        dev.flint.music.settings.ThemeMode.SYSTEM -> androidx.compose.foundation.isSystemInDarkTheme()
        dev.flint.music.settings.ThemeMode.DARK -> true
        dev.flint.music.settings.ThemeMode.LIGHT -> false
    }
    // The bar wears the colour of what is playing, not of the page it happens to be sitting on: that is
    // what ties it to the music while you browse somewhere else entirely. The palette is the same cached
    // one the player and the album page use, so this costs a map lookup.
    val palette = if (prefs.coverColors) {
        rememberCoverPalette(vm.cover(state.current?.coverArt, CoverSize.ROW)?.takeUnless(::isProviderCover), dark, prefs.amoled)
    } else null
    val scheme = MaterialTheme.colorScheme
    val sheet = LocalPlayerSheet.current
    NowPlayingPalette(palette)
    Surface(
        shape = CardShape, color = slab, contentColor = content,
        shadowElevation = 6.dp,
        modifier = Modifier.fillMaxWidth()
            .semantics { contentDescription = "Now playing bar" }
            .onGloballyPositioned { sheet.miniTop = it.positionInRoot().y }
            // Up opens the player, following the finger the whole way; see PlayerSheet.
            .dragsSheet(sheet),
    ) {
        // The tap has to be a child of the drag detectors, not a sibling behind them: a pointerInput
        // waiting for drag slop swallows a tap offered to a clickable further up the same chain.
        Surface(onClick = onOpen, color = Color.Transparent, contentColor = content) {
        Row(Modifier.fillMaxWidth().padding(end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            // What is playing slides aside for the next (or last) song, which comes in from the other
            // edge already showing; the buttons stay where they are. Radio has no neighbours.
            val song = state.current
            val track: @Composable (dev.flint.music.ffi.Song?, Boolean) -> Unit = { s, real ->
                Row(Modifier.fillMaxWidth().padding(start = 8.dp, top = 7.dp, bottom = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                    Cover(
                        vm.cover(s?.coverArt, CoverSize.ROW), 42.dp,
                        if (real) Modifier.onGloballyPositioned {
                            // Only the whole square: slid part-way out of the bar it is clipped, and a
                            // thumbnail with no width is not something to grow the cover from.
                            val b = it.boundsInRoot()
                            if (b.height > 0f && kotlin.math.abs(b.width - b.height) < 1f) sheet.miniCover = b
                        } else Modifier, radius = 7.dp,
                    )
                    Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                        // A long title here reads itself out twice when the song comes on and then
                        // settles back to the ellipsis. The full player scrolls its title for as long
                        // as it is open; this bar is open for as long as the app is, and a line
                        // walking there all afternoon is exactly the kind of thing the missing
                        // progress bar above is missing for.
                        Text(
                            s?.title ?: title, Modifier.readable(iterations = 2),
                            maxLines = 1, softWrap = false, overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.bodyLarge,
                        )
                        Text(
                            (if (real) state.error else null) ?: s?.artist ?: "Radio", maxLines = 1, overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.bodySmall,
                            color = if (real && state.error != null) scheme.error else content.copy(alpha = 0.65f),
                        )
                    }
                }
            }
            SwipeCarousel(
                current = song,
                previous = state.queue.getOrNull(state.previousIndex)?.takeIf { song != null },
                next = state.queue.getOrNull(state.nextIndex)?.takeIf { song != null },
                same = { a, b -> a?.id == b?.id },
                onPrevious = vm::previousItem, onNext = vm::next,
                modifier = Modifier.weight(1f),
                item = track,
            )
            IconButton(vm::toggle) { PlayPauseGlyph(state.playing, state.buffering, 26.dp, 20.dp) }
            IconButton(vm::next) { Icon(Icons.Filled.FastForward, "Next", Modifier.size(25.dp)) }
        }
        }
    }
}

/**
 * The colours of the track that is playing, published once by the mini player so the tab bar under it
 * can wear the same tint. A plain holder rather than a CompositionLocal provider, because the two
 * composables are siblings: the bar is drawn after the player has worked its palette out.
 */
private val nowPlaying = androidx.compose.runtime.mutableStateOf<PagePalette?>(null)

@Composable
fun nowPlayingPalette(): PagePalette? = nowPlaying.value

private val pagePalette = androidx.compose.runtime.mutableStateOf<PagePalette?>(null)

/**
 * How much room the floating chrome takes at the bottom. Screens add it to the bottom of their own
 * scrolling content, so a list can run underneath the mini player - the page's colour reaches the
 * bottom edge of the screen, and the last row is still reachable.
 */
val LocalChromeInset = androidx.compose.runtime.compositionLocalOf { 0.dp }

/** A tinted page (an album, an artist, the player) lends its colours to the chrome while it is open. */
@Composable
fun PageTint(palette: PagePalette?) {
    androidx.compose.runtime.DisposableEffect(palette) {
        pagePalette.value = palette
        onDispose { if (pagePalette.value === palette) pagePalette.value = null }
    }
}

@Composable
private fun currentPageTint(): PagePalette? = pagePalette.value

@Composable
private fun NowPlayingPalette(palette: PagePalette?) {
    androidx.compose.runtime.LaunchedEffect(palette) { nowPlaying.value = palette }
}

/**
 * A row of records, one showing: a sideways drag slides the showing one aside and brings its neighbour
 * in from the other edge, already drawn. Let go past a third of the way, or flicked, the neighbour
 * lands and [onNext] / [onPrevious] runs; it stays drawn in place of [current] until [current] is that
 * song too (the player answers a few frames later), so the old one never comes back for a frame.
 * [item]'s second argument is true for the one really showing. Nothing runs until a finger is down.
 */
@Composable
internal fun <T> SwipeCarousel(
    current: T, previous: T?, next: T?, same: (T?, T?) -> Boolean,
    onPrevious: () -> Unit, onNext: () -> Unit,
    modifier: Modifier = Modifier,
    item: @Composable (T?, Boolean) -> Unit,
) {
    val scope = androidx.compose.runtime.rememberCoroutineScope()
    val haptics = androidx.compose.ui.platform.LocalHapticFeedback.current
    // Straight from the finger, not through a coroutine per pointer event: those queued up on a flick
    // and landed after the settle had started, which pulled the row back mid-change. Same story as the
    // sleeve's; see SleeveCarousel.
    var offset by remember { androidx.compose.runtime.mutableFloatStateOf(0f) }
    var moving by remember { mutableStateOf<kotlinx.coroutines.Job?>(null) }
    val hasBefore by androidx.compose.runtime.rememberUpdatedState(previous != null)
    val hasAfter by androidx.compose.runtime.rememberUpdatedState(next != null)
    val nextNow by androidx.compose.runtime.rememberUpdatedState(next)
    val previousNow by androidx.compose.runtime.rememberUpdatedState(previous)
    // The neighbour that has been slid in, shown until the player has caught up with it.
    var landed by remember { mutableStateOf<Any?>(NONE) }
    val currentNow by androidx.compose.runtime.rememberUpdatedState(current)
    androidx.compose.runtime.LaunchedEffect(landed) {
        if (landed === NONE) return@LaunchedEffect
        @Suppress("UNCHECKED_CAST")
        kotlinx.coroutines.withTimeoutOrNull(2_000) {
            androidx.compose.runtime.snapshotFlow { same(currentNow, landed as T?) }.first { it }
        }
        landed = NONE
    }
    Box(
        modifier.clipToBounds().pointerInput(Unit) {
            val tracker = androidx.compose.ui.input.pointer.util.VelocityTracker()
            var x = 0f
            val release: (Float) -> Unit = { v ->
                val o = offset
                val w = size.width.toFloat()
                val go = when {
                    o < 0f && hasAfter && (v < -900f || o < -w * 0.3f) -> -1
                    o > 0f && hasBefore && (v > 900f || o > w * 0.3f) -> 1
                    else -> 0
                }
                val running = moving
                moving = scope.launch {
                    running?.cancelAndJoin()
                    val settle = androidx.compose.animation.core.spring<Float>(dampingRatio = 1f, stiffness = 560f)
                    if (go == 0) {
                        androidx.compose.animation.core.animate(offset, 0f, v, settle) { value, _ -> offset = value }
                        return@launch
                    }
                    haptics.performHapticFeedback(androidx.compose.ui.hapticfeedback.HapticFeedbackType.LongPress)
                    val arriving = if (go < 0) nextNow else previousNow
                    var changed = false
                    try {
                        if (AppMotion.reduce) offset = go * w
                        else androidx.compose.animation.core.animate(offset, go * w, v, settle) { value, _ -> offset = value }
                        landed = arriving
                        offset = 0f
                        changed = true
                        if (go < 0) onNext() else onPrevious()
                    } finally {
                        if (!changed) { offset = 0f; if (go < 0) onNext() else onPrevious() }
                    }
                }
            }
            detectHorizontalDragGestures(
                onDragStart = { tracker.resetTracking(); x = 0f; moving?.cancel() },
                onDragEnd = { release(tracker.calculateVelocity().x) },
                onDragCancel = { release(0f) },
            ) { change, d ->
                x += d
                tracker.addPosition(change.uptimeMillis, androidx.compose.ui.geometry.Offset(x, 0f))
                val w = size.width.toFloat()
                val moved = offset + d
                val allowed = (moved > 0f && hasBefore) || (moved < 0f && hasAfter)
                offset = if (allowed) moved.coerceIn(-w, w) else (offset + d * 0.2f).coerceIn(-w * 0.06f, w * 0.06f)
            }
        },
    ) {
        @Suppress("UNCHECKED_CAST")
        val showing = if (landed === NONE) current else landed as T?
        Box(Modifier.graphicsLayer { translationX = offset }) { item(showing, landed === NONE) }
        Box(Modifier.graphicsLayer { val o = offset; alpha = if (o < 0f) 1f else 0f; translationX = o + size.width }) { item(next, false) }
        Box(Modifier.graphicsLayer { val o = offset; alpha = if (o > 0f) 1f else 0f; translationX = o - size.width }) { item(previous, false) }
    }
}

private val NONE = Any()
