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
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.StarBorder
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
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
    Text(
        text, Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = Space.gutter, vertical = 15.dp),
        style = MaterialTheme.typography.bodyLarge,
    )
}

@OptIn(ExperimentalMaterial3Api::class)
@Composable
fun SongMenu(
    song: Song,
    actions: ActionsViewModel,
    onDismiss: () -> Unit,
    /**
     * Set when the player's own ⋯ opened this. The sleep timer is a property of the evening, not of
     * the song, so it has no business on a row's menu in a list - but the player needs it somewhere
     * now that the bottom of that screen belongs to the output switcher, the way Apple's does.
     */
    player: dev.flint.music.app.vm.PlayerViewModel? = null,
) {
    val nav = LocalNav.current
    val downloads by actions.downloads.collectAsState()
    var picking by remember { mutableStateOf(false) }
    var details by remember { mutableStateOf(false) }
    var sleeping by remember { mutableStateOf(false) }
    if (sleeping && player != null) { SleepMenu(player) { sleeping = false; onDismiss() }; return }
    if (details) { TrackInfo(song) { details = false; onDismiss() }; return }
    if (picking) { PlaylistPicker(listOf(song), actions) { picking = false; onDismiss() }; return }

    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.verticalScroll(rememberScrollState()).navigationBarsPadding()) {
            // The track leads the sheet, the way the row it came from looked.
            Row(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 4.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                Cover(actions.cover(song.coverArt, CoverSize.ROW), 52.dp, radius = 8.dp)
                Column(Modifier.weight(1f).padding(start = 12.dp)) {
                    Text(song.title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                    Text("${song.artist} · ${song.album}", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                    if (song.suffix.isNotEmpty()) Caption(
                        listOfNotNull(song.suffix.uppercase(), song.bitRate.takeIf { it > 0u }?.let { "$it kbps" }, song.samplingRate.takeIf { it > 0u }?.let { "${it.toInt() / 1000.0} kHz" }, song.bitDepth.takeIf { it > 0u }?.let { "$it bit" }).joinToString(" · "),
                    )
                }
            }
            Row(Modifier.padding(horizontal = 14.dp)) {
                for (n in 1..5) IconButton({ actions.rate(song, if (song.userRating.toInt() == n) 0 else n); onDismiss() }) {
                    Icon(if (n <= song.userRating.toInt()) Icons.Filled.Star else Icons.Filled.StarBorder, "Rate $n")
                }
            }
            Hairline(startIndent = Space.gutter)
            if (song.isExternal) Item("Add to library (${providerOf(song.id) ?: "provider"})") { actions.addToLibrary(song.id, isAlbum = false); onDismiss() }
            player?.let { Item("Sleep timer…") { sleeping = true } }
            Item("Play next") { actions.playNext(listOf(song)); onDismiss() }
            Item("Add to queue") { actions.enqueue(listOf(song)); onDismiss() }
            Item("Start radio from this song") { actions.startRadio(song); onDismiss() }
            if (!song.isExternal) Item("Instant mix") { actions.instantMix(song); onDismiss() }
            if (!song.isExternal) Item("Exclude from mixes") { actions.excludeFromMixes(song); onDismiss() }
            val fav = LocalStarMarks.current.effectiveStar(dev.flint.music.data.StarKind.SONG, song.id, song.starred)
            Item(if (fav) "Remove from favourites" else "Add to favourites") { actions.star(song, !fav); onDismiss() }
            Item("Add to playlist…") { picking = true }
            if (song.id in downloads.doneIds || song.id in downloads.pendingIds) Item("Remove download") { actions.removeDownloads(listOf(song.id)); onDismiss() }
            else Item("Download") { actions.download(listOf(song)); onDismiss() }
            if (!song.isExternal) Item("Share link") { actions.share(song.id); onDismiss() }
            song.albumId?.let { id -> Item("Go to album") { nav.album(id); onDismiss() } }
            if (song.artists.size > 1) song.artists.filter { it.id.isNotEmpty() }.forEach { a -> Item("Go to ${a.name}") { nav.artist(a.id); onDismiss() } }
            else song.artistId?.let { id -> Item("Go to artist") { nav.artist(id); onDismiss() } }
            Item("Details") { details = true }
        }
    }
}

