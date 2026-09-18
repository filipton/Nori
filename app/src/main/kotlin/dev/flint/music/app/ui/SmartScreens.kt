package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Spacer
import androidx.compose.material3.Surface
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
    LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
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
        item { Text("Matched against the synced library: sync it in Settings so every song can be found.", Modifier.padding(16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
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
            LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
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
    Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = 24.dp + LocalChromeInset.current)) {
        Row(Modifier.padding(start = 4.dp, end = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Smart playlist", Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
            TextButton({ error = vm.save(draft) { nav.back() } }) { Text("Save", style = MaterialTheme.typography.titleSmall) }
        }
        FormField(draft.name, { draft = draft.copy(name = it) }, Modifier.fillMaxWidth().padding(horizontal = Space.gutter), label = { Text("Name") }, singleLine = true)
        Row(Modifier.padding(horizontal = Space.gutter, vertical = 12.dp), Arrangement.spacedBy(8.dp)) {
            Chip("Match all", draft.all) { draft = draft.copy(all = true) }
            Chip("Match any", !draft.all) { draft = draft.copy(all = false) }
        }
        // One rule, one card: the field and the comparison on the first line, what to compare against on
        // the second. In a row they fought over the width and the value box ended up a sliver.
        draft.rules.forEachIndexed { i, r ->
            fun set(n: SmartRule) { draft = draft.copy(rules = draft.rules.toMutableList().also { it[i] = n }) }
            Surface(
                shape = CardShape, color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.06f).over(MaterialTheme.colorScheme.background),
                contentColor = MaterialTheme.colorScheme.onSurface,
                modifier = Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 5.dp),
            ) {
                Column(Modifier.padding(start = 12.dp, end = 4.dp, top = 4.dp, bottom = 10.dp)) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Pick(r.field, SmartDraft.TEXTS + SmartDraft.NUMBERS + SmartDraft.DATES + SmartDraft.FLAGS) { f -> set(r.copy(field = f, op = SmartDraft.ops(f).first())) }
                        Pick(r.op, SmartDraft.ops(r.field)) { o -> set(r.copy(op = o)) }
                        Spacer(Modifier.weight(1f))
                        IconButton({ draft = draft.copy(rules = draft.rules.filterIndexed { j, _ -> j != i }.ifEmpty { listOf(SmartRule()) }) }, Modifier.size(38.dp)) {
                            Icon(Icons.Filled.Close, "Remove rule", Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                    if (r.op !in SmartDraft.FLAG_OPS) FormField(
                        r.value, { set(r.copy(value = it)) }, Modifier.fillMaxWidth().padding(end = 8.dp), singleLine = true,
                        placeholder = { Text(if (r.op == "between") "from to" else if (r.op.endsWith("Days")) "days" else "value") },
                    )
                }
            }
        }
        ActionRow("Add rule", Icons.Filled.Add, { draft = draft.copy(rules = draft.rules + SmartRule()) }, divider = false)
        Row(Modifier.padding(start = Space.gutter, end = Space.gutter, top = 6.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Sort by", style = MaterialTheme.typography.bodyLarge)
            Pick(draft.sortField, SmartDraft.SORTS) { draft = draft.copy(sortField = it) }
            Spacer(Modifier.weight(1f))
            Chip("Descending", draft.descending) { draft = draft.copy(descending = !draft.descending) }
        }
        Row(Modifier.padding(horizontal = Space.gutter, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Limit", style = MaterialTheme.typography.bodyLarge)
            FormField(
                if (draft.limit > 0) "${draft.limit}" else "", { draft = draft.copy(limit = it.toIntOrNull() ?: 0) },
                Modifier.padding(start = 12.dp).width(130.dp), singleLine = true, placeholder = { Text("none") },
            )
        }
        error?.let { Text(it, Modifier.padding(Space.gutter), color = MaterialTheme.colorScheme.error) }
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
    LazyColumn(state = list, contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
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
    Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = 24.dp + LocalChromeInset.current)) {
        Row(Modifier.padding(start = 4.dp, end = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Listening", Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
        }
        LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            items(listOf(7 to "Week", 30 to "Month", 365 to "Year", 0 to "All time")) { (d, l) -> Chip(l, days == d) { days = d } }
        }
        val st = s ?: return@Column

        // The headline: two numbers worth reading from across the room, the rest as a grid of tiles.
        Column(Modifier.padding(horizontal = Space.gutter, vertical = 14.dp)) {
            Text("${st.plays}", style = MaterialTheme.typography.displaySmall, color = MaterialTheme.colorScheme.primary)
            Text("plays · ${duration(st.listenedMs / 1000)} listened", style = MaterialTheme.typography.titleMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Row(Modifier.padding(horizontal = 14.dp), Arrangement.spacedBy(8.dp)) {
            StatTile("${st.distinctSongs}", "songs", Modifier.weight(1f))
            StatTile("${st.distinctArtists}", "artists", Modifier.weight(1f))
            StatTile("${st.distinctAlbums}", "albums", Modifier.weight(1f))
        }
        Row(Modifier.padding(horizontal = 14.dp, vertical = 8.dp), Arrangement.spacedBy(8.dp)) {
            StatTile("${st.skips}", "skips", Modifier.weight(1f))
            StatTile("${st.activeDays}", "active days", Modifier.weight(1f))
            StatTile("${st.longestStreakDays}", "day streak", Modifier.weight(1f))
        }

        val peak = st.playsPerHour.withIndex().maxByOrNull { it.value }
        if (peak != null && peak.value > 0u) {
            SectionTitle("When you listen")
            HourChart(st.playsPerHour.map { it.toInt() })
            Text(
                "Most around ${peak.index}:00" + st.playsPerWeekday.withIndex().maxByOrNull { it.value }
                    ?.let { ", mostly on ${listOf("Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays", "Sundays")[it.index]}" }.orEmpty(),
                Modifier.padding(horizontal = Space.gutter, vertical = 6.dp),
                style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }

        if (st.topSongs.isNotEmpty()) SectionTitle("Top songs")
        st.topSongs.forEachIndexed { i, t -> RankRow(i + 1, t.song.title, t.song.artist, "${t.plays}") }
        if (st.topArtists.isNotEmpty()) SectionTitle("Top artists")
        st.topArtists.forEachIndexed { i, t -> RankRow(i + 1, t.name, duration(t.listenedMs / 1000), "${t.plays}") }
        if (st.topAlbums.isNotEmpty()) SectionTitle("Top albums")
        st.topAlbums.forEachIndexed { i, t -> RankRow(i + 1, t.name, "", "${t.plays}") }
        if (st.topGenres.isNotEmpty()) SectionTitle("Top genres")
        st.topGenres.forEachIndexed { i, t -> RankRow(i + 1, t.name, "", "${t.plays}") }
    }
}

/** One number and what it counts, on its own plate. */
@Composable
private fun StatTile(value: String, label: String, modifier: Modifier = Modifier) {
    Surface(
        shape = CardShape, color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.06f).over(MaterialTheme.colorScheme.background),
        contentColor = MaterialTheme.colorScheme.onSurface, modifier = modifier,
    ) {
        Column(Modifier.padding(horizontal = 12.dp, vertical = 12.dp)) {
            Text(value, style = MaterialTheme.typography.headlineSmall, maxLines = 1)
            Caption(label, Modifier.padding(top = 2.dp))
        }
    }
}

/** Twenty-four bars, drawn: the shape of a listening day says more than "most around 20:00" alone. */
@Composable
private fun HourChart(perHour: List<Int>) {
    val peak = (perHour.maxOrNull() ?: 0).coerceAtLeast(1)
    val bar = MaterialTheme.colorScheme.primary
    val dim = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.12f)
    Column(Modifier.padding(horizontal = Space.gutter)) {
        androidx.compose.foundation.Canvas(Modifier.fillMaxWidth().height(96.dp)) {
            val gap = size.width / 24f * 0.28f
            val w = size.width / 24f - gap
            perHour.forEachIndexed { h, plays ->
                val x = h * (w + gap)
                val tall = size.height * (plays / peak.toFloat())
                drawRoundRect(dim, androidx.compose.ui.geometry.Offset(x, 0f), androidx.compose.ui.geometry.Size(w, size.height), androidx.compose.ui.geometry.CornerRadius(w / 2f, w / 2f))
                if (plays > 0) drawRoundRect(
                    bar, androidx.compose.ui.geometry.Offset(x, size.height - tall),
                    androidx.compose.ui.geometry.Size(w, tall), androidx.compose.ui.geometry.CornerRadius(w / 2f, w / 2f),
                )
            }
        }
        Row(Modifier.fillMaxWidth().padding(top = 4.dp), Arrangement.SpaceBetween) {
            listOf("00", "06", "12", "18", "23").forEach { Caption(it) }
        }
    }
}

/** A place in a chart: rank, what it is, and how often. */
@Composable
private fun RankRow(rank: Int, title: String, subtitle: String, count: String) {
    Column {
        Row(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 9.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(
                "$rank", Modifier.width(28.dp), style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            Column(Modifier.weight(1f).padding(end = 10.dp)) {
                Text(title, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis)
                if (subtitle.isNotEmpty()) Text(
                    subtitle, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                    maxLines = 1, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                )
            }
            Text(count, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.primary)
        }
        Hairline(startIndent = Space.gutter + 28.dp)
    }
}
