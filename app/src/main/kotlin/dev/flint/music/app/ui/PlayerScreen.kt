package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Bedtime
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Repeat
import androidx.compose.material.icons.filled.RepeatOne
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material.icons.filled.SkipNext
import androidx.compose.material.icons.filled.SkipPrevious
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.PrimaryTabRow
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.Load
import dev.flint.music.app.vm.PlayerViewModel
import dev.flint.music.playback.Repeat
import kotlinx.coroutines.delay
import kotlinx.coroutines.isActive

@Composable
fun MiniPlayer(vm: PlayerViewModel, onOpen: () -> Unit) {
    val state by vm.state.collectAsStateWithLifecycle()
    val title = state.current?.title ?: state.radio ?: return
    // A real clickable surface, so the whole bar is one labelled target for a screen reader (and for tools/perf-suite.sh).
    Surface(onClick = onOpen, tonalElevation = 3.dp, modifier = Modifier.semantics { contentDescription = "Now playing bar" }) {
        // No progress bar here on purpose: it would tick for as long as the app is open.
        Row(Modifier.fillMaxWidth().padding(start = 12.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Cover(vm.cover(state.current?.coverArt, CoverSize.ROW), 44.dp)
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis)
                Text(state.error ?: state.current?.artist ?: "Radio", maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = if (state.error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
            }
            if (state.buffering) CircularProgressIndicator(Modifier.size(24.dp), strokeWidth = 2.dp)
            IconButton(vm::toggle) { Icon(if (state.playing) Icons.Filled.Pause else Icons.Filled.PlayArrow, "Play/pause") }
            IconButton(vm::next) { Icon(Icons.Filled.SkipNext, "Next") }
        }
    }
}

@Composable
fun PlayerScreen(vm: PlayerViewModel, actions: ActionsViewModel) {
    val state by vm.state.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    val menu = LocalSongMenu.current
    var tab by rememberSaveable { mutableIntStateOf(0) }
    var sleepMenu by remember { mutableStateOf(false) }

    Column(Modifier.fillMaxSize()) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.Filled.KeyboardArrowDown, "Close") }
            PrimaryTabRow(tab, Modifier.weight(1f)) { listOf("Playing", "Queue", "Lyrics").forEachIndexed { i, t -> Tab(tab == i, { tab = i }, text = { Text(t) }) } }
            Box {
                IconButton({ sleepMenu = true }) { Icon(Icons.Filled.Bedtime, "Sleep timer", tint = if (state.sleepAt > 0 || state.sleepAtEndOfTrack) MaterialTheme.colorScheme.primary else LocalContentColor.current) }
                DropdownMenu(sleepMenu, { sleepMenu = false }) {
                    for (m in listOf(15, 30, 45, 60)) DropdownMenuItem({ Text("$m minutes") }, { vm.sleep(m); sleepMenu = false })
                    DropdownMenuItem({ Text("End of track") }, { vm.sleep(0, endOfTrack = true); sleepMenu = false })
                    for (n in listOf(2, 3, 5, 10)) DropdownMenuItem({ Text("After $n songs") }, { vm.sleep(0, songs = n); sleepMenu = false })
                    DropdownMenuItem({ Text("Off") }, { vm.sleep(0); sleepMenu = false })
                }
            }
            state.current?.let { s -> IconButton({ menu(s) }) { Icon(Icons.Filled.MoreVert, "More") } }
        }
        Box(Modifier.weight(1f)) {
            when (tab) {
                0 -> Column(Modifier.fillMaxSize().padding(24.dp), Arrangement.Center, Alignment.CenterHorizontally) {
                    Box(Modifier.fillMaxWidth().aspectRatio(1f)) { Cover(vm.cover(state.current?.coverArt, CoverSize.FULL), 0.dp, Modifier.fillMaxSize()) }
                    Text(state.current?.title ?: state.radio ?: "Nothing playing", Modifier.padding(top = 20.dp), style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(state.current?.let { "${it.artist} · ${it.album}" } ?: "", maxLines = 1, overflow = TextOverflow.Ellipsis, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    state.current?.let { s ->
                        Text(listOfNotNull(s.suffix.uppercase().ifEmpty { null }, s.bitRate.takeIf { it > 0u }?.let { "$it kbps" }, s.samplingRate.takeIf { it > 0u }?.let { "${it.toInt() / 1000.0} kHz" }).joinToString(" · "), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    state.error?.let { Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }
                }
                1 -> Queue(vm)
                2 -> LyricsView(vm, state.playing)
            }
        }
        SeekBar(vm, state.playing, state.durationMs)
        Row(Modifier.fillMaxWidth().padding(bottom = 24.dp), Arrangement.SpaceEvenly, Alignment.CenterVertically) {
            IconButton(vm::toggleShuffle) { Icon(Icons.Filled.Shuffle, "Shuffle", tint = if (state.shuffle) MaterialTheme.colorScheme.primary else LocalContentColor.current) }
            IconButton(vm::previous) { Icon(Icons.Filled.SkipPrevious, "Previous", Modifier.size(36.dp)) }
            FilledIconButton(vm::toggle, Modifier.size(64.dp)) {
                if (state.buffering) CircularProgressIndicator(Modifier.size(28.dp), color = LocalContentColor.current, strokeWidth = 2.dp)
                else Icon(if (state.playing) Icons.Filled.Pause else Icons.Filled.PlayArrow, "Play/pause", Modifier.size(36.dp))
            }
            IconButton(vm::next) { Icon(Icons.Filled.SkipNext, "Next", Modifier.size(36.dp)) }
            IconButton(vm::cycleRepeat) { Icon(if (state.repeat == Repeat.ONE) Icons.Filled.RepeatOne else Icons.Filled.Repeat, "Repeat", tint = if (state.repeat != Repeat.OFF) MaterialTheme.colorScheme.primary else LocalContentColor.current) }
        }
    }
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

@Composable
private fun SeekBar(vm: PlayerViewModel, playing: Boolean, durationMs: Long) {
    val pos = position(vm, playing, 1000)
    var dragging by remember { mutableStateOf(false) }
    var drag by remember { mutableFloatStateOf(0f) }
    val d = durationMs.coerceAtLeast(1)
    Column(Modifier.padding(horizontal = 24.dp)) {
        Slider(
            value = if (dragging) drag else (pos.toFloat() / d).coerceIn(0f, 1f), enabled = durationMs > 0,
            onValueChange = { dragging = true; drag = it }, onValueChangeFinished = { vm.seekTo((drag * d).toLong()); dragging = false },
        )
        Row(Modifier.fillMaxWidth(), Arrangement.SpaceBetween) {
            Text(duration((if (dragging) (drag * d).toLong() else pos) / 1000), style = MaterialTheme.typography.labelSmall)
            Text(duration(durationMs / 1000), style = MaterialTheme.typography.labelSmall)
        }
    }
}

@Composable
private fun Queue(vm: PlayerViewModel) {
    val state by vm.state.collectAsStateWithLifecycle()
    val list = rememberLazyListState(initialFirstVisibleItemIndex = state.index.coerceAtLeast(0))
    LazyColumn(state = list) {
        itemsIndexed(state.queue, key = { i, s -> "$i-${s.id}" }, contentType = { _, _ -> "song" }) { i, s ->
            Row(Modifier.fillMaxWidth().clickable { vm.skipTo(i) }.padding(start = 16.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Cover(vm.cover(s.coverArt, CoverSize.ROW), 40.dp)
                Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                    Text(s.title, maxLines = 1, overflow = TextOverflow.Ellipsis, color = if (i == state.index) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface)
                    Text(s.artist, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                IconButton({ vm.remove(i) }) { Icon(Icons.Filled.Close, "Remove") }
            }
        }
    }
}

