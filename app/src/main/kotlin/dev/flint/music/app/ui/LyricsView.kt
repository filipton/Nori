package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.gestures.animateScrollBy
import kotlinx.coroutines.withContext
import kotlinx.coroutines.launch
import kotlinx.coroutines.coroutineScope
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.drawscope.clipPath
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.drawText
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.MoreHoriz
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.StarBorder
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.Load
import dev.flint.music.app.vm.PlayerViewModel
import dev.flint.music.app.vm.SettingsViewModel
import dev.flint.music.ffi.LyricLine
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

/**
 * How far into [line] the singing is at [ms], as a UTF-16 offset with a fraction: 7.5 means half of
 * the character at index 7. Inside a word the sweep is linear; between words it rests at the word's end.
 */
private fun sungOffset(line: LyricLine, ms: Long): Float {
    if (line.words.isEmpty()) return if (ms >= line.startMs) line.text.length.toFloat() else 0f
    var at = 0f
    for (w in line.words) {
        if (ms >= w.endMs) { at = w.end.toFloat(); continue }
        if (ms > w.startMs) at = w.start.toFloat() + (w.end.toInt() - w.start.toInt()) * ((ms - w.startMs).toFloat() / (w.endMs - w.startMs).coerceAtLeast(1))
        break
    }
    return at
}

/** The part of a laid-out paragraph before [offset], as a clip: whole visual lines above, a partial one at the boundary. */
private fun sungRegion(layout: TextLayoutResult, offset: Float): Path {
    val path = Path()
    val index = offset.toInt().coerceIn(0, layout.layoutInput.text.length)
    val row = layout.getLineForOffset(index)
    for (r in 0 until row) path.addRect(Rect(0f, layout.getLineTop(r), layout.size.width.toFloat(), layout.getLineBottom(r)))
    val from = layout.getHorizontalPosition(index, true)
    val next = (index + 1).coerceAtMost(layout.layoutInput.text.length)
    val to = if (layout.getLineForOffset(next) == row) layout.getHorizontalPosition(next, true) else layout.getLineRight(row)
    val x = from + (to - from) * (offset - index)
    path.addRect(Rect(layout.getLineLeft(row), layout.getLineTop(row), x, layout.getLineBottom(row)))
    return path
}

