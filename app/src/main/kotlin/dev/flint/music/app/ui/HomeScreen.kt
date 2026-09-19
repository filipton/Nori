package dev.flint.music.app.ui

import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.material.icons.filled.DragHandle
import androidx.compose.ui.zIndex
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.material3.MaterialTheme
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.MoreHoriz
import androidx.compose.material3.Text
import androidx.compose.runtime.setValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.mutableStateOf
import androidx.compose.material3.IconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.DropdownMenu
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.Box
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.tween
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.HomeViewModel
import dev.flint.music.settings.HomeRow
import dev.flint.music.ffi.Album

@Composable
fun HomeScreen(actions: ActionsViewModel, vm: HomeViewModel = viewModel()) {
    val load by vm.ui.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    val settings: dev.flint.music.app.vm.SettingsViewModel = viewModel()
    var rearranging by remember { mutableStateOf(false) }
    if (rearranging) { RowOrder(settings) { rearranging = false }; return }
    LoadBox(load) { ui ->
        val arrival = rememberArrival()
        val rise = with(LocalDensity.current) { Arrival.RISE.toPx() }
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            // Shuffling the whole library and picking the server's queue back up are things you do
            // occasionally, so they live behind the title's menu rather than as two buttons across the
            // top of the page: what belongs at the top of this screen is music.
            item(key = "title") {
                var menu by remember { mutableStateOf(false) }
                LargeTitle("Listen now", Modifier.arriving(arrival, 0, rise)) {
                    Box {
                        IconButton({ menu = true }) { Icon(Icons.Filled.MoreHoriz, "More", Modifier.size(22.dp)) }
                        DropdownMenu(menu, { menu = false }) {
                            DropdownMenuItem({ Text("Shuffle everything") }, { actions.shuffleAll(); menu = false })
                            DropdownMenuItem({ Text("Resume from server") }, { actions.resumeFromServer(); menu = false })
                            DropdownMenuItem({ Text("Rearrange rows") }, { rearranging = true; menu = false })
                        }
                    }
                }
            }
            // Favourites are always here; the mixes join them when the taste model is on (MixesViewModel).
            item(key = "mixes") { Column(Modifier.arriving(arrival, 1, rise)) { SectionTitle("For you"); MixTiles() } }
            if (ui.pinned.isNotEmpty()) item(key = "pinned") {
                Column(Modifier.arriving(arrival, 2, rise)) {
                    SectionTitle("Pinned playlists")
                    LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        items(ui.pinned, key = { it.id }) { p -> CoverCard(p.name, "${p.songCount} songs", vm.cover(p.coverArt, CoverSize.CARD), 150.dp, { nav.playlist(p.id) }) }
                    }
                }
            }
            // The shelves carry on the count the sections above started, so each one arrives a moment
            // after the one over it; an empty shelf is not drawn and does not take a place in the order.
            var place = if (ui.pinned.isNotEmpty()) 3 else 2
            ui.rows.forEach { (row, albums) ->
                if (albums.isEmpty()) return@forEach
                shelf(row.title, albums, vm, arrival, place++, rise)
            }
        }
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.shelf(
    title: String,
    albums: List<Album>,
    vm: HomeViewModel,
    arrival: State<Float>,
    place: Int,
    rise: Float,
) {
    item(key = title, contentType = "shelf") {
        val nav = LocalNav.current
        Column(Modifier.arriving(arrival, place, rise)) {
            SectionTitle(title)
            LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                items(albums, key = { it.id }, contentType = { "album" }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 150.dp, { nav.album(a.id) }) }
            }
        }
    }
}

/**
 * The home page's sections do not appear all at once: each one fades up and rises the last few pixels
 * into place a moment after the one above it, so the page reads top to bottom the way the pages it is
 * part of now arrive (see PageMotion in App.kt). It is one run, started when the page's content first
 * exists and over inside four tenths of a second, and the fourth section is the last to wait - with a
 * dozen shelves the page would otherwise still be assembling itself after the user had begun to scroll.
 */
private object Arrival {
    /** How long the whole run lasts, and how far apart two sections start within it. */
    const val RUN = 380
    private const val STEP = 40
    /** Each section's own move, and the last place that still waits its turn. */
    private const val MOVE = 220
    private const val LAST = 4

    /** The same curve the pages use: quick to appear, unhurried about coming to rest. */
    private val Settle = CubicBezierEasing(0.05f, 0.7f, 0.1f, 1f)

    val RISE = 14.dp

    /** Where one section is in its own move, given how far the single run driving all of them has got. */
    fun at(run: Float, place: Int): Float {
        val ms = run * RUN - minOf(place, LAST) * STEP
        return Settle.transform((ms / MOVE).coerceIn(0f, 1f))
    }
}

