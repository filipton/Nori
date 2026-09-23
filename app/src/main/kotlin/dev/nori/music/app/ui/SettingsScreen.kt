package dev.nori.music.app.ui

import android.content.Intent
import android.media.audiofx.AudioEffect
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.outlined.AutoAwesome
import androidx.compose.material.icons.outlined.CloudDownload
import androidx.compose.material.icons.outlined.Dns
import androidx.compose.material.icons.outlined.GraphicEq
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.LibraryMusic
import androidx.compose.material.icons.outlined.Lyrics
import androidx.compose.material.icons.outlined.Palette
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.nori.music.app.vm.SettingsViewModel
import dev.nori.music.ffi.SettingRow
import dev.nori.music.ffi.SettingsSection
import dev.nori.music.settings.ServerProfile

/**
 * Search lands on a row, not on a page: the group page is told which row to reveal, the row reports
 * where it is, and the page scrolls there and lets the highlight fade out. A row's key comes with it
 * from the core (`settings_schema::setting_key`), the same key the search result carries.
 */
class SettingSpotlight(val key: String?, val onPlaced: (Int) -> Unit)

val LocalSpotlight = androidx.compose.runtime.compositionLocalOf { SettingSpotlight(null) {} }

/** The wash that says "this is the one you searched for", fading out once you have seen it. */
@Composable
private fun Modifier.spotlight(key: String?): Modifier {
    val spot = LocalSpotlight.current
    val on = key != null && spot.key == key
    if (!on) return this
    val fade = remember { androidx.compose.animation.core.Animatable(1f) }
    LaunchedEffect(Unit) {
        kotlinx.coroutines.delay(900)
        fade.animateTo(0f, androidx.compose.animation.core.tween(1400))
    }
    val colour = MaterialTheme.colorScheme.primary
    return this
        .onGloballyPositioned { spot.onPlaced(it.positionInWindow().y.toInt()) }
        .drawBehind { drawRect(colour.copy(alpha = 0.22f * fade.value)) }
}

@Composable
fun Toggle(title: String, detail: String, value: Boolean, enabled: Boolean = true, key: String? = null, onChange: (Boolean) -> Unit) {
    Column {
    Row(
        Modifier.fillMaxWidth().spotlight(key).clickable(enabled) { onChange(!value) }
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = 14.dp).alpha(if (enabled) 1f else DIMMED)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        NoriSwitch(value, onChange, enabled = enabled)
    }
    Hairline(startIndent = 16.dp)
    }
}

/** What a setting the app is going to ignore looks like: still there, still readable, plainly not live. */
private const val DIMMED = 0.38f

@Composable
private fun Choice(row: SettingRow.Choice, onChange: (String) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val dim = if (row.enabled) 1f else DIMMED
    Column {
    Row(Modifier.fillMaxWidth().spotlight(row.key).clickable(row.enabled) { open = true }.padding(horizontal = 16.dp, vertical = 15.dp)) {
        Text(row.title, Modifier.weight(1f).alpha(dim), style = MaterialTheme.typography.bodyLarge)
        Text(row.shown, Modifier.alpha(dim), color = MaterialTheme.colorScheme.primary)
        DropdownMenu(open, { open = false }) { row.options.forEach { o -> DropdownMenuItem({ Text(o.label) }, { onChange(o.value); open = false }) } }
    }
    Hairline(startIndent = 16.dp)
    }
}

/**
 * The rounded plate the rows of a group sit on. The content colour is spelled out: Material resolves
 * it to Unspecified for any colour it does not recognise as one of its own roles, and text inside then
 * renders almost black - which is why every setting's title was dimmer than its own description.
 */
@Composable
private fun SettingsCard(content: @Composable ColumnScope.() -> Unit) {
    Surface(
        shape = CardShape,
        color = LocalLook.current.color(dev.nori.music.look.CoverLook.FORM),
        contentColor = MaterialTheme.colorScheme.onSurface,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 6.dp),
    ) { Column(content = content) }
}

/** Each group's icon; the groups themselves, their order and their words are the core's (`settings_schema`). */
private fun groupIcon(id: String): ImageVector = when (id) {
    "servers" -> Icons.Outlined.Dns
    "playing" -> Icons.Outlined.PlayCircle
    "sound" -> Icons.Outlined.GraphicEq
    "look" -> Icons.Outlined.Palette
    "lyrics" -> Icons.Outlined.Lyrics
    "library" -> Icons.Outlined.LibraryMusic
    "data" -> Icons.Outlined.CloudDownload
    "about" -> Icons.Outlined.Info
    else -> Icons.Outlined.AutoAwesome
}

