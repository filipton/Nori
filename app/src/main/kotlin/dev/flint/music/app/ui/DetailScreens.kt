package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.FavoriteBorder
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.AlbumViewModel
import dev.flint.music.app.vm.ArtistViewModel
import dev.flint.music.app.vm.GenreViewModel
import dev.flint.music.app.vm.PlayerViewModel
import dev.flint.music.app.vm.PlaylistViewModel
import dev.flint.music.ffi.Song

@Composable
private fun Header(title: String, subtitle: String, coverUrl: String?, actions: @Composable () -> Unit = {}) {
    val nav = LocalNav.current
    Column {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.titleLarge, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Text(subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            actions()
        }
        if (coverUrl != null) Cover(coverUrl, 220.dp, Modifier.padding(16.dp).align(Alignment.CenterHorizontally))
    }
}

@Composable
private fun PlayButtons(songs: List<Song>, actions: ActionsViewModel) {
    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), Arrangement.spacedBy(8.dp)) {
        Button({ actions.play(songs) }, Modifier.weight(1f), enabled = songs.isNotEmpty()) { Icon(Icons.Filled.PlayArrow, null); Text("Play") }
        OutlinedButton({ actions.shuffle(songs) }, Modifier.weight(1f), enabled = songs.isNotEmpty()) { Icon(Icons.Filled.Shuffle, null); Text("Shuffle") }
        IconButton({ actions.download(songs) }, enabled = songs.isNotEmpty()) { Icon(Icons.Filled.Download, "Download all") }
    }
}

private fun LazyListScope.songs(songs: List<Song>, actions: ActionsViewModel, playingId: String?, done: Set<String>, menu: (Song) -> Unit, cover: ((Song) -> String?)? = null) {
    itemsIndexed(songs, key = { i, s -> "$i-${s.id}" }, contentType = { _, _ -> "song" }) { i, s ->
        SongRow(s, cover?.invoke(s), { actions.play(songs, i) }, { menu(s) }, number = if (cover == null) s.track.toInt() else null, playing = s.id == playingId, downloaded = s.id in done)
    }
}

@Composable
private fun playingId(): String? {
    val player: PlayerViewModel = viewModel()
    val id by player.currentId.collectAsStateWithLifecycle()
    return id
}

@Composable
fun AlbumScreen(id: String, actions: ActionsViewModel, vm: AlbumViewModel = viewModel()) {
    LaunchedEffect(id) { vm.open(id) }
    val load by vm.ui.collectAsStateWithLifecycle()
    val done = actions.downloads.collectAsState().value.doneIds
    val menu = LocalSongMenu.current
    val nav = LocalNav.current
    val playing = playingId()
    LoadBox(load) { d ->
        LazyColumn {
            item(key = "header") {
                Header(d.album.name, listOfNotNull(d.album.artist, d.album.year.takeIf { it > 0u }?.toString(), "${d.songs.size} songs", duration(d.songs.sumOf { it.duration.toLong() })).joinToString(" · "), vm.cover(d.album.coverArt, CoverSize.FULL)) {
                    IconButton({ actions.starAlbum(d.album.id, !d.album.starred) }) { Icon(if (d.album.starred) Icons.Filled.Favorite else Icons.Filled.FavoriteBorder, "Favourite") }
                }
                PlayButtons(d.songs, actions)
                d.album.artistId?.let { a -> Text("More by ${d.album.artist}", Modifier.padding(horizontal = 16.dp).clickable { nav.artist(a) }, color = MaterialTheme.colorScheme.primary) }
            }
            songs(d.songs, actions, playing, done, menu)
        }
    }
}

@Composable
fun ArtistScreen(id: String, actions: ActionsViewModel, vm: ArtistViewModel = viewModel()) {
    LaunchedEffect(id) { vm.open(id) }
    val load by vm.ui.collectAsStateWithLifecycle()
    val done = actions.downloads.collectAsState().value.doneIds
    val menu = LocalSongMenu.current
    val nav = LocalNav.current
    val playing = playingId()
    LoadBox(load) { ui ->
        LazyColumn {
            item(key = "header") {
                Header(ui.detail.artist.name, "${ui.detail.albums.size} albums", vm.cover(ui.detail.artist.coverArt, CoverSize.FULL)) {
                    IconButton({ actions.starArtist(ui.detail.artist.id, !ui.detail.artist.starred) }) { Icon(if (ui.detail.artist.starred) Icons.Filled.Favorite else Icons.Filled.FavoriteBorder, "Favourite") }
                }
                ui.info?.biography?.let { Text(it.substringBefore("<a "), Modifier.padding(horizontal = 16.dp), maxLines = 4, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall) }
                SectionTitle("Albums")
                LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(ui.detail.albums, key = { it.id }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 120.dp, { nav.album(a.id) }) }
                }
                if (ui.top.isNotEmpty()) SectionTitle("Top songs")
            }
            songs(ui.top, actions, playing, done, menu) { vm.cover(it.coverArt, CoverSize.ROW) }
            ui.info?.similar?.takeIf { it.isNotEmpty() }?.let { similar ->
                item(key = "similar") {
                    SectionTitle("Similar artists")
                    LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                        items(similar.filter { it.id.isNotEmpty() }, key = { it.id }) { a -> Text(a.name, Modifier.clickable { nav.artist(a.id) }.padding(8.dp), color = MaterialTheme.colorScheme.primary) }
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
    val done = actions.downloads.collectAsState().value.doneIds
    val menu = LocalSongMenu.current
    val playing = playingId()
    LoadBox(load) { d ->
        LazyColumn {
            item(key = "header") {
                Header(d.playlist.name, "${d.songs.size} songs · ${duration(d.songs.sumOf { it.duration.toLong() })}", vm.cover(d.playlist.coverArt, CoverSize.FULL))
                PlayButtons(d.songs, actions)
            }
            songs(d.songs, actions, playing, done, menu) { vm.cover(it.coverArt, CoverSize.ROW) }
        }
    }
}

@Composable
fun GenreScreen(name: String, actions: ActionsViewModel, vm: GenreViewModel = viewModel()) {
    LaunchedEffect(name) { vm.open(name) }
    val load by vm.ui.collectAsStateWithLifecycle()
    val done = actions.downloads.collectAsState().value.doneIds
    val menu = LocalSongMenu.current
    val playing = playingId()
    LoadBox(load) { list ->
        LazyColumn {
            item(key = "header") { Header(name, "${list.size} songs", null); PlayButtons(list, actions) }
            songs(list, actions, playing, done, menu) { vm.cover(it.coverArt, CoverSize.ROW) }
        }
    }
}
