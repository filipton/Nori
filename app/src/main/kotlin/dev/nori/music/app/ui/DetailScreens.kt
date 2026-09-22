package dev.nori.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.PlaylistAdd
import androidx.compose.material.icons.filled.Download
import androidx.compose.material.icons.filled.IosShare
import androidx.compose.material.icons.filled.Favorite
import androidx.compose.material.icons.filled.FavoriteBorder
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
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
import dev.nori.music.app.vm.ActionsViewModel
import dev.nori.music.app.vm.AlbumViewModel
import dev.nori.music.app.vm.ArtistUi
import dev.nori.music.app.vm.ArtistViewModel
import dev.nori.music.app.vm.FolderViewModel
import dev.nori.music.app.vm.GenreViewModel
import dev.nori.music.app.vm.Load
import dev.nori.music.app.vm.PlayerViewModel
import dev.nori.music.app.vm.PlaylistViewModel
import dev.nori.music.app.vm.SettingsViewModel
import dev.nori.music.ffi.Album
import dev.nori.music.ffi.AlbumDetail
import dev.nori.music.ffi.Artist
import dev.nori.music.ffi.PlaylistDetail
import dev.nori.music.ffi.Song
import dev.nori.music.settings.Prefs

@Composable
private fun Header(title: String, subtitle: String, coverUrl: String?, actions: @Composable () -> Unit = {}) {
    val nav = LocalNav.current
    var fullscreen by remember { mutableStateOf(false) }
    if (fullscreen && coverUrl != null) androidx.compose.ui.window.Dialog({ fullscreen = false }) { Cover(coverUrl, 0.dp, Modifier.fillMaxWidth().clickable { fullscreen = false }) }
    Column {
        Row(Modifier.padding(end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.headlineSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                Caption(subtitle, Modifier.padding(top = 2.dp))
            }
            actions()
        }
        if (coverUrl != null) Cover(coverUrl, 220.dp, Modifier.padding(Space.gutter).align(Alignment.CenterHorizontally).clickable { fullscreen = true }, radius = Radius.card)
    }
}

/**
 * Play and shuffle live in the hero now; a long playlist still wants a way to narrow itself. An album
 * does not, which is why its page no longer offers this: nobody reaches for a search box to find a
 * track among ten they can already see.
 */
@Composable
private fun FilterField(count: Int, filter: String, onFilter: (String) -> Unit) {
    if (count <= 12 && filter.isEmpty()) return
    SearchField(filter, onFilter, "Filter", Modifier.padding(horizontal = Space.gutter, vertical = 4.dp))
}

