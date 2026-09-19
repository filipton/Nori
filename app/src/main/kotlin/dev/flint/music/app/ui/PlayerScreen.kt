package dev.flint.music.app.ui

import androidx.compose.runtime.collectAsState
import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.spring
import androidx.compose.foundation.MarqueeSpacing
import androidx.compose.foundation.background
import androidx.compose.foundation.basicMarquee
import kotlin.math.roundToInt
import androidx.compose.ui.layout.layout
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.zIndex
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.material.icons.filled.DragHandle
import androidx.compose.foundation.gestures.detectDragGestures
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
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
import androidx.compose.material.icons.filled.Bluetooth
import androidx.compose.material.icons.filled.Cast
import androidx.compose.material.icons.filled.Headphones
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
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.layout.positionInRoot
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
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.requiredSize
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.ui.geometry.Rect
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
import kotlinx.coroutines.flow.first

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
@OptIn(androidx.compose.animation.ExperimentalSharedTransitionApi::class)
@Composable
fun PlayerScreen(vm: PlayerViewModel, actions: ActionsViewModel) {
    val state by vm.state.collectAsStateWithLifecycle()
    val marks = LocalStarMarks.current
    val sheet = LocalPlayerSheet.current
    val menu = LocalSongMenu.current
    val playerMenu = LocalPlayerMenu.current
    var panel by rememberSaveable { mutableStateOf(Panel.ART) }
    // A panel's button opens it, and pressed again goes back to the artwork.
    val choose: (Panel) -> Unit = { panel = if (panel == it) Panel.ART else it }
    // Where the sleeve ends, so the page behind it can be drawn at the same scale. Written on layout,
    // read in the draw phase; it only moves when the window does.
    var sleeveBottom by remember { mutableFloatStateOf(0f) }
    var sleeveHeight by remember { mutableFloatStateOf(0f) }
    var sleepMenu by remember { mutableStateOf(false) }

    val settingsVm: SettingsViewModel = viewModel()
    val prefs by settingsVm.prefs.collectAsStateWithLifecycle()
    val dark = when (prefs.theme) { ThemeMode.SYSTEM -> isSystemInDarkTheme(); ThemeMode.DARK -> true; ThemeMode.LIGHT -> false }
    val coverUrl = vm.cover(state.current?.coverArt, CoverSize.FULL)
    // One picture for the sleeve and for the cover in flight (see SleeveArt).
    val sleeveArt = rememberSleeveArt(coverUrl)
    // AMOLED black everywhere else, but the player keeps the cover's colours unless asked not to: in
    // black, the page under the sleeve was pure black and the picture looked cut off, where Apple's
    // carries the record's colour down the whole screen.
    val black = prefs.amoled && !prefs.playerColours
    val found = if (prefs.coverColors) rememberCoverPalette(vm.cover(state.current?.coverArt, CoverSize.ROW)?.takeUnless(::isProviderCover), dark, black) else null
    // The page's colours change with the song by cross-fading, not in one frame, and they hold the last
    // song's colours while the new cover's are worked out - going to the plain page and then to the new
    // colours was two changes where there should be one. A song that really has none (no artwork) gets
    // the plain page once it has had a moment to find some.
    var palette by remember { mutableStateOf(found) }
    var fadingFrom by remember { mutableStateOf<PagePalette?>(null) }
    val washFade = remember { androidx.compose.animation.core.Animatable(1f) }
    LaunchedEffect(found) {
        if (found == palette) return@LaunchedEffect
        if (found == null) delay(1200)
        fadingFrom = palette
        palette = found
        if (fadingFrom != null && !AppMotion.reduce) { washFade.snapTo(0f); washFade.animateTo(1f, androidx.compose.animation.core.tween(520)) }
        fadingFrom = null
    }

    TintedTheme(palette) {
        val scheme = MaterialTheme.colorScheme
        if (LocalPlayerShown.current) SystemBarIcons(scheme.background)
        Box(
            // Pull down from anywhere on the artwork page and the whole player follows the finger down;
            // the lyrics and queue need a vertical drag to scroll, so there only the handle does.
            Modifier.fillMaxSize().dragsSheet(sheet, enabled = panel == Panel.ART),
        ) {
            // The page is the cover itself, enlarged and smoothed, lined up with the sleeve. No seam
            // gradient over it: the sleeve carries its own dissolve at its bottom edge, and a gradient
            // anchored to the top of the screen only laid a flat slab over the wash above the sleeve.
            val wash: androidx.compose.ui.graphics.drawscope.DrawScope.(PagePalette?) -> Unit = { p ->
                if (p != null) {
                    // Lyrics and queue have no sleeve on screen, and a player opened straight into
                    // one of them has never measured it: use where it would be, so those panels get
                    // the same picture behind them rather than one stretched row from the very top.
                    val h = if (sleeveHeight > 0f) sleeveHeight else size.width / SLEEVE
                    val b = if (sleeveBottom > 0f) sleeveBottom else h
                    drawSleeveWash(p, b, h, size.height)
                } else drawRect(scheme.background)
            }
            // While the colours change, the old page stays underneath and the new one fades in over it.
            Box(Modifier.matchParentSize().drawBehind { wash(fadingFrom ?: palette) })
            if (fadingFrom != null) Box(Modifier.matchParentSize().graphicsLayer { alpha = washFade.value }.drawBehind { wash(palette) })
            if (panel == Panel.ART) FlyingCover(sheet, vm.cover(state.current?.coverArt, CoverSize.ROW), sleeveArt, palette, sleeveHeight > 0f)
            // Artwork, lyrics and queue dissolve into each other rather than cutting. The incoming panel
            // fades in over the outgoing one, which stays fully drawn underneath until it is covered: the
            // transport is the same in all three, and fading both copies at once dimmed it half-way. (A
            // zero-length fade-out delayed to the end was not held: the old panel vanished on frame one.)
            androidx.compose.animation.SharedTransitionLayout {
            androidx.compose.animation.AnimatedContent(
                targetState = panel,
                transitionSpec = {
                    androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(240)) togetherWith
                        androidx.compose.animation.ExitTransition.KeepUntilTransitionsFinished
                },
                label = "panel",
            ) { panel ->
            // The seek bar, the transport, the volume and the icons are in every panel but not at the same
            // height. Shared, only one copy of each is drawn during the dissolve, and it moves from where it
            // was to where it goes; dissolved like the rest, both copies showed and the controls doubled.
            @Composable fun kept(key: String) = Modifier.sharedElement(rememberSharedContentState(key), this@AnimatedContent)
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
                // The sleeve draws further down than it takes up: the title and artist are laid out over
                // its last stretch, which is already going soft, the way Apple's are. Measured on `w4`,
                // their cover is sharp to about 53 % of the screen and still leaves a faint trace behind
                // the title at 56.5 % and the artist at 59-61 %. A sleeve that ended above the text left
                // the text sitting on bare page, which is what read as the cover being out of place.
                if (panel == Panel.ART) Box(
                    Modifier.fillMaxWidth()
                        .layout { measurable, constraints ->
                            val placeable = measurable.measure(constraints)
                            val takes = (placeable.height * (1f - SLEEVE_UNDER_TEXT)).toInt()
                            layout(placeable.width, takes) { placeable.place(0, 0) }
                        }
                        .onGloballyPositioned {
                            // What is drawn, not what the column was told: the wash lines up with the
                            // picture, and the picture runs on under the title.
                            val drawn = it.size.width / SLEEVE
                            sleeveBottom = it.positionInRoot().y + drawn
                            sleeveHeight = drawn
                        },
                ) {
                    // While the sheet moves, the cover on screen is FlyingCover's; this one takes over
                    // the moment the sheet arrives, in exactly the same place.
                    Box(Modifier.graphicsLayer { alpha = if (sheet.progress.value >= 1f || sheet.miniCover == Rect.Zero) 1f else 0f }) {
                        Artwork(
                            vm, sleeveArt, palette,
                            state.queue.getOrNull(state.previousIndex)?.let { vm.cover(it.coverArt, CoverSize.FULL) },
                            state.queue.getOrNull(state.nextIndex)?.let { vm.cover(it.coverArt, CoverSize.FULL) },
                        )
                    }
                    Handle(Modifier.align(Alignment.TopCenter).statusBarsPadding(), Color.White.copy(alpha = 0.55f), sheet)
                } else {
                    Handle(Modifier.statusBarsPadding(), scheme.onSurface.copy(alpha = 0.35f), sheet)
                    Box(Modifier.weight(1f).then(if (panel == Panel.QUEUE) Modifier.padding(horizontal = 26.dp) else Modifier)) {
                        if (panel == Panel.QUEUE) Queue(vm) else LyricsView(vm, actions, state.playing)
                    }
                }

                // The column's spare height. Measured off `w4` by row profile, Apple put the transport
                // 10.6 % of the screen above the volume slider and the bottom icons 7.7 % clear of the
                // home indicator. The three controls at the bottom are deliberately closer together than
                // that here - the owner found Apple's own spacing too loose on a 20:9 screen, which is
                // taller than the 19.5:9 those percentages were taken from - and the space that frees
                // up goes underneath them rather than between them.
                if (panel == Panel.ART) Spacer(Modifier.weight(0.02f))
                // The lyrics view carries its own header - a thumbnail with the title, the favourite and
                // the menu beside it, the way Apple's does - so this block would be the second copy of it.
                if (panel != Panel.LYRICS) Row(
                    Modifier.fillMaxWidth().padding(start = PLAYER_GUTTER, end = PLAYER_GUTTER, top = 2.dp),
                    Arrangement.spacedBy(10.dp), Alignment.CenterVertically,
                ) {
                    Column(Modifier.weight(1f)) {
                        Text(
                            state.current?.title ?: state.radio ?: "Nothing playing",
                            Modifier.readable(), style = MaterialTheme.typography.titleLarge,
                            maxLines = 1, softWrap = false, overflow = TextOverflow.Ellipsis,
                        )
                        Text(
                            // The artist alone, as Apple writes it. "Artist · Album" ran two ellipses into each
                            // other the moment the album had a long name; the album is one tap away on ⋯.
                            state.current?.artist ?: "",
                            // Apple holds this line back from the title rather than colouring it: a
                            // saturated accent here is the one thing that made the screen read as Material.
                            style = MaterialTheme.typography.titleMedium, color = scheme.onSurface.copy(alpha = 0.6f),
                            maxLines = 1, overflow = TextOverflow.Ellipsis,
                        )
                    }
                    state.current?.let { s ->
                        val starred = marks.effectiveStar(dev.flint.music.data.StarKind.SONG, s.id, s.starred)
                        Row(Modifier, Arrangement.spacedBy(16.dp), Alignment.CenterVertically) {
                            TitleCircle(
                                if (starred) Icons.Filled.Star else Icons.Filled.StarBorder,
                                "Favourite", starred,
                            ) { actions.star(s, !starred) }
                            TitleCircle(Icons.Filled.MoreHoriz, "More", false) { playerMenu(s) }
                        }
                    }
                }
                state.error?.let { Text(it, Modifier.padding(horizontal = PLAYER_GUTTER), color = scheme.error, style = MaterialTheme.typography.bodySmall) }
                if (panel != Panel.LYRICS) state.current?.let { s ->
                    val line = listOfNotNull(
                        s.suffix.uppercase().ifEmpty { null },
                        s.bitRate.takeIf { it > 0u }?.let { "$it kbps" },
                        s.samplingRate.takeIf { it > 0u }?.let { "${it.toInt() / 1000.0} kHz" },
                    ).joinToString(" · ")
                    // Apple shows nothing here. This audience wants it, so it stays - but well under
                    // the artist line, as a caption you read when you look for it.
                    Text(
                        line, Modifier.padding(horizontal = PLAYER_GUTTER),
                        style = MaterialTheme.typography.labelSmall,
                        color = scheme.onSurface.copy(alpha = 0.38f),
                        maxLines = 1, overflow = TextOverflow.Ellipsis,
                    )
                }

                Box(kept("seek")) { SeekBar(vm, state.playing, state.durationMs) }

                // Three controls, plain glyphs with no containers. Shuffle and repeat live in the queue header.
                // Sized off `w4` as a share of the screen's width: Apple's pause glyph stands 9.8 % of the
                // width tall and the skip glyphs are 9.7 % wide; these were about a fifth smaller. The
                // seek bar, volume bar and bottom icons below were scaled by their own measured ratios.
                // Apple leaves a clear gap between the times and these, rather than letting them follow on.
                Row(kept("transport").fillMaxWidth().padding(top = 24.dp), Arrangement.spacedBy(34.dp, Alignment.CenterHorizontally), Alignment.CenterVertically) {
                    IconButton(vm::previous, Modifier.size(72.dp)) { Icon(Icons.Filled.FastRewind, "Previous", Modifier.size(55.dp)) }
                    IconButton(vm::toggle, Modifier.size(84.dp)) {
                        PlayPauseGlyph(state.playing, state.buffering, 70.dp, 28.dp)
                    }
                    IconButton(vm::next, Modifier.size(72.dp)) { Icon(Icons.Filled.FastForward, "Next", Modifier.size(55.dp)) }
                }

                if (panel == Panel.ART) Spacer(Modifier.weight(0.17f))
                Box(kept("volume")) { VolumeRow(vm) }

                Row(kept("icons").fillMaxWidth().padding(top = 2.dp, bottom = 4.dp), Arrangement.SpaceEvenly, Alignment.CenterVertically) {
                    PanelButton(Icons.Filled.Lyrics, "Lyrics", panel == Panel.LYRICS) { choose(Panel.LYRICS) }
                    // Apple's middle glyph is AirPlay, not a sleep timer: on this screen the thing worth
                    // one tap is where the sound is going. The sleep timer moved to the ⋯ on the title row,
                    // which is where a setting for the evening belongs.
                    OutputButton()
                    PanelButton(Icons.AutoMirrored.Filled.QueueMusic, "Queue", panel == Panel.QUEUE) { choose(Panel.QUEUE) }
                }
                if (panel == Panel.ART) Spacer(Modifier.weight(0.19f))
            }
            }
            }
            if (panel == Panel.ART) ScrimIconButton(
                Icons.Filled.KeyboardArrowDown, "Close", sheet::close,
                Modifier.align(Alignment.TopStart).statusBarsPadding().padding(start = 8.dp, top = 6.dp),
            ) else IconButton(sheet::close, Modifier.align(Alignment.TopStart).statusBarsPadding().padding(4.dp)) {
                Icon(Icons.Filled.KeyboardArrowDown, "Close", Modifier.size(26.dp))
            }
        }
    }
}

