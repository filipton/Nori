package dev.flint.music.app.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.background
import kotlin.math.roundToInt
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.zIndex
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.material.icons.filled.DragHandle
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.QueueMusic
import androidx.compose.material.icons.filled.Bedtime
import androidx.compose.foundation.layout.Spacer
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.FastForward
import androidx.compose.material.icons.filled.FastRewind
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.Lyrics
import androidx.compose.material.icons.filled.MoreHoriz
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Repeat
import androidx.compose.material.icons.filled.RepeatOne
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material.icons.filled.SkipPrevious
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.StarBorder
import androidx.compose.material.icons.automirrored.filled.VolumeDown
import androidx.compose.material.icons.automirrored.filled.VolumeUp
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.geometry.CornerRadius
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.PlayerViewModel
import dev.flint.music.app.vm.SettingsViewModel
import dev.flint.music.playback.Repeat
import dev.flint.music.settings.ThemeMode
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch

/**
 * A drag that follows the finger and decides on release: past [threshold] of the element's size in
 * the drag direction the matching action runs, otherwise it springs back. [horizontal] picks the axis.
 * Nothing runs until a finger is down, so this costs nothing while music plays.
 */
internal fun Modifier.flingActions(
    horizontal: Boolean, threshold: Float = 0.28f,
    onStart: (() -> Unit)? = null, onEnd: (() -> Unit)? = null,
): Modifier = composed {
    val offset = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    pointerInput(horizontal, onStart != null, onEnd != null) {
        val extent = { (if (horizontal) size.width else size.height).toFloat() }
        val release: () -> Unit = {
            val v = offset.value
            val fired = kotlin.math.abs(v) > extent() * threshold
            if (fired) {
                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                if (v < 0) onEnd?.invoke() else onStart?.invoke()
            }
            scope.launch { offset.animateTo(0f, spring(stiffness = Spring.StiffnessMediumLow)) }
        }
        val drag: (Float) -> Unit = { d ->
            // Only follow the finger in a direction that has an action; the other way resists.
            val next = offset.value + d
            val allowed = (next > 0 && onStart != null) || (next < 0 && onEnd != null)
            scope.launch { offset.snapTo(if (allowed) next.coerceIn(-extent(), extent()) else next * 0.15f) }
        }
        if (horizontal) detectHorizontalDragGestures(onDragEnd = release, onDragCancel = release) { _, d -> drag(d) }
        else detectVerticalDragGestures(onDragEnd = release, onDragCancel = release) { _, d -> drag(d) }
    }.graphicsLayer {
        if (horizontal) translationX = offset.value else translationY = offset.value
        alpha = 1f - (kotlin.math.abs(offset.value) / 1200f).coerceAtMost(0.5f)
    }
}

private enum class Panel { ART, QUEUE, LYRICS }

/**
 * Now playing, the way a full-screen player should feel: the page is a wash of the artwork's own
 * colours, the artwork is a large rounded card that shrinks when the music stops, and the controls
 * sit in one column under it. Lyrics and the queue take the artwork's place rather than opening a
 * second screen, so the transport never moves.
 *
 * The only thing that ticks is the seek bar, and only while this screen is resumed and playing.
 */