/** The sleep choices on their own sheet, so the song's menu is not buried under eleven of them. */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun SleepMenu(player: dev.flint.music.app.vm.PlayerViewModel, onDone: () -> Unit) {
    val state by player.state.collectAsState()
    ModalBottomSheet(onDismissRequest = onDone) {
        Column(Modifier.verticalScroll(rememberScrollState()).navigationBarsPadding()) {
            SectionTitle("Sleep timer")
            val running = state.sleepAt > 0 || state.sleepAtEndOfTrack
            if (running) Item("Off") { player.sleep(0); onDone() }
            for (m in listOf(15, 30, 45, 60)) Item("$m minutes") { player.sleep(m); onDone() }
            Item("End of track") { player.sleep(0, endOfTrack = true); onDone() }
            for (n in listOf(2, 3, 5, 10)) Item("After $n songs") { player.sleep(0, songs = n); onDone() }
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

/** Everything the server said about one file. */
@Composable
fun TrackInfo(song: Song, onDone: () -> Unit) {
    val rows = listOfNotNull(
        "Title" to song.title, "Artist" to song.artists.joinToString(", ") { it.name }.ifEmpty { song.artist }, "Album" to song.album,
        "Track" to listOfNotNull(song.discNumber.takeIf { it > 0u }?.let { "disc $it" }, song.track.takeIf { it > 0u }?.let { "track $it" }).joinToString(", "),
        "Year" to song.year.takeIf { it > 0u }?.toString(), "Genre" to song.genre, "Duration" to duration(song.duration.toLong()),
        "Format" to listOfNotNull(song.suffix.uppercase().ifEmpty { null }, song.contentType.ifEmpty { null }).joinToString(" · "),
        "Quality" to listOfNotNull(song.bitRate.takeIf { it > 0u }?.let { "$it kbps" }, song.samplingRate.takeIf { it > 0u }?.let { "${it.toInt() / 1000.0} kHz" }, song.bitDepth.takeIf { it > 0u }?.let { "$it bit" }, song.channelCount.takeIf { it > 0u }?.let { "$it ch" }).joinToString(" · "),
        "Size" to song.size.takeIf { it > 0u }?.let { "%.1f MB".format(it.toDouble() / 1_048_576) },
        "ReplayGain" to song.replayGain?.let { g -> listOfNotNull(g.trackGain?.let { "track %+.2f dB".format(it) }, g.albumGain?.let { "album %+.2f dB".format(it) }, g.trackPeak?.let { "peak %.3f".format(it) }).joinToString(" · ") },
        "BPM" to song.bpm.takeIf { it > 0u }?.toString(), "Plays (server)" to song.playCount.takeIf { it > 0u }?.toString(), "Last played" to song.played?.take(16)?.replace('T', ' '),
        "Added" to song.created?.take(10), "Path" to song.path, "MusicBrainz" to song.musicBrainzId, "Comment" to song.comment, "Id" to song.id,
    ).filter { !it.second.isNullOrBlank() }
    AlertDialog(
        onDismissRequest = onDone, title = { Text("Details") },
        text = { Column(Modifier.verticalScroll(rememberScrollState())) { rows.forEach { (k, v) -> Text(k, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant); Text(v!!, Modifier.padding(bottom = 8.dp)) } } },
        confirmButton = { TextButton(onDone) { Text("Close") } },
    )
}

/** Shown instead of nothing while songs are selected: the batch actions. */
@Composable
fun SelectionBar(actions: ActionsViewModel) {
    val selection by actions.selection.collectAsState()
    if (selection.isEmpty()) return
    var picking by remember { mutableStateOf(false) }
    if (picking) PlaylistPicker(selection, actions) { picking = false; actions.clearSelection() }
    androidx.compose.material3.Surface(tonalElevation = 6.dp) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
            Text("${selection.size} selected", Modifier.weight(1f).padding(start = 8.dp))
            TextButton({ actions.play(selection); actions.clearSelection() }) { Text("Play") }
            TextButton({ actions.playNext(selection); actions.clearSelection() }) { Text("Next") }
            TextButton({ actions.enqueue(selection); actions.clearSelection() }) { Text("Queue") }
            TextButton({ picking = true }) { Text("Playlist") }
            TextButton({ actions.download(selection); actions.clearSelection() }) { Text("Get") }
            IconButton(actions::clearSelection) { Icon(androidx.compose.material.icons.Icons.Filled.Close, "Clear selection") }
        }
    }
}