/**
 * Width over height of the player's sleeve. Album art is square - Apple's too - so theirs is the
 * square scaled up and cropped at the left and right edges to fill a taller box. Crop the region of
 * `w4` that spans where a full-width square would have ended and the flowers below that line are as
 * sharp as the ones above it, with a strip of red tape running across it unbroken: it is the picture,
 * not the blur behind it. That is the whole trick, and it is why their sleeve can touch the top edge
 * and still reach down behind the title, which no square can do.
 */
/**
 * Where the sound is going, and one tap to change it. The glyph says which kind of output is carrying
 * the music, the way Apple's AirPlay mark fills in when something is connected.
 *
 * The picker itself is Android's own: `Settings.Panel.ACTION_MEDIA_OUTPUT` is the documented way in
 * and lists Bluetooth, wired, USB and any Cast target the system knows about - far more than this app
 * could offer on its own, and the same sheet the media notification opens. Some builds do not carry
 * that panel; they get SystemUI's dialog, and a device with neither is simply told what it is playing
 * through rather than being left with a button that does nothing.
 */
@Composable
private fun OutputButton() {
    val settings: SettingsViewModel = viewModel()
    val output by settings.currentOutput.collectAsStateWithLifecycle()
    val context = LocalContext.current
    val scheme = MaterialTheme.colorScheme
    val elsewhere = output != dev.flint.music.playback.Outputs.SPEAKER
    val icon = when {
        output.startsWith("USB") -> Icons.Filled.Headphones
        output.startsWith("Bluetooth") -> Icons.Filled.Bluetooth
        output.startsWith("Wired") -> Icons.Filled.Headphones
        else -> Icons.Filled.Cast
    }
    IconButton({ openOutputPicker(context, output) }) {
        Icon(icon, "Output: $output", Modifier.size(27.dp), tint = if (elsewhere) scheme.primary else scheme.onSurfaceVariant)
    }
}

