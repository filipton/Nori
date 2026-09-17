package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Star
import androidx.compose.material.icons.filled.StarBorder
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
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
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.ffi.Playlist
import dev.flint.music.ffi.Song

@Composable
private fun Item(text: String, onClick: () -> Unit) {
    Text(text, Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 24.dp, vertical = 14.dp))
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SongMenu(song: Song, actions: ActionsViewModel, onDismiss: () -> Unit) {
    val nav = LocalNav.current
    val downloads by actions.downloads.collectAsState()
    var picking by remember { mutableStateOf(false) }
    if (picking) { PlaylistPicker(listOf(song), actions) { picking = false; onDismiss() }; return }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.verticalScroll(rememberScrollState()).navigationBarsPadding()) {
            Text(song.title, Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.titleMedium)
            Text("${song.artist} · ${song.album}", Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodySmall)
            if (song.suffix.isNotEmpty()) Text(
                listOfNotNull(song.suffix.uppercase(), song.bitRate.takeIf { it > 0u }?.let { "$it kbps" }, song.samplingRate.takeIf { it > 0u }?.let { "${it.toInt() / 1000.0} kHz" }, song.bitDepth.takeIf { it > 0u }?.let { "$it bit" }).joinToString(" · "),
                Modifier.padding(horizontal = 24.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Row(Modifier.padding(horizontal = 12.dp)) {
                for (n in 1..5) IconButton({ actions.rate(song, if (song.userRating.toInt() == n) 0 else n); onDismiss() }) {
                    Icon(if (n <= song.userRating.toInt()) Icons.Filled.Star else Icons.Filled.StarBorder, "Rate $n")
                }
            }
            HorizontalDivider()
            Item("Play next") { actions.playNext(listOf(song)); onDismiss() }
            Item("Add to queue") { actions.enqueue(listOf(song)); onDismiss() }
            Item("Start radio from this song") { actions.startRadio(song); onDismiss() }
            Item(if (song.starred) "Remove from favourites" else "Add to favourites") { actions.star(song, !song.starred); onDismiss() }
            Item("Add to playlist…") { picking = true }
            if (song.id in downloads.doneIds || song.id in downloads.pendingIds) Item("Remove download") { actions.removeDownloads(listOf(song.id)); onDismiss() }
            else Item("Download") { actions.download(listOf(song)); onDismiss() }
            if (!song.isExternal) Item("Share link") { actions.share(song.id); onDismiss() }
            song.albumId?.let { id -> Item("Go to album") { nav.album(id); onDismiss() } }
            song.artistId?.let { id -> Item("Go to artist") { nav.artist(id); onDismiss() } }
        }
    }
}

@Composable
fun PlaylistPicker(songs: List<Song>, actions: ActionsViewModel, onDone: () -> Unit) {
    var playlists by remember { mutableStateOf<List<Playlist>?>(null) }
    var name by remember { mutableStateOf("") }
    LaunchedEffect(Unit) { playlists = actions.playlists() }
    AlertDialog(
        onDismissRequest = onDone,
        title = { Text("Add to playlist") },
        text = {
            Column(Modifier.verticalScroll(rememberScrollState())) {
                OutlinedTextField(name, { name = it }, label = { Text("New playlist") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                playlists?.forEach { p -> Text(p.name, Modifier.fillMaxWidth().clickable { actions.addToPlaylist(p, songs); onDone() }.padding(vertical = 12.dp)) }
            }
        },
        confirmButton = { TextButton({ actions.addToNewPlaylist(name.trim(), songs); onDone() }, enabled = name.isNotBlank()) { Text("Create") } },
        dismissButton = { TextButton(onDone) { Text("Cancel") } },
    )
}
