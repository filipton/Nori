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
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.SearchViewModel
import dev.flint.music.app.vm.SearchScope

@Composable
fun SearchScreen(actions: ActionsViewModel, vm: SearchViewModel = viewModel()) {
    val ui by vm.ui.collectAsStateWithLifecycle()
    val downloads by actions.downloads.collectAsState()
    val nav = LocalNav.current
    val menu = LocalSongMenu.current
    Column {
        LargeTitle("Search")
        SearchField(
            ui.query, vm::setQuery, "Songs, albums, artists",
            Modifier.padding(horizontal = Space.gutter, vertical = 8.dp), testTag = "search",
        )
        if (ui.searching) LinearProgressIndicator(Modifier.fillMaxWidth())
        ui.error?.let { Text("Server search failed: $it — showing offline results", Modifier.padding(horizontal = 16.dp), color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall) }

        if (ui.hasProviders || ui.scope != SearchScope.EVERYTHING) LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            items(listOf(SearchScope.EVERYTHING to "Everything", SearchScope.LIBRARY to "In library", SearchScope.PROVIDERS to "Not in library yet")) { (sc, label) ->
                Chip(label, ui.scope == sc) { vm.setScope(sc) }
            }
        }
        val r = ui.shown
        if (r == null) {
            LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
                if (ui.history.isNotEmpty()) item {
                    Row(Modifier.fillMaxWidth().padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text("Recent searches", Modifier.weight(1f).padding(start = 4.dp), style = MaterialTheme.typography.titleLarge)
                        TextButton(vm::clearHistory) { Text("Clear") }
                    }
                }
                items(ui.history, key = { it }) { q -> Text(q, Modifier.fillMaxWidth().clickable { vm.setQuery(q) }.padding(horizontal = Space.gutter, vertical = 13.dp), style = MaterialTheme.typography.bodyLarge) }
            }
            return@Column
        }
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            if (r.artists.isNotEmpty()) item(key = "artists") {
                SectionTitle("Artists")
                LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(r.artists, key = { it.id }) { a ->
                        ArtistCard(a.name, "", vm.cover(a.coverArt, CoverSize.ROW), 96.dp, onClick = { vm.remember(); nav.artist(a.id) })
                    }
                }
            }
            if (r.albums.isNotEmpty()) item(key = "albums") {
                SectionTitle("Albums")
                LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(r.albums, key = { it.id }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 120.dp, { vm.remember(); nav.album(a.id) }) }
                }
            }
            if (r.songs.isNotEmpty()) item(key = "songs") { SectionTitle("Songs") }
            itemsIndexed(r.songs, key = { _, s -> s.id }, contentType = { _, _ -> "song" }) { _, s ->
                // One tap plays one song: a search result list is not an album, and with octo-fiesta
                // queueing the rest would make the server download every provider track in it.
                SongRow(s, vm.cover(s.coverArt, CoverSize.ROW), onClick = { vm.remember(); actions.play(listOf(s)) }, onMenu = { menu(s) }, downloaded = s.id in downloads.doneIds)
            }
            if (ui.fromServer && r.songs.isEmpty() && r.albums.isEmpty() && r.artists.isEmpty()) item { EmptyNote("Nothing found") }
        }
    }
}
