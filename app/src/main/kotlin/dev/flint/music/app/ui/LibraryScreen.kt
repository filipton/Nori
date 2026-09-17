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
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.PrimaryScrollableTabRow
import androidx.compose.material3.Tab
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.AlbumsViewModel
import dev.flint.music.app.vm.ArtistsViewModel
import dev.flint.music.app.vm.GenresViewModel
import dev.flint.music.app.vm.PlaylistsViewModel
import dev.flint.music.app.vm.RadioViewModel
import dev.flint.music.app.vm.StarredViewModel
import dev.flint.music.data.AlbumSort

private val sections = listOf("Albums", "Artists", "Playlists", "Favourites", "Genres", "Radio", "Downloads")

@Composable
fun LibraryScreen(actions: ActionsViewModel) {
    var tab by rememberSaveable { mutableIntStateOf(0) }
    Column {
        PrimaryScrollableTabRow(tab, edgePadding = 8.dp) { sections.forEachIndexed { i, s -> Tab(tab == i, { tab = i }, text = { Text(s) }) } }
        // Only the visible section is composed, so only its view model loads anything.
        when (tab) {
            0 -> Albums()
            1 -> Artists()
            2 -> Playlists(actions)
            3 -> Favourites(actions)
            4 -> Genres()
            5 -> Radio()
            6 -> Downloads(actions)
        }
    }
}

@Composable
private fun Albums(vm: AlbumsViewModel = viewModel()) {
    val albums by vm.albums.collectAsStateWithLifecycle()
    val sort by vm.sort.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    Column {
        LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            items(listOf(AlbumSort.BY_NAME to "A–Z", AlbumSort.BY_ARTIST to "Artist", AlbumSort.NEWEST to "Added", AlbumSort.RECENT to "Played", AlbumSort.FREQUENT to "Most played", AlbumSort.STARRED to "Favourites")) { (s, label) ->
                FilterChip(sort == s, { vm.setSort(s) }, { Text(label) })
            }
        }
        LazyVerticalGrid(GridCells.Adaptive(132.dp), contentPadding = PaddingValues(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            itemsIndexed(albums, key = { _, a -> a.id }, contentType = { _, _ -> "album" }) { i, a ->
                if (i >= albums.size - 12) vm.loadMore()
                AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 132.dp, { nav.album(a.id) }, Modifier.fillMaxWidth())
            }
        }
    }
}

