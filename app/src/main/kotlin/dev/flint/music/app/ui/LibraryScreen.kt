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
import dev.flint.music.app.vm.DecadesViewModel
import dev.flint.music.app.vm.FoldersViewModel
import dev.flint.music.app.vm.SongSort
import dev.flint.music.app.vm.SongsViewModel
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.snapshotFlow
import kotlinx.coroutines.launch
import dev.flint.music.app.vm.GenresViewModel
import dev.flint.music.app.vm.PlaylistsViewModel
import dev.flint.music.app.vm.RadioViewModel
import dev.flint.music.app.vm.StarredViewModel
import dev.flint.music.data.AlbumSort

private val sections = listOf("Albums", "Artists", "Songs", "Playlists", "Favourites", "Genres", "Decades", "Folders", "Radio", "Downloads")

@Composable
fun LibraryScreen(actions: ActionsViewModel) {
    var tab by rememberSaveable { mutableIntStateOf(0) }
    Column {
        PrimaryScrollableTabRow(tab, edgePadding = 8.dp) { sections.forEachIndexed { i, s -> Tab(tab == i, { tab = i }, text = { Text(s) }) } }
        // Only the visible section is composed, so only its view model loads anything.
        when (sections[tab]) {
            "Albums" -> Albums()
            "Artists" -> Artists()
            "Songs" -> SongsScreen(actions, null)
            "Playlists" -> Playlists(actions)
            "Favourites" -> Favourites(actions)
            "Genres" -> Genres()
            "Decades" -> Decades()
            "Folders" -> Folders()
            "Radio" -> Radio()
            "Downloads" -> Downloads(actions)
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
            items(listOf(AlbumSort.BY_NAME to "A–Z", AlbumSort.BY_ARTIST to "Artist", AlbumSort.NEWEST to "Added", AlbumSort.RECENT to "Played", AlbumSort.FREQUENT to "Most played", AlbumSort.STARRED to "Favourites", AlbumSort.BY_YEAR to "Year", AlbumSort.HIGHEST to "Rating", AlbumSort.RANDOM to "Random")) { (s, label) ->
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
    var filter by remember { mutableStateOf("") }
    val list = rememberLazyListState()
    val scope = rememberCoroutineScope()
    LoadBox(load) { all ->
        val artists = remember(all, filter) { if (filter.isBlank()) all else all.filter { it.name.contains(filter, true) } }
        // First row of each initial, for the index on the right edge.
        val letters = remember(artists) { artists.withIndex().groupBy { it.value.name.firstOrNull()?.uppercaseChar()?.takeIf(Char::isLetter) ?: '#' }.mapValues { it.value.first().index }.toSortedMap() }
        Column {
            OutlinedTextField(filter, { filter = it }, Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp), singleLine = true, placeholder = { Text("Filter artists") })
            Row {
                LazyColumn(Modifier.weight(1f), state = list) {
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
                if (letters.size > 3) Column(Modifier.padding(end = 2.dp).verticalScroll(rememberScrollState())) {
                    letters.forEach { (c, index) -> Text("$c", Modifier.clickable { scope.launch { list.scrollToItem(index) } }.padding(horizontal = 8.dp, vertical = 1.dp), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.primary) }
                }
            }
        }
    }
}

/** Every indexed song, sorted; with [decade] set, only that decade. Reads the offline index, never the network. */
@Composable
fun SongsScreen(actions: ActionsViewModel, decade: Int?, vm: SongsViewModel = viewModel(key = "songs-$decade")) {
    LaunchedEffect(decade) { vm.setYears(decade?.let { it..it + 9 }) }
    val songs by vm.songs.collectAsStateWithLifecycle()
    val sort by vm.sort.collectAsStateWithLifecycle()
    val starred by vm.starredOnly.collectAsStateWithLifecycle()
    val done = actions.downloads.collectAsState().value.doneIds
    val selection by actions.selection.collectAsStateWithLifecycle()
    val selected = remember(selection) { selection.mapTo(HashSet()) { it.id } }
    val player: dev.flint.music.app.vm.PlayerViewModel = viewModel()
    val playing by player.currentId.collectAsStateWithLifecycle()
    val menu = LocalSongMenu.current
    val list = rememberLazyListState()
    // Ask for the next page a screenful before the end, from a snapshot observer rather than from inside item composition.
    LaunchedEffect(list, songs.size) { snapshotFlow { (list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0) >= songs.size - 40 }.collect { if (it) vm.loadMore() } }
    Column {
        if (decade != null) SectionTitle("${decade}s")
        LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            item { FilterChip(starred, { vm.setStarredOnly(!starred) }, { Text("★") }) }
            items(SongSort.entries) { s -> FilterChip(sort == s, { vm.setSort(s) }, { Text(s.label) }) }
        }
        if (songs.isEmpty()) Text("Nothing in the offline index yet. Settings → Sync all fills it.", Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        LazyColumn(state = list) { songRows(songs, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) }) }
    }
}

@Composable
private fun Decades(vm: DecadesViewModel = viewModel()) {
    val load by vm.decades.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LoadBox(load) { decades ->
        LazyColumn {
            if (decades.isEmpty()) item { Text("Nothing in the offline index yet. Settings → Sync all fills it.", Modifier.padding(16.dp)) }
            items(decades, key = { it.name }) { d ->
                Row(Modifier.fillMaxWidth().clickable { nav.decade(d.name.toInt()) }.padding(16.dp)) {
                    Text("${d.name}s", Modifier.weight(1f)); Text("${d.songCount}", color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
    }
}

@Composable
private fun Folders(vm: FoldersViewModel = viewModel()) {
    val load by vm.roots.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LoadBox(load) { roots -> LazyColumn { items(roots, key = { it.id }) { f -> Text("📁  ${f.name}", Modifier.fillMaxWidth().clickable { nav.folder(f.id) }.padding(horizontal = 16.dp, vertical = 14.dp)) } } }
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
            songRows(s.songs, actions, null, emptySet(), emptySet(), menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
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
        songRows(d.done, actions, null, d.doneIds, emptySet(), menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
    }
}
