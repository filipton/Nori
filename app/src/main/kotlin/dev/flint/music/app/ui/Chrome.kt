package dev.flint.music.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
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
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Pause
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.SkipNext
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.PlayerViewModel

/**
 * The chrome that never leaves: what is playing, and where to go. Apple Music stacks them into one
 * floating slab with a rounded top and a hairline between the two halves, and the page scrolls
 * underneath it. That is what this is - one surface, two rows, no boxes and no Material indicator pill.
 */
@Composable
fun BottomChrome(player: PlayerViewModel, actions: ActionsViewModel, route: String?, tabs: List<Tab>, onTab: (String) -> Unit, onOpenPlayer: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Column {
        SelectionBar(actions)
        Surface(
            color = scheme.onSurface.copy(alpha = 0.06f).over(scheme.background),
            shape = RoundedCornerShape(topStart = Radius.tile, topEnd = Radius.tile),
            modifier = Modifier.fillMaxWidth(),
        ) {
            Column {
                MiniPlayer(player, onOpenPlayer)
                Row(
                    Modifier.fillMaxWidth().navigationBarsPadding().padding(top = 6.dp, bottom = 4.dp),
                    Arrangement.SpaceEvenly, Alignment.CenterVertically,
                ) { tabs.forEach { t -> TabButton(t, selected = route == t.route) { onTab(t.route) } } }
            }
        }
    }
}

data class Tab(val route: String, val label: String, val icon: ImageVector)

@Composable
private fun TabButton(tab: Tab, selected: Boolean, onClick: () -> Unit) {
    val color = if (selected) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant
    Column(
        Modifier.clickable(onClick = onClick).padding(horizontal = 18.dp, vertical = 4.dp).semantics { contentDescription = tab.label },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(tab.icon, null, Modifier.size(23.dp), tint = color)
        Text(tab.label, Modifier.padding(top = 2.dp), style = MaterialTheme.typography.labelSmall, color = color)
    }
}

/**
 * The bar above the tabs: artwork, what is playing, and the two controls a thumb wants. Sideways
 * flings skip, an upward fling opens the player - the same gestures the full screen answers to.
 * No progress bar on purpose: it would tick for as long as the app is open.
 */
@Composable
fun MiniPlayer(vm: PlayerViewModel, onOpen: () -> Unit) {
    val state by vm.state.collectAsStateWithLifecycle()
    val title = state.current?.title ?: state.radio ?: return
    val scheme = MaterialTheme.colorScheme
    Column(
        Modifier.fillMaxWidth()
            .semantics { contentDescription = "Now playing bar" }
            .flingActions(horizontal = true, onStart = vm::previous, onEnd = vm::next)
            .flingActions(horizontal = false, threshold = 0.5f, onEnd = onOpen),
    ) {
        // The tap has to be a child of the drag detectors, not a sibling behind them: a pointerInput
        // waiting for drag slop swallows a tap offered to a clickable further up the same chain.
        Surface(onClick = onOpen, color = androidx.compose.ui.graphics.Color.Transparent) {
        Row(Modifier.fillMaxWidth().padding(start = 12.dp, end = 4.dp, top = 8.dp, bottom = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Cover(vm.cover(state.current?.coverArt, CoverSize.ROW), 44.dp, radius = 6.dp)
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge)
                Text(
                    state.error ?: state.current?.artist ?: "Radio", maxLines = 1, overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodySmall,
                    color = if (state.error != null) scheme.error else scheme.onSurfaceVariant,
                )
            }
            if (state.buffering) CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
            IconButton(vm::toggle) { Icon(if (state.playing) Icons.Filled.Pause else Icons.Filled.PlayArrow, "Play/pause", Modifier.size(26.dp)) }
            IconButton(vm::next) { Icon(Icons.Filled.SkipNext, "Next", Modifier.size(24.dp)) }
        }
        }
        Hairline(startIndent = 12.dp)
    }
}