/**
 * The one animation behind the whole page. It runs once: a section scrolled away and back reads a run
 * that finished long ago, and fresh data for a page already showing does not start it again, so nothing
 * here moves while the page is sitting still.
 */
@Composable
private fun rememberArrival(): State<Float> {
    val plain = reduceMotion()
    val progress = remember { Animatable(0f) }
    // Linear, because the curve belongs to each section's own move and not to the clock they share.
    LaunchedEffect(plain) {
        if (plain) progress.snapTo(1f)
        else if (progress.value < 1f) progress.animateTo(1f, tween(Arrival.RUN, easing = LinearEasing))
    }
    return progress.asState()
}

/** Read in the draw phase: the run redraws the sections on screen and recomposes none of them. */
private fun Modifier.arriving(arrival: State<Float>, place: Int, rise: Float) = graphicsLayer {
    val t = Arrival.at(arrival.value, place)
    alpha = t
    translationY = rise * (1f - t)
}


/**
 * Drag the shelves into the order you want them in. Whoever likes "Random" at the top of their home
 * page should not have to go hunting through Settings for a column of Up buttons to get it there.
 */
@Composable
private fun RowOrder(settings: dev.flint.music.app.vm.SettingsViewModel, onDone: () -> Unit) {
    val prefs by settings.prefs.collectAsStateWithLifecycle()
    val order = prefs.homeRows
    // Tracked by which shelf is being held, never by its position: the position changes the instant the
    // list reorders, and a gesture keyed on that is cancelled mid-drag - which is why a row could only
    // be moved one place per press. The offset keeps the held row under the finger while the rest slide
    // past it, so the gap follows the finger instead of the row snapping away from it.
    var held by remember { mutableStateOf<HomeRow?>(null) }
    var dragOffset by remember { mutableFloatStateOf(0f) }
    var rowHeight by remember { mutableFloatStateOf(0f) }
    val haptics = androidx.compose.ui.platform.LocalHapticFeedback.current

    Column(Modifier.fillMaxSize()) {
        LargeTitle("Rearrange") {
            androidx.compose.material3.TextButton(onDone) { Text("Done", style = MaterialTheme.typography.titleSmall) }
        }
        Caption("Hold a handle and drag", Modifier.padding(start = Space.gutter, bottom = 8.dp))
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            itemsIndexed(order, key = { _, r -> r.name }) { _, row ->
                val dragged = held == row
                Row(
                    Modifier.fillMaxWidth()
                        .zIndex(if (dragged) 1f else 0f)
                        .graphicsLayer {
                            if (dragged) { translationY = dragOffset; shadowElevation = 14f; scaleX = 1.02f; scaleY = 1.02f }
                        }
                        .onGloballyPositioned { if (rowHeight == 0f) rowHeight = it.size.height.toFloat() }
                        .padding(horizontal = Space.gutter, vertical = 14.dp),
                    verticalAlignment = androidx.compose.ui.Alignment.CenterVertically,
                ) {
                    Text(row.title, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
                    Icon(
                        androidx.compose.material.icons.Icons.Filled.DragHandle, "Reorder",
                        tint = if (dragged) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                        // Keyed on the row, which never changes, so one press can carry it the whole way.
                        modifier = Modifier.size(44.dp).padding(10.dp).pointerInput(row) {
                            detectDragGestures(
                                onDragStart = {
                                    held = row; dragOffset = 0f
                                    haptics.performHapticFeedback(androidx.compose.ui.hapticfeedback.HapticFeedbackType.LongPress)
                                },
                                onDragEnd = { held = null; dragOffset = 0f },
                                onDragCancel = { held = null; dragOffset = 0f },
                            ) { change, drag ->
                                change.consume()
                                dragOffset += drag.y
                                val h = rowHeight.takeIf { it > 0f } ?: return@detectDragGestures
                                var at = settings.prefs.value.homeRows.indexOf(row)
                                while (dragOffset >= h && at < settings.prefs.value.homeRows.lastIndex) {
                                    settings.moveHomeRow(at, at + 1); at++; dragOffset -= h
                                    haptics.performHapticFeedback(androidx.compose.ui.hapticfeedback.HapticFeedbackType.TextHandleMove)
                                }
                                while (dragOffset <= -h && at > 0) {
                                    settings.moveHomeRow(at, at - 1); at--; dragOffset += h
                                    haptics.performHapticFeedback(androidx.compose.ui.hapticfeedback.HapticFeedbackType.TextHandleMove)
                                }
                            }
                        },
                    )
                }
                Hairline()
            }
        }
    }
}