@Composable
fun LyricsView(vm: PlayerViewModel, actions: ActionsViewModel, playing: Boolean) {
    val load by vm.lyrics.collectAsStateWithLifecycle()
    val playerState by vm.state.collectAsStateWithLifecycle()
    val settings: SettingsViewModel = viewModel()
    val prefs by settings.prefs.collectAsStateWithLifecycle()
    val found = (load as? Load.Ready)?.data
    val lyrics = found?.lyrics
    if (prefs.lyricsKeepScreenOn) { val view = LocalView.current; DisposableEffect(playing) { view.keepScreenOn = playing; onDispose { view.keepScreenOn = false } } }
    val song = playerState.current
    if (lyrics == null || lyrics.lines.isEmpty()) {
        Column(Modifier.fillMaxSize()) {
            LyricsHeader(vm, actions, song)
            Box(Modifier.fillMaxSize().weight(1f), Alignment.Center) { Text(if (load is Load.Loading) "Loading…" else "No lyrics", color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
        return
    }

    var nudgeMs by remember(lyrics) { mutableLongStateOf(0L) }
    var resumed by remember { mutableStateOf(false) }
    LifecycleResumeEffect(Unit) { resumed = true; onPauseOrDispose { resumed = false } }
    // Only sweep when the lyrics actually carry per-word times (enhanced LRC, or a server's structured
    // cues). Spreading a line's duration across its words by length looks right for a beat and then
    // drifts badly on a held note or a fast line, which reads as broken sync - a line at a time is
    // honest and stays in step.
    val sweep = prefs.lyricsSweep && lyrics.synced && lyrics.wordTimed
    // The playhead as the lyrics see it. With the sweep on it is read once per frame (a local computation in the
    // controller, no IPC) and only the draw phase of the active line looks at it; otherwise three times a second.
    var now by remember { mutableLongStateOf(vm.positionMs) }
    LaunchedEffect(playing, resumed, sweep, lyrics.synced) {
        now = vm.positionMs + nudgeMs
        if (!lyrics.synced) return@LaunchedEffect
        var frame = 0
        while (playing && resumed && isActive) {
            // Every second display frame is plenty for a text fill, and half the redraws.
            if (sweep) { withFrameMillis { }; if (++frame % 2 == 1) continue } else delay(300)
            val t = vm.positionMs + nudgeMs
            // Between words, and while a held note keeps the boundary still, nothing on screen changes: do not invalidate.
            val line = lyrics.lines.getOrNull(lyrics.lines.indexOfLast { it.startMs <= t })
            if (!sweep || line == null || lyrics.lines.indexOfLast { it.startMs <= now } != lyrics.lines.indexOfLast { it.startMs <= t } || kotlin.math.abs(sungOffset(line, t) - sungOffset(line, now)) >= 0.04f) now = t
        }
    }
    val active by remember(lyrics) { derivedStateOf { if (lyrics.synced) lyrics.lines.indexOfLast { it.startMs <= now } else -1 } }

    val style = when (prefs.lyricsSize) { 0 -> MaterialTheme.typography.titleMedium; 2 -> MaterialTheme.typography.headlineMedium; else -> MaterialTheme.typography.headlineSmall }
    val bright = MaterialTheme.colorScheme.onSurface
    val dim = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.35f)
    val page = MaterialTheme.colorScheme.background
    val list = rememberLazyListState()
    Column(Modifier.fillMaxSize()) {
        LyricsHeader(vm, actions, song)
        BoxWithConstraints(Modifier.fillMaxSize().weight(1f)) {
        val third = with(LocalDensity.current) { (maxHeight / 3).roundToPx() }
        // The line being sung rests a third of the way down, and the list glides there. It used to call
        // animateScrollToItem, whose default spring is so stiff that over one line's distance it is
        // done in two or three frames - on screen, a teleport. This is a measured ease instead, slow
        // enough to follow with the eye, the way Apple's lyrics move. A line that is not on screen at
        // all (a seek, the first line after opening) is still jumped to: gliding past a whole song's
        // worth of words is not a transition, it is a wait.
        val plain = reduceMotion()
        val motion = appMotion()
        // How far through the change from `was` to `active` the lines are: 0 is the old line lit, 1 the
        // new one. Read in the draw phase (graphicsLayer alpha below), so a change redraws the few lines
        // on screen and recomposes none of them.
        val glide = remember(lyrics) { Animatable(1f) }
        var was by remember(lyrics) { mutableIntStateOf(active) }
        LaunchedEffect(active, plain) {
            if (active < 0) return@LaunchedEffect
            val here = list.layoutInfo.visibleItemsInfo.firstOrNull { it.index == active }
            if (plain || here == null) {
                list.scrollToItem(active, -third); was = active; glide.snapTo(1f)
                return@LaunchedEffect
            }
            // scrollToItem(active, -third) would leave the line at offset `third`; glide by the difference.
            val distance = (here.offset - third).toFloat()
            glide.snapTo(0f)
            // Under the app's own motion scale: with Android's animations off, Compose would otherwise
            // finish both of these on the first frame, which is exactly the jump this is here to prevent.
            withContext(motion) {
                coroutineScope {
                    launch { glide.animateTo(1f, androidx.compose.animation.core.tween(LYRIC_GLIDE_MS, easing = LyricEase)) }
                    if (kotlin.math.abs(distance) >= 1f) {
                        list.animateScrollBy(distance, androidx.compose.animation.core.tween(LYRIC_GLIDE_MS, easing = LyricEase))
                    }
                }
            }
            was = active
        }
        // The words fade out towards both ends of the panel by becoming transparent, not by having a
        // colour painted over them. The page behind is the cover's blur and varies across the width;
        // any colour laid on top to hide the words ends on one flat value, and where that flat value
        // meets the blur below it there is a dead straight line - measured on a phone at the exact
        // bottom edge of this panel. A mask paints nothing, so there is nothing to meet anything.
        LazyColumn(
            state = list,
            contentPadding = PaddingValues(start = 20.dp, end = 20.dp, top = 8.dp, bottom = maxHeight / 2),
            modifier = Modifier
                .graphicsLayer { compositingStrategy = androidx.compose.ui.graphics.CompositingStrategy.Offscreen }
                .drawWithContent {
                    drawContent()
                    drawRect(
                        Brush.verticalGradient(
                            // Gone by the source label at the bottom, so the two never sit on each other.
                            0f to Color.Transparent, 0.05f to Color.Black,
                            0.66f to Color.Black, 0.92f to Color.Transparent,
                        ),
                        blendMode = androidx.compose.ui.graphics.BlendMode.DstIn,
                    )
                },
        ) {
            itemsIndexed(lyrics.lines, key = { i, _ -> i }) { i, line ->
                Column(Modifier.fillMaxWidth().clickable(enabled = lyrics.synced) { vm.seekTo((line.startMs - nudgeMs).coerceAtLeast(0)) }.padding(vertical = 8.dp)) {
                    val weight = if (line.background) FontWeight.Normal else FontWeight.SemiBold
                    when {
                        !lyrics.synced -> Text(line.text, style = style, color = bright)
                        i == active && sweep -> SweepLine(line, style.copy(fontWeight = weight), dim, bright) { now }
                        else -> {
                            // The line lights up over the same moment the list moves, instead of flicking
                            // between two colours - "the song moved on" rather than "the screen blinked".
                            // Every state is the same colour at a different strength, so this is one alpha,
                            // blended in the draw phase from the old line's state to the new one's.
                            fun strength(lit: Int) = if (i == lit) 1f else if (i < lit) 0.35f * 0.55f else 0.35f
                            Text(
                                line.text, style = style.copy(fontWeight = weight), color = bright,
                                modifier = Modifier.graphicsLayer {
                                    val from = strength(was); val to = strength(active)
                                    alpha = from + (to - from) * glide.value
                                },
                            )
                        }
                    }
                    if (prefs.lyricsTranslation) line.translation?.let { Text(it, style = MaterialTheme.typography.bodyMedium, color = if (i == active) bright.copy(alpha = 0.8f) else dim) }
                }
            }
        }
        // Where the words came from, and how to nudge them. Nudging is for the handful of songs whose
        // timings are wrong, so it is not worth a permanent bar across the bottom of the lyrics: the
        // corner carries where they came from, and tapping it opens the two buttons. It closes again
        // on the next tap, and stays open while the offset is not zero so the number can be read.
        var tuning by remember(lyrics) { mutableStateOf(false) }
        val source = found.source.takeIf { it != dev.flint.music.data.LyricsSource.SERVER }
        val open = tuning || nudgeMs != 0L
        if (lyrics.synced || source != null) androidx.compose.material3.Surface(
            onClick = { if (lyrics.synced) tuning = !tuning },
            enabled = lyrics.synced,
            shape = PillShape,
            color = if (open) MaterialTheme.colorScheme.onSurface.copy(alpha = 0.10f).over(MaterialTheme.colorScheme.background) else Color.Transparent,
            contentColor = MaterialTheme.colorScheme.onSurface,
            modifier = Modifier.align(if (open) Alignment.BottomCenter else Alignment.BottomStart).padding(start = 12.dp, bottom = 6.dp),
        ) {
            Row(Modifier.padding(horizontal = 6.dp, vertical = 2.dp), Arrangement.spacedBy(2.dp), Alignment.CenterVertically) {
                // Closed, this is the one thing worth saying: whose words these are.
                Text(
                    source?.label ?: "Timing", Modifier.padding(horizontal = 8.dp),
                    style = MaterialTheme.typography.labelSmall, color = dim,
                )
                if (open) {
                    if (nudgeMs != 0L) Text("%+.1f s".format(nudgeMs / 1000f), style = MaterialTheme.typography.labelSmall)
                    TextButton({ nudgeMs -= 250 }) { Text("Later", style = MaterialTheme.typography.labelLarge) }
                    TextButton({ nudgeMs += 250 }) { Text("Sooner", style = MaterialTheme.typography.labelLarge) }
                    if (nudgeMs != 0L) TextButton({ nudgeMs = 0L }) { Text("Reset", style = MaterialTheme.typography.labelLarge) }
                }
            }
        }
    }
    }
}

/**
 * Apple's lyrics header: a small rounded thumbnail with the title beside it, so the words get the
 * whole middle of the screen instead of fighting the title block for it.
 */
@Composable
private fun LyricsHeader(vm: PlayerViewModel, actions: ActionsViewModel, song: dev.flint.music.ffi.Song?) {
    val menu = LocalSongMenu.current
    val marks = LocalStarMarks.current
    Row(
        Modifier.fillMaxWidth().padding(start = 20.dp, end = 12.dp, top = 4.dp, bottom = 8.dp),
        Arrangement.spacedBy(12.dp), Alignment.CenterVertically,
    ) {
        Cover(vm.cover(song?.coverArt, CoverSize.ROW), 64.dp, radius = 9.dp)
        Column(Modifier.weight(1f)) {
            Text(
                song?.title ?: "", style = MaterialTheme.typography.titleMedium,
                maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            )
            Text(
                song?.artist ?: "", style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.6f),
                maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            )
        }
        song?.let { s ->
            val starred = marks.effectiveStar(dev.flint.music.data.StarKind.SONG, s.id, s.starred)
            TitleCircle(if (starred) Icons.Filled.Star else Icons.Filled.StarBorder, "Favourite", starred) { actions.star(s, !starred) }
            TitleCircle(Icons.Filled.MoreHoriz, "More", false) { menu(s) }
        }
    }
}

/**
 * The active line: drawn dim, then drawn again bright through a clip that ends where the singing is.
 * [position] is read in the draw phase only, so a new frame redraws this one text and recomposes nothing.
 */
@Composable
private fun SweepLine(line: LyricLine, style: TextStyle, dim: Color, bright: Color, position: () -> Long) {
    var layout by remember(line) { mutableStateOf<TextLayoutResult?>(null) }
    Text(
        line.text, style = style, color = dim, onTextLayout = { layout = it },
        modifier = Modifier.drawWithContent {
            drawContent()
            val l = layout ?: return@drawWithContent
            clipPath(sungRegion(l, sungOffset(line, position()))) { drawText(l, color = bright) }
        },
    )
}

/**
 * How long a line change takes, scroll and colour together so they arrive at the same moment. Long
 * enough that the eye follows the words up rather than losing its place; short enough that a fast
 * verse, a line every second or so, is never still catching up.
 */
private const val LYRIC_GLIDE_MS = 620

/** Ease in and out, soft at both ends, the curve iOS uses for its own scrolling transitions. */
private val LyricEase = androidx.compose.animation.core.CubicBezierEasing(0.25f, 0.1f, 0.25f, 1f)