@Composable
fun PlayerScreen(vm: PlayerViewModel, actions: ActionsViewModel) {
    val state by vm.state.collectAsStateWithLifecycle()
    val marks = LocalStarMarks.current
    val nav = LocalNav.current
    val menu = LocalSongMenu.current
    var panel by rememberSaveable { mutableStateOf(Panel.ART) }
    var sleepMenu by remember { mutableStateOf(false) }

    val settingsVm: SettingsViewModel = viewModel()
    val prefs by settingsVm.prefs.collectAsStateWithLifecycle()
    val dark = when (prefs.theme) { ThemeMode.SYSTEM -> isSystemInDarkTheme(); ThemeMode.DARK -> true; ThemeMode.LIGHT -> false }
    val coverUrl = vm.cover(state.current?.coverArt, CoverSize.FULL)
    val palette = if (prefs.coverColors) rememberCoverPalette(vm.cover(state.current?.coverArt, CoverSize.ROW)?.takeUnless(::isProviderCover), dark, prefs.amoled) else null

    TintedTheme(palette) {
        val scheme = MaterialTheme.colorScheme
        SystemBarIcons(scheme.background)
        Box(
            Modifier.fillMaxSize().drawBehind {
                // The page is the cover itself, enlarged and smoothed, with the seam gradient over the
                // top so the picture dissolves into it rather than stopping. See drawPageWash.
                if (palette != null) drawPageWash(palette, size.height) else drawRect(scheme.background)
            },
        ) {
            Column(Modifier.fillMaxSize().navigationBarsPadding()) {
                // The artwork bleeds to all three edges like the sleeve it is - up under the status bar
                // as well, which is the whole point: Apple's has no top edge, and giving it one drew a
                // line across the screen. The handle and the close button float over it instead.
                // Queue keeps the screen's side margin, lyrics lay out their own.
                //
                // The sleeve takes exactly its square and no more. Giving it the column's spare height
                // instead left a band of empty wash under it twice as deep as Apple's, because the
                // controls below are shorter than the space that was left over; the spare height now
                // sits above the volume slider, which is where Apple's is.
                if (panel == Panel.ART) Box(Modifier.fillMaxWidth()) {
                    Artwork(vm, coverUrl, palette)
                    Handle(Modifier.align(Alignment.TopCenter).statusBarsPadding(), Color.White.copy(alpha = 0.55f), nav::back)
                } else {
                    Handle(Modifier.statusBarsPadding(), scheme.onSurface.copy(alpha = 0.35f), nav::back)
                    Box(Modifier.weight(1f).then(if (panel == Panel.QUEUE) Modifier.padding(horizontal = 26.dp) else Modifier)) {
                        if (panel == Panel.QUEUE) Queue(vm) else LyricsView(vm, actions, state.playing)
                    }
                }

                // The column's spare height, split the way Apple's screen splits it: a band under the
                // sleeve, a deeper one over the volume slider, and a third under the bottom row - their
                // icons sit well clear of the home indicator rather than against it. Measured off
                // `w4`: 7 % of the screen under the artwork, 10 % above the volume, 11 % below the icons.
                if (panel == Panel.ART) Spacer(Modifier.weight(0.30f))
                // The lyrics view carries its own header - a thumbnail with the title, the favourite and
                // the menu beside it, the way Apple's does - so this block would be the second copy of it.
                if (panel != Panel.LYRICS) Row(
                    Modifier.fillMaxWidth().padding(start = 26.dp, end = 16.dp, top = 8.dp),
                    Arrangement.spacedBy(10.dp), Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            state.current?.title ?: state.radio ?: "Nothing playing",
                            style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            state.current?.let { listOfNotNull(it.artist.ifEmpty { null }, it.album.ifEmpty { null }).joinToString(" · ") } ?: "",
                            // Apple holds this line back from the title rather than colouring it: a
                            // saturated accent here is the one thing that made the screen read as Material.
                            style = MaterialTheme.typography.titleMedium, color = scheme.onSurface.copy(alpha = 0.6f),
                            maxLines = 1, overflow = TextOverflow.Ellipsis,
                        )
                    }
                    state.current?.let { s ->
                        val starred = marks.effectiveStar(dev.flint.music.data.StarKind.SONG, s.id, s.starred)
                        Row(Modifier, Arrangement.spacedBy(10.dp), Alignment.CenterVertically) {
                            TitleCircle(
                                if (starred) Icons.Filled.Star else Icons.Filled.StarBorder,
                                "Favourite", starred,
                            ) { actions.star(s, !starred) }
                            TitleCircle(Icons.Filled.MoreHoriz, "More", false) { menu(s) }
                        }
                    }
                }
                state.error?.let { Text(it, Modifier.padding(horizontal = 26.dp), color = scheme.error, style = MaterialTheme.typography.bodySmall) }
                if (panel != Panel.LYRICS) state.current?.let { s ->
                    val line = listOfNotNull(
                        s.suffix.uppercase().ifEmpty { null },
                        s.bitRate.takeIf { it > 0u }?.let { "$it kbps" },
                        s.samplingRate.takeIf { it > 0u }?.let { "${it.toInt() / 1000.0} kHz" },
                    ).joinToString(" · ")
                    // Apple shows nothing here. This audience wants it, so it stays - but well under
                    // the artist line, as a caption you read when you look for it.
                    Text(
                        line, Modifier.padding(horizontal = 26.dp, vertical = 2.dp),
                        style = MaterialTheme.typography.labelSmall,
                        color = scheme.onSurface.copy(alpha = 0.38f),
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                }

                SeekBar(vm, state.playing, state.durationMs)

                // Three controls, plain glyphs with no containers. Shuffle and repeat live in the queue header.
                // Apple leaves a clear gap between the times and these, rather than letting them follow on.
                Row(Modifier.fillMaxWidth().padding(top = 12.dp), Arrangement.spacedBy(46.dp, Alignment.CenterHorizontally), Alignment.CenterVertically) {
                    IconButton(vm::previous, Modifier.size(64.dp)) { Icon(Icons.Filled.FastRewind, "Previous", Modifier.size(42.dp)) }
                    IconButton(vm::toggle, Modifier.size(72.dp)) {
                        Box(Modifier.fillMaxSize(), Alignment.Center) {
                            if (state.buffering) CircularProgressIndicator(Modifier.size(28.dp), color = LocalContentColor.current, strokeWidth = 2.dp)
                            else Icon(
                                if (state.playing) Icons.Filled.Pause else Icons.Filled.PlayArrow, "Play/pause",
                                Modifier.size(52.dp),
                            )
                        }
                    }
                    IconButton(vm::next, Modifier.size(64.dp)) { Icon(Icons.Filled.FastForward, "Next", Modifier.size(42.dp)) }
                }

                if (panel == Panel.ART) Spacer(Modifier.weight(0.50f))
                VolumeRow(vm)

                Row(Modifier.fillMaxWidth().padding(top = 10.dp, bottom = 14.dp), Arrangement.SpaceEvenly, Alignment.CenterVertically) {
                    PanelButton(Icons.Filled.Lyrics, "Lyrics", panel == Panel.LYRICS) { panel = if (panel == Panel.LYRICS) Panel.ART else Panel.LYRICS }
                    Box {
                        PanelButton(Icons.Filled.Bedtime, "Sleep timer", state.sleepAt > 0 || state.sleepAtEndOfTrack) { sleepMenu = true }
                        DropdownMenu(sleepMenu, { sleepMenu = false }) {
                            for (m in listOf(15, 30, 45, 60)) DropdownMenuItem({ Text("$m minutes") }, { vm.sleep(m); sleepMenu = false })
                            DropdownMenuItem({ Text("End of track") }, { vm.sleep(0, endOfTrack = true); sleepMenu = false })
                            for (n in listOf(2, 3, 5, 10)) DropdownMenuItem({ Text("After $n songs") }, { vm.sleep(0, songs = n); sleepMenu = false })
                            DropdownMenuItem({ Text("Off") }, { vm.sleep(0); sleepMenu = false })
                        }
                    }
                    PanelButton(Icons.AutoMirrored.Filled.QueueMusic, "Queue", panel == Panel.QUEUE) { panel = if (panel == Panel.QUEUE) Panel.ART else Panel.QUEUE }
                }
                if (panel == Panel.ART) Spacer(Modifier.weight(0.20f))
            }
            if (panel == Panel.ART) ScrimIconButton(
                Icons.Filled.KeyboardArrowDown, "Close", nav::back,
                Modifier.align(Alignment.TopStart).statusBarsPadding().padding(start = 8.dp, top = 6.dp),
            ) else IconButton(nav::back, Modifier.align(Alignment.TopStart).statusBarsPadding().padding(4.dp)) {
                Icon(Icons.Filled.KeyboardArrowDown, "Close", Modifier.size(26.dp))
            }
        }
    }
}

