package dev.flint.music.app.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.History
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.HomeViewModel
import dev.flint.music.ffi.Album

@Composable
fun HomeScreen(actions: ActionsViewModel, vm: HomeViewModel = viewModel()) {
    val load by vm.ui.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    val settings: dev.flint.music.app.vm.SettingsViewModel = viewModel()
    val mixes = settings.prefs.collectAsStateWithLifecycle().value.tasteModel
    LoadBox(load) { ui ->
        LazyColumn {
            item(key = "title") { LargeTitle("Listen now") }
            item(key = "actions") {
                Row(Modifier.padding(horizontal = Space.gutter, vertical = 10.dp), Arrangement.spacedBy(10.dp)) {
                    PillButton("Shuffle", Icons.Filled.Shuffle, actions::shuffleAll, Modifier.weight(1f))
                    PillButton("Resume", Icons.Filled.History, actions::resumeFromServer, Modifier.weight(1f))
                }
            }
            if (mixes) item(key = "mixes") { SectionTitle("For you"); MixTiles() }
            if (ui.pinned.isNotEmpty()) item(key = "pinned") {
                SectionTitle("Pinned playlists")
                LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(ui.pinned, key = { it.id }) { p -> CoverCard(p.name, "${p.songCount} songs", vm.cover(p.coverArt, CoverSize.CARD), 150.dp, { nav.playlist(p.id) }) }
                }
            }
            ui.rows.forEach { (row, albums) -> shelf(row.title, albums, vm) }
        }
    }
}

private fun androidx.compose.foundation.lazy.LazyListScope.shelf(title: String, albums: List<Album>, vm: HomeViewModel) {
    if (albums.isEmpty()) return
    item(key = title, contentType = "shelf") {
        val nav = LocalNav.current
        SectionTitle(title)
        LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            items(albums, key = { it.id }, contentType = { "album" }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 150.dp, { nav.album(a.id) }) }
        }
    }
}
