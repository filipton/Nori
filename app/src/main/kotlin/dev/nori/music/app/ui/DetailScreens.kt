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
import dev.nori.music.ffi.model.Album
import dev.nori.music.ffi.library.AlbumDetail
import dev.nori.music.ffi.model.Artist
import dev.nori.music.ffi.library.PlaylistDetail
import dev.nori.music.ffi.model.Song
import dev.nori.music.settings.Prefs

@Composable
private fun Header(title: String, subtitle: String, coverUrl: String?, actions: @Composable () -> Unit = {}) {
    val nav = LocalNav.current
    var fullscreen by remember { mutableStateOf(false) }
    if (fullscreen && coverUrl != null) androidx.compose.ui.window.Dialog({ fullscreen = false }) { Cover(coverUrl, 0.dp, Modifier.fillMaxWidth().clickable { fullscreen = false }) }
    Column {
        Row(Modifier.padding(end = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, say.back) }
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
    val filtering = filter.isNotEmpty()
    if (!remember(count, filtering) { dev.nori.music.ffi.library.filterOffered(count.toUInt(), filtering) }) return
    SearchField(filter, onFilter, say.filter, Modifier.padding(horizontal = Space.gutter, vertical = 4.dp))
}

/** The row of buttons the screens without a hero still use. */
@Composable
private fun PlayButtons(songs: List<Song>, actions: ActionsViewModel) {
    val player: PlayerViewModel = viewModel()
    val shuffling by player.state.collectAsStateWithLifecycle()
    Row(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 8.dp), Arrangement.spacedBy(10.dp)) {
        PillButton(say.play, Icons.Filled.PlayArrow, { actions.play(songs) }, Modifier.weight(1f), prominent = true, enabled = songs.isNotEmpty())
        PillButton(
            say.shuffle, Icons.Filled.Shuffle, { actions.shuffle(songs) }, Modifier.weight(1f),
            prominent = shuffling.shuffle, enabled = songs.isNotEmpty(),
        )
        IconButton({ actions.enqueue(songs) }, enabled = songs.isNotEmpty()) { Icon(Icons.AutoMirrored.Filled.PlaylistAdd, say.addAllToQueue) }
        IconButton({ actions.download(songs) }, enabled = songs.isNotEmpty()) { Icon(Icons.Filled.Download, say.downloadAll) }
    }
}

/**
 * The songs a filter keeps, by title or artist in either case (nori-core's `TextIndex`). The index takes
 * the list's text once, and only when a filter is first typed; each keystroke then sends the filter.
 */
@Composable
private fun rememberMatching(songs: List<Song>, q: String): List<Song> {
    val index = remember(songs) { lazy { dev.nori.music.ffi.library.TextIndex(songs.map { listOf(it.title, it.artist) }) } }
    return remember(index, q) { if (q.isBlank()) songs else index.value.view(q).rows.map { songs[it.toInt()] } }
}

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

/** An album's songs by disc, as the core laid them out when the album was read (`pages::album_discs`). */
private fun discsOf(d: AlbumDetail): List<Pair<dev.nori.music.ffi.library.DiscGroup, List<Song>>> =
    d.discs.map { g -> g to g.songs.map { d.songs[it.toInt()] } }

/** An artist's releases by kind, as the core grouped them when the artist was read (`pages::release_groups`). */
private fun groupsOf(d: dev.nori.music.ffi.library.ArtistDetail): List<Pair<String, List<Album>>> =
    d.groups.map { g -> say.releaseShelf(g) to g.albums.map { d.albums[it.toInt()] } }


/**
 * "Download" is the wrong word once the songs are already here, and so is offering all of them when
 * only a few are missing. This says what is actually left to do - and offers to give the space back
 * when there is nothing left.
 */
@Composable
internal fun downloadEntry(songs: List<Song>, done: Set<String>, actions: ActionsViewModel): Pair<String, () -> Unit> {
    val missing = remember(songs, done) { songs.filterNot { it.id in done } }
    // What it does is the core's (`menus::download_entry`); what it says, Say's.
    val act = remember(songs.size, missing.size) { dev.nori.music.ffi.library.downloadEntry(songs.size.toUInt(), missing.size.toUInt()) }
    val label = remember(act, missing.size) { say.downloadEntry(act, missing.size) }
    return label to when (act) {
        dev.nori.music.ffi.library.DownloadAct.ALL -> { { actions.download(songs) } }
        dev.nori.music.ffi.library.DownloadAct.MISSING -> { { actions.download(missing) } }
        dev.nori.music.ffi.library.DownloadAct.REMOVE -> { { actions.undownload(songs) } }
    }
}

