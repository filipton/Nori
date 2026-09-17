package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.PlaylistAdd
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.FavoriteBorder
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.PushPin
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalUriHandler
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.AlbumViewModel
import dev.flint.music.app.vm.ArtistViewModel
import dev.flint.music.app.vm.FolderViewModel
import dev.flint.music.app.vm.GenreViewModel
import dev.flint.music.app.vm.PlayerViewModel
import dev.flint.music.app.vm.PlaylistViewModel
import dev.flint.music.app.vm.SettingsViewModel
import dev.flint.music.ffi.Album
import dev.flint.music.ffi.Song

@Composable
private fun Header(title: String, subtitle: String, coverUrl: String?, actions: @Composable () -> Unit = {}) {
    val nav = LocalNav.current
    var fullscreen by remember { mutableStateOf(false) }
    if (fullscreen && coverUrl != null) androidx.compose.ui.window.Dialog({ fullscreen = false }) { Cover(coverUrl, 0.dp, Modifier.fillMaxWidth().clickable { fullscreen = false }) }
    Column {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            actions()
        }
        if (coverUrl != null) Cover(coverUrl, 220.dp, Modifier.padding(16.dp).align(Alignment.CenterHorizontally).clickable { fullscreen = true })
    }
}

@Composable
private fun PlayButtons(songs: List<Song>, actions: ActionsViewModel, filter: String? = null, onFilter: ((String) -> Unit)? = null) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), Arrangement.spacedBy(8.dp)) {
        Button({ actions.play(songs) }, Modifier.weight(1f), enabled = songs.isNotEmpty()) { Icon(Icons.Filled.PlayArrow, null); Text("Play") }
        OutlinedButton({ actions.shuffle(songs) }, Modifier.weight(1f), enabled = songs.isNotEmpty()) { Icon(Icons.Filled.Shuffle, null); Text("Shuffle") }
        IconButton({ actions.enqueue(songs) }, enabled = songs.isNotEmpty()) { Icon(Icons.AutoMirrored.Filled.PlaylistAdd, "Add all to queue") }
        IconButton({ actions.download(songs) }, enabled = songs.isNotEmpty()) { Icon(Icons.Filled.Download, "Download all") }
    }
    if (onFilter != null && (songs.size > 12 || !filter.isNullOrEmpty())) {
        OutlinedTextField(filter.orEmpty(), onFilter, Modifier.fillMaxWidth().padding(horizontal = 16.dp), singleLine = true, placeholder = { Text("Filter") })
    }
}

private fun List<Song>.matching(q: String) = if (q.isBlank()) this else filter { it.title.contains(q, true) || it.artist.contains(q, true) }

@Composable
private fun playingId(): String? {
    val player: PlayerViewModel = viewModel()
    val id by player.currentId.collectAsStateWithLifecycle()
    return id
}

@Composable
private fun selectedIds(actions: ActionsViewModel): Set<String> {
    val selection by actions.selection.collectAsStateWithLifecycle()
    return remember(selection) { selection.mapTo(HashSet()) { it.id } }
}

private fun quality(songs: List<Song>): String? {
    val s = songs.firstOrNull() ?: return null
    val lossless = s.suffix.lowercase() in setOf("flac", "alac", "wav", "aiff", "ape", "wv", "dsf", "dff")
    return listOfNotNull(s.suffix.uppercase().ifEmpty { null }, if (lossless && s.bitDepth > 0u) "${s.bitDepth}/${s.samplingRate.toInt() / 1000.0}" else s.bitRate.takeIf { it > 0u }?.let { "$it kbps" }).joinToString(" ").ifEmpty { null }
}