private fun openOutputPicker(context: android.content.Context, output: String) {
    // Android 14 and later have a public call for exactly this, and it is the one that works on a
    // current phone: the same output switcher the media controls open, listing Bluetooth, wired, USB
    // and any Cast target the system knows about.
    if (android.os.Build.VERSION.SDK_INT >= 34 &&
        runCatching { android.media.MediaRouter2.getInstance(context).showSystemOutputSwitcher() }.getOrDefault(false)
    ) return
    // Android 11 to 13: SystemUI opens the same dialog on a *broadcast*, not an activity. The first
    // version of this started it as an activity, which can never resolve - on a phone the button did
    // nothing but name the output.
    if (android.os.Build.VERSION.SDK_INT >= 30) {
        val sent = runCatching {
            context.sendBroadcast(
                android.content.Intent("com.android.systemui.action.LAUNCH_MEDIA_OUTPUT_DIALOG")
                    .setPackage("com.android.systemui")
                    .putExtra("package_name", context.packageName),
            )
        }.isSuccess
        if (sent && android.os.Build.VERSION.SDK_INT < 34) return
    }
    // Android 10: the Settings panel.
    if (android.os.Build.VERSION.SDK_INT >= 29 && runCatching {
            context.startActivity(
                android.content.Intent("android.settings.panel.action.MEDIA_OUTPUT")
                    .putExtra("com.android.settings.panel.extra.PACKAGE_NAME", context.packageName)
                    .addFlags(android.content.Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }.isSuccess
    ) return
    android.widget.Toast.makeText(context, "Playing through $output", android.widget.Toast.LENGTH_SHORT).show()
}

/**
 * The player's side margin. Apple keeps its title, seek bar and title-row buttons 8.2 % of the
 * screen's width in from each edge - measured on `w4` - which on a 411 dp wide phone is 33 dp; ours sat
 * at 26 and 16, so everything read as pushed against the sides.
 */
private val PLAYER_GUTTER = 33.dp

private const val SLEEVE = 0.74f

/**
 * How much of the sleeve's height runs on underneath the title block instead of above it. With the
 * sleeve at [SLEEVE] this puts the title where `w4` has it, 56.5 % of the screen, with the picture's
 * blurred tail behind it.
 */
private const val SLEEVE_UNDER_TEXT = 0.095f

/** Drag it down, or tap it, to put the player away. */
@Composable
private fun Handle(modifier: Modifier, colour: Color, sheet: PlayerSheet) {
    val onBack = sheet::close
    Box(
        modifier.fillMaxWidth().dragsSheet(sheet).padding(vertical = 10.dp),
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
private fun Artwork(vm: PlayerViewModel, art: SleeveArt, palette: PagePalette?, previousUrl: String?, nextUrl: String?) {
    Box(Modifier.fillMaxWidth(), Alignment.TopCenter) {
        Box(
            // Not square. Measure `w4` and Apple's sleeve runs from the very top edge of the screen down
            // to about half of it - 977 wide by roughly 1050 tall - so it is the cover scaled to fill and
            // cropped a little at the sides. That is how it manages to have no top edge *and* reach down
            // behind the title; a full-width square can only do one or the other. Cover crops already.
            Modifier.fillMaxWidth().aspectRatio(SLEEVE),
        ) {
            SleeveCarousel(art, previousUrl, nextUrl, onPrevious = vm::previousItem, onNext = vm::next)
            // Just enough shade under the status bar for its icons to read on a pale cover; the same
            // amount the album page uses, and invisible against anything darker.
            Box(
                Modifier.fillMaxWidth().fillMaxHeight(0.16f)
                    .background(Brush.verticalGradient(0f to Color.Black.copy(alpha = 0.30f), 1f to Color.Transparent)),
            )
            // The sleeve goes soft rather than stopping: its bottom third cross-fades into the same
            // cover, blurred, which the page behind it is already drawing at the same scale.
            if (palette != null) Box(
                Modifier.fillMaxSize().drawBehind { drawSleeveMelt(palette, 0.19f) },
            )
        }
    }
}

/**
 * The cover in flight: from the mini player's thumbnail to the sleeve, one picture changing size and
 * place with the sheet's progress, never a thumbnail swapped for a sleeve. It rides in the sheet's own
 * coordinates - the thumbnail's place relative to the mini player's top at 0, the sleeve's at 1 - so it
 * moves with the sheet and only has to grow.
 *
 * It is the whole square the entire way, never a cropped window on it: as it grows it simply runs off
 * both sides of the screen, and at the end the screen's own edges crop it to exactly what the sleeve
 * shows (the sleeve is the square cropped at the sides), so the last frame is the real sleeve pixel
 * for pixel and the hand-over cannot be seen. A window narrowing from square to the sleeve's shape
 * read as the picture being cut while it moved.
 *
 * The picture is laid out once, as the full square at the sleeve's height, and moved and grown only
 * by a layer transform. Growing it by layout
 * instead gave the image a new size every frame, and each new size was a new decode and a new texture:
 * the flight stalled for a third of a second at a time.
 *
 * The small rendition the mini player already has sits under the large one, so the first frame of a
 * flight shows the picture even before the large one has come out of the cache.
 */
@Composable
private fun FlyingCover(sheet: PlayerSheet, rowUrl: String?, art: SleeveArt, palette: PagePalette?, measured: Boolean) {
    val flying by remember { androidx.compose.runtime.derivedStateOf { sheet.progress.value < 1f } }
    if (!flying || !measured || sheet.miniCover == Rect.Zero) return
    val density = androidx.compose.ui.platform.LocalDensity.current
    val thumbRadius = with(density) { 7.dp.toPx() }
    androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxSize()) {
        val w = constraints.maxWidth.toFloat()
        val h = w / SLEEVE
        val side = with(density) { h.toDp() }
        Box(
            Modifier.requiredSize(side).align(Alignment.TopStart)
                .graphicsLayer {
                    val t = sheet.progress.value.coerceIn(0f, 1f)
                    val from = sheet.miniCover.translate(0f, -sheet.travel)
                    fun mix(a: Float, b: Float) = a + (b - a) * t
                    val k = (mix(from.height, h) / h).coerceAtLeast(0.01f)
                    transformOrigin = androidx.compose.ui.graphics.TransformOrigin(0f, 0f)
                    scaleX = k; scaleY = k
                    // The square is wider than the screen, so layout centres it with its sides hanging
                    // off both edges; that is where it ends up. It starts on the thumbnail.
                    val overhang = (w - size.width) / 2f
                    translationX = mix(from.left, overhang) - overhang
                    translationY = mix(from.top, 0f)
                    shape = RoundedCornerShape(thumbRadius * (1f - t) / k)
                    clip = true
                },
        ) {
            if (art.current == null) coil3.compose.AsyncImage(rowUrl, null, Modifier.fillMaxSize(), contentScale = androidx.compose.ui.layout.ContentScale.Crop)
            SleeveImage(art, Modifier.fillMaxSize())
            // The sleeve's melt into the page, over the part of the square the screen will show. It fades
            // in only over the last stretch, once the square's sides are nearly off the screen: earlier,
            // the melt stopping short of the square's edges showed as a notch at its bottom corners.
            if (palette != null) Box(
                Modifier.align(Alignment.Center).requiredSize(with(density) { w.toDp() }, side)
                    .graphicsLayer {
                        val e = ((sheet.progress.value - 0.75f) / 0.25f).coerceIn(0f, 1f)
                        alpha = e * e * (3f - 2f * e)
                    }
                    .drawBehind { drawSleeveMelt(palette, 0.19f) },
            )
        }
    }
}

/**
 * The sleeve's picture across songs. On a skip the old cover stays while the new one loads, and the
 * new one fades in over it; the sleeve used to go blank for as long as the server took to render the
 * next cover. But only briefly: a cover that has not come after [HOLD_MS] means the old picture is
 * now standing under the wrong title, so it fades out to the plate, whose sheen says the new one is on
 * its way, and the new one fades in from there. The very first picture fades in from the plate too,
 * unless it came straight from memory, where a fade would only be a delay.
 *
 * One of these feeds both the sleeve and the cover in flight: two requests for the same picture in the
 * same frame each decoded their own bitmap, and the second was uploaded to the GPU on the frame the
 * sleeve took over - a stall exactly at the landing.
 */
@androidx.compose.runtime.Stable
private class SleeveArt {
    /** What is showing, fading in over [previous] at [fade]. */
    var current by mutableStateOf<androidx.compose.ui.graphics.painter.Painter?>(null)
    val fade = androidx.compose.animation.core.Animatable(1f)
    /** The picture being left, at [previousAlpha]. */
    var previous by mutableStateOf<androidx.compose.ui.graphics.painter.Painter?>(null)
    val previousAlpha = androidx.compose.animation.core.Animatable(1f)
    var loading by mutableStateOf(true)
    /** The address of [current]: what a swipe waits for before it hands the sleeve back. */
    var shownUrl by mutableStateOf<String?>(null)
    /** The next picture goes straight in: a swipe has already slid it into place. */
    var snapNext = false

    /** Lets the current picture go, fading it out to the plate. */
    suspend fun letGo() {
        val leaving = current ?: return
        previous = leaving; current = null
        if (AppMotion.reduce) previousAlpha.snapTo(0f)
        else { previousAlpha.snapTo(1f); previousAlpha.animateTo(0f, androidx.compose.animation.core.tween(360)) }
        previous = null
    }
}

private const val HOLD_MS = 600L

/**
 * The sleeve as one record in a row of them: a sideways drag slides it and brings the next (or the
 * last) record in from the other edge, already drawn, the way Apple's does. Let go past a third of the
 * way, or flicked, the old one goes all the way off and the new one all the way in, and only then does
 * the song change. The new record then stays drawn over the sleeve until the sleeve has the same
 * picture, so there is no second change: no fade, no plate, no old cover coming back for a frame.
 *
 * The neighbours' pictures come from the cache the player keeps warm (PlayerViewModel's covers ahead),
 * so they are normally there before the finger is. Nothing here runs until a finger is down.
 */
@Composable
private fun SleeveCarousel(art: SleeveArt, previousUrl: String?, nextUrl: String?, onPrevious: () -> Unit, onNext: () -> Unit) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    val haptics = LocalHapticFeedback.current
    val offset = remember { Animatable(0f) }
    // 0 at rest, 1 while a finger holds the record: it lifts off the page - a little smaller, rounded,
    // with a shadow - and the cover's own blur shows round it. It goes back down once the song is in.
    val lift = remember { Animatable(0f) }
    val density = androidx.compose.ui.platform.LocalDensity.current
    val gap = with(density) { 18.dp.toPx() }
    val radius = with(density) { 22.dp.toPx() }
    val elevation = with(density) { 18.dp.toPx() }
    @Composable fun neighbour(url: String?) = coil3.compose.rememberAsyncImagePainter(
        remember(url) { coil3.request.ImageRequest.Builder(context).data(url).size(CoverSize.FULL).build() },
        filterQuality = androidx.compose.ui.graphics.FilterQuality.Low,
    )
    val before = neighbour(previousUrl)
    val after = neighbour(nextUrl)
    val hasBefore by androidx.compose.runtime.rememberUpdatedState(previousUrl != null)
    val hasAfter by androidx.compose.runtime.rememberUpdatedState(nextUrl != null)
    // The record that has been slid in, drawn over the sleeve until the sleeve shows it too. The drawn
    // picture itself, not the painter: the painter is handed the following song's address next.
    var landed by remember { mutableStateOf<androidx.compose.ui.graphics.painter.Painter?>(null) }
    var landedUrl by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(landedUrl) {
        val url = landedUrl ?: return@LaunchedEffect
        kotlinx.coroutines.withTimeoutOrNull(4_000) { androidx.compose.runtime.snapshotFlow { art.shownUrl }.first { it == url } }
        landed = null; landedUrl = null
    }
    val plateColour = MaterialTheme.colorScheme.surfaceVariant
    androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxSize()) {
    val widthPx = constraints.maxWidth.toFloat()
    val heightPx = constraints.maxHeight.toFloat()
    val sideDp = with(density) { heightPx.toDp() }
    Box(
        Modifier.fillMaxSize().pointerInput(Unit) {
            val tracker = androidx.compose.ui.input.pointer.util.VelocityTracker()
            var x = 0f
            val release: (Float) -> Unit = { v ->
                val o = offset.value
                val w = size.width.toFloat()
                val go = when {
                    o < 0f && hasAfter && (v < -FLICK_PX || o < -w * TURN) -> -1
                    o > 0f && hasBefore && (v > FLICK_PX || o > w * TURN) -> 1
                    else -> 0
                }
                val down = spring<Float>(dampingRatio = 0.8f, stiffness = 240f)
                scope.launch {
                    val settle = spring<Float>(dampingRatio = 1f, stiffness = 520f)
                    if (go == 0) {
                        launch { lift.animateTo(0f, down) }
                        offset.animateTo(0f, settle, initialVelocity = v); return@launch
                    }
                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    val painter = if (go < 0) after else before
                    val url = if (go < 0) nextUrl else previousUrl
                    // Where the neighbour sits once the record is fully lifted, which is where the lift is headed.
                    // A record is the whole square, as tall as the sleeve.
                    val side = size.height.toFloat()
                    val span = side * liftedScale(lift.targetValue, w, side) + gap
                    if (AppMotion.reduce) offset.snapTo(go * span) else offset.animateTo(go * span, settle, initialVelocity = v)
                    // Same frame: the incoming record takes the middle, the sleeve goes back under it.
                    landed = (painter.state.value as? coil3.compose.AsyncImagePainter.State.Success)?.painter
                    landedUrl = url
                    art.snapNext = true
                    offset.snapTo(0f)
                    if (go < 0) onNext() else onPrevious()
                    // The new record settles back into the sleeve, with the slightest give.
                    lift.animateTo(0f, down)
                }
            }
            detectHorizontalDragGestures(
                onDragStart = {
                    tracker.resetTracking(); x = 0f
                    scope.launch { offset.stop() }
                    if (!AppMotion.reduce) scope.launch { lift.animateTo(1f, spring(dampingRatio = 0.9f, stiffness = 420f)) }
                },
                onDragEnd = { release(tracker.calculateVelocity().x) },
                onDragCancel = { release(0f) },
            ) { change, d ->
                x += d
                tracker.addPosition(change.uptimeMillis, Offset(x, 0f))
                val w = size.width.toFloat()
                val next = offset.value + d
                // Towards a record that is not there it gives a little and no more.
                val allowed = (next > 0f && hasBefore) || (next < 0f && hasAfter)
                scope.launch { offset.snapTo(if (allowed) next.coerceIn(-w, w) else (offset.value + d * 0.2f).coerceIn(-w * 0.06f, w * 0.06f)) }
            }
        },
    ) {
        // One record, lifted by [lift] and moved by [dx]; [fade] is its brightness against the page. All
        // of it read in the draw phase: a drag moves layers and recomposes nothing.
        // Each record is the cover's whole square, as tall as the sleeve and so wider than the screen: at
        // rest the screen's edges crop it to exactly the sleeve, and lifted it shrinks until all of it is
        // on screen - the sides the sleeve hides come into view as the record is picked up.
        fun Modifier.record(dx: (Float, Float) -> Float, fade: (Float) -> Float) = align(Alignment.Center).requiredSize(sideDp).graphicsLayer {
            val l = lift.value
            val s = liftedScale(l, widthPx, size.height)
            scaleX = s; scaleY = s
            val span = size.width * s + gap
            val o = offset.value
            translationX = dx(o, span)
            alpha = fade((kotlin.math.abs(o) / span).coerceIn(0f, 1f))
            if (l > 0f) {
                shape = RoundedCornerShape(radius * l / s)
                clip = true
                shadowElevation = elevation * l
            }
        }
        val o0 = { offset.value }
        Box(Modifier.fillMaxSize().record({ o, _ -> o }, { f -> 1f - 0.35f * f })) { SleeveImage(art, Modifier.fillMaxSize()) }
        // Each neighbour waits just off its edge and is drawn only while it is being pulled in, coming up
        // from a little dimmer as it arrives.
        Box(Modifier.fillMaxSize().record({ o, span -> o + span }, { f -> if (o0() < 0f) 0.55f + 0.45f * f else 0f }).background(plateColour)) {
            androidx.compose.foundation.Image(after, null, Modifier.fillMaxSize(), contentScale = androidx.compose.ui.layout.ContentScale.Crop)
        }
        Box(Modifier.fillMaxSize().record({ o, span -> o - span }, { f -> if (o0() > 0f) 0.55f + 0.45f * f else 0f }).background(plateColour)) {
            androidx.compose.foundation.Image(before, null, Modifier.fillMaxSize(), contentScale = androidx.compose.ui.layout.ContentScale.Crop)
        }
        if (landedUrl != null) Box(Modifier.fillMaxSize().record({ _, _ -> 0f }, { 1f }).background(plateColour)) {
            landed?.let { androidx.compose.foundation.Image(it, null, Modifier.fillMaxSize(), contentScale = androidx.compose.ui.layout.ContentScale.Crop) }
        }
    }
}
}

/** How much of the screen's width a held record takes. */
private const val LIFTED_WIDTH = 0.86f

/** The scale of a record [side] tall at lift [l], on a sleeve [width] wide: 1 at rest, the whole square at 86 % of the width held. */
private fun liftedScale(l: Float, width: Float, side: Float): Float {
    val held = if (side > 0f) (LIFTED_WIDTH * width / side).coerceAtMost(1f) else 1f
    return 1f - (1f - held) * l
}

/** Past this share of the width a slow drag changes the record. */
private const val TURN = 0.3f

/** A release faster than this, in pixels a second, changes the record whatever the distance. */
private const val FLICK_PX = 1000f

@Composable
private fun rememberSleeveArt(url: String?): SleeveArt {
    val context = LocalContext.current
    val art = remember { SleeveArt() }
    val painter = coil3.compose.rememberAsyncImagePainter(
        remember(url) { coil3.request.ImageRequest.Builder(context).data(url).size(CoverSize.FULL).build() },
        filterQuality = androidx.compose.ui.graphics.FilterQuality.Low,
    )
    val state by painter.state.collectAsState()
    LaunchedEffect(state) {
        when (val st = state) {
            is coil3.compose.AsyncImagePainter.State.Success -> if (st.painter !== art.current) {
                art.loading = false
                art.shownUrl = url
                val swiped = art.snapNext.also { art.snapNext = false }
                val instant = swiped || art.current == null && art.previous == null && st.result.dataSource == coil3.decode.DataSource.MEMORY_CACHE
                // The picture on screen stays underneath at full strength while the new one covers it; one
                // already fading out to the plate carries on from where it is.
                art.current?.let { art.previous = it; art.previousAlpha.snapTo(1f) }
                art.current = st.painter
                if (instant || AppMotion.reduce) art.fade.snapTo(1f)
                else { art.fade.snapTo(0f); art.fade.animateTo(1f, androidx.compose.animation.core.tween(if (art.previous == null) 320 else 480)) }
                art.previous = null
            }
            is coil3.compose.AsyncImagePainter.State.Loading -> {
                if (art.current == null) art.loading = true
                else { delay(HOLD_MS); art.loading = true; art.letGo() }
            }
            // Nothing to show for this song: back to the plate rather than keep the last cover.
            is coil3.compose.AsyncImagePainter.State.Error -> { art.loading = false; art.letGo() }
            else -> {}
        }
    }
    return art
}

@Composable
private fun SleeveImage(art: SleeveArt, modifier: Modifier) {
    Box(modifier.loadingSheen(art.loading, MaterialTheme.colorScheme.onSurface)) {
        art.previous?.let { androidx.compose.foundation.Image(it, null, Modifier.fillMaxSize().graphicsLayer { alpha = art.previousAlpha.value }, contentScale = androidx.compose.ui.layout.ContentScale.Crop) }
        art.current?.let { androidx.compose.foundation.Image(it, null, Modifier.fillMaxSize().graphicsLayer { alpha = art.fade.value }, contentScale = androidx.compose.ui.layout.ContentScale.Crop) }
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
        Box(Modifier.fillMaxSize(), Alignment.Center) { Icon(icon, label, Modifier.size(25.dp)) }
    }
}

/**
 * The phone's music-stream volume, read live so the hardware keys never leave it stale. Ticks only
 * while this screen is resumed; a drag writes straight through and updates the thumb itself.
 */
@Composable
private fun VolumeRow(vm: PlayerViewModel) {
    val scheme = MaterialTheme.colorScheme
    // Pushed by the system the moment it changes - no polling, nothing ticking while the screen is open.
    val system by vm.volume.collectAsStateWithLifecycle()
    var dragging by remember { mutableStateOf(false) }
    var level by remember { mutableFloatStateOf(vm.volumeFraction()) }
    // What is drawn. A change from outside - the volume keys, another app - eases over; a drag is followed exactly.
    val shown = remember { androidx.compose.animation.core.Animatable(level) }
    val plain = reduceMotion()
    val scope = rememberCoroutineScope()
    LaunchedEffect(system, dragging) {
        if (dragging) return@LaunchedEffect
        level = system
        if (plain) shown.snapTo(system)
        else shown.animateTo(system, androidx.compose.animation.core.tween(180))
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
            scope.launch { shown.snapTo(f) }
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
                    val h = 7.dp.toPx()
                    val y = (size.height - h) / 2f
                    val r = CornerRadius(h / 2f, h / 2f)
                    drawRoundRect(track, Offset(0f, y), Size(size.width, h), r)
                    drawRoundRect(filled, Offset(0f, y), Size(size.width * shown.value, h), r)
                    // No knob unless a finger is on it: Apple's volume slider is a filled bar and
                    // nothing else, and a permanent white circle is the most Material thing on the screen.
                    if (dragging) drawCircle(filled, h * 1.15f, Offset(size.width * shown.value, size.height / 2f))
                },
        )
        Icon(Icons.AutoMirrored.Filled.VolumeUp, null, Modifier.size(20.dp), tint = scheme.onSurfaceVariant)
    }
}