/** Drag it down, or tap it, to put the player away. */
@Composable
private fun Handle(modifier: Modifier, colour: Color, onBack: () -> Unit) {
    Box(
        modifier.fillMaxWidth().flingActions(horizontal = false, threshold = 0.5f, onStart = onBack).padding(vertical = 10.dp),
        Alignment.Center,
    ) {
        // Clickable inside the drag detector, not outside it, or the tap never arrives.
        Box(Modifier.clickable(onClick = onBack).padding(8.dp)) {
            Box(Modifier.width(38.dp).height(5.dp).background(colour, CircleShape))
        }
    }
}

/**
 * The artwork full-bleed: edge to edge, square corners, no shadow - the sleeve it is. Its bottom
 * third melts into the page wash (transparent to the wash colour at that height), so there is no
 * line where the picture ends - the same dissolve the album page uses.
 */
@Composable
private fun Artwork(vm: PlayerViewModel, coverUrl: String?, palette: PagePalette?) {
    Box(Modifier.fillMaxWidth(), Alignment.TopCenter) {
        Box(
            Modifier.fillMaxWidth().aspectRatio(1f)
                .flingActions(horizontal = true, onStart = vm::previous, onEnd = vm::next),
        ) {
            Cover(coverUrl, 0.dp, Modifier.fillMaxSize(), radius = 0.dp)
            // Just enough shade under the status bar for its icons to read on a pale cover; the same
            // amount the album page uses, and invisible against anything darker.
            Box(
                Modifier.fillMaxWidth().fillMaxHeight(0.16f)
                    .background(Brush.verticalGradient(0f to Color.Black.copy(alpha = 0.30f), 1f to Color.Transparent)),
            )
            if (palette != null) {
                val melt = blend(palette.edge, palette.background, 0.9f)
                Box(
                    Modifier.fillMaxWidth().fillMaxHeight(0.38f).align(Alignment.BottomCenter)
                        .background(Brush.verticalGradient(0f to Color.Transparent, 1f to melt)),
                )
            }
        }
    }
}