@Composable
fun AlbumScreen(id: String, actions: ActionsViewModel, vm: AlbumViewModel = viewModel()) {
    LaunchedEffect(id) { vm.open(id) }
    val load by vm.ui.collectAsStateWithLifecycle()
    val done = actions.downloads.collectAsState().value.doneIds
    val selected = selectedIds(actions)
    val menu = LocalSongMenu.current
    val nav = LocalNav.current
    val playing = playingId()
    var filter by remember { mutableStateOf("") }
    LoadBox(load) { d ->
        val discs = remember(d, filter) { d.songs.matching(filter).groupBy { it.discNumber.toInt().coerceAtLeast(1) }.toSortedMap() }
        LazyColumn {
            item(key = "header") {
                Header(d.album.name, listOfNotNull(d.album.artist, d.album.year.takeIf { it > 0u }?.toString(), "${d.songs.size} songs", duration(d.songs.sumOf { it.duration.toLong() }), quality(d.songs), "explicit".takeIf { d.album.explicitStatus == "explicit" }).joinToString(" · "), vm.cover(d.album.coverArt, CoverSize.FULL)) {
                    IconButton({ actions.starAlbum(d.album.id, !d.album.starred) }) { Icon(if (d.album.starred) Icons.Filled.Favorite else Icons.Filled.FavoriteBorder, "Favourite") }
                }
                PlayButtons(d.songs, actions, filter) { filter = it }
                d.album.artistId?.let { a -> Text("More by ${d.album.artist}", Modifier.padding(horizontal = 16.dp).clickable { nav.artist(a) }, color = MaterialTheme.colorScheme.primary) }
            }
            discs.forEach { (disc, tracks) ->
                if (discs.size > 1) item(key = "disc$disc") {
                    val title = d.discTitles.firstOrNull { it.disc.toInt() == disc }?.title
                    SectionTitle(if (title.isNullOrBlank()) "Disc $disc" else "Disc $disc · $title")
                }
                // Tapping plays the whole album from that track, not just its disc.
                songRows(tracks, actions, playing, done, selected, menu, numbered = true, keyPrefix = "d$disc-", context = d.songs)
            }
        }
    }
}

private val releaseOrder = listOf("Album", "EP", "Single", "Live", "Compilation", "Soundtrack", "Remix", "Other")

private fun Album.group(): String = when {
    releaseTypes.isEmpty() -> if (isCompilation) "Compilation" else "Album"
    else -> releaseTypes.firstOrNull { t -> releaseOrder.any { it.equals(t, true) } && !t.equals("Album", true) }?.replaceFirstChar(Char::uppercase) ?: releaseTypes.first().replaceFirstChar(Char::uppercase)
}

@Composable
fun ArtistScreen(id: String, actions: ActionsViewModel, vm: ArtistViewModel = viewModel()) {
    LaunchedEffect(id) { vm.open(id) }
    val load by vm.ui.collectAsStateWithLifecycle()
    val done = actions.downloads.collectAsState().value.doneIds
    val selected = selectedIds(actions)
    val menu = LocalSongMenu.current
    val nav = LocalNav.current
    val playing = playingId()
    val uri = LocalUriHandler.current
    var leaving by remember { mutableStateOf<String?>(null) }
    leaving?.let { url ->
        AlertDialog({ leaving = null }, title = { Text("Open in browser?") }, text = { Text(url) },
            confirmButton = { TextButton({ runCatching { uri.openUri(url) }; leaving = null }) { Text("Open") } }, dismissButton = { TextButton({ leaving = null }) { Text("Cancel") } })
    }
    LoadBox(load) { ui ->
        val groups = remember(ui.detail) { ui.detail.albums.sortedByDescending { it.year }.groupBy { it.group() }.toSortedMap(compareBy { g -> releaseOrder.indexOf(g).let { if (it < 0) 99 else it } }) }
        LazyColumn {
            item(key = "header") {
                Header(ui.detail.artist.name, "${ui.detail.albums.size} releases", vm.cover(ui.detail.artist.coverArt, CoverSize.FULL)) {
                    IconButton({ actions.starArtist(ui.detail.artist.id, !ui.detail.artist.starred) }) { Icon(if (ui.detail.artist.starred) Icons.Filled.Favorite else Icons.Filled.FavoriteBorder, "Favourite") }
                }
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), Arrangement.spacedBy(8.dp)) {
                    Button({ actions.playArtist(ui.detail.albums) }, Modifier.weight(1f)) { Icon(Icons.Filled.PlayArrow, null); Text("Play") }
                    OutlinedButton({ actions.playArtist(ui.detail.albums, shuffle = true) }, Modifier.weight(1f)) { Icon(Icons.Filled.Shuffle, null); Text("Shuffle") }
                    IconButton({ actions.queueArtist(ui.detail.albums) }) { Icon(Icons.AutoMirrored.Filled.PlaylistAdd, "Add artist to queue") }
                    IconButton({ actions.downloadArtist(ui.detail.albums) }) { Icon(Icons.Filled.Download, "Download all albums") }
                }
                ui.info?.biography?.let { Text(it.substringBefore("<a "), Modifier.padding(horizontal = 16.dp), maxLines = 4, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall) }
                Row(Modifier.padding(horizontal = 8.dp)) {
                    ui.info?.lastFmUrl?.let { u -> TextButton({ leaving = u }) { Text("last.fm") } }
                    ui.info?.musicBrainzId?.let { m -> TextButton({ leaving = "https://musicbrainz.org/artist/$m" }) { Text("MusicBrainz") } }
                }
            }
            groups.forEach { (group, albums) ->
                item(key = "g-$group") {
                    SectionTitle(if (group.endsWith("s")) group else "${group}s")
                    LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        items(albums, key = { it.id }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 120.dp, { nav.album(a.id) }) }
                    }
                }
            }
            if (ui.top.isNotEmpty()) item(key = "top") { SectionTitle("Top songs") }
            songRows(ui.top, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) }, keyPrefix = "top-")
            ui.info?.similar?.filter { it.id.isNotEmpty() }?.takeIf { it.isNotEmpty() }?.let { similar ->
                item(key = "similar") {
                    SectionTitle("Similar artists")
                    LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        items(similar, key = { it.id }) { a -> Text(a.name, Modifier.clickable { nav.artist(a.id) }.padding(8.dp), color = MaterialTheme.colorScheme.primary) }
                    }
                }
            }
        }
    }
}