/**
 * A titled block of rows on its own plate. A group page used to be one plate of twenty rows, which is
 * where things got lost; a few short, named sections are what make a page scannable.
 */
@Composable
private fun Section(title: String, content: @Composable ColumnScope.() -> Unit) {
    Text(
        title.uppercase(),
        Modifier.padding(start = 30.dp, end = 30.dp, top = 16.dp, bottom = 2.dp),
        style = MaterialTheme.typography.labelMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
    )
    SettingsCard(content)
}

/** A row with a line of text and a button at its end: a count, an action. */
@Composable
private fun ActionRowSetting(row: SettingRow.Action, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().spotlight(row.key).padding(start = 16.dp, end = 8.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(row.title, style = MaterialTheme.typography.bodyLarge)
            Text(row.detail, style = MaterialTheme.typography.bodySmall, color = if (row.error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
        }
        TextButton(onClick, enabled = row.enabled) { Text(row.button) }
    }
    Hairline(startIndent = 16.dp)
}

@Composable
fun SettingsScreen(vm: SettingsViewModel) {
    val nav = LocalNav.current
    var query by remember { mutableStateOf("") }
    // Titles first, then anything whose explanation mentions it: searching "oled" finds AMOLED black
    // (the core's `settings_search`), asked once per change of the query.
    val hits = remember(query) { if (query.isBlank()) emptyList() else vm.searchSettings(query) }
    Column {
        LargeTitle("Settings")
        SearchField(query, { query = it }, "Search settings", Modifier.padding(horizontal = Space.gutter, vertical = 6.dp))
        if (query.isNotBlank()) {
            // A result is the setting itself: tapping opens its page and puts the finger on the row.
            LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
                items(hits, key = { it.group + it.title }) { e ->
                    Column(Modifier.clickable { nav.settingsGroup(e.group, e.key) }) {
                        Column(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 11.dp)) {
                            Text(e.title, style = MaterialTheme.typography.bodyLarge)
                            Text(
                                e.detail,
                                style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                                maxLines = 2, overflow = androidx.compose.ui.text.style.TextOverflow.Ellipsis,
                            )
                        }
                        Hairline(startIndent = Space.gutter)
                    }
                }
                if (hits.isEmpty()) item { EmptyNote("Nothing matches \"$query\"") }
            }
            return@Column
        }
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            items(vm.settingsGroups, key = { it.id }) { g ->
                Column(Modifier.clickable { nav.settingsGroup(g.id) }) {
                    Row(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 13.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(groupIcon(g.id), null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary)
                        Column(Modifier.weight(1f).padding(start = 14.dp)) {
                            Text(g.title, style = MaterialTheme.typography.bodyLarge)
                            Text(g.summary, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        Icon(Icons.AutoMirrored.Filled.KeyboardArrowRight, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Hairline(startIndent = Space.gutter + 36.dp)
                }
            }
        }
    }
}

/** One group's page: its rows on their plates, scrolled to whichever row the search sent us to. */
@Composable
fun SettingsGroupScreen(vm: SettingsViewModel, id: String, highlight: String = "") {
    val nav = LocalNav.current
    val p by vm.prefs.collectAsStateWithLifecycle()
    val facts by vm.settingsFacts.collectAsStateWithLifecycle()
    // The page's facts, fetched while it is open: the songs AutoMix measured, what is stored on the
    // phone, the server's music folders.
    LaunchedEffect(id, p.autoMix) { if (id == "playing" && p.autoMix) vm.refreshAnalysed() }
    LaunchedEffect(id) { if (id == "data") vm.refreshStorage() }
    LaunchedEffect(id, p.activeServerId) { if (id == "servers") vm.loadMusicFolders() }
    // One call for the whole page, again only when the settings or its facts change.
    val page = remember(id, p, facts) { vm.settingsPage(id, facts) } ?: return
    val scroll = rememberScrollState()
    var target by remember { mutableIntStateOf(-1) }
    LaunchedEffect(target) { if (target >= 0) scroll.animateScrollTo((scroll.value + target - 400).coerceAtLeast(0)) }
    CompositionLocalProvider(LocalSpotlight provides SettingSpotlight(highlight.ifEmpty { null }) { y -> if (target < 0) target = y }) {
        Column(Modifier.verticalScroll(scroll)) {
            Row(Modifier.padding(start = 4.dp, end = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
                IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
                Text(page.title, Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
            }
            when (id) {
                "about" -> AboutContent({ title, content -> Section(title, content) }) { nav.settingsGroup("licences") }
                "licences" -> LicencesContent { title, content -> Section(title, content) }
                else -> page.sections.forEach { s -> SettingsSectionRows(vm, s) }
            }
            Spacer(Modifier.height(Space.section + LocalChromeInset.current))
        }
    }
}

/** One section of a page as the core laid it out; this only draws the rows and hands back what was picked. */
@Composable
private fun SettingsSectionRows(vm: SettingsViewModel, section: SettingsSection) {
    val nav = LocalNav.current
    val context = LocalContext.current
    val p by vm.prefs.collectAsStateWithLifecycle()
    var editing by remember { mutableStateOf<ServerProfile?>(null) }
    editing?.let { e -> androidx.compose.ui.window.Dialog({ editing = null }, androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) { LoginScreen(vm, e) { editing = null } } }
    val act: (String) -> Unit = { action ->
        when (action) {
            "equalizer" -> nav.equalizer()
            "system-effects" -> runCatching {
                context.startActivity(
                    Intent(AudioEffect.ACTION_DISPLAY_AUDIO_EFFECT_CONTROL_PANEL).putExtra(AudioEffect.EXTRA_PACKAGE_NAME, context.packageName)
                        .putExtra(AudioEffect.EXTRA_CONTENT_TYPE, AudioEffect.CONTENT_TYPE_MUSIC),
                )
            }
            "downloads" -> nav.downloads()
            "add-server" -> editing = vm.newProfile()
            else -> vm.act(action)
        }
    }
    Section(section.title) {
        section.rows.forEach { row ->
            when (row) {
                is SettingRow.Toggle -> Toggle(row.title, row.detail, row.on, enabled = row.enabled, key = row.key) { on -> vm.set(row.name, on.toString()) }
                is SettingRow.Choice -> Choice(row) { v -> vm.set(row.name, v) }
                is SettingRow.Note -> Text(row.text, Modifier.padding(horizontal = Space.gutter, vertical = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                is SettingRow.Link -> {
                    val dim = if (row.dimmed) DIMMED else 1f
                    Row(Modifier.fillMaxWidth().spotlight(row.key).clickable { act(row.action) }.padding(horizontal = 16.dp, vertical = 15.dp)) {
                        Text(row.title, Modifier.weight(1f).alpha(dim), style = MaterialTheme.typography.bodyLarge)
                        if (row.status.isNotEmpty()) Text(row.status, Modifier.alpha(dim), color = MaterialTheme.colorScheme.primary)
                    }
                    if (row.divider) Hairline(startIndent = 16.dp)
                }
                is SettingRow.Action -> ActionRowSetting(row) { act(row.action) }
                is SettingRow.Info -> {
                    Column(Modifier.fillMaxWidth().spotlight(row.key).padding(horizontal = 16.dp, vertical = 12.dp)) {
                        Text(row.title, style = MaterialTheme.typography.bodyLarge)
                        Text(row.detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Hairline(startIndent = 16.dp)
                }
                is SettingRow.Slider -> {
                    Text(row.label, Modifier.padding(start = 16.dp, end = 16.dp, top = 10.dp), style = MaterialTheme.typography.bodySmall)
                    NoriSlider(row.value, row.min..row.max, { v -> vm.set(row.name, v.toString()) }, Modifier.padding(horizontal = 16.dp), centred = row.centred)
                }
                is SettingRow.Palette -> Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    row.colours.forEach { c ->
                        androidx.compose.foundation.layout.Box(
                            Modifier.size(if (row.chosen == c) 36.dp else 30.dp).background(androidx.compose.ui.graphics.Color(c), androidx.compose.foundation.shape.CircleShape).clickable { vm.set(row.name, c.toString()) },
                        )
                    }
                }
                is SettingRow.Server -> {
                    Row(Modifier.fillMaxWidth().clickable(enabled = !row.active) { p.servers.firstOrNull { it.id == row.id }?.let(vm::switchServer) }.padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(row.label, color = if (row.active) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface)
                            Text(row.detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        TextButton({ editing = p.servers.firstOrNull { it.id == row.id } }) { Text("Edit") }
                        TextButton({ vm.removeServer(row.id) }) { Text("Remove") }
                    }
                    Hairline(startIndent = 16.dp)
                }
                is SettingRow.Button -> TextButton({ act(row.action) }, Modifier.padding(horizontal = 8.dp)) { Text(row.title) }
            }
        }
    }
}