/** The row of buttons the screens without a hero still use. */
@Composable
private fun PlayButtons(songs: List<Song>, actions: ActionsViewModel) {
    val player: PlayerViewModel = viewModel()
    val shuffling by player.state.collectAsStateWithLifecycle()
    Row(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 8.dp), Arrangement.spacedBy(10.dp)) {
        PillButton("Play", Icons.Filled.PlayArrow, { actions.play(songs) }, Modifier.weight(1f), prominent = true, enabled = songs.isNotEmpty())
        PillButton(
            "Shuffle", Icons.Filled.Shuffle, { actions.shuffle(songs) }, Modifier.weight(1f),
            prominent = shuffling.shuffle, enabled = songs.isNotEmpty(),
        )
        IconButton({ actions.enqueue(songs) }, enabled = songs.isNotEmpty()) { Icon(Icons.AutoMirrored.Filled.PlaylistAdd, "Add all to queue") }
        IconButton({ actions.download(songs) }, enabled = songs.isNotEmpty()) { Icon(Icons.Filled.Download, "Download all") }
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


/**
 * "Download" is the wrong word once the songs are already here, and so is offering all of them when
 * only a few are missing. This says what is actually left to do - and offers to give the space back
 * when there is nothing left.
 */
@Composable
internal fun downloadEntry(songs: List<Song>, done: Set<String>, actions: ActionsViewModel): Pair<String, () -> Unit> {
    val missing = songs.filterNot { it.id in done }
    return when {
        songs.isEmpty() -> "Download" to { actions.download(songs) }
        missing.isEmpty() -> "Remove downloads" to { actions.undownload(songs) }
        missing.size == songs.size -> "Download" to { actions.download(songs) }
        else -> "Download the other ${missing.size}" to { actions.download(missing) }
    }
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
    // What the row that opened this page already knew: enough for the hero to be there from the first
    // frame of the slide (PageMotion). Without it the page arrives as a bare card and fills in when
    // the server answers - see docs/motion.md item 16. A deep link or "Go to album" from a song has
    // no hint, so those still wait behind LoadBox.
    val hint = nav.albumHint(id)
    val detail = (load as? Load.Ready)?.data
    val album = detail?.album ?: hint
    if (album == null) {
        LoadBox(load) { d -> AlbumBody(d, actions, vm, done, selected, menu, playing, nav) }
        return
    }
    val discs = remember(detail) {
        detail?.songs?.groupBy { it.discNumber.toInt().coerceAtLeast(1) }?.toSortedMap().orEmpty()
    }
    // This page's own queue: its songs, plus a song of this record some other queue carried along.
    val songIds = remember(detail) { detail?.songs?.mapTo(HashSet()) { it.id } ?: emptySet() }
    HeroPage(
        coverUrl = vm.cover(album.coverArt, CoverSize.FULL),
        title = album.name,
        subtitle = album.artist,
        caption = if (detail != null) listOfNotNull(
            album.year.takeIf { it > 0u }?.toString(), "${detail.songs.size} songs",
            duration(detail.songs.sumOf { it.duration.toLong() }), quality(detail.songs),
            "explicit".takeIf { album.explicitStatus == "explicit" },
        ).joinToString(" · ") else listOfNotNull(
            album.year.takeIf { it > 0u }?.toString(),
            album.songCount.takeIf { it > 0u }?.let { "$it songs" },
            album.duration.takeIf { it > 0u }?.let { duration(it.toLong()) },
        ).joinToString(" · "),
        onSubtitle = album.artistId?.let { a ->
            { nav.artist(a, Artist(a, album.artist, album.coverArt, null, 0u, false, false)) }
        },
        // Play and shuffle wait for the songs: pressing them with an empty list would queue nothing.
        // The row itself is reserved ([awaitingPlay]) so the page does not reflow when they land.
        awaitingPlay = detail == null && load !is Load.Failed,
        onPlay = detail?.let { d -> { actions.play(d.songs) } },
        onShuffle = detail?.let { d -> { actions.shuffle(d.songs) } },
        playingHere = { s -> s.current?.let { c -> c.id in songIds || c.albumId == album.id } == true },
        actions = {
            val albumStarred = LocalStarMarks.current.effectiveStar(dev.nori.music.data.StarKind.ALBUM, album.id, album.starred)
            FavoriteCircle(albumStarred) { actions.starAlbum(album.id, !albumStarred); Unit }
            // Fixed 46 dp slot: More fades in when the songs land so the Play pill never shifts.
            Box(Modifier.size(46.dp), contentAlignment = Alignment.Center) {
                androidx.compose.animation.AnimatedVisibility(
                    visible = detail != null,
                    enter = androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(if (AppMotion.reduce) 0 else 220)),
                    exit = androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(if (AppMotion.reduce) 0 else 120)),
                ) {
                    MoreCircle(listOf("Add to queue" to { actions.enqueue(detail!!.songs) }, downloadEntry(detail!!.songs, done, actions)))
                }
            }
        },
    ) {
        when {
            detail != null -> item(key = "body") {
                Arrive {
                    Column {
                        if (album.isExternal || album.id.startsWith("pl-")) {
                            TextButton({ actions.addToLibrary(album.id, isAlbum = true) }, Modifier.padding(horizontal = 12.dp)) {
                                Text("Add the whole ${if (album.id.startsWith("pl-")) "playlist" else "album"} to the library (${providerOf(album.id) ?: "provider"})")
                            }
                        }
                        discs.forEach { (disc, tracks) ->
                            if (discs.size > 1) {
                                val title = detail.discTitles.firstOrNull { it.disc.toInt() == disc }?.title
                                SectionTitle(if (title.isNullOrBlank()) "Disc $disc" else "Disc $disc · $title")
                            }
                            val (onRight, onLeft) = actions.swipes
                            tracks.forEachIndexed { i, s ->
                                SongRow(
                                    s, null,
                                    onClick = { actions.tap(detail.songs, detail.songs.indexOfFirst { it.id == s.id }.coerceAtLeast(0)) },
                                    onMenu = { menu(s) },
                                    number = s.track.toInt(), playing = s.id == playing, downloaded = s.id in done,
                                    selected = s.id in selected, onLongClick = { actions.toggleSelected(s) },
                                    swipeRight = rowSwipe(onRight, s, actions),
                                    swipeLeft = rowSwipe(onLeft, s, actions),
                                    divider = i < tracks.lastIndex,
                                    showArtist = !s.artist.equals(album.artist, ignoreCase = true),
                                )
                            }
                        }
                    }
                }
            }
            load is Load.Failed -> item(key = "fail") {
                Text(
                    (load as Load.Failed).message,
                    Modifier.fillMaxWidth().padding(Space.gutter),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                )
            }
            // The hero is already the page; a spinner under it would be a second thing to look at
            // while the songs are on a short wire. Empty until they land.
            else -> Unit
        }
    }
}

