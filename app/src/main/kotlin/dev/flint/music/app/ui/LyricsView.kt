package dev.flint.music.app.ui

import androidx.compose.animation.togetherWith
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
import androidx.compose.foundation.interaction.collectIsDraggedAsState
import androidx.compose.ui.Modifier
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
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
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.FavoriteBorder
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
    val shown = LocalPlayerShown.current
    if (prefs.lyricsKeepScreenOn && shown) { val view = LocalView.current; DisposableEffect(playing) { view.keepScreenOn = playing; onDispose { view.keepScreenOn = false } } }
    Column(Modifier.fillMaxSize()) {
        LyricsHeader(vm, actions, playerState.current)
        // Loading, nothing found, or the words - each fades into the next rather than replacing it, the
        // words included: they rise out of the loader instead of appearing in one frame. A new song goes
        // back through the loader, so the last song's lyrics never sit on screen under the new title.
        val found = (load as? Load.Ready)?.data?.takeIf { it.lyrics.lines.isNotEmpty() }
        val phase: Any = found ?: if (load is Load.Loading) LyricsPhase.LOADING else LyricsPhase.NONE
        androidx.compose.animation.AnimatedContent(
            phase, Modifier.fillMaxSize().weight(1f),
            transitionSpec = {
                (androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(380, delayMillis = 80)) +
                    androidx.compose.animation.slideInVertically(androidx.compose.animation.core.tween(420, delayMillis = 80)) { it / 40 }) togetherWith
                    androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(200))
            },
            label = "lyrics",
        ) { p ->
            when (p) {
                is dev.flint.music.data.FoundLyrics -> LyricsBody(vm, p, playing)
                // In the middle, where "No lyrics" would be: it is waiting, not a line of words yet.
                LyricsPhase.LOADING -> Box(Modifier.fillMaxSize(), Alignment.Center) { LoadingDots(dot = 9.dp) }
                else -> Box(Modifier.fillMaxSize(), Alignment.Center) { Text("No lyrics", color = MaterialTheme.colorScheme.onSurfaceVariant) }
            }
        }
    }
}

private enum class LyricsPhase { LOADING, NONE }

/** How long the words stay where a finger left them before they come back to the song. */
private const val READING_MS = 4_000L

