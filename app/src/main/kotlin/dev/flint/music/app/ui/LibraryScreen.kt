package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.size
import androidx.compose.ui.draw.clip
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.DropdownMenu
import androidx.compose.material.icons.filled.SwapVert
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.grid.GridCells
import androidx.compose.foundation.lazy.grid.rememberLazyGridState
import androidx.compose.foundation.lazy.grid.LazyVerticalGrid
import androidx.compose.foundation.lazy.grid.itemsIndexed
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Radio
import androidx.compose.material.icons.filled.FileDownload
import androidx.compose.material.icons.filled.Folder
import androidx.compose.material.icons.filled.Downloading
import androidx.compose.material.icons.filled.Delete
import androidx.compose.material.icons.filled.Favorite
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

/**
 * One line that says how the list is ordered and opens the alternatives, instead of a second row of
 * chips under the first: two stacked chip strips made this screen read as a toolbar.
 */
@Composable
private fun <T> SortMenu(options: List<Pair<T, String>>, value: T, onChange: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Box(Modifier.padding(start = Space.gutter - 4.dp, top = 2.dp)) {
        Row(
            Modifier.clip(PillShape).clickable { open = true }.padding(horizontal = 6.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(Icons.Filled.SwapVert, null, Modifier.size(17.dp), tint = MaterialTheme.colorScheme.primary)
            Text(
                options.firstOrNull { it.first == value }?.second ?: "Sort",
                Modifier.padding(start = 4.dp), style = MaterialTheme.typography.labelLarge,
                color = MaterialTheme.colorScheme.primary,
            )
        }
        DropdownMenu(open, { open = false }) {
            options.forEach { (v, label) -> DropdownMenuItem({ Text(label) }, { onChange(v); open = false }) }
        }
    }
}

private val sections = listOf("Albums", "Favourites", "Artists", "Songs", "Playlists", "Smart", "History", "Genres", "Decades", "Folders", "Radio", "Downloads")

@Composable
fun LibraryScreen(actions: ActionsViewModel) {
    var tab by rememberSaveable { mutableIntStateOf(0) }
    Column {
        LargeTitle("Library")
        // A scrolling row of pills, not a tab strip with an underline: twelve sections in a Material tab
        // row reads as a toolbar, and the library is a place to browse.
        LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter, vertical = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            itemsIndexed(sections, key = { _, s -> s }) { i, s -> Chip(s, tab == i) { tab = i } }
        }
        // Only the visible section is composed, so only its view model loads anything.
        when (sections[tab]) {
            "Albums" -> Albums()
            "Artists" -> Artists()
            "Songs" -> SongsScreen(actions, null)
            "Playlists" -> Playlists(actions)
            "Smart" -> SmartList()
            "History" -> HistoryList(actions)
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
        SortMenu(
            listOf(AlbumSort.BY_NAME to "A–Z", AlbumSort.BY_ARTIST to "Artist", AlbumSort.NEWEST to "Added", AlbumSort.RECENT to "Played", AlbumSort.FREQUENT to "Most played", AlbumSort.STARRED to "Favourites", AlbumSort.BY_YEAR to "Year", AlbumSort.RANDOM to "Random"),
            sort, vm::setSort,
        )
        // Ask for the covers just past the fold while the ones on screen are still arriving.
        val list = rememberLazyGridState()
        PrefetchCovers(
            remember(albums, list.firstVisibleItemIndex) {
                albums.drop(list.firstVisibleItemIndex + 6).take(12).map { vm.cover(it.coverArt, CoverSize.CARD) }
            },
        )
        // Two columns, counted rather than measured: an adaptive grid gave a wide phone a third column,
        // and a cover a third of the way across the screen is too small to recognise a sleeve by, which
        // is the only reason to show covers instead of a list of names.
        LazyVerticalGrid(GridCells.Fixed(2), state = list, contentPadding = PaddingValues(start = Space.gutter, end = Space.gutter, top = Space.gutter, bottom = Space.gutter + LocalChromeInset.current), horizontalArrangement = Arrangement.spacedBy(12.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            itemsIndexed(albums, key = { _, a -> a.id }, contentType = { _, _ -> "album" }) { i, a ->
                if (i >= albums.size - 12) vm.loadMore()
                AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 132.dp, { nav.album(a.id) }, Modifier.fillMaxWidth(), fill = true)
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
            SearchField(filter, { filter = it }, "Filter artists", Modifier.padding(horizontal = Space.gutter, vertical = 4.dp))
            Row {
                LazyColumn(Modifier.weight(1f), state = list, contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
                    items(artists, key = { it.id }, contentType = { "artist" }) { a ->
                        Column {
                            Row(Modifier.fillMaxWidth().clickable { nav.artist(a.id) }.padding(horizontal = Space.gutter, vertical = 7.dp), verticalAlignment = Alignment.CenterVertically) {
                                Cover(vm.cover(a.coverArt, CoverSize.ROW), 48.dp, radius = 24.dp)
                                Column(Modifier.padding(start = 12.dp)) {
                                    Text(a.name, style = MaterialTheme.typography.bodyLarge)
                                    Text("${a.albumCount} albums", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                }
                            }
                            Hairline(startIndent = Space.gutter + 60.dp)
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
        LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            item { Chip("★ Favourites", starred) { vm.setStarredOnly(!starred) } }
            items(SongSort.entries) { s -> Chip(s.label, sort == s) { vm.setSort(s) } }
        }
        if (songs.isEmpty()) EmptyNote("Nothing in the offline index yet. Settings → Library → Sync all fills it.")
        LazyColumn(state = list, contentPadding = PaddingValues(bottom = LocalChromeInset.current)) { songRows(songs, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) }) }
    }
}

@Composable
private fun Decades(vm: DecadesViewModel = viewModel()) {
    val load by vm.decades.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LoadBox(load) { decades ->
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            if (decades.isEmpty()) item { EmptyNote("Nothing in the offline index yet. Settings → Library → Sync all fills it.") }
            items(decades, key = { it.name }) { d ->
                NavRow("${d.name}s", { nav.decade(d.name.toInt()) }, trailing = "${d.songCount}", chevron = true)
            }
        }
    }
}

@Composable
private fun Folders(vm: FoldersViewModel = viewModel()) {
    val load by vm.roots.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LoadBox(load) { roots ->
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            items(roots, key = { it.id }) { f ->
                NavRow(
                    f.name, { nav.folder(f.id) }, chevron = true,
                    leading = { Icon(Icons.Filled.Folder, null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary) },
                )
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
    val context = androidx.compose.ui.platform.LocalContext.current
    val pickM3u = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.OpenDocument()) { uri ->
        if (uri != null) runCatching { context.contentResolver.openInputStream(uri)?.use { it.readBytes().decodeToString() } }.getOrNull()?.let { text ->
            actions.importM3u(uri.lastPathSegment?.substringAfterLast('/')?.substringBeforeLast('.') ?: "Imported", text)
        }
    }
    if (creating) AlertDialog(
        onDismissRequest = { creating = false }, title = { Text("New playlist") },
        text = { FormField(name, { name = it }, label = { Text("Name") }, singleLine = true) },
        confirmButton = { TextButton({ vm.create(name.trim()); name = ""; creating = false }, enabled = name.isNotBlank()) { Text("Create") } },
    )
    LoadBox(load) { playlists ->
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            item { ActionRow("New playlist", Icons.Filled.Add, { creating = true }) }
            item { ActionRow("Import M3U…", Icons.Filled.FileDownload, { pickM3u.launch(arrayOf("*/*")) }) }
            if (playlists.isEmpty()) item { EmptyNote("No playlists yet") }
            items(playlists, key = { it.id }) { p ->
                NavRow(
                    p.name, { nav.playlist(p.id) },
                    subtitle = "${p.songCount} songs · ${duration(p.duration.toLong())}",
                    leading = { Cover(vm.cover(p.coverArt, CoverSize.ROW), 48.dp) },
                    action = { IconButton({ vm.delete(p.id) }, Modifier.size(40.dp)) { Icon(Icons.Filled.Delete, "Delete", Modifier.size(19.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) } },
                )
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
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            // The songs as one list to play, the page the "Favourites" tile on Home opens too.
            item(key = "all") {
                NavRow(
                    "Favourite songs", { nav.mix(dev.flint.music.app.vm.FAVOURITES_MIX) }, chevron = true,
                    subtitle = s.songs.count { !it.isExternal }.let { n -> "$n song${if (n == 1) "" else "s"}" },
                    leading = { Icon(Icons.Filled.Favorite, null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary) },
                )
            }
            if (s.albums.isNotEmpty()) item(key = "albums") {
                LazyRow(contentPadding = PaddingValues(Space.gutter), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(s.albums, key = { it.id }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 120.dp, { nav.album(a.id) }) }
                }
            }
            items(s.artists, key = { "ar" + it.id }) { a ->
                NavRow(
                    a.name, { nav.artist(a.id) }, chevron = true,
                    leading = { Cover(vm.cover(a.coverArt, CoverSize.ROW), 44.dp, radius = 22.dp) },
                )
            }
            songRows(s.songs, actions, null, emptySet(), emptySet(), menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
        }
    }
}

@Composable
private fun Genres(vm: GenresViewModel = viewModel()) {
    val load by vm.genres.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LoadBox(load) { genres ->
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            items(genres, key = { it.name }) { g ->
                NavRow(g.name, { nav.genre(g.name) }, trailing = "${g.songCount}", chevron = true)
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
        text = { Column { FormField(name, { name = it }, label = { Text("Name") }, singleLine = true); Spacer(Modifier.height(10.dp)); FormField(url, { url = it }, label = { Text("Stream URL") }, singleLine = true) } },
        confirmButton = { TextButton({ vm.add(name.trim(), url.trim()); name = ""; url = ""; adding = false }, enabled = name.isNotBlank() && url.startsWith("http")) { Text("Add") } },
    )
    LoadBox(load) { stations ->
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            item { ActionRow("New station", Icons.Filled.Add, { adding = true }) }
            if (stations.isEmpty()) item { EmptyNote("No stations yet") }
            items(stations, key = { it.id }) { s ->
                NavRow(
                    s.name, { vm.play(s) },
                    leading = { Icon(Icons.Filled.Radio, null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary) },
                    action = { IconButton({ vm.delete(s.id) }, Modifier.size(40.dp)) { Icon(Icons.Filled.Delete, "Delete", Modifier.size(19.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) } },
                )
            }
        }
    }
}

@Composable
private fun Downloads(actions: ActionsViewModel) {
    val d by actions.downloads.collectAsState()
    val menu = LocalSongMenu.current
    val vm: StarredViewModel = viewModel()
    val marks by actions.downloadMarks.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
        // The way to the queue, always there: what is on its way now, or where it went.
        item(key = "queue") {
            val failed = marks.values.count { it.phase == dev.flint.music.downloads.DownloadPhase.FAILED }
            val waiting = (d.pending.size - failed).coerceAtLeast(0)
            NavRow(
                "Download queue", nav::downloads, chevron = true,
                subtitle = listOfNotNull(
                    "$waiting to go".takeIf { waiting > 0 },
                    "$failed failed".takeIf { failed > 0 },
                ).joinToString(" · ").ifEmpty { "Nothing downloading" },
                leading = { Icon(Icons.Filled.Downloading, null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary) },
            )
        }
        if (d.done.isEmpty()) item { EmptyNote("Nothing downloaded yet") }
        songRows(d.done, actions, null, d.doneIds, emptySet(), menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
    }
}
