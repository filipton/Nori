package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.background
import androidx.compose.ui.draw.clip
import androidx.compose.material.icons.filled.AutoAwesome
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.PlayArrow
import androidx.compose.material.icons.filled.Shuffle
import androidx.compose.material3.Button
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
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
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.lifecycle.viewmodel.compose.viewModel
import dev.flint.music.app.vm.ActionsViewModel
import dev.flint.music.app.vm.HistoryViewModel
import dev.flint.music.app.vm.MixesViewModel
import dev.flint.music.app.vm.PlayerViewModel
import dev.flint.music.app.vm.SmartDraft
import dev.flint.music.app.vm.SmartRule
import dev.flint.music.app.vm.SmartViewModel

@Composable
private fun <T> Pick(value: T, options: List<T>, modifier: Modifier = Modifier, label: (T) -> String = { it.toString() }, onPick: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Text(label(value), modifier.clickable { open = true }.padding(8.dp), color = MaterialTheme.colorScheme.primary)
    DropdownMenu(open, { open = false }) { options.forEach { o -> DropdownMenuItem({ Text(label(o)) }, { onPick(o); open = false }) } }
}

/** The mixes the core draws from the index and the listening history. */
@Composable
fun MixTiles(vm: MixesViewModel = viewModel()) {
    // A mix has no artwork, so it gets a colour of its own instead: a tile the size of a cover, with the
    // name on it. Chips made the row read as a filter bar; these read as something to play.
    val palette = listOf(0xFF8E3BD6, 0xFFD63B6B, 0xFF1E88E5, 0xFF00897B, 0xFFE0662B, 0xFF5C6BC0)
    LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter), horizontalArrangement = Arrangement.spacedBy(12.dp)) {
        itemsIndexed(vm.tiles, key = { _, m -> m.title }) { i, m ->
            val seed = androidx.compose.ui.graphics.Color(palette[i % palette.size])
            Box(
                Modifier.size(150.dp).clip(CardShape)
                    .background(androidx.compose.ui.graphics.Brush.linearGradient(listOf(seed, blend(seed, androidx.compose.ui.graphics.Color.Black, 0.45f))))
                    .clickable { vm.play(m) }
                    .padding(14.dp),
            ) {
                Icon(Icons.Filled.AutoAwesome, null, Modifier.align(Alignment.TopEnd).size(18.dp), tint = androidx.compose.ui.graphics.Color.White.copy(alpha = 0.75f))
                Text(
                    m.title, Modifier.align(Alignment.BottomStart),
                    style = MaterialTheme.typography.titleMedium, color = androidx.compose.ui.graphics.Color.White,
                    maxLines = 2, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                )
            }
        }
    }
}

@Composable
fun SmartList(vm: SmartViewModel = viewModel()) {
    val saved by vm.saved.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LazyColumn {
        item { ActionRow("New smart playlist", Icons.Filled.Add, { nav.smartEdit("") }) }
        items(saved, key = { it.id }) { p ->
            NavRow(
                p.name, { nav.smart(p.id) },
                action = {
                    TextButton({ nav.smartEdit(p.id) }) { Text("Edit") }
                    IconButton({ vm.delete(p.id) }, Modifier.size(40.dp)) { Icon(Icons.Filled.Close, "Delete", Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) }
                },
            )
        }
        item { SectionTitle("Ready made") }
        items(vm.defaults, key = { it.id }) { p ->
            Row(Modifier.fillMaxWidth().clickable { nav.smart(p.id) }.padding(start = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
                Text(p.name, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge); TextButton({ nav.smartEdit(p.id) }) { Text("Copy") }
            }
        }
        item { Text("Evaluated in the Rust core over the offline index: sync it (Settings) so every song can match.", Modifier.padding(16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
    }
}

@Composable
fun SmartScreen(id: String, actions: ActionsViewModel, vm: SmartViewModel = viewModel()) {
    val saved by vm.saved.collectAsStateWithLifecycle()
    val playlist = remember(id, saved) { vm.find(id) }
    LaunchedEffect(playlist?.json) { playlist?.let { vm.open(it.json) } }
    val load by vm.songs.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    val menu = LocalSongMenu.current
    val done = actions.downloads.collectAsState().value.doneIds
    val selection by actions.selection.collectAsStateWithLifecycle()
    val player: PlayerViewModel = viewModel()
    val playing by player.currentId.collectAsStateWithLifecycle()
    Column {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text(playlist?.name ?: "Smart playlist", Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
            TextButton({ nav.smartEdit(id) }) { Text("Edit") }
        }
        LoadBox(load) { songs ->
            LazyColumn {
                item {
                    Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp), Arrangement.spacedBy(8.dp)) {
                        Button({ actions.play(songs) }, Modifier.weight(1f), enabled = songs.isNotEmpty()) { Icon(Icons.Filled.PlayArrow, null); Text("Play") }
                        OutlinedButton({ actions.shuffle(songs) }, Modifier.weight(1f), enabled = songs.isNotEmpty()) { Icon(Icons.Filled.Shuffle, null); Text("Shuffle") }
                        TextButton({ actions.download(songs) }, enabled = songs.isNotEmpty()) { Text("Get") }
                    }
                    Text("${songs.size} songs · ${duration(songs.sumOf { it.duration.toLong() })}", Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall)
                }
                songRows(songs, actions, playing, done, selection.mapTo(HashSet()) { it.id }, menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) })
            }
        }
    }
}