@Composable
private fun AlbumBody(
    d: AlbumDetail,
    actions: ActionsViewModel,
    vm: AlbumViewModel,
    done: Set<String>,
    selected: Set<String>,
    menu: (Song) -> Unit,
    playing: String?,
    nav: Nav,
) {
    // An album is short enough to scroll and its running order is the point of it, so the songs
    // stay exactly as the record has them, grouped by disc and never narrowed.
    val discs = remember(d) { d.songs.groupBy { it.discNumber.toInt().coerceAtLeast(1) }.toSortedMap() }
    val songIds = remember(d) { d.songs.mapTo(HashSet()) { it.id } }
    HeroPage(
        coverUrl = vm.cover(d.album.coverArt, CoverSize.FULL),
        title = d.album.name,
        subtitle = d.album.artist,
        caption = listOfNotNull(
            d.album.year.takeIf { it > 0u }?.toString(), "${d.songs.size} songs",
            duration(d.songs.sumOf { it.duration.toLong() }), quality(d.songs),
            "explicit".takeIf { d.album.explicitStatus == "explicit" },
        ).joinToString(" · "),
        onSubtitle = d.album.artistId?.let { a ->
            { nav.artist(a, Artist(a, d.album.artist, d.album.coverArt, null, 0u, false, false)) }
        },
        onPlay = { actions.play(d.songs) },
        onShuffle = { actions.shuffle(d.songs) },
        playingHere = { s -> s.current?.let { c -> c.id in songIds || c.albumId == d.album.id } == true },
        actions = {
            val albumStarred = LocalStarMarks.current.effectiveStar(dev.nori.music.data.StarKind.ALBUM, d.album.id, d.album.starred)
            FavoriteCircle(albumStarred) { actions.starAlbum(d.album.id, !albumStarred); Unit }
            MoreCircle(listOf("Add to queue" to { actions.enqueue(d.songs) }, downloadEntry(d.songs, done, actions)))
        },
    ) {
        if (d.album.isExternal || d.album.id.startsWith("pl-")) item(key = "header") {
            TextButton({ actions.addToLibrary(d.album.id, isAlbum = true) }, Modifier.padding(horizontal = 12.dp)) {
                Text("Add the whole ${if (d.album.id.startsWith("pl-")) "playlist" else "album"} to the library (${providerOf(d.album.id) ?: "provider"})")
            }
        }
        discs.forEach { (disc, tracks) ->
            if (discs.size > 1) item(key = "disc$disc") {
                val title = d.discTitles.firstOrNull { it.disc.toInt() == disc }?.title
                SectionTitle(if (title.isNullOrBlank()) "Disc $disc" else "Disc $disc · $title")
            }
            songRows(tracks, actions, playing, done, selected, menu, numbered = true, keyPrefix = "d$disc-", context = d.songs, pageArtist = d.album.artist)
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
    val hint = nav.artistHint(id)
    val ui = (load as? Load.Ready)?.data
    val artist = ui?.detail?.artist ?: hint
    if (artist == null) {
        LoadBox(load) { ready -> ArtistBody(ready, actions, vm, done, selected, menu, playing, nav) { leaving = it } }
        return
    }
    val groups = remember(ui?.detail) {
        ui?.detail?.albums?.sortedByDescending { it.year }?.groupBy { it.group() }
            ?.toSortedMap(compareBy { g -> releaseOrder.indexOf(g).let { if (it < 0) 99 else it } })
            .orEmpty()
    }
    // An artist's page has no song list of its own - the queue is every album's songs - so what
    // makes it the page playing is a song of one of its albums sounding.
    val albumIds = remember(ui?.detail) { ui?.detail?.albums?.mapTo(HashSet()) { it.id } ?: emptySet() }
    HeroPage(
        coverUrl = vm.cover(artist.coverArt, CoverSize.FULL),
        title = artist.name,
        caption = if (ui != null) "${ui.detail.albums.size} releases"
            else artist.albumCount.takeIf { it > 0u }?.let { "$it releases" }.orEmpty(),
        onPlay = ui?.let { ready -> { actions.playArtist(ready.detail.albums) } },
        onShuffle = ui?.let { ready -> { actions.playArtist(ready.detail.albums, shuffle = true) } },
        awaitingPlay = ui == null && load !is Load.Failed,
        playingHere = { s -> s.current?.albumId?.let(albumIds::contains) == true },
        actions = {
            val artistStarred = LocalStarMarks.current.effectiveStar(dev.nori.music.data.StarKind.ARTIST, artist.id, artist.starred)
            FavoriteCircle(artistStarred) { actions.starArtist(artist.id, !artistStarred); Unit }
            Box(Modifier.size(46.dp), contentAlignment = Alignment.Center) {
                androidx.compose.animation.AnimatedVisibility(
                    visible = ui != null,
                    enter = androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(if (AppMotion.reduce) 0 else 220)),
                    exit = androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(if (AppMotion.reduce) 0 else 120)),
                ) {
                    MoreCircle(
                        listOf(
                            "Add to queue" to { actions.queueArtist(ui!!.detail.albums) },
                            "Download everything" to { actions.downloadArtist(ui!!.detail.albums) },
                        ),
                    )
                }
            }
        },
    ) {
        when {
            ui != null -> item(key = "body") {
                Arrive {
                    Column {
                        ui.info?.biography?.let { Text(it.substringBefore("<a "), Modifier.padding(horizontal = Space.gutter), maxLines = 4, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        Row(Modifier.padding(horizontal = 12.dp)) {
                            ui.info?.lastFmUrl?.let { u -> TextButton({ leaving = u }) { Text("last.fm") } }
                            ui.info?.musicBrainzId?.let { m -> TextButton({ leaving = "https://musicbrainz.org/artist/$m" }) { Text("MusicBrainz") } }
                        }
                        groups.forEach { (group, albums) ->
                            SectionTitle(if (group.endsWith("s")) group else "${group}s")
                            LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                items(albums, key = { it.id }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 120.dp, { nav.album(a.id, a) }) }
                            }
                        }
                        if (ui.top.isNotEmpty()) {
                            SectionTitle("Top songs")
                            val (onRight, onLeft) = actions.swipes
                            ui.top.forEachIndexed { i, s ->
                                SongRow(
                                    s, vm.cover(s.coverArt, CoverSize.ROW),
                                    onClick = { actions.tap(ui.top, i) }, onMenu = { menu(s) },
                                    playing = s.id == playing, downloaded = s.id in done,
                                    selected = s.id in selected, onLongClick = { actions.toggleSelected(s) },
                                    swipeRight = rowSwipe(onRight, s, actions), swipeLeft = rowSwipe(onLeft, s, actions),
                                    divider = i < ui.top.lastIndex,
                                )
                            }
                        }
                        ui.info?.similar?.filter { it.id.isNotEmpty() }?.takeIf { it.isNotEmpty() }?.let { similar ->
                            SectionTitle("Similar artists")
                            LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                items(similar, key = { it.id }) { a ->
                                    Text(a.name, Modifier.clickable { nav.artist(a.id, a) }.padding(8.dp), color = MaterialTheme.colorScheme.primary)
                                }
                            }
                        }
                    }
                }
            }
            load is Load.Failed -> item(key = "fail") {
                Text(
                    (load as Load.Failed).message,
                    Modifier.fillMaxWidth().padding(Space.gutter),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                )
            }
            else -> Unit
        }
    }
}