/** A title-row circle: translucent fill, light glyph, 48 dp across with a 44 dp hit region or better. */
@Composable
internal fun TitleCircle(icon: ImageVector, label: String, selected: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(
        onClick = onClick, shape = CircleShape,
        color = scheme.onSurface.copy(alpha = 0.12f).over(scheme.background),
        contentColor = if (selected) scheme.primary else scheme.onSurface,
        modifier = Modifier.size(42.dp),
    ) {
        Box(Modifier.fillMaxSize(), Alignment.Center) { Icon(icon, label, Modifier.size(20.dp)) }
    }
}

/**
 * The phone's music-stream volume, read live so the hardware keys never leave it stale. Ticks only
 * while this screen is resumed; a drag writes straight through and updates the thumb itself.
 */
@Composable
private fun VolumeRow(vm: PlayerViewModel) {
    val scheme = MaterialTheme.colorScheme
    var level by remember { mutableFloatStateOf(vm.volumeFraction()) }
    var dragging by remember { mutableStateOf(false) }
    var resumed by remember { mutableStateOf(false) }
    LifecycleResumeEffect(Unit) { resumed = true; onPauseOrDispose { resumed = false } }
    LaunchedEffect(resumed) {
        while (resumed && isActive) {
            if (!dragging) level = vm.volumeFraction()
            delay(1000)
        }
    }
    Row(
        Modifier.fillMaxWidth().padding(horizontal = 52.dp, vertical = 2.dp),
        Arrangement.spacedBy(12.dp), Alignment.CenterVertically,
    ) {
        Icon(Icons.AutoMirrored.Filled.VolumeDown, null, Modifier.size(16.dp), tint = scheme.onSurfaceVariant)
        val track = scheme.onSurface.copy(alpha = 0.22f)
        val filled = scheme.onSurface.copy(alpha = 0.85f)
        val pick: (Float, Float) -> Unit = { x, w ->
            val f = (x / w).coerceIn(0f, 1f)
            level = f; vm.setVolumeFraction(f)
        }
        Box(
            Modifier.weight(1f).height(34.dp)
                .pointerInput(Unit) {
                    detectHorizontalDragGestures(
                        onDragStart = { dragging = true; pick(it.x, size.width.toFloat()) },
                        onDragEnd = { dragging = false },
                        onDragCancel = { dragging = false },
                    ) { change, _ -> pick(change.position.x, size.width.toFloat()) }
                }
                .pointerInput(Unit) { detectTapGestures { pick(it.x, size.width.toFloat()) } }
                .drawBehind {
                    val h = 8.dp.toPx()
                    val y = (size.height - h) / 2f
                    val r = CornerRadius(h / 2f, h / 2f)
                    drawRoundRect(track, Offset(0f, y), Size(size.width, h), r)
                    drawRoundRect(filled, Offset(0f, y), Size(size.width * level, h), r)
                    // No knob unless a finger is on it: Apple's volume slider is a filled bar and
                    // nothing else, and a permanent white circle is the most Material thing on the screen.
                    if (dragging) drawCircle(filled, h * 1.15f, Offset(size.width * level, size.height / 2f))
                },
        )
        Icon(Icons.AutoMirrored.Filled.VolumeUp, null, Modifier.size(20.dp), tint = scheme.onSurfaceVariant)
    }
}