@Composable
fun SmartEditScreen(id: String, vm: SmartViewModel = viewModel()) {
    val saved by vm.saved.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    var draft by remember(id, saved.size) { mutableStateOf(vm.find(id)?.let { SmartDraft.from(it)?.let { d -> if (id.startsWith("default-")) d.copy(id = "") else d } } ?: SmartDraft()) }
    var error by remember { mutableStateOf<String?>(null) }
    Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Smart playlist", Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
            TextButton({ error = vm.save(draft) { nav.back() } }) { Text("Save") }
        }
        OutlinedTextField(draft.name, { draft = draft.copy(name = it) }, Modifier.fillMaxWidth().padding(horizontal = 16.dp), label = { Text("Name") }, singleLine = true)
        Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), Arrangement.spacedBy(8.dp)) {
            FilterChip(draft.all, { draft = draft.copy(all = true) }, { Text("Match all") })
            FilterChip(!draft.all, { draft = draft.copy(all = false) }, { Text("Match any") })
        }
        draft.rules.forEachIndexed { i, r ->
            fun set(n: SmartRule) { draft = draft.copy(rules = draft.rules.toMutableList().also { it[i] = n }) }
            Row(Modifier.padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Pick(r.field, SmartDraft.TEXTS + SmartDraft.NUMBERS + SmartDraft.DATES + SmartDraft.FLAGS) { f -> set(r.copy(field = f, op = SmartDraft.ops(f).first())) }
                Pick(r.op, SmartDraft.ops(r.field)) { o -> set(r.copy(op = o)) }
                if (r.op !in SmartDraft.FLAG_OPS) OutlinedTextField(r.value, { set(r.copy(value = it)) }, Modifier.weight(1f), singleLine = true, placeholder = { Text(if (r.op == "between") "from to" else if (r.op.endsWith("Days")) "days" else "value") })
                else Text("", Modifier.weight(1f))
                IconButton({ draft = draft.copy(rules = draft.rules.filterIndexed { j, _ -> j != i }.ifEmpty { listOf(SmartRule()) }) }) { Icon(Icons.Filled.Close, "Remove rule") }
            }
        }
        TextButton({ draft = draft.copy(rules = draft.rules + SmartRule()) }, Modifier.padding(horizontal = 8.dp)) { Text("Add rule") }
        Row(Modifier.padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Sort by", Modifier.padding(start = 8.dp)); Pick(draft.sortField, SmartDraft.SORTS) { draft = draft.copy(sortField = it) }
            FilterChip(draft.descending, { draft = draft.copy(descending = !draft.descending) }, { Text("Descending") })
        }
        Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Limit"); OutlinedTextField(if (draft.limit > 0) "${draft.limit}" else "", { draft = draft.copy(limit = it.toIntOrNull() ?: 0) }, Modifier.padding(start = 12.dp).width(120.dp), singleLine = true, placeholder = { Text("none") })
        }
        error?.let { Text(it, Modifier.padding(16.dp), color = MaterialTheme.colorScheme.error) }
    }
}