/**
 * A line too long for its width reads itself out: it sits still for a moment, so the start can be
 * read, then walks slowly sideways and comes back round, the way the title does in Apple's player.
 * A line that fits is left alone - the modifier only animates while the text overflows.
 *
 * It scrolls only while the player is really on screen. The player stays composed behind the rest of
 * the app (see LocalPlayerShown), and a title quietly walking about down there would hold a frame
 * clock awake for nothing.
 */
@Composable
internal fun Modifier.readable(): Modifier =
    if (!LocalPlayerShown.current) this
    else basicMarquee(
        iterations = Int.MAX_VALUE,
        repeatDelayMillis = 2600,
        initialDelayMillis = 2600,
        spacing = MarqueeSpacing(46.dp),
        velocity = 26.dp,
    )

@Composable
private fun PanelButton(icon: androidx.compose.ui.graphics.vector.ImageVector, label: String, on: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    IconButton(onClick) { Icon(icon, label, Modifier.size(27.dp), tint = if (on) scheme.primary else scheme.onSurfaceVariant) }
}

/** The only ticking thing in the app, and only while this screen is resumed and music is playing. */
@Composable
private fun position(vm: PlayerViewModel, playing: Boolean, everyMs: Long): Long {
    var pos by remember { mutableLongStateOf(vm.positionMs) }
    var resumed by remember { mutableStateOf(false) }
    LifecycleResumeEffect(Unit) { resumed = true; onPauseOrDispose { resumed = false } }
    val shown = LocalPlayerShown.current
    LaunchedEffect(playing, resumed, shown) {
        pos = vm.positionMs
        while (playing && resumed && shown && isActive) { delay(everyMs); pos = vm.positionMs }
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
    // Duration is read through the gesture rather than keying it: a track that learns its real length
    // mid-scrub would restart the detector and the finger would lift on a dead pointer.
    val d by rememberUpdatedState(durationMs.coerceAtLeast(1))
    var dragging by remember { mutableStateOf(false) }
    var drag by remember { mutableFloatStateOf(0f) }
    val fraction = (if (dragging) drag else pos.toFloat() / d).coerceIn(0f, 1f)
    val track = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.22f)
    val filled = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.85f)
    // Held, the bar thickens and the dot grows, the way Apple's does, so the scrub is felt as well as
    // seen. Animated both ways: nothing here changes size in one frame.
    val thickness by animateFloatAsState(if (dragging) 11f else 7.3f, spring(0.9f, 420f), label = "seek")
    val knob by animateFloatAsState(if (dragging) 1.5f else 0f, spring(0.9f, 420f), label = "knob")
    Column(Modifier.padding(horizontal = PLAYER_GUTTER, vertical = 4.dp)) {
        Box(
            // The strip is wider than the hairline it draws: a thumb is not a mouse, and the 26 dp this
            // used to be was easy to miss by a few pixels and hit the sheet instead.
            Modifier.fillMaxWidth().height(34.dp)
                .pointerInput(Unit) {
                    // Written out rather than assembled from detectHorizontalDragGestures and
                    // detectTapGestures, because both of those let the gesture go: the pointer is
                    // claimed on touch-down and every move is consumed, so the player sheet's own
                    // vertical drag can no longer take a scrub that runs a few degrees off level.
                    // It used to, and then the finger lifted on a cancelled gesture and the song
                    // never moved - the bar had followed the finger the whole way, which is what
                    // made it look as though seeking was broken rather than stolen.
                    awaitEachGesture {
                        val down = awaitFirstDown(requireUnconsumed = false)
                        down.consume()
                        drag = (down.position.x / size.width).coerceIn(0f, 1f)
                        dragging = true
                        var seek = true
                        while (true) {
                            val change = awaitPointerEvent().changes.firstOrNull { it.id == down.id }
                            // The pointer vanished from the event: the window took it (a call, a
                            // system gesture). Leave the song where it was.
                            if (change == null) { seek = false; break }
                            drag = (change.position.x / size.width).coerceIn(0f, 1f)
                            change.consume()
                            if (!change.pressed) break
                        }
                        // Apple seeks on release, not while the finger moves: one seek, at the end,
                        // and the sound carries on undisturbed until then.
                        if (seek) vm.seekTo((drag * d).toLong())
                        dragging = false
                    }
                }
                .drawBehind {
                    val h = thickness.dp.toPx()
                    val y = (size.height - h) / 2f
                    val r = CornerRadius(h / 2f, h / 2f)
                    drawRoundRect(track, Offset(0f, y), Size(size.width, h), r)
                    drawRoundRect(filled, Offset(0f, y), Size(size.width * fraction, h), r)
                    if (knob > 0.01f) drawCircle(filled, h * knob, Offset(size.width * fraction, size.height / 2f))
                },
        )
        Row(Modifier.fillMaxWidth(), Arrangement.SpaceBetween, Alignment.CenterVertically) {
            Text(duration((if (dragging) (drag * d).toLong() else pos) / 1000), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            // The centre slot carries whatever needs saying: an error, or the sleep timer. Empty
            // the rest of the time, holding the space so the times never move.
            // Apple writes a word here while a transition is running; the rest of the time the slot holds
            // its space so the two times either side never move. `pos` above ticks this once a second.
            val centre = state.error ?: when {
                vm.mixing -> "Mixing"
                state.sleepAtEndOfTrack -> "Sleep · end of track"
                // elapsedRealtime, not wall clock: sleepAt is set from SystemClock (PlayerConnection),
                // and subtracting one from the other gives a number about fifty years wide, which the
                // coerce below then turned into a cheerful "1 min" for every timer ever set.
                state.sleepAt > 0 -> "Sleep · ${((state.sleepAt - android.os.SystemClock.elapsedRealtime() + 59_999) / 60_000).coerceAtLeast(1)} min"
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
    val list = rememberLazyListState(initialFirstVisibleItemIndex = state.order.indexOf(state.index).coerceAtLeast(0))
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
    // In the order the songs will play, which under shuffle is not the order of the list itself. A drag
    // moves a song within the list, so reordering is offered only when the two are the same.
    val order = state.order.takeIf { it.size == state.queue.size } ?: state.queue.indices.toList()
    val reorderable = !state.shuffle
    LazyColumn(Modifier.fillMaxSize().weight(1f), state = list) {
        itemsIndexed(order, key = { _, i -> "$i-${state.queue[i].id}" }, contentType = { _, _ -> "song" }) { at, i ->
            val s = state.queue[i]
            val held = at == from
            val shift = when {
                from < 0 -> 0f
                held -> dragOffset
                at in (from + 1)..target -> -rowHeight
                at in target until from -> rowHeight
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
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        // Added by hand: plays before the rest of the queue carries on.
                        if (i in state.queued) Icon(Icons.AutoMirrored.Filled.QueueMusic, "Added by you", Modifier.padding(end = 4.dp).size(14.dp), tint = MaterialTheme.colorScheme.primary)
                        Text(s.artist, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                IconButton({ vm.remove(i) }, Modifier.size(38.dp)) {
                    Icon(Icons.Filled.Close, "Remove", Modifier.size(19.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                androidx.compose.animation.AnimatedVisibility(
                    reorderable,
                    enter = androidx.compose.animation.fadeIn() + androidx.compose.animation.expandHorizontally(),
                    exit = androidx.compose.animation.fadeOut() + androidx.compose.animation.shrinkHorizontally(),
                ) { Icon(
                    Icons.Filled.DragHandle, "Reorder",
                    tint = if (held) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                    modifier = Modifier.size(44.dp).padding(11.dp).pointerInput(Unit) {
                        detectDragGestures(
                            onDragStart = {
                                from = at; dragOffset = 0f
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
                ) }
            }
        }
    }
    }
}
