package dev.flint.music.app.ui

import androidx.compose.foundation.background
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
import androidx.compose.ui.graphics.vector.ImageVector
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
    // Apple Music floats its chrome: the mini player and the tab bar are two separate rounded slabs with
    // air around them, not a wall across the bottom of the screen. They also take a hint of the colour of
    // whatever page you are on, which is what ties the artwork to the frame around it.
    val tint = currentPalette()
    val slab = tint?.let { blend(it.background, it.edge, 0.16f) } ?: scheme.onSurface.copy(alpha = 0.08f).over(scheme.background)
    val content = tint?.onBackground ?: scheme.onSurface
    Column {
        SelectionBar(actions)
        Box(Modifier.padding(horizontal = 10.dp)) { MiniPlayer(player, onOpenPlayer, slab, content) }
        Surface(
            shape = PillShape, color = slab, shadowElevation = 8.dp,
            modifier = Modifier.padding(start = 10.dp, end = 10.dp, top = 8.dp, bottom = 4.dp),
        ) {
            Row(
                Modifier.fillMaxWidth().padding(horizontal = 6.dp, vertical = 6.dp),
                Arrangement.SpaceEvenly, Alignment.CenterVertically,
            ) { tabs.forEach { t -> TabButton(t, selected = route == t.route, tinted = tint != null) { onTab(t.route) } } }
        }
        Spacer(Modifier.navigationBarsPadding())
    }
}

data class Tab(val route: String, val label: String, val icon: ImageVector)

@Composable
private fun TabButton(tab: Tab, selected: Boolean, tinted: Boolean, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    // The same solid glyph in both states, colour alone saying where you are, and the selected one in a
    // soft pill - swapping outline for solid belongs to Files and Photos, not to a music app. No ripple
    // either: a tab bar answers instantly and silently, and a spreading circle reads as Android.
    val color = if (selected) scheme.primary else scheme.onSurfaceVariant.copy(alpha = if (tinted) 0.9f else 0.8f)
    val press = remember { androidx.compose.foundation.interaction.MutableInteractionSource() }
    Column(
        Modifier.clip(PillShape)
            .background(if (selected) scheme.primary.copy(alpha = 0.16f) else androidx.compose.ui.graphics.Color.Transparent)
            .clickable(interactionSource = press, indication = null, onClick = onClick)
            .padding(horizontal = 14.dp, vertical = 5.dp)
            .semantics { contentDescription = tab.label },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Icon(tab.icon, null, Modifier.size(23.dp), tint = color)
        Text(
            tab.label, Modifier.padding(top = 1.dp),
            style = MaterialTheme.typography.labelSmall.copy(fontSize = 10.sp, letterSpacing = 0.sp),
            color = color, fontWeight = FontWeight.Medium,
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
    NowPlayingPalette(palette)
    Surface(
        shape = CardShape, color = slab, contentColor = content,
        shadowElevation = 6.dp,
        modifier = Modifier.fillMaxWidth()
            .semantics { contentDescription = "Now playing bar" }
            .flingActions(horizontal = true, onStart = vm::previous, onEnd = vm::next)
            .flingActions(horizontal = false, threshold = 0.5f, onEnd = onOpen),
    ) {
        // The tap has to be a child of the drag detectors, not a sibling behind them: a pointerInput
        // waiting for drag slop swallows a tap offered to a clickable further up the same chain.
        Surface(onClick = onOpen, color = androidx.compose.ui.graphics.Color.Transparent) {
        Row(Modifier.fillMaxWidth().padding(start = 8.dp, end = 4.dp, top = 7.dp, bottom = 7.dp), verticalAlignment = Alignment.CenterVertically) {
            Cover(vm.cover(state.current?.coverArt, CoverSize.ROW), 42.dp, radius = 7.dp)
            Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
                Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge)
                Text(
                    state.error ?: state.current?.artist ?: "Radio", maxLines = 1, overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodySmall,
                    color = if (state.error != null) scheme.error else content.copy(alpha = 0.65f),
                )
            }
            if (state.buffering) CircularProgressIndicator(Modifier.size(22.dp), strokeWidth = 2.dp)
            IconButton(vm::toggle) { Icon(if (state.playing) Icons.Filled.Pause else Icons.Filled.PlayArrow, "Play/pause", Modifier.size(26.dp)) }
            IconButton(vm::next) { Icon(Icons.Filled.SkipNext, "Next", Modifier.size(24.dp)) }
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

/** A tinted page (an album, an artist, the player) lends its colours to the chrome while it is open. */
@Composable
fun PageTint(palette: PagePalette?) {
    androidx.compose.runtime.DisposableEffect(palette) {
        pagePalette.value = palette
        onDispose { if (pagePalette.value === palette) pagePalette.value = null }
    }
}

@Composable
private fun currentPalette(): PagePalette? = pagePalette.value ?: nowPlaying.value

@Composable
private fun NowPlayingPalette(palette: PagePalette?) {
    androidx.compose.runtime.LaunchedEffect(palette) { nowPlaying.value = palette }
}