/** The offer on a provider's album or playlist page (the core's `library_offer`), under the hero. */
@Composable
private fun LibraryOffer(id: String, external: Boolean, actions: ActionsViewModel) {
    val offer = remember(id, external) { dev.nori.music.ffi.library.libraryOffer(id, external)?.let(say::libraryOffer) } ?: return
    TextButton({ actions.addToLibrary(id, isAlbum = true) }, Modifier.padding(horizontal = 12.dp)) { Text(offer) }
}

/**
 * An album's own queue before its songs are read: any song of the record some other queue carried along.
 * Once they are, the album comes with its queue (`AlbumDetail.queue`), and so do playlists and artists.
 */
private fun albumHintQueue(album: Album) = dev.nori.music.ffi.library.PageQueue(dev.nori.music.ffi.library.PageOwn.Songs(emptyList(), album.id))

/** A list's own queue: its songs, whatever order they are being played in. */
internal fun songsQueue(songs: List<Song>?) = dev.nori.music.ffi.library.PageQueue(dev.nori.music.ffi.library.PageOwn.Songs(songs?.map { it.id }.orEmpty(), null))

/** Makes a playlist a favourite on this phone, or not (the core's `pins_toggled`). */
private fun SettingsViewModel.pin(id: String, on: Boolean) =
    update { it.copy(pinnedPlaylists = dev.nori.music.ffi.library.pinsToggled(it.pinnedPlaylists, id, on)) }

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
    val discs = remember(detail) { detail?.let(::discsOf).orEmpty() }
    // The album's own caption once it is read; until then what the row that opened it knew.
    val caption = remember(detail, album) { detail?.let(say::albumCaption) ?: say.albumHintCaption(album.year.toInt(), album.songCount.toInt(), album.duration.toLong()) }
    val queue = detail?.queue ?: remember(album.id) { albumHintQueue(album) }
    HeroPage(
        coverUrl = vm.cover(album.coverArt, CoverSize.FULL),
        title = album.name,
        subtitle = album.artist,
        caption = caption,
        onSubtitle = album.artistId?.let { a ->
            { nav.artist(a, Artist(a, album.artist, album.coverArt, null, 0u, false, false)) }
        },
        // Play and shuffle wait for the songs: pressing them with an empty list would queue nothing.
        // The row itself is reserved ([awaitingPlay]) so the page does not reflow when they land.
        awaitingPlay = detail == null && load !is Load.Failed,
        onPlay = detail?.let { d -> { actions.play(d.songs) } },
        onShuffle = detail?.let { d -> { actions.shuffle(d.songs) } },
        queue = queue,
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
                    MoreCircle(listOf(say.addToQueue to { actions.enqueue(detail!!.songs) }, downloadEntry(detail!!.songs, done, actions)))
                }
            }
        },
    ) {
        when {
            detail != null -> item(key = "body") {
                Arrive {
                    Column {
                        LibraryOffer(album.id, album.isExternal, actions)
                        discs.forEach { (disc, tracks) ->
                            // Only an album of several discs heads them (the core leaves the one disc's empty).
                            if (disc.headed) SectionTitle(remember(disc) { say.discHeading(disc) })
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
                                    line = disc.lines[i],
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
    val discs = remember(d) { discsOf(d) }
    HeroPage(
        coverUrl = vm.cover(d.album.coverArt, CoverSize.FULL),
        title = d.album.name,
        subtitle = d.album.artist,
        caption = remember(d) { say.albumCaption(d) },
        onSubtitle = d.album.artistId?.let { a ->
            { nav.artist(a, Artist(a, d.album.artist, d.album.coverArt, null, 0u, false, false)) }
        },
        onPlay = { actions.play(d.songs) },
        onShuffle = { actions.shuffle(d.songs) },
        queue = d.queue,
        actions = {
            val albumStarred = LocalStarMarks.current.effectiveStar(dev.nori.music.data.StarKind.ALBUM, d.album.id, d.album.starred)
            FavoriteCircle(albumStarred) { actions.starAlbum(d.album.id, !albumStarred); Unit }
            MoreCircle(listOf(say.addToQueue to { actions.enqueue(d.songs) }, downloadEntry(d.songs, done, actions)))
        },
    ) {
        item(key = "header") { LibraryOffer(d.album.id, d.album.isExternal, actions) }
        discs.forEach { (disc, tracks) ->
            if (disc.headed) item(key = "disc${disc.disc}") { SectionTitle(remember(disc) { say.discHeading(disc) }) }
            songRows(tracks, actions, playing, done, selected, menu, numbered = true, keyPrefix = "d${disc.disc}-", context = d.songs, lines = disc.lines)
        }
    }
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
        AlertDialog({ leaving = null }, title = { Text(say.openInBrowser) }, text = { Text(url) },
            confirmButton = { TextButton({ runCatching { uri.openUri(url) }; leaving = null }) { Text(say.open) } }, dismissButton = { TextButton({ leaving = null }) { Text(say.cancel) } })
    }
    val hint = nav.artistHint(id)
    val ui = (load as? Load.Ready)?.data
    val artist = ui?.detail?.artist ?: hint
    if (artist == null) {
        LoadBox(load) { ready -> ArtistBody(ready, actions, vm, done, selected, menu, playing, nav) { leaving = it } }
        return
    }
    val groups = remember(ui?.detail) { ui?.detail?.let(::groupsOf).orEmpty() }
    val queue = ui?.detail?.queue
    val similar = remember(ui?.info) { ui?.info?.similar?.let { dev.nori.music.ffi.library.similarArtists(it) }.orEmpty() }
    HeroPage(
        coverUrl = vm.cover(artist.coverArt, CoverSize.FULL),
        title = artist.name,
        caption = remember(ui?.detail, artist.albumCount) {
            if (ui != null) say.releases(ui.detail.albums.size)
            else artist.albumCount.takeIf { it > 0u }?.let { say.releases(it.toInt()) }.orEmpty()
        },
        onPlay = ui?.let { ready -> { actions.playArtist(ready.detail.artist.id) } },
        onShuffle = ui?.let { ready -> { actions.playArtist(ready.detail.artist.id, shuffle = true) } },
        awaitingPlay = ui == null && load !is Load.Failed,
        queue = queue,
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
                            say.addToQueue to { actions.queueArtist(ui!!.detail.artist.id) },
                            say.downloadEverything to { actions.downloadArtist(ui!!.detail.artist.id) },
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
                        ui.info?.biography?.let { Text(remember(it) { dev.nori.music.ffi.library.biography(it) }, Modifier.padding(horizontal = Space.gutter), maxLines = 4, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
                        Row(Modifier.padding(horizontal = 12.dp)) {
                            ui.info?.lastFmUrl?.let { u -> TextButton({ leaving = u }) { Text("last.fm") } }
                            ui.info?.musicBrainzId?.let { m -> TextButton({ leaving = dev.nori.music.ffi.library.musicbrainzArtistUrl(m) }) { Text("MusicBrainz") } }
                        }
                        groups.forEach { (group, albums) ->
                            SectionTitle(group)
                            LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                                items(albums, key = { it.id }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 120.dp, { nav.album(a.id, a) }) }
                            }
                        }
                        if (ui.top.isNotEmpty()) {
                            SectionTitle(say.topSongs)
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
                        if (similar.isNotEmpty()) {
                            SectionTitle(say.similarArtists)
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
    val groups = remember(ui.detail) { groupsOf(ui.detail) }
    val similar = remember(ui.info) { ui.info?.similar?.let { dev.nori.music.ffi.library.similarArtists(it) }.orEmpty() }
    HeroPage(
        coverUrl = vm.cover(ui.detail.artist.coverArt, CoverSize.FULL),
        title = ui.detail.artist.name,
        caption = remember(ui.detail) { say.releases(ui.detail.albums.size) },
        onPlay = { actions.playArtist(ui.detail.artist.id) },
        onShuffle = { actions.playArtist(ui.detail.artist.id, shuffle = true) },
        queue = ui.detail.queue,
        actions = {
            val artistStarred = LocalStarMarks.current.effectiveStar(dev.nori.music.data.StarKind.ARTIST, ui.detail.artist.id, ui.detail.artist.starred)
            FavoriteCircle(artistStarred) { actions.starArtist(ui.detail.artist.id, !artistStarred); Unit }
            MoreCircle(
                listOf(
                    say.addToQueue to { actions.queueArtist(ui.detail.artist.id) },
                    say.downloadEverything to { actions.downloadArtist(ui.detail.artist.id) },
                ),
            )
        },
    ) {
        item(key = "header") {
            ui.info?.biography?.let { Text(remember(it) { dev.nori.music.ffi.library.biography(it) }, Modifier.padding(horizontal = Space.gutter), maxLines = 4, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant) }
            Row(Modifier.padding(horizontal = 12.dp)) {
                ui.info?.lastFmUrl?.let { u -> TextButton({ leave(u) }) { Text("last.fm") } }
                ui.info?.musicBrainzId?.let { m -> TextButton({ leave(dev.nori.music.ffi.library.musicbrainzArtistUrl(m)) }) { Text("MusicBrainz") } }
            }
        }
        groups.forEach { (group, albums) ->
            item(key = "g-$group") {
                SectionTitle(group)
                LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
                    items(albums, key = { it.id }) { a -> AlbumCard(a, vm.cover(a.coverArt, CoverSize.CARD), 120.dp, { nav.album(a.id, a) }) }
                }
            }
        }
        if (ui.top.isNotEmpty()) item(key = "top") { SectionTitle(say.topSongs) }
        songRows(ui.top, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) }, keyPrefix = "top-")
        if (similar.isNotEmpty()) {
            item(key = "similar") {
                SectionTitle(say.similarArtists)
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
        LoadBox(load) { d -> PlaylistBody(d, id, actions, vm, settings, prefs, done, selected, menu, playing, filter, { filter = it }, { exportM3u.launch(dev.nori.music.ffi.library.m3uFileName(d.playlist.name)) }) }
        return
    }
    val shown = rememberMatching(detail?.songs.orEmpty(), filter)
    HeroPage(
        coverUrl = vm.cover(playlist.coverArt, CoverSize.FULL),
        title = playlist.name,
        subtitle = playlist.comment?.ifEmpty { null },
        caption = remember(detail, playlist) { detail?.let { say.listCaption(it.songs.size, it.seconds.toLong(), true) } ?: say.albumHintCaption(0, playlist.songCount.toInt(), playlist.duration.toLong()) },
        onPlay = detail?.let { d -> { actions.play(d.songs) } },
        onShuffle = detail?.let { d -> { actions.shuffle(d.songs) } },
        awaitingPlay = detail == null && load !is Load.Failed,
        queue = detail?.queue,
        actions = {
            val pinned = id in prefs.pinnedPlaylists
            // A favourite, drawn and named as every other favourite in the app is: a heart, filled
            // when it is one. It was a pin with one look for both states, so there was no telling
            // from the page whether this playlist was on the home page or not. The server has no
            // way to star a playlist, so it is kept on this phone, and the home page's shelf of
            // them reads it.
            FavoriteCircle(pinned) { settings.pin(id, !pinned); Unit }
            Box(Modifier.size(46.dp), contentAlignment = Alignment.Center) {
                androidx.compose.animation.AnimatedVisibility(
                    visible = detail != null,
                    enter = androidx.compose.animation.fadeIn(androidx.compose.animation.core.tween(if (AppMotion.reduce) 0 else 220)),
                    exit = androidx.compose.animation.fadeOut(androidx.compose.animation.core.tween(if (AppMotion.reduce) 0 else 120)),
                ) {
                    MoreCircle(
                        listOf(
                            say.addToQueue to { actions.enqueue(detail!!.songs) },
                            downloadEntry(detail!!.songs, done, actions),
                            say.exportPlaylistFile to { exportM3u.launch(dev.nori.music.ffi.library.m3uFileName(playlist.name)) },
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
    val shown = rememberMatching(d.songs, filter)
    HeroPage(
        coverUrl = vm.cover(d.playlist.coverArt, CoverSize.FULL),
        title = d.playlist.name,
        subtitle = d.playlist.comment?.ifEmpty { null },
        caption = remember(d) { say.listCaption(d.songs.size, d.seconds.toLong(), true) },
        onPlay = { actions.play(d.songs) },
        onShuffle = { actions.shuffle(d.songs) },
        queue = d.queue,
        actions = {
            val pinned = id in prefs.pinnedPlaylists
            FavoriteCircle(pinned) { settings.pin(id, !pinned); Unit }
            MoreCircle(
                listOf(
                    say.addToQueue to { actions.enqueue(d.songs) },
                    downloadEntry(d.songs, done, actions),
                    say.exportPlaylistFile to onExport,
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
            item(key = "header") { Header(name, remember(list.size) { say.songs(list.size) }, null); PlayButtons(list, actions) }
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
            item(key = "header") {
                Header(
                    remember(d.name) { say.folderTitle(d.name) },
                    remember(d) { say.folderCaption(d.folders.size, d.songs.size) }, null,
                )
                if (d.songs.isNotEmpty()) PlayButtons(d.songs, actions)
            }
            items(d.folders, key = { "f" + it.id }) { f -> Text(remember(f.name) { say.folderRow(f.name) }, Modifier.fillMaxWidth().clickable { nav.folder(f.id) }.padding(horizontal = 16.dp, vertical = 14.dp)) }
            songRows(d.songs, actions, playing, done, selected, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
        }
    }
}
