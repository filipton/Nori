package dev.flint.music.app.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.composed
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.unit.IntOffset
import dev.flint.music.app.vm.ActionsViewModel
import kotlinx.coroutines.launch
import kotlin.math.roundToInt
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CloudDownload
import androidx.compose.material.icons.filled.DownloadDone
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.MoreVert
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.runtime.remember
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import coil3.request.ImageRequest
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import coil3.compose.AsyncImage
import dev.flint.music.app.vm.Load
import dev.flint.music.ffi.Album
import dev.flint.music.ffi.Song

/** Cover sizes are bucketed so the server, the HTTP cache and the image cache all see few distinct URLs. */
object CoverSize { const val ROW = 160; const val CARD = 320; const val FULL = 800 }

/**
 * Square on purpose: a rounded clip gives every cover its own GPU layer, and a grid has dozens on screen.
 * The request is remembered and sized up front, so scrolling neither rebuilds it nor waits for layout to size it.
 */
@Composable
fun Cover(url: String?, size: Dp, modifier: Modifier = Modifier) {
    val context = LocalContext.current
    val px = with(LocalDensity.current) { size.roundToPx() }
    val request = remember(url, px) {
        ImageRequest.Builder(context).data(url).apply { if (px > 0) size(px) }.build()
    }
    AsyncImage(
        model = request, contentDescription = null, contentScale = ContentScale.Crop, filterQuality = FilterQuality.Low,
        modifier = (if (px > 0) modifier.size(size) else modifier).background(MaterialTheme.colorScheme.surfaceVariant),
    )
}

fun duration(seconds: Long): String = if (seconds >= 3600) "%d:%02d:%02d".format(seconds / 3600, seconds / 60 % 60, seconds % 60) else "%d:%02d".format(seconds / 60, seconds % 60)

/**
 * Sideways drag on a row: past a third of its width the action fires and the row springs back.
 * Nothing is allocated for rows that are never touched beyond one Animatable.
 */
fun Modifier.swipeActions(enabled: Boolean, onSwipe: (right: Boolean) -> Unit): Modifier = if (!enabled) this else composed {
    val offset = remember { Animatable(0f) }
    val scope = rememberCoroutineScope()
    pointerInput(Unit) {
        detectHorizontalDragGestures(
            onDragEnd = {
                val fired = kotlin.math.abs(offset.value) > size.width / 3f
                if (fired) onSwipe(offset.value > 0)
                scope.launch { offset.animateTo(0f) }
            },
            onDragCancel = { scope.launch { offset.animateTo(0f) } },
        ) { _, delta -> scope.launch { offset.snapTo((offset.value + delta).coerceIn(-size.width / 2f, size.width / 2f)) } }
    }.offset { IntOffset(offset.value.roundToInt(), 0) }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
fun SongRow(
    song: Song, coverUrl: String?, onClick: () -> Unit, onMenu: () -> Unit, modifier: Modifier = Modifier,
    number: Int? = null, playing: Boolean = false, downloaded: Boolean = false,
    selected: Boolean = false, onLongClick: (() -> Unit)? = null, onSwipe: ((Boolean) -> Unit)? = null,
) {
    Row(
        modifier.fillMaxWidth()
            .swipeActions(onSwipe != null) { onSwipe?.invoke(it) }
            .background(if (selected) MaterialTheme.colorScheme.secondaryContainer else androidx.compose.ui.graphics.Color.Transparent)
            .combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(start = 16.dp, top = 6.dp, bottom = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        if (number != null) Text(if (number > 0) "$number" else "", Modifier.width(32.dp), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        else Cover(coverUrl, 48.dp)
        Column(Modifier.weight(1f).padding(horizontal = 12.dp)) {
            Text(song.title, maxLines = 1, overflow = TextOverflow.Ellipsis, color = if (playing) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface)
            Text((if (song.explicitStatus == "explicit") "🅴 " else "") + song.artist, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        val tint = MaterialTheme.colorScheme.onSurfaceVariant
        if (song.isExternal) Icon(Icons.Filled.CloudDownload, "Not in library yet", Modifier.size(16.dp), tint)
        if (downloaded) Icon(Icons.Filled.DownloadDone, "Downloaded", Modifier.size(16.dp), tint)
        if (song.starred) Icon(Icons.Filled.Favorite, "Favourite", Modifier.padding(start = 4.dp).size(16.dp), tint)
        if (song.duration > 0u) Text(duration(song.duration.toLong()), Modifier.padding(start = 8.dp), style = MaterialTheme.typography.bodySmall, color = tint)
        IconButton(onMenu) { Icon(Icons.Filled.MoreVert, "More") }
    }
}

/** A song list wired to the configured tap, swipe and selection behaviour; every screen that lists songs uses this. */
fun LazyListScope.songRows(
    songs: List<Song>, actions: ActionsViewModel, playingId: String?, downloaded: Set<String>, selected: Set<String>, menu: (Song) -> Unit,
    numbered: Boolean = false, cover: (Song) -> String? = { null }, keyPrefix: String = "",
    /** The list a tap plays from, when [songs] is only a slice of it (one disc of an album, a filtered view). */
    context: List<Song> = songs,
) {
    val swipe = actions.swipeEnabled
    itemsIndexed(songs, key = { i, s -> "$keyPrefix$i-${s.id}" }, contentType = { _, _ -> "song" }) { i, s ->
        SongRow(
            s, if (numbered) null else cover(s), onClick = { if (context === songs) actions.tap(songs, i) else actions.tap(context, context.indexOfFirst { it.id == s.id }.coerceAtLeast(0)) }, onMenu = { menu(s) },
            number = if (numbered) s.track.toInt() else null, playing = s.id == playingId, downloaded = s.id in downloaded,
            selected = s.id in selected, onLongClick = { actions.toggleSelected(s) }, onSwipe = if (swipe) ({ right -> actions.swipe(s, right) }) else null,
        )
    }
}

@Composable
fun CoverCard(title: String, subtitle: String, coverUrl: String?, size: Dp, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.width(size).clickable(onClick = onClick)) {
        Cover(coverUrl, size)
        Text(title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(top = 4.dp))
        Text(subtitle, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
}

@Composable
fun AlbumCard(album: Album, coverUrl: String?, size: Dp, onClick: () -> Unit, modifier: Modifier = Modifier) =
    CoverCard(album.name, if (album.year > 0u && album.artist.isNotEmpty()) "${album.artist} · ${album.year}" else album.artist, coverUrl, size, onClick, modifier)

@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier) {
    Text(text, modifier.padding(horizontal = 16.dp, vertical = 8.dp), style = MaterialTheme.typography.titleMedium)
}

@Composable
fun <T> LoadBox(load: Load<T>, modifier: Modifier = Modifier, content: @Composable (T) -> Unit) {
    when (load) {
        is Load.Ready -> content(load.data)
        is Load.Loading -> Box(modifier.fillMaxSize(), Alignment.Center) { CircularProgressIndicator() }
        is Load.Failed -> Column(modifier.fillMaxSize().padding(24.dp), Arrangement.Center, Alignment.CenterHorizontally) {
            Text("Could not load", style = MaterialTheme.typography.titleMedium)
            Text(load.message, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}