@Composable
private fun Artists(vm: ArtistsViewModel = viewModel()) {
    val load by vm.artists.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LoadBox(load) { artists ->
        LazyColumn {
            items(artists, key = { it.id }, contentType = { "artist" }) { a ->
                Row(Modifier.fillMaxWidth().clickable { nav.artist(a.id) }.padding(horizontal = 16.dp, vertical = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Cover(vm.cover(a.coverArt, CoverSize.ROW), 48.dp)
                    Column(Modifier.padding(start = 12.dp)) {
                        Text(a.name)
                        Text("${a.albumCount} albums", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
            }
        }
    }
}

@Composable
private fun Playlists(actions: ActionsViewModel, vm: PlaylistsViewModel = viewModel()) {
    val load by vm.playlists.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    var creating by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    if (creating) AlertDialog(
        onDismissRequest = { creating = false }, title = { Text("New playlist") },
        text = { OutlinedTextField(name, { name = it }, singleLine = true) },
        confirmButton = { TextButton({ vm.create(name.trim()); name = ""; creating = false }, enabled = name.isNotBlank()) { Text("Create") } },
    )
    LoadBox(load) { playlists ->
        LazyColumn {
            item { Row(Modifier.fillMaxWidth().clickable { creating = true }.padding(16.dp)) { Icon(Icons.Filled.Add, null); Text("New playlist", Modifier.padding(start = 12.dp)) } }
            items(playlists, key = { it.id }) { p ->
                Row(Modifier.fillMaxWidth().clickable { nav.playlist(p.id) }.padding(start = 16.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    Cover(vm.cover(p.coverArt, CoverSize.ROW), 48.dp)
                    Column(Modifier.weight(1f).padding(start = 12.dp)) {
                        Text(p.name)
                        Text("${p.songCount} songs · ${duration(p.duration.toLong())}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    IconButton({ vm.delete(p.id) }) { Icon(Icons.Filled.Delete, "Delete") }
                }
            }
        }
    }
}

@Composable
private fun Favourites(actions: ActionsViewModel, vm: StarredViewModel = viewModel()) {
    val load by vm.starred.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    val menu = LocalSongMenu.current
    LoadBox(load) { s ->
        LazyColumn {
            if (s.albums.isNotEmpty()) item(key = "albums") {
                LazyRow(contentPadding = PaddingValues(16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(s.albums, key = { it.id }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 120.dp, { nav.album(a.id) }) }
                }
            }
            items(s.artists, key = { "ar" + it.id }) { a -> Text(a.name, Modifier.fillMaxWidth().clickable { nav.artist(a.id) }.padding(16.dp)) }
            itemsIndexed(s.songs, key = { _, x -> x.id }, contentType = { _, _ -> "song" }) { i, x ->
                SongRow(x, vm.cover(x.coverArt, CoverSize.ROW), { actions.play(s.songs, i) }, { menu(x) })
            }
        }
    }
}

@Composable
private fun Genres(vm: GenresViewModel = viewModel()) {
    val load by vm.genres.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LoadBox(load) { genres ->
        LazyColumn {
            items(genres, key = { it.name }) { g ->
                Row(Modifier.fillMaxWidth().clickable { nav.genre(g.name) }.padding(16.dp)) {
                    Text(g.name, Modifier.weight(1f))
                    Text("${g.songCount}", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

@Composable
private fun Radio(vm: RadioViewModel = viewModel()) {
    val load by vm.stations.collectAsStateWithLifecycle()
    var adding by remember { mutableStateOf(false) }
    var name by remember { mutableStateOf("") }
    var url by remember { mutableStateOf("") }
    if (adding) AlertDialog(
        onDismissRequest = { adding = false }, title = { Text("New station") },
        text = { Column { OutlinedTextField(name, { name = it }, label = { Text("Name") }, singleLine = true); OutlinedTextField(url, { url = it }, label = { Text("Stream URL") }, singleLine = true) } },
        confirmButton = { TextButton({ vm.add(name.trim(), url.trim()); name = ""; url = ""; adding = false }, enabled = name.isNotBlank() && url.startsWith("http")) { Text("Add") } },
    )
    LoadBox(load) { stations ->
        LazyColumn {
            item { Row(Modifier.fillMaxWidth().clickable { adding = true }.padding(16.dp)) { Icon(Icons.Filled.Add, null); Text("New station", Modifier.padding(start = 12.dp)) } }
            items(stations, key = { it.id }) { s ->
                Row(Modifier.fillMaxWidth().clickable { vm.play(s) }.padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text(s.name, Modifier.weight(1f))
                    IconButton({ vm.delete(s.id) }) { Icon(Icons.Filled.Delete, "Delete") }
                }
            }
        }
    }
}

@Composable
private fun Downloads(actions: ActionsViewModel) {
    val d by actions.downloads.collectAsState()
    val menu = LocalSongMenu.current
    val vm: StarredViewModel = viewModel()
    LazyColumn {
        if (d.pending.isNotEmpty()) item(key = "pending") { Text("${d.pending.size} downloading…", Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant) }
        if (d.done.isEmpty() && d.pending.isEmpty()) item { Text("Nothing downloaded yet", Modifier.padding(16.dp)) }
        itemsIndexed(d.done, key = { _, s -> s.id }, contentType = { _, _ -> "song" }) { i, s ->
            SongRow(s, vm.cover(s.coverArt, CoverSize.ROW), { actions.play(d.done, i) }, { menu(s) }, downloaded = true)
        }
    }
}