@Composable
fun PlaylistScreen(id: String, actions: ActionsViewModel, vm: PlaylistViewModel = viewModel()) {
    LaunchedEffect(id) { vm.open(id) }
    val load by vm.ui.collectAsStateWithLifecycle()
    val settings: SettingsViewModel = viewModel()
    val prefs by settings.prefs.collectAsStateWithLifecycle()
    val done = actions.downloads.collectAsState().value.doneIds
    val selected = selectedIds(actions)
    val menu = LocalSongMenu.current
    val playing = playingId()
    var filter by remember { mutableStateOf("") }
    val context = androidx.compose.ui.platform.LocalContext.current
    val current = (load as? dev.flint.music.app.vm.Load.Ready)?.data
    val exportM3u = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.CreateDocument("audio/x-mpegurl")) { uri ->
        if (uri != null && current != null) runCatching { context.contentResolver.openOutputStream(uri)?.use { it.write(actions.exportM3u(current.playlist.name, current.songs).toByteArray()) } }
    }
    LoadBox(load) { d ->
        val shown = remember(d, filter) { d.songs.matching(filter) }
        LazyColumn {
            item(key = "header") {
                Header(d.playlist.name, "${d.songs.size} songs · ${duration(d.songs.sumOf { it.duration.toLong() })}", vm.cover(d.playlist.coverArt, CoverSize.FULL)) {
                    val pinned = id in prefs.pinnedPlaylists
                    IconButton({ settings.update { it.copy(pinnedPlaylists = if (pinned) it.pinnedPlaylists - id else it.pinnedPlaylists + id) } }) {
                        Icon(Icons.Filled.PushPin, if (pinned) "Unpin from home" else "Pin to home", tint = if (pinned) MaterialTheme.colorScheme.primary else LocalContentColor.current)
                    }
                }
                PlayButtons(d.songs, actions, filter) { filter = it }
                TextButton({ exportM3u.launch("${d.playlist.name}.m3u8") }, Modifier.padding(horizontal = 8.dp)) { Text("Export M3U") }
            }
            songRows(shown, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
        }
    }
}

@Composable
fun GenreScreen(name: String, actions: ActionsViewModel, vm: GenreViewModel = viewModel()) {
    LaunchedEffect(name) { vm.open(name) }
    val load by vm.ui.collectAsStateWithLifecycle()
    val done = actions.downloads.collectAsState().value.doneIds
    val selected = selectedIds(actions)
    val menu = LocalSongMenu.current
    val playing = playingId()
    LoadBox(load) { list ->
        LazyColumn {
            item(key = "header") { Header(name, "${list.size} songs", null); PlayButtons(list, actions) }
            songRows(list, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
        }
    }
}

/** One level of the server's folder tree: subfolders first, then the files in it. */
@Composable
fun FolderScreen(id: String, actions: ActionsViewModel, vm: FolderViewModel = viewModel()) {
    LaunchedEffect(id) { vm.open(id) }
    val load by vm.ui.collectAsStateWithLifecycle()
    val done = actions.downloads.collectAsState().value.doneIds
    val selected = selectedIds(actions)
    val menu = LocalSongMenu.current
    val nav = LocalNav.current
    val playing = playingId()
    LoadBox(load) { d ->
        LazyColumn {
            item(key = "header") { Header(d.name.ifEmpty { "Folder" }, "${d.folders.size} folders · ${d.songs.size} songs", null); if (d.songs.isNotEmpty()) PlayButtons(d.songs, actions) }
            items(d.folders, key = { "f" + it.id }) { f -> Text("📁  ${f.name}", Modifier.fillMaxWidth().clickable { nav.folder(f.id) }.padding(horizontal = 16.dp, vertical = 14.dp)) }
            songRows(d.songs, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
        }
    }
}