@Composable
fun HistoryList(actions: ActionsViewModel, vm: HistoryViewModel = viewModel()) {
    val entries by vm.entries.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    val menu = LocalSongMenu.current
    val list = rememberLazyListState()
    LaunchedEffect(list, entries.size) { snapshotFlow { (list.layoutInfo.visibleItemsInfo.lastOrNull()?.index ?: 0) >= entries.size - 30 }.collect { if (it) vm.loadMore() } }
    val songs = remember(entries) { entries.map { it.song } }
    LazyColumn(state = list) {
        item { Row(Modifier.padding(horizontal = 8.dp)) { TextButton(nav::stats) { Text("Listening stats") }; TextButton(vm::clear) { Text("Clear history") } } }
        if (entries.isEmpty()) item { Text("Nothing played yet, or the listening history is switched off in Settings → Features.", Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant) }
        songRows(songs, actions, null, emptySet(), emptySet(), menu, cover = { vm.cover(it.coverArt, CoverSize.ROW) }, keyPrefix = "h")
    }
}

/** The year in review, any time of year: everything comes from one query in the core. */
@Composable
fun StatsScreen(vm: HistoryViewModel = viewModel()) {
    var days by remember { mutableStateOf(365) }
    LaunchedEffect(days) { vm.loadStats(days) }
    val s by vm.stats.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = 24.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically) { IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }; Text("Listening stats", style = MaterialTheme.typography.titleLarge) }
        Row(Modifier.padding(horizontal = 16.dp), Arrangement.spacedBy(8.dp)) { listOf(7 to "Week", 30 to "Month", 365 to "Year", 0 to "All time").forEach { (d, l) -> FilterChip(days == d, { days = d }, { Text(l) }) } }
        val st = s ?: return@Column
        Text("${st.plays} plays · ${duration(st.listenedMs / 1000)} listened", Modifier.padding(16.dp), style = MaterialTheme.typography.headlineSmall)
        Text("${st.distinctSongs} songs · ${st.distinctArtists} artists · ${st.distinctAlbums} albums · ${st.skips} skips · ${st.activeDays} active days · longest streak ${st.longestStreakDays} days", Modifier.padding(horizontal = 16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        val peak = st.playsPerHour.withIndex().maxByOrNull { it.value }
        if (peak != null && peak.value > 0u) Text("You listen most around ${peak.index}:00" + st.playsPerWeekday.withIndex().maxByOrNull { it.value }?.let { ", mostly on ${listOf("Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays", "Sundays")[it.index]}" }.orEmpty(), Modifier.padding(16.dp))
        if (st.topSongs.isNotEmpty()) SectionTitle("Top songs")
        st.topSongs.forEachIndexed { i, t -> Text("${i + 1}. ${t.song.title} — ${t.song.artist}  (${t.plays})", Modifier.padding(horizontal = 16.dp, vertical = 3.dp)) }
        if (st.topArtists.isNotEmpty()) SectionTitle("Top artists")
        st.topArtists.forEachIndexed { i, t -> Text("${i + 1}. ${t.name}  (${t.plays} plays, ${duration(t.listenedMs / 1000)})", Modifier.padding(horizontal = 16.dp, vertical = 3.dp)) }
        if (st.topAlbums.isNotEmpty()) SectionTitle("Top albums")
        st.topAlbums.forEachIndexed { i, t -> Text("${i + 1}. ${t.name}  (${t.plays})", Modifier.padding(horizontal = 16.dp, vertical = 3.dp)) }
        if (st.topGenres.isNotEmpty()) SectionTitle("Top genres")
        st.topGenres.forEachIndexed { i, t -> Text("${i + 1}. ${t.name}  (${t.plays})", Modifier.padding(horizontal = 16.dp, vertical = 3.dp)) }
    }
}