@Composable
private fun PanelButton(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, on: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    IconButton(onClick) { Icon(icon, label, Modifier.size(23.dp), tint = if (on) scheme.primary else scheme.onSurfaceVariant) }
}

/** The only ticking thing in the app, and only while this screen is resumed and music is playing. */
@Composable
private fun position(vm: PlayerViewModel, playing: Boolean, everyMs: Long): Long {
    var pos by remember { mutableLongStateOf(vm.positionMs) }
    var resumed by remember { mutableStateOf(false) }
    LifecycleResumeEffect(Unit) { resumed = true; onPauseOrDispose { resumed = false } }
    LaunchedEffect(playing, resumed) {
        pos = vm.positionMs
        while (playing && resumed && isActive) { delay(everyMs); pos = vm.positionMs }
    }
    return pos
}

/**
 * A hairline seek bar, drawn rather than assembled: two rounded rectangles and a dot, which is both
 * what it should look like and cheaper than a Slider with its own layers and ripples.
 */
@Composable
private fun SeekBar(vm: PlayerViewModel, playing: Boolean, durationMs: Long) {
    val state by vm.state.collectAsStateWithLifecycle()
    val pos = position(vm, playing, 1000)
    val d = durationMs.coerceAtLeast(1)
    var dragging by remember { mutableStateOf(false) }
    var drag by remember { mutableFloatStateOf(0f) }
    val fraction = (if (dragging) drag else pos.toFloat() / d).coerceIn(0f, 1f)
    val track = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.22f)
    val filled = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.85f)
    Column(Modifier.padding(horizontal = 26.dp, vertical = 8.dp)) {
        Box(
            Modifier.fillMaxWidth().height(26.dp)
                .pointerInput(d) {
                    detectHorizontalDragGestures(
                        onDragStart = { dragging = true; drag = (it.x / size.width).coerceIn(0f, 1f) },
                        onDragEnd = { vm.seekTo((drag * d).toLong()); dragging = false },
                        onDragCancel = { dragging = false },
                    ) { change, _ -> drag = (change.position.x / size.width).coerceIn(0f, 1f) }
                }
                .pointerInput(d) { detectTapGestures { vm.seekTo(((it.x / size.width).coerceIn(0f, 1f) * d).toLong()) } }
                .drawBehind {
                    val h = 5.dp.toPx()
                    val y = (size.height - h) / 2f
                    val r = CornerRadius(h / 2f, h / 2f)
                    drawRoundRect(track, Offset(0f, y), Size(size.width, h), r)
                    drawRoundRect(filled, Offset(0f, y), Size(size.width * fraction, h), r)
                    if (dragging) drawCircle(filled, h * 1.6f, Offset(size.width * fraction, size.height / 2f))
                },
        )
        Row(Modifier.fillMaxWidth(), Arrangement.SpaceBetween, Alignment.CenterVertically) {
            Text(duration((if (dragging) (drag * d).toLong() else pos) / 1000), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            // The centre slot carries whatever needs saying: an error, or the sleep timer. Empty
            // the rest of the time, holding the space so the times never move.
            val centre = state.error ?: when {
                state.sleepAtEndOfTrack -> "Sleep · end of track"
                state.sleepAt > 0 -> "Sleep · ${((state.sleepAt - System.currentTimeMillis()) / 60_000).coerceAtLeast(1)} min"
                else -> ""
            }
            Text(
                centre, Modifier.weight(1f).padding(horizontal = 8.dp),
                style = MaterialTheme.typography.labelSmall,
                color = if (state.error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                maxLines = 1, overflow = TextOverflow.Ellipsis,
            )
            Text("-" + duration(((d - (if (dragging) (drag * d).toLong() else pos)).coerceAtLeast(0)) / 1000), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@Composable
private fun Queue(vm: PlayerViewModel) {
    val state by vm.state.collectAsStateWithLifecycle()
    val list = rememberLazyListState(initialFirstVisibleItemIndex = state.index.coerceAtLeast(0))
    // Nothing is reordered until the finger lifts. The held row follows it, the rows it passes step out
    // of the way, and the gap travels with it - reordering live would change the keys under the gesture
    // and cancel it, which is why a row could only ever be moved one place at a time.
    var from by remember { mutableIntStateOf(-1) }
    var dragOffset by remember { mutableFloatStateOf(0f) }
    var rowHeight by remember { mutableFloatStateOf(0f) }
    val haptics = LocalHapticFeedback.current
    val moved = if (from >= 0 && rowHeight > 0f) (dragOffset / rowHeight).roundToInt() else 0
    val target = (from + moved).coerceIn(0, (state.queue.size - 1).coerceAtLeast(0))

    // Shuffle and repeat live here, pinned above the list - not in the transport, and never scrolled
    // away (the list opens at the playing row, which used to hide them).
    Column(Modifier.fillMaxSize()) {
        Row(Modifier.fillMaxWidth(), Arrangement.SpaceBetween, Alignment.CenterVertically) {
            Caption("Playing next", Modifier.padding(top = 4.dp, bottom = 8.dp))
            Row(Modifier, Arrangement.spacedBy(4.dp), Alignment.CenterVertically) {
                val scheme = MaterialTheme.colorScheme
                IconButton(vm::toggleShuffle, Modifier.size(44.dp)) {
                    Icon(Icons.Filled.Shuffle, "Shuffle", Modifier.size(22.dp), tint = if (state.shuffle) scheme.primary else scheme.onSurfaceVariant)
                }
                IconButton(vm::cycleRepeat, Modifier.size(44.dp)) {
                    Icon(
                        if (state.repeat == Repeat.ONE) Icons.Filled.RepeatOne else Icons.Filled.Repeat, "Repeat",
                        Modifier.size(22.dp), tint = if (state.repeat != Repeat.OFF) scheme.primary else scheme.onSurfaceVariant,
                    )
                }
            }
        }
    LazyColumn(Modifier.fillMaxSize().weight(1f), state = list) {
        itemsIndexed(state.queue, key = { i, s -> "$i-${s.id}" }, contentType = { _, _ -> "song" }) { i, s ->
            val held = i == from
            val shift = when {
                from < 0 -> 0f
                held -> dragOffset
                i in (from + 1)..target -> -rowHeight
                i in target until from -> rowHeight
                else -> 0f
            }
            Row(
                Modifier.fillMaxWidth()
                    .zIndex(if (held) 1f else 0f)
                    .graphicsLayer { translationY = shift; if (held) { shadowElevation = 14f; scaleX = 1.02f; scaleY = 1.02f } }
                    .onGloballyPositioned { if (rowHeight == 0f) rowHeight = it.size.height.toFloat() }
                    .clickable { vm.skipTo(i) }
                    .padding(vertical = 6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Cover(vm.cover(s.coverArt, CoverSize.ROW), 44.dp, radius = 6.dp)
                Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                    Text(
                        s.title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge,
                        color = if (i == state.index) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface,
                    )
                    Text(s.artist, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                IconButton({ vm.remove(i) }, Modifier.size(38.dp)) {
                    Icon(Icons.Filled.Close, "Remove", Modifier.size(19.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Icon(
                    Icons.Filled.DragHandle, "Reorder",
                    tint = if (held) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(44.dp).padding(11.dp).pointerInput(Unit) {
                        detectDragGestures(
                            onDragStart = {
                                from = i; dragOffset = 0f
                                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                            },
                            onDragEnd = {
                                // Worked out here, from the state as it is now: pointerInput(Unit) keeps
                                // the block it was created with, so anything computed during composition
                                // is frozen at its first value - which quietly meant "did not move".
                                val h = rowHeight
                                val size = vm.state.value.queue.size
                                val to = if (h > 0f) (from + (dragOffset / h).roundToInt()).coerceIn(0, (size - 1).coerceAtLeast(0)) else from
                                if (from >= 0 && to != from) vm.move(from, to)
                                from = -1; dragOffset = 0f
                            },
                            onDragCancel = { from = -1; dragOffset = 0f },
                        ) { change, drag -> change.consume(); dragOffset += drag.y }
                    },
                )
            }
        }
    }
    }
}