/** The words of one song, in time with it. */
@Composable
private fun LyricsBody(vm: PlayerViewModel, found: dev.flint.music.data.FoundLyrics, playing: Boolean) {
    val lyrics = found.lyrics
    val settings: SettingsViewModel = viewModel()
    val prefs by settings.prefs.collectAsStateWithLifecycle()
    val shown = LocalPlayerShown.current
    var nudgeMs by remember(lyrics) { mutableLongStateOf(0L) }
    var resumed by remember { mutableStateOf(false) }
    LifecycleResumeEffect(Unit) { resumed = true; onPauseOrDispose { resumed = false } }
    // Put away with the player is the same as paused: nothing to keep in step.
    val live = resumed && shown
    // Only sweep when the lyrics actually carry per-word times (enhanced LRC, or a server's structured
    // cues). Spreading a line's duration across its words by length looks right for a beat and then
    // drifts badly on a held note or a fast line, which reads as broken sync - a line at a time is
    // honest and stays in step.
    val sweep = prefs.lyricsSweep && lyrics.synced && lyrics.wordTimed
    // The playhead as the lyrics see it. With the sweep on it is read once per frame (a local computation in the
    // controller, no IPC) and only the draw phase of the active line looks at it; otherwise three times a second.
    var now by remember(lyrics) { mutableLongStateOf(vm.positionMs) }
    // Each line's change is as long as the line allows and is centred on the moment it is sung, so the line
    // is fully lit as the singing starts rather than a third of a second after. A fixed 620 ms glide that
    // began on the timestamp both lagged every line and, on a verse faster than that, never finished - the
    // next change always arrived first. `switchAt` is when a line takes over: its timestamp less half its
    // own change.
    val timing = remember(lyrics) { LyricTiming.of(lyrics.lines.map { it.startMs.toLong() }) }
    // Keyed on the lyrics themselves: on the next song this loop must time the new lines, not the old.
    LaunchedEffect(playing, live, sweep, lyrics) {
        now = vm.positionMs + nudgeMs
        if (!lyrics.synced) return@LaunchedEffect
        var frame = 0
        while (playing && live && isActive) {
            // Every second display frame is plenty for a text fill, and half the redraws.
            if (sweep) { withFrameMillis { }; if (++frame % 2 == 1) continue } else {
                // Sleep until the next line takes over and wake once, instead of looking three times a
                // second and changing up to 300 ms late. Capped, so a seek is picked up within half a second.
                val t0 = vm.positionMs + nudgeMs
                val next = timing.nextSwitchAfter(t0)
                delay(if (next == null) 500L else (next - t0).coerceIn(8L, 500L))
            }
            val t = vm.positionMs + nudgeMs
            // Between words, and while a held note keeps the boundary still, nothing on screen changes: do not invalidate.
            val line = lyrics.lines.getOrNull(lyrics.lines.indexOfLast { it.startMs <= t })
            if (!sweep || line == null || lyrics.lines.indexOfLast { it.startMs <= now } != lyrics.lines.indexOfLast { it.startMs <= t } || kotlin.math.abs(sungOffset(line, t) - sungOffset(line, now)) >= 0.04f) now = t
        }
    }
    val active by remember(lyrics) { derivedStateOf { if (lyrics.synced) timing.lineAt(now) else -1 } }
    // How long the change into the current line takes; the scroll and every line's fade use the same.
    val glideMs = if (active >= 0) timing.glideMs(active) else LYRIC_GLIDE_MS

    val style = when (prefs.lyricsSize) { 0 -> MaterialTheme.typography.titleMedium; 2 -> MaterialTheme.typography.headlineMedium; else -> MaterialTheme.typography.headlineSmall }
    val bright = MaterialTheme.colorScheme.onSurface
    val dim = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.35f)
    val page = MaterialTheme.colorScheme.background
    val list = rememberLazyListState()
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val third = with(LocalDensity.current) { (maxHeight / 3).roundToPx() }
        // The line being sung rests a third of the way down, and the list glides there. It used to call
        // animateScrollToItem, whose default spring is so stiff that over one line's distance it is
        // done in two or three frames - on screen, a teleport. This is a measured ease instead, slow
        // enough to follow with the eye, the way Apple's lyrics move. A line that is not on screen at
        // all (a seek, the first line after opening) is still jumped to: gliding past a whole song's
        // worth of words is not a transition, it is a wait.
        val plain = reduceMotion()
        // Reading ahead, or back, is allowed: while a finger is on the words nothing scrolls them, and
        // for a few seconds after it lifts they stay where they were put. Then the lyrics come back to
        // the line being sung - gliding, not jumping, because this one is a return to the song rather
        // than a seek, and a jump here would look like the list had been snatched away.
        val dragged by list.interactionSource.collectIsDraggedAsState()
        var follow by remember(lyrics) { mutableStateOf(true) }
        var returning by remember(lyrics) { mutableStateOf(false) }
        LaunchedEffect(dragged) {
            if (dragged) { follow = false; returning = false } else if (!follow) {
                delay(READING_MS)
                returning = true
                follow = true
            }
        }
        // Only the scroll lives here; each line's brightness is its own (below). A scroll cut short by the
        // next line is simply continued from wherever the list is, so it cannot jump either.
        LaunchedEffect(active, plain, lyrics, follow) {
            if (!follow) return@LaunchedEffect
            // Before the first line (a new song, an intro), or lyrics that are not timed: back to the top.
            // The list outlives a song, so without this the next song opened where the last one ended.
            if (active < 0) { if (list.firstVisibleItemIndex != 0 || list.firstVisibleItemScrollOffset != 0) list.scrollToItem(0); return@LaunchedEffect }
            val here = list.layoutInfo.visibleItemsInfo.firstOrNull { it.index == active }
            if (returning) {
                returning = false
                if (plain) list.scrollToItem(active, -third) else list.animateScrollToItem(active, -third)
                return@LaunchedEffect
            }
            if (plain || here == null) { list.scrollToItem(active, -third); return@LaunchedEffect }
            // scrollToItem(active, -third) would leave the line at offset `third`; glide by the difference.
            val distance = (here.offset - third).toFloat()
            if (kotlin.math.abs(distance) < 1f) return@LaunchedEffect
            list.animateScrollBy(distance, androidx.compose.animation.core.tween(glideMs, easing = LyricEase))
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
                Column(Modifier.fillMaxWidth().clickable(enabled = lyrics.synced) { vm.seekTo((line.startMs - nudgeMs).coerceAtLeast(0)); now = line.startMs.toLong() }.padding(vertical = 8.dp)) {
                    val weight = if (line.background) FontWeight.Normal else FontWeight.SemiBold
                    // Every line owns its brightness and always moves it *from wherever it is now*
                    // towards what it should be. An earlier version drove all lines from one shared
                    // old-line/new-line blend, and when the next line arrived before a blend had
                    // finished it restarted from a line two changes back - one frame of the wrong
                    // line fully lit. Nothing here can jump: a change mid-way just turns it round.
                    val target = if (!lyrics.synced || i == active) 1f else if (i < active) PAST_LINE else NEXT_LINE
                    val strength = remember { Animatable(target) }
                    LaunchedEffect(target, plain) {
                        if (plain) strength.snapTo(target)
                        else strength.animateTo(target, androidx.compose.animation.core.tween(glideMs, easing = LyricEase))
                    }
                    when {
                        !lyrics.synced -> Text(line.text, style = style, color = bright)
                        i == active && sweep -> SweepLine(line, style.copy(fontWeight = weight), dim, bright) { now }
                        else -> {
                            Text(
                                line.text, style = style.copy(fontWeight = weight), color = bright,
                                // Read in the draw phase: a fading line redraws, it does not recompose.
                                modifier = Modifier.graphicsLayer { alpha = strength.value },
                            )
                        }
                    }
                    // The translation follows its line's fade rather than switching on the moment the line
                    // is reached - otherwise it lit up a frame ahead of the words above it.
                    if (prefs.lyricsTranslation) line.translation?.let {
                        Text(it, style = MaterialTheme.typography.bodyMedium, color = bright.copy(alpha = 0.8f), modifier = Modifier.graphicsLayer { alpha = strength.value })
                    }
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
                // Closed, this is the one thing worth saying: whose words these are - and, when they
                // came without timings, that they did. Unsung words are all one brightness and a tap on
                // one goes nowhere, which looks broken unless the corner says why.
                Text(
                    listOfNotNull(source?.label ?: "Timing".takeIf { lyrics.synced }, "not timed".takeIf { !lyrics.synced })
                        .joinToString(" · "),
                    Modifier.padding(horizontal = 8.dp),
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
        // The player is put away from here as well as from the artwork, and the cover has to have
        // somewhere to fly from: this thumbnail is it. Measured only with the sheet fully open, so the
        // rectangle is in the sheet's own coordinates and does not move while the sheet does.
        val sheet = LocalPlayerSheet.current
        Box(
            Modifier.onGloballyPositioned { if (sheet.progress.value >= 0.999f) sheet.panelCover = it.boundsInRoot() }
                // While it is in flight it is the flying copy that is drawn, not this one.
                .graphicsLayer { alpha = if (sheet.progress.value >= 1f || sheet.miniCover == Rect.Zero) 1f else 0f },
        ) {
            Cover(vm.cover(song?.coverArt, CoverSize.ROW), 64.dp, radius = 9.dp)
        }
        androidx.compose.runtime.DisposableEffect(sheet) { onDispose { sheet.panelCover = androidx.compose.ui.geometry.Rect.Zero } }
        Column(Modifier.weight(1f)) {
            Text(
                song?.title ?: "", Modifier.readable(), style = MaterialTheme.typography.titleMedium,
                maxLines = 1, softWrap = false, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            )
            Text(
                song?.artist ?: "", style = MaterialTheme.typography.bodySmall,
                color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.6f),
                maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
            )
        }
        song?.let { s ->
            val starred = marks.effectiveStar(dev.flint.music.data.StarKind.SONG, s.id, s.starred)
            TitleCircle(if (starred) Icons.Filled.Favorite else Icons.Filled.FavoriteBorder, "Favourite", starred) { actions.star(s, !starred) }
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

/** How lit a line is once it has been sung, and before it is reached. */
private const val PAST_LINE = 0.35f * 0.55f
private const val NEXT_LINE = 0.35f

/**
 * When each lyric line takes over and how long the change into it lasts. The change is 85 % of the gap
 * to the next line, between [MIN_GLIDE_MS] and [LYRIC_GLIDE_MS], so a fast verse gets quick changes
 * that finish before the next one; and it starts half its length before the line's timestamp, so it is
 * centred on the moment the line is sung.
 */
private class LyricTiming(private val switchAt: LongArray, private val glide: IntArray) {
    /** The line that should be lit at playhead [t], or -1 before the first. */
    fun lineAt(t: Long): Int {
        var lo = 0; var hi = switchAt.size - 1; var best = -1
        while (lo <= hi) { val mid = (lo + hi) ushr 1; if (switchAt[mid] <= t) { best = mid; lo = mid + 1 } else hi = mid - 1 }
        return best
    }
    fun nextSwitchAfter(t: Long): Long? { val i = lineAt(t) + 1; return if (i < switchAt.size) switchAt[i] else null }
    fun glideMs(line: Int): Int = glide.getOrElse(line) { LYRIC_GLIDE_MS }

    companion object {
        fun of(starts: List<Long>): LyricTiming {
            val n = starts.size
            val glide = IntArray(n) { i ->
                val gap = if (i + 1 < n) starts[i + 1] - starts[i] else Long.MAX_VALUE
                (gap.coerceAtMost(10_000L) * 0.85).toInt().coerceIn(MIN_GLIDE_MS, LYRIC_GLIDE_MS)
            }
            // A line may not take over before the one ahead of it has, however short its own gap.
            val switchAt = LongArray(n)
            for (i in 0 until n) {
                val lead = glide[i] / 2
                switchAt[i] = if (i == 0) starts[i] - lead else maxOf(starts[i] - lead, switchAt[i - 1] + 1)
            }
            return LyricTiming(switchAt, glide)
        }
    }
}

/** The shortest a line change gets, on the fastest verse. Below this it stops reading as movement. */
private const val MIN_GLIDE_MS = 160