@Composable
private fun ArtistBody(
    ui: ArtistUi,
    actions: ActionsViewModel,
    vm: ArtistViewModel,
    done: Set<String>,
    selected: Set<String>,
    menu: (Song) -> Unit,
    playing: String?,
    nav: Nav,
    leave: (String) -> Unit,
) {
    val groups = remember(ui.detail) { ui.detail.albums.sortedByDescending { it.year }.groupBy { it.group() }.toSortedMap(compareBy { g -> releaseOrder.indexOf(g).let { if (it < 0) 99 else it } }) }
    val albumIds = remember(ui.detail) { ui.detail.albums.mapTo(HashSet()) { it.id } }
    HeroPage(
        coverUrl = vm.cover(ui.detail.artist.coverArt, CoverSize.FULL),
        title = ui.detail.artist.name,
        caption = "${ui.detail.albums.size} releases",
        onPlay = { actions.playArtist(ui.detail.albums) },
        onShuffle = { actions.playArtist(ui.detail.albums, shuffle = true) },
        playingHere = { s -> s.current?.albumId?.let(albumIds::contains) == true },
        actions = {
            val artistStarred = LocalStarMarks.current.effectiveStar(dev.nori.music.data.StarKind.ARTIST, ui.detail.artist.id, ui.detail.artist.starred)
            FavoriteCircle(artistStarred) { actions.starArtist(ui.detail.artist.id, !artistStarred); Unit }
            MoreCircle(
                listOf(
                    "Add to queue" to { actions.queueArtist(ui.detail.albums) },
                    "Download everything" to { actions.downloadArtist(ui.detail.albums) },
                ),
            )
        },
    ) {
        item(key = "header") {
            ui.info?.biography?.let { Text(it.substringBefore("<a "), Modifier.padding(horizontal = Space.gutter), maxLines = 4, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            Row(Modifier.padding(horizontal = 12.dp)) {
                ui.info?.lastFmUrl?.let { u -> TextButton({ leave(u) }) { Text("last.fm") } }
                ui.info?.musicBrainzId?.let { m -> TextButton({ leave("https://musicbrainz.org/artist/$m") }) { Text("MusicBrainz") } }
            }
        }
        groups.forEach { (group, albums) ->
            item(key = "g-$group") {
                SectionTitle(if (group.endsWith("s")) group else "${group}s")
                LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(albums, key = { it.id }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 120.dp, { nav.album(a.id, a) }) }
                }
            }
        }
        if (ui.top.isNotEmpty()) item(key = "top") { SectionTitle("Top songs") }
        songRows(ui.top, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) }, keyPrefix = "top-")
        ui.info?.similar?.filter { it.id.isNotEmpty() }?.takeIf { it.isNotEmpty() }?.let { similar ->
            item(key = "similar") {
                SectionTitle("Similar artists")
                LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(similar, key = { it.id }) { a ->
                        Text(a.name, Modifier.clickable { nav.artist(a.id, a) }.padding(8.dp), color = MaterialTheme.colorScheme.primary)
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
    val nav = LocalNav.current
    val detail = (load as? Load.Ready)?.data
    val hint = nav.playlistHint(id)
    val playlist = detail?.playlist ?: hint
    val exportM3u = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.CreateDocument("audio/x-mpegurl")) { uri ->
        if (uri != null && detail != null) runCatching { context.contentResolver.openOutputStream(uri)?.use { it.write(actions.exportM3u(detail.playlist.name, detail.songs).toByteArray()) } }
    }
    if (playlist == null) {
        LoadBox(load) { d -> PlaylistBody(d, id, actions, vm, settings, prefs, done, selected, menu, playing, filter, { filter = it }, { exportM3u.launch("${d.playlist.name}.m3u8") }) }
        return
    }
    val shown = remember(detail, filter) { detail?.songs?.matching(filter).orEmpty() }
    // This playlist's own queue: its songs, whatever order they are being played in.
    val songIds = remember(detail) { detail?.songs?.mapTo(HashSet()) { it.id } ?: emptySet() }
    HeroPage(
        coverUrl = vm.cover(playlist.coverArt, CoverSize.FULL),
        title = playlist.name,
        subtitle = playlist.comment?.ifEmpty { null },
        caption = if (detail != null) "${detail.songs.size} songs · ${duration(detail.songs.sumOf { it.duration.toLong() })}"
            else listOfNotNull(
                playlist.songCount.takeIf { it > 0u }?.let { "$it songs" },
                playlist.duration.takeIf { it > 0u }?.let { duration(it.toLong()) },
            ).joinToString(" · "),
        onPlay = detail?.let { d -> { actions.play(d.songs) } },
        onShuffle = detail?.let { d -> { actions.shuffle(d.songs) } },
        awaitingPlay = detail == null && load !is Load.Failed,
        playingHere = { s -> s.current?.id?.let(songIds::contains) == true },
        actions = {
            val pinned = id in prefs.pinnedPlaylists
            // A favourite, drawn and named as every other favourite in the app is: a heart, filled
            // when it is one. It was a pin with one look for both states, so there was no telling
            // from the page whether this playlist was on the home page or not. The server has no
            // way to star a playlist, so it is kept on this phone, and the home page's shelf of
            // them reads it.
            FavoriteCircle(pinned) { settings.update { it.copy(pinnedPlaylists = if (pinned) it.pinnedPlaylists - id else it.pinnedPlaylists + id) }; Unit }
            Box(Modifier.size(46.dp), contentAlignment = Alignment.Center) {
                androidx.compose.animation.AnimatedVisibility(
                    visible = detail != null,
                    enter = androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(if (AppMotion.reduce) 0 else 220)),
                    exit = androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(if (AppMotion.reduce) 0 else 120)),
                ) {
                    MoreCircle(
                        listOf(
                            "Add to queue" to { actions.enqueue(detail!!.songs) },
                            downloadEntry(detail!!.songs, done, actions),
                            "Export M3U" to { exportM3u.launch("${playlist.name}.m3u8") },
                        ),
                    )
                }
            }
        },
    ) {
        when {
            detail != null -> item(key = "body") {
                Arrive {
                    Column {
                        FilterField(detail.songs.size, filter) { filter = it }
                        val (onRight, onLeft) = actions.swipes
                        shown.forEachIndexed { i, s ->
                            SongRow(
                                s, vm.cover(s.coverArt, CoverSize.ROW),
                                onClick = { actions.tap(detail.songs, detail.songs.indexOfFirst { it.id == s.id }.coerceAtLeast(0)) },
                                onMenu = { menu(s) },
                                playing = s.id == playing, downloaded = s.id in done,
                                selected = s.id in selected, onLongClick = { actions.toggleSelected(s) },
                                swipeRight = rowSwipe(onRight, s, actions), swipeLeft = rowSwipe(onLeft, s, actions),
                                divider = i < shown.lastIndex,
                            )
                        }
                    }
                }
            }
            load is Load.Failed -> item(key = "fail") {
                Text(
                    (load as Load.Failed).message,
                    Modifier.fillMaxWidth().padding(Space.gutter),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                    textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                )
            }
            else -> Unit
        }
    }
}

