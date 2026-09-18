package dev.flint.music.app.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.CloudDownload
import androidx.compose.material.icons.filled.DownloadDone
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.MoreHoriz
import androidx.compose.material.icons.filled.MusicNote
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.composed
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import coil3.compose.AsyncImage
import coil3.request.CachePolicy
import coil3.request.ImageRequest
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.Load
import dev.flint.music.ffi.Album
import dev.flint.music.ffi.Song
import kotlinx.coroutines.launch
import kotlin.math.roundToInt

/** A cover of an octo-fiesta provider item (external song, album, artist or playlist). */
fun isProviderCover(url: String) = url.contains("&id=ext-") || url.contains("&id=pl-")

/** "ext-deezer-song-123" -> "Deezer": which service an octo-fiesta item comes from. */
fun providerOf(id: String): String? = id.takeIf { it.startsWith("ext-") || it.startsWith("pl-") }
    ?.split('-')?.getOrNull(1)?.replaceFirstChar(Char::uppercase)
    ?.let { mapOf("Squidwtf" to "SquidWTF").getOrDefault(it, it) }

/** Cover sizes are bucketed so the server, the HTTP cache and the image cache all see few distinct URLs. */
object CoverSize { const val ROW = 160; const val CARD = 320; const val FULL = 800 }

/**
 * Artwork with the app's corner radius. The request is remembered and sized up front, so scrolling
 * neither rebuilds it nor waits for layout to size it; the rounded clip is a plain render-node clip,
 * which the GPU does for free and which a grid of covers needs to not look like a spreadsheet.
 * Pass `radius = 0.dp` for the full-bleed artwork at the top of a page.
 */