@Composable
private fun PlaylistBody(
    d: PlaylistDetail,
    id: String,
    actions: ActionsViewModel,
    vm: PlaylistViewModel,
    settings: SettingsViewModel,
    prefs: Prefs,
    done: Set<String>,
    selected: Set<String>,
    menu: (Song) -> Unit,
    playing: String?,
    filter: String,
    onFilter: (String) -> Unit,
    onExport: () -> Unit,
) {
    val shown = remember(d, filter) { d.songs.matching(filter) }
    val songIds = remember(d) { d.songs.mapTo(HashSet()) { it.id } }
    HeroPage(
        coverUrl = vm.cover(d.playlist.coverArt, CoverSize.FULL),
        title = d.playlist.name,
        subtitle = d.playlist.comment?.ifEmpty { null },
        caption = "${d.songs.size} songs · ${duration(d.songs.sumOf { it.duration.toLong() })}",
        onPlay = { actions.play(d.songs) },
        onShuffle = { actions.shuffle(d.songs) },
        playingHere = { s -> s.current?.id?.let(songIds::contains) == true },
        actions = {
            val pinned = id in prefs.pinnedPlaylists
            FavoriteCircle(pinned) { settings.update { it.copy(pinnedPlaylists = if (pinned) it.pinnedPlaylists - id else it.pinnedPlaylists + id) }; Unit }
            MoreCircle(
                listOf(
                    "Add to queue" to { actions.enqueue(d.songs) },
                    downloadEntry(d.songs, done, actions),
                    "Export M3U" to onExport,
                ),
            )
        },
    ) {
        item(key = "header") { FilterField(d.songs.size, filter) { onFilter(it) } }
        songRows(shown, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
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
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
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
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            item(key = "header") { Header(d.name.ifEmpty { "Folder" }, "${d.folders.size} folders · ${d.songs.size} songs", null); if (d.songs.isNotEmpty()) PlayButtons(d.songs, actions) }
            items(d.folders, key = { "f" + it.id }) { f -> Text("📁  ${f.name}", Modifier.fillMaxWidth().clickable { nav.folder(f.id) }.padding(horizontal = 16.dp, vertical = 14.dp)) }
            songRows(d.songs, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
        }
    }
}