@Composable
fun Cover(url: String?, size: Dp, modifier: Modifier = Modifier, radius: Dp = Radius.cover) {
    val context = LocalContext.current
    val px = with(LocalDensity.current) { size.roundToPx() }
    val request = remember(url, px) {
        ImageRequest.Builder(context).data(url).apply {
            if (px > 0) size(px)
            // octo-fiesta draws a "not downloaded" badge on provider covers and replaces the picture once the
            // track is in the library, under the same id. Never store those, or the badge sticks forever.
            if (url != null && isProviderCover(url)) { diskCachePolicy(CachePolicy.DISABLED); memoryCachePolicy(CachePolicy.READ_ONLY) }
        }.build()
    }
    val shape = remember(radius) { androidx.compose.foundation.shape.RoundedCornerShape(radius) }
    val scheme = MaterialTheme.colorScheme
    // A flat grey square is what makes a library of half-loaded covers look broken. Underneath every
    // cover sits a soft two-tone plate with a note on it, which is what shows while the picture loads
    // and what stays when a track simply has no artwork. It is one gradient, drawn, and costs nothing.
    val plate = remember(scheme.surfaceVariant) {
        Brush.linearGradient(listOf(scheme.onSurface.copy(alpha = 0.13f).over(scheme.background), scheme.onSurface.copy(alpha = 0.06f).over(scheme.background)))
    }
    Box((if (px > 0) modifier.size(size) else modifier).then(if (radius > 0.dp) Modifier.clip(shape) else Modifier).background(plate)) {
        if (url == null) Icon(
            Icons.Filled.MusicNote, null,
            Modifier.align(Alignment.Center).size(if (size > 0.dp) size * 0.34f else 40.dp),
            tint = scheme.onSurface.copy(alpha = 0.22f),
        )
        AsyncImage(
            model = request, contentDescription = null, contentScale = ContentScale.Crop, filterQuality = FilterQuality.Low,
            modifier = Modifier.fillMaxSize(),
        )
    }
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

/**
 * One track. Numbered rows (an album) carry no artwork; everywhere else the cover leads. The row ends
 * in a hairline that starts where the text starts, which is what keeps a long list from reading as a
 * stack of boxes.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun SongRow(
    song: Song, coverUrl: String?, onClick: () -> Unit, onMenu: () -> Unit, modifier: Modifier = Modifier,
    number: Int? = null, playing: Boolean = false, downloaded: Boolean = false,
    selected: Boolean = false, onLongClick: (() -> Unit)? = null, onSwipe: ((Boolean) -> Unit)? = null,
    divider: Boolean = true,
) {
    val scheme = MaterialTheme.colorScheme
    Column(modifier.fillMaxWidth().background(if (selected) scheme.secondaryContainer else Color.Transparent)) {
        Row(
            Modifier.fillMaxWidth()
                .swipeActions(onSwipe != null) { onSwipe?.invoke(it) }
                .combinedClickable(onClick = onClick, onLongClick = onLongClick)
                .padding(start = Space.gutter, top = 9.dp, bottom = 9.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (number != null) Text(
                if (number > 0) "$number" else "", Modifier.width(26.dp), textAlign = TextAlign.Center,
                style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant,
            ) else Cover(coverUrl, 46.dp, radius = 6.dp)
            Column(Modifier.weight(1f).padding(start = if (number != null) 14.dp else 12.dp, end = 8.dp)) {
                Text(
                    song.title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyLarge,
                    color = if (playing) scheme.primary else scheme.onSurface,
                )
                Text(
                    (if (song.explicitStatus == "explicit") "🅴 " else "") + song.artist,
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                    style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant,
                )
            }
            val tint = scheme.onSurfaceVariant
            if (song.isExternal) {
                Icon(Icons.Filled.CloudDownload, "Not in library yet", Modifier.size(15.dp), tint)
                providerOf(song.id)?.let { Text(it, Modifier.padding(start = 3.dp), style = MaterialTheme.typography.labelSmall, color = tint) }
            }
            if (downloaded) Icon(Icons.Filled.DownloadDone, "Downloaded", Modifier.size(15.dp), tint)
            if (song.starred) Icon(Icons.Filled.Favorite, "Favourite", Modifier.padding(start = 4.dp).size(15.dp), tint)
            if (song.duration > 0u) Text(duration(song.duration.toLong()), Modifier.padding(start = 8.dp), style = MaterialTheme.typography.bodySmall, color = tint)
            IconButton(onMenu, Modifier.size(40.dp)) { Icon(Icons.Filled.MoreHoriz, "More", Modifier.size(20.dp), tint) }
        }
        if (divider) Hairline(startIndent = if (number != null) Space.gutter + 40.dp else Space.gutter + 58.dp)
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
            divider = i < songs.lastIndex,
        )
    }
}

/** A cover with its title under it: the tile every shelf and grid is made of. */
@Composable
fun CoverCard(title: String, subtitle: String, coverUrl: String?, size: Dp, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.width(size).clickable(onClick = onClick)) {
        Cover(coverUrl, size, radius = Radius.card)
        Text(
            title, maxLines = 1, overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.bodyMedium.copy(fontSize = 14.sp, fontWeight = FontWeight.Medium),
            modifier = Modifier.padding(top = 8.dp),
        )
        if (subtitle.isNotEmpty()) Text(
            subtitle, maxLines = 1, overflow = TextOverflow.Ellipsis,
            style = MaterialTheme.typography.bodySmall.copy(fontSize = 12.5f.sp), color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

/** A round portrait, for artists. */
@Composable
fun ArtistCard(name: String, subtitle: String, coverUrl: String?, size: Dp, onClick: () -> Unit, modifier: Modifier = Modifier) {
    Column(modifier.width(size).clickable(onClick = onClick), horizontalAlignment = Alignment.CenterHorizontally) {
        Cover(coverUrl, size, radius = size / 2)
        Text(
            name, maxLines = 1, overflow = TextOverflow.Ellipsis, textAlign = TextAlign.Center,
            style = MaterialTheme.typography.bodyMedium, modifier = Modifier.padding(top = 7.dp),
        )
        if (subtitle.isNotEmpty()) Text(
            subtitle, maxLines = 1, overflow = TextOverflow.Ellipsis, textAlign = TextAlign.Center,
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
fun AlbumCard(album: Album, coverUrl: String?, size: Dp, onClick: () -> Unit, modifier: Modifier = Modifier) =
    CoverCard(
        album.name,
        listOfNotNull(album.artist.ifEmpty { null }, album.year.takeIf { it > 0u }?.toString(), providerOf(album.id)?.let { "☁ $it" }).joinToString(" · "),
        coverUrl, size, onClick, modifier,
    )

@Composable
fun SectionTitle(text: String, modifier: Modifier = Modifier) = SectionHeader(text, modifier)

@Composable
fun <T> LoadBox(load: Load<T>, modifier: Modifier = Modifier, content: @Composable (T) -> Unit) {
    when (load) {
        is Load.Ready -> content(load.data)
        is Load.Loading -> Box(modifier.fillMaxSize(), Alignment.Center) { CircularProgressIndicator(strokeWidth = 2.dp) }
        is Load.Failed -> Column(modifier.fillMaxSize().padding(Space.gutter), Arrangement.Center, Alignment.CenterHorizontally) {
            Text("Could not load", style = MaterialTheme.typography.titleLarge)
            Text(load.message, Modifier.padding(top = 4.dp), color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
        }
    }
}

/** Big, quiet type for an empty list: "Nothing here yet". */
@Composable
fun EmptyNote(text: String, modifier: Modifier = Modifier) = Text(
    text, modifier.fillMaxWidth().padding(Space.gutter), textAlign = TextAlign.Center,
    style = MaterialTheme.typography.bodyMedium.copy(fontWeight = FontWeight.Medium), color = MaterialTheme.colorScheme.onSurfaceVariant,
)
