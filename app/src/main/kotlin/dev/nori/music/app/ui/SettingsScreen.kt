package dev.nori.music.app.ui

import android.content.Intent
import android.media.audiofx.AudioEffect
import androidx.compose.foundation.clickable
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.drawBehind
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.material.icons.outlined.PlayCircle
import androidx.compose.material.icons.outlined.Palette
import androidx.compose.material.icons.outlined.Lyrics
import androidx.compose.material.icons.outlined.List
import androidx.compose.material.icons.outlined.LibraryMusic
import androidx.compose.material.icons.outlined.GraphicEq
import androidx.compose.material.icons.outlined.Dns
import androidx.compose.material.icons.outlined.CloudDownload
import androidx.compose.material.icons.outlined.AutoAwesome
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.material3.Surface
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Slider
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.nori.music.app.vm.SettingsViewModel
import dev.nori.music.downloads.formatBytes
import dev.nori.music.playback.Equalizer
import dev.nori.music.settings.Quality
import dev.nori.music.settings.ThemeMode
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.size
import dev.nori.music.settings.SwipeAction
import dev.nori.music.settings.TapAction
import dev.nori.music.settings.ServerProfile
import androidx.compose.runtime.LaunchedEffect
import dev.nori.music.settings.ReplayGainMode

private val qualities = listOf(Quality() to "Original", Quality(320, "mp3") to "MP3 320", Quality(192, "opus") to "Opus 192", Quality(128, "opus") to "Opus 128", Quality(96, "opus") to "Opus 96", Quality(64, "opus") to "Opus 64")


/**
 * Search lands on a row, not on a page: the group page is told which row to reveal, the row reports
 * where it is, and the page scrolls there and lets the highlight fade out. A row's key is its own
 * title, so nothing has to be kept in step by hand.
 */
class SettingSpotlight(val key: String?, val onPlaced: (Int) -> Unit)

val LocalSpotlight = androidx.compose.runtime.compositionLocalOf { SettingSpotlight(null) {} }

fun settingKey(title: String) = title.lowercase().replace(Regex("[^a-z0-9]+"), "-").trim('-')

/** The wash that says "this is the one you searched for", fading out once you have seen it. */
@Composable
private fun Modifier.spotlight(title: String): Modifier {
    val spot = LocalSpotlight.current
    val on = spot.key == settingKey(title)
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
fun Toggle(title: String, detail: String, value: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Column {
    Row(
        Modifier.fillMaxWidth().spotlight(title).clickable(enabled) { onChange(!value) }
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
private fun <T> Choice(title: String, value: T, options: List<Pair<T, String>>, enabled: Boolean = true, onChange: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    val dim = if (enabled) 1f else DIMMED
    Column {
    Row(Modifier.fillMaxWidth().spotlight(title).clickable(enabled) { open = true }.padding(horizontal = 16.dp, vertical = 15.dp)) {
        Text(title, Modifier.weight(1f).alpha(dim), style = MaterialTheme.typography.bodyLarge)
        Text(options.firstOrNull { it.first == value }?.second ?: "$value", Modifier.alpha(dim), color = MaterialTheme.colorScheme.primary)
        DropdownMenu(open, { open = false }) { options.forEach { (v, label) -> DropdownMenuItem({ Text(label) }, { onChange(v); open = false }) } }
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
        color = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.07f).over(MaterialTheme.colorScheme.background),
        contentColor = MaterialTheme.colorScheme.onSurface,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 6.dp),
    ) { Column(content = content) }
}

/** A settings group: its own page, so the root of Settings is seven rows instead of eighty. */
private data class Group(val id: String, val title: String, val icon: ImageVector, val summary: String)

private val groups = listOf(
    // Connect first, then what plays, then how it sounds, then how it looks.
    Group("servers", "Servers", Icons.Outlined.Dns, "Accounts and music folder"),
    Group("playing", "Playback", Icons.Outlined.PlayCircle, "Crossfade, AutoMix, queue"),
    Group("sound", "Sound", Icons.Outlined.GraphicEq, "Equalizer, volume, output"),
    Group("look", "Appearance", Icons.Outlined.Palette, "Theme, colours, size"),
    Group("lyrics", "Lyrics", Icons.Outlined.Lyrics, "Display and sources"),
    Group("library", "Library", Icons.Outlined.LibraryMusic, "Gestures, search, history"),
    Group("data", "Downloads and storage", Icons.Outlined.CloudDownload, "Quality, downloads, space"),
    Group("about", "About", Icons.Outlined.Info, "Version, build, licences"),
)

/** Pages reached from inside another page rather than from the list: About's licences. */
private val subpages = listOf(
    Group("licences", "Licences", Icons.Outlined.Info, "What this app is made of"),
)

/** One searchable row: which page it lives on, its title, and the words under it. */
private data class Entry(val group: String, val title: String, val hint: String)

/**
 * What the search can find. A row is matched by its own title, so the entry here and the row on the
 * page cannot drift apart in wording - only in existence, which a missing result makes obvious. The
 * hints are the rows' own descriptions, plus the jargon someone might type (ReplayGain, AMOLED).
 */
private val index = listOf(
    Entry("playing", "Crossfade", "One song fades into the next"),
    Entry("playing", "AutoMix", "Blends songs like a DJ, matching the beat"),
    Entry("playing", "Longest mix", ""),
    Entry("playing", "Match the beat", "Nudges the next song's speed so the beats line up"),
    Entry("playing", "Biggest speed change", ""),
    Entry("playing", "Keep the pitch", "Off lets the pitch follow the speed"),
    Entry("playing", "Swap the bass", "The new song's bass replaces the old one's"),
    Entry("playing", "Muffle the ending", "The outgoing song fades out muffled"),
    Entry("playing", "Echo out clashes", "Overlapping vocals end in an echo instead"),
    Entry("playing", "Measured songs", "Tempo and beats, measured on this phone"),
    Entry("playing", "Keep albums gapless", "No mixing between songs of the same album"),
    Entry("playing", "Fade on play and pause", "A short fade when you play, pause, seek or skip"),
    Entry("playing", "Speed", ""),
    Entry("playing", "Pitch", ""),
    Entry("playing", "Skip silence", "Cuts quiet gaps in and between songs"),
    Entry("playing", "Previous goes back a song", "Instead of restarting the current one"),
    Entry("playing", "Mix up artists when shuffling", "Avoids the same artist or album twice in a row"),
    Entry("playing", "Skip explicit songs", "Songs your server marks explicit"),
    Entry("playing", "Keep playing when the queue ends", "Adds more music automatically"),
    Entry("playing", "Carry on with", "Songs, or a whole album at a time"),
    Entry("playing", "Chosen by", "Similar music, the same artist, genre or era"),
    Entry("playing", "Skip songs that won't play", "Up to three in a row"),
    Entry("playing", "Play downloads when offline", "If the server drops, keep playing from downloads"),
    Entry("sound", "Equalizer and crossfeed", ""),
    Entry("sound", "AutoEQ for headphones", "Applies a known correction curve when headphones connect"),
    Entry("sound", "Remember sound per device", "Each device keeps its own equalizer settings"),
    Entry("sound", "System audio effects", ""),
    Entry("sound", "Even out volume", "ReplayGain. Quiet and loud songs play at the same level"),
    Entry("sound", "Volume for songs without tags", ""),
    Entry("sound", "High quality output", "Plays 24-bit files in full. Turns the equalizer off"),
    Entry("sound", "Bit perfect USB DAC", "Sends the file to a USB DAC unchanged. Android 14 and later"),
    Entry("sound", "Save battery while playing", "Offload. The audio chip decodes instead of the processor"),
    Entry("look", "Theme", "Light, dark or the same as the phone"),
    Entry("look", "Black background", "AMOLED. True black in dark mode, saves power on OLED"),
    Entry("look", "Player in the cover's colours", "Off makes the player black too"),
    Entry("look", "Wallpaper colours", "Material You. Accent colour from your wallpaper"),
    Entry("look", "Colours from the cover", "Pages take their colours from the artwork"),
    Entry("look", "Blur the bottom of the cover", "The player's artwork softens into the page"),
    Entry("look", "Confirm favourites", "A short message when you favourite or unfavourite something"),
    Entry("look", "Text and button size", ""),
    Entry("look", "Less movement", "Shorter, simpler animations"),
    Entry("look", "Animate anyway", "Keeps animations on even when Android's are off"),
    Entry("lyrics", "Fill in words as they're sung", "For lyrics timed word by word"),
    Entry("lyrics", "Text size", ""),
    Entry("lyrics", "Show translations", "When your server has them"),
    Entry("lyrics", "Keep the screen on", "While lyrics are shown and music plays"),
    Entry("lyrics", "Find missing lyrics online", "Asks LRCLIB. Sends the artist and song name"),
    Entry("library", "Tapping a song", ""),
    Entry("library", "Swipe right", ""),
    Entry("library", "Swipe left", ""),
    Entry("library", "Offline search", "Song names kept on the phone so search works without a connection"),
    Entry("library", "Search delay", "How long to wait after you stop typing"),
    Entry("library", "Keep listening history", "Stored on this phone. Powers mixes and stats"),
    Entry("library", "Tell the server what you play", "Scrobbling. Sends your plays to your server"),
    Entry("library", "Count a play after", ""),
    Entry("library", "Look things up online", "Update checks and missing lyrics. Sends the artist and song name"),
    Entry("data", "Quality on Wi-Fi", ""),
    Entry("data", "Quality on mobile data", ""),
    Entry("data", "Quality for downloads", ""),
    Entry("data", "Downloads at once", "How many songs download at the same time"),
    Entry("data", "Download the whole library", ""),
    Entry("data", "Load ahead on Wi-Fi", "Songs fetched before you get to them"),
    Entry("data", "Load ahead on mobile data", ""),
    Entry("data", "Load covers ahead", ""),
    Entry("data", "Space for streamed music", "How much streamed music to keep on the phone"),
    Entry("data", "Stored on this phone", "Streamed music, covers, downloads and the library"),
    Entry("data", "Streamed music", "Clear the streamed music. Downloads stay"),
    Entry("data", "Covers", "Clear the covers. They are fetched again when needed"),
    Entry("about", "Licences", "Open source libraries, fonts and data, and their terms"),
    Entry("servers", "Music folder", ""),
    Entry("servers", "Bitrate limit on the second address", ""),
)

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
private fun ActionRowSetting(title: String, detail: String, button: String, enabled: Boolean = true, detailColour: androidx.compose.ui.graphics.Color? = null, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().spotlight(title).padding(start = 16.dp, end = 8.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = detailColour ?: MaterialTheme.colorScheme.onSurfaceVariant)
        }
        TextButton(onClick, enabled = enabled) { Text(button) }
    }
    Hairline(startIndent = 16.dp)
}


/**
 * What lives on the phone, and a way to throw the throwaway parts out. Downloads are the permanent
 * copy and are removed where they are listed; the streamed music and the covers rebuild themselves.
 */
@Composable
private fun StorageRows(vm: SettingsViewModel) {
    val storage by vm.storage.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    LaunchedEffect(Unit) { vm.refreshStorage() }
    Column(Modifier.fillMaxWidth().spotlight("Stored on this phone").padding(horizontal = 16.dp, vertical = 12.dp)) {
        Text("Stored on this phone", style = MaterialTheme.typography.bodyLarge)
        Text(
            "${formatBytes(storage.streamBytes)} streamed · ${formatBytes(storage.coverBytes)} covers · " +
                "${formatBytes(storage.downloadBytes)} in ${storage.downloadSongs} downloads · ${formatBytes(storage.indexBytes)} library",
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
    Hairline(startIndent = 16.dp)
    val clearing = if (storage.busy) "Clearing…" else "Clear"
    ActionRowSetting("Streamed music", "Oldest goes first. Downloads stay.", clearing, enabled = !storage.busy && storage.streamBytes > 0, onClick = vm::clearStreamCache)
    ActionRowSetting("Covers", "Fetched again when needed.", clearing, enabled = !storage.busy && storage.coverBytes > 0, onClick = vm::clearCovers)
    ActionRowSetting("Downloads", "Remove them from the Downloads page.", "Show", onClick = nav::downloads)
}

@Composable
fun SettingsScreen(vm: SettingsViewModel) {
    val nav = LocalNav.current
    var query by remember { mutableStateOf("") }
    val hits = remember(query) {
        val q = query.trim()
        when {
            q.isBlank() -> emptyList()
            // Titles first, then anything whose explanation mentions it: searching "oled" should find
            // AMOLED black, and "gapless" the switch that keeps an album gapless.
            else -> index.filter { it.title.contains(q, true) } + index.filter { !it.title.contains(q, true) && it.hint.contains(q, true) }
        }
    }
    Column {
        LargeTitle("Settings")
        SearchField(query, { query = it }, "Search settings", Modifier.padding(horizontal = Space.gutter, vertical = 6.dp))
        if (query.isNotBlank()) {
            // A result is the setting itself: tapping opens its page and puts the finger on the row.
            LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
                items(hits, key = { it.group + it.title }) { e ->
                    Column(Modifier.clickable { nav.settingsGroup(e.group, settingKey(e.title)) }) {
                        Column(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 11.dp)) {
                            Text(e.title, style = MaterialTheme.typography.bodyLarge)
                            Text(
                                groups.first { it.id == e.group }.title + (if (e.hint.isNotEmpty()) " · " + e.hint else ""),
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
            items(groups, key = { it.id }) { g ->
                Column(Modifier.clickable { nav.settingsGroup(g.id) }) {
                    Row(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 13.dp), verticalAlignment = Alignment.CenterVertically) {
                        Icon(g.icon, null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary)
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

/** One group's page: its rows on a single plate, scrolled to whichever row the search sent us to. */
@Composable
fun SettingsGroupScreen(vm: SettingsViewModel, id: String, highlight: String = "") {
    val nav = LocalNav.current
    val group = (groups + subpages).firstOrNull { it.id == id } ?: return
    val scroll = rememberScrollState()
    var target by remember { mutableIntStateOf(-1) }
    LaunchedEffect(target) { if (target >= 0) scroll.animateScrollTo((scroll.value + target - 400).coerceAtLeast(0)) }
    CompositionLocalProvider(LocalSpotlight provides SettingSpotlight(highlight.ifEmpty { null }) { y -> if (target < 0) target = y }) {
        Column(Modifier.verticalScroll(scroll)) {
            Row(Modifier.padding(start = 4.dp, end = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
                IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
                Text(group.title, Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
            }
            GroupContent(id, vm)
            Spacer(Modifier.height(Space.section + LocalChromeInset.current))
        }
    }
}

@Composable
private fun GroupContent(id: String, vm: SettingsViewModel) {
    val p by vm.prefs.collectAsStateWithLifecycle()
    val dac by vm.dac.collectAsStateWithLifecycle()
    val sync by vm.sync.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    val context = LocalContext.current
    val folders by vm.musicFolders.collectAsStateWithLifecycle()
    var editing by remember { mutableStateOf<ServerProfile?>(null) }
    LaunchedEffect(p.activeServerId) { vm.loadMusicFolders() }
    editing?.let { e -> androidx.compose.ui.window.Dialog({ editing = null }, androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) { LoginScreen(vm, e) { editing = null } }; }
    val note: @Composable (String) -> Unit = { text ->
        Text(text, Modifier.padding(horizontal = Space.gutter, vertical = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
    when (id) {
        "playing" -> {
            // Bit-perfect output and high quality output both mean exactly the file's samples reach the
            // DAC, so nothing may be mixed into them: every transition here, and skipping silence, is
            // off while either is on. Said here, or the crossfade stays set and silently does nothing.
            val untouched = p.hiRes || dac.bitPerfect
            val live = !untouched
            Section("Between songs") {
                if (untouched) note(if (dac.bitPerfect) "Off while the USB DAC is in bit perfect mode." else "Off while high quality output is on.")
                if (!p.autoMix) Choice("Crossfade", p.crossfadeSec, listOf(0 to "Off", 2 to "2 s", 4 to "4 s", 6 to "6 s", 8 to "8 s", 12 to "12 s"), enabled = live) { v -> vm.update { it.copy(crossfadeSec = v) } }
                Toggle("AutoMix", "Blends songs like a DJ, matching the beat.", p.autoMix, enabled = live) { on -> vm.update { it.copy(autoMix = on) } }
                if (p.autoMix) {
                    Choice("Longest mix", p.autoMixMaxS, listOf(6 to "6 s", 8 to "8 s", 12 to "12 s", 16 to "16 s", 24 to "24 s"), enabled = live) { v -> vm.update { it.copy(autoMixMaxS = v) } }
                    Toggle("Match the beat", "Nudges the next song's speed so the beats line up.", p.autoMixBeatMatch, enabled = live) { on -> vm.update { it.copy(autoMixBeatMatch = on) } }
                    if (p.autoMixBeatMatch) {
                        Choice("Biggest speed change", p.autoMixMaxTempoPct, listOf(2f to "2 %", 4f to "4 %", 6f to "6 %", 8f to "8 %"), enabled = live) { v -> vm.update { it.copy(autoMixMaxTempoPct = v) } }
                        Toggle("Keep the pitch", "Off lets the pitch follow the speed, up to 2 %.", p.autoMixKeepPitch, enabled = live) { on -> vm.update { it.copy(autoMixKeepPitch = on) } }
                    }
                    Toggle("Swap the bass", "The new song's bass replaces the old one's.", p.autoMixBassSwap, enabled = live) { on -> vm.update { it.copy(autoMixBassSwap = on) } }
                    Toggle("Muffle the ending", "The outgoing song fades out muffled.", p.autoMixFilters, enabled = live) { on -> vm.update { it.copy(autoMixFilters = on) } }
                    Toggle("Echo out clashes", "Overlapping vocals end in an echo instead.", p.autoMixEchoOut, enabled = live) { on -> vm.update { it.copy(autoMixEchoOut = on) } }
                    val analysed by vm.analysed.collectAsStateWithLifecycle()
                    LaunchedEffect(Unit) { vm.refreshAnalysed() }
                    ActionRowSetting("Measured songs", "$analysed songs measured for tempo and beats.", "Measure again", enabled = analysed > 0, onClick = vm::clearAnalyses)
                }
                Toggle("Keep albums gapless", "No mixing between songs of the same album.", p.crossfadeKeepAlbums, enabled = live) { on -> vm.update { it.copy(crossfadeKeepAlbums = on) } }
                Choice("Fade on play and pause", p.fadeMs, listOf(0 to "Off", 150 to "150 ms", 300 to "300 ms", 500 to "500 ms", 1000 to "1 s")) { v -> vm.update { it.copy(fadeMs = v) } }
            }
            Section("Controls") {
                Toggle("Previous goes back a song", "Instead of restarting the current one.", p.previousAlwaysSkips) { on -> vm.update { it.copy(previousAlwaysSkips = on) } }
                Choice("Speed", p.speed, listOf(0.75f to "0.75×", 1f to "Normal", 1.25f to "1.25×", 1.5f to "1.5×", 2f to "2×")) { v -> vm.update { it.copy(speed = v) } }
                Choice("Pitch", p.pitch, listOf(0.9f to "−10 %", 0.95f to "−5 %", 1f to "Normal", 1.05f to "+5 %", 1.1f to "+10 %")) { v -> vm.update { it.copy(pitch = v) } }
                Toggle("Skip silence", if (untouched) "Off while the audio is played untouched." else "Cuts quiet gaps in and between songs.", p.skipSilence, enabled = live) { on -> vm.update { it.copy(skipSilence = on) } }
            }
            Section("Queue") {
                Toggle("Mix up artists when shuffling", "Avoids the same artist or album twice in a row.", p.weightedShuffle) { on -> vm.update { it.copy(weightedShuffle = on) } }
                Toggle("Skip explicit songs", "Songs your server marks explicit.", p.skipExplicit) { on -> vm.update { it.copy(skipExplicit = on) } }
                Toggle("Keep playing when the queue ends", "Adds more music automatically.", p.autoFill) { on -> vm.update { it.copy(autoFill = on) } }
                // What arrives and what it is chosen by are two separate questions, so they are two rows:
                // somebody who listens to records wants the next record, whatever it is picked by.
                if (p.autoFill) {
                    Choice("Carry on with", p.autoFillKind, dev.nori.music.settings.AutoFillKind.entries.map { it to it.label }) { v -> vm.update { it.copy(autoFillKind = v) } }
                    Choice("Chosen by", p.autoFillBasis, dev.nori.music.settings.AutoFillBasis.entries.map { it to it.label }) { v -> vm.update { it.copy(autoFillBasis = v) } }
                }
            }
            Section("When something goes wrong") {
                Toggle("Skip songs that won't play", "Up to three in a row.", p.skipOnError) { on -> vm.update { it.copy(skipOnError = on) } }
                Toggle("Play downloads when offline", "If the server drops, keep playing from downloads.", p.bridgeOffline) { on -> vm.update { it.copy(bridgeOffline = on) } }
            }
        }
        "sound" -> {
            // The chain is taken out of the path by the same rule the transitions are, so say so here
            // rather than leaving the row reading "On" over sound it is not touching.
            val bypassed = p.hiRes || dac.bitPerfect
            Section("Equalizer") {
                Row(Modifier.fillMaxWidth().spotlight("Equalizer and crossfeed").clickable(onClick = nav::equalizer).padding(horizontal = 16.dp, vertical = 15.dp)) {
                    Text("Equalizer and crossfeed", Modifier.weight(1f).alpha(if (bypassed) DIMMED else 1f), style = MaterialTheme.typography.bodyLarge)
                    Text(if (bypassed) "Off now" else if (p.dsp) "On" else "Off", Modifier.alpha(if (bypassed) DIMMED else 1f), color = MaterialTheme.colorScheme.primary)
                }
                Hairline(startIndent = 16.dp)
                Toggle("AutoEQ for headphones", "Applies a known correction curve when headphones connect.", p.autoEqAuto) { on -> vm.update { it.copy(autoEqAuto = on) } }
                Toggle("Remember sound per device", "Each device keeps its own equalizer settings.", p.profilePerOutput) { on -> vm.update { it.copy(profilePerOutput = on) } }
                Text("System audio effects", Modifier.fillMaxWidth().spotlight("System audio effects").clickable {
                    runCatching { context.startActivity(Intent(AudioEffect.ACTION_DISPLAY_AUDIO_EFFECT_CONTROL_PANEL).putExtra(AudioEffect.EXTRA_PACKAGE_NAME, context.packageName).putExtra(AudioEffect.EXTRA_CONTENT_TYPE, AudioEffect.CONTENT_TYPE_MUSIC)) }
                }.padding(horizontal = 16.dp, vertical = 15.dp), style = MaterialTheme.typography.bodyLarge)
            }
            Section("Volume") {
                Choice("Even out volume", p.replayGain, listOf(ReplayGainMode.OFF to "Off", ReplayGainMode.TRACK to "Per song", ReplayGainMode.ALBUM to "Per album", ReplayGainMode.AUTO to "Automatic")) { m -> vm.update { it.copy(replayGain = m) } }
                if (p.replayGain != ReplayGainMode.OFF) {
                    Text("Overall level ${signedDb(p.preampDb)} dB", Modifier.padding(start = 16.dp, end = 16.dp, top = 10.dp), style = MaterialTheme.typography.bodySmall)
                    NoriSlider(p.preampDb, -12f..6f, { v -> vm.update { it.copy(preampDb = v) } }, Modifier.padding(horizontal = 16.dp), centred = true)
                    Choice("Volume for songs without tags", p.untaggedGainDb, listOf(0f to "0 dB", -3f to "−3 dB", -6f to "−6 dB", -9f to "−9 dB", -12f to "−12 dB")) { v -> vm.update { it.copy(untaggedGainDb = v) } }
                }
            }
            Section("Output") {
                Toggle("High quality output", "Plays 24-bit files in full. Turns the equalizer off. Starts with the next song.", p.hiRes) { on -> vm.update { it.copy(hiRes = on) } }
                Toggle(
                    "Bit perfect USB DAC",
                    when {
                        dac.bitPerfect -> "On: ${dac.device}, ${dac.sampleRate / 1000.0} kHz, ${dac.bits}-bit."
                        dac.blockedBy != null -> "${dac.device ?: "USB DAC"}: ${dac.blockedBy}"
                        dac.device != null && dac.supported -> "${dac.device} connected. Starts with the music."
                        dac.device != null -> "${dac.device} connected, but this phone can't do bit perfect with it."
                        else -> "Sends the file to a USB DAC unchanged. Skips the equalizer and volume levelling. Android 14 and later."
                    },
                    p.bitPerfect,
                ) { on -> vm.update { it.copy(bitPerfect = on) } }
                // What is actually going out, rather than what was asked for: the one line that settles "is it
                // even reaching the DAC?" without a cable to a laptop.
                val detail = listOfNotNull(
                    dac.modes.takeIf { it.isNotEmpty() }?.let { "Offers " + it.joinToString(", ") },
                    dac.playing?.let { "playing $it" },
                    dac.track?.let { "output $it" },
                )
                if (detail.isNotEmpty()) note(detail.joinToString("  ·  "))
                Toggle(
                    "Save battery while playing",
                    when {
                        dac.device != null -> "Not available while a USB DAC is connected."
                        p.offload && (p.dsp || p.crossfadeSec > 0 || p.skipSilence || p.speed != 1f) -> "Paused now: the equalizer or another effect is on."
                        else -> "The audio chip decodes instead of the processor. Pauses while effects are on."
                    },
                    p.offload,
                ) { on -> vm.update { it.copy(offload = on) } }
            }
        }
        "look" -> {
            Section("Theme") {
                Choice("Theme", p.theme, listOf(ThemeMode.SYSTEM to "Same as the phone", ThemeMode.LIGHT to "Light", ThemeMode.DARK to "Dark")) { v -> vm.update { it.copy(theme = v) } }
                Toggle("Black background", "True black in dark mode. Saves power on OLED screens.", p.amoled) { on -> vm.update { it.copy(amoled = on) } }
                if (p.amoled) Toggle("Player in the cover's colours", "Off makes the player black too.", p.playerColours) { on -> vm.update { it.copy(playerColours = on) } }
                if (android.os.Build.VERSION.SDK_INT >= 31) Toggle("Wallpaper colours", "Accent colour from your wallpaper.", p.dynamicColor) { on -> vm.update { it.copy(dynamicColor = on) } }
                if (!p.dynamicColor || android.os.Build.VERSION.SDK_INT < 31) Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 12.dp), horizontalArrangement = Arrangement.spacedBy(10.dp), verticalAlignment = Alignment.CenterVertically) {
                    listOf(0xFF6750A4, 0xFF1E88E5, 0xFF00897B, 0xFF43A047, 0xFFF4511E, 0xFFE53935, 0xFFD81B60, 0xFF8E24AA).forEach { c ->
                        androidx.compose.foundation.layout.Box(
                            Modifier.size(if (p.accent == c) 36.dp else 30.dp).background(androidx.compose.ui.graphics.Color(c), androidx.compose.foundation.shape.CircleShape).clickable { vm.update { it.copy(accent = c) } },
                        )
                    }
                }
            }
            Section("Cover art") {
                Toggle("Colours from the cover", "Pages take their colours from the artwork.", p.coverColors) { on -> vm.update { it.copy(coverColors = on) } }
                if (android.os.Build.VERSION.SDK_INT >= 31) Toggle("Blur the bottom of the cover", "The player's artwork softens into the page.", p.softSleeve) { on -> vm.update { it.copy(softSleeve = on) } }
            }
            Section("Messages") {
                Toggle("Confirm favourites", "A short message when you favourite or unfavourite something.", p.favouriteNotice) { on -> vm.update { it.copy(favouriteNotice = on) } }
            }
            Section("Size and motion") {
                Choice(
                    "Text and button size", p.uiScale,
                    listOf(0f to "Automatic", 0.9f to "Smaller", 1f to "Same as the phone", 1.1f to "Larger"),
                ) { v -> vm.update { it.copy(uiScale = v) } }
                Toggle("Less movement", "Shorter, simpler animations.", p.reduceMotion) { on -> vm.update { it.copy(reduceMotion = on) } }
                if (!p.reduceMotion) Toggle("Animate anyway", "Keeps animations on even when Android's are off.", p.ignoreSystemMotion) { on -> vm.update { it.copy(ignoreSystemMotion = on) } }
            }
        }
        "lyrics" -> {
            Section("Display") {
                Toggle("Fill in words as they're sung", "For lyrics timed word by word.", p.lyricsSweep) { on -> vm.update { it.copy(lyricsSweep = on) } }
                Choice("Text size", p.lyricsSize, listOf(0 to "Small", 1 to "Medium", 2 to "Large")) { v -> vm.update { it.copy(lyricsSize = v) } }
                Toggle("Show translations", "When your server has them.", p.lyricsTranslation) { on -> vm.update { it.copy(lyricsTranslation = on) } }
                Toggle("Keep the screen on", "While lyrics are shown and music plays.", p.lyricsKeepScreenOn) { on -> vm.update { it.copy(lyricsKeepScreenOn = on) } }
            }
            // The switch for looking things up at all lives in Library, but somebody looking for lyrics
            // looks here, so the lyrics half of it is offered here too and turns the other one on with it.
            Section("Source") {
                Toggle("Find missing lyrics online", "Asks LRCLIB. Sends the artist and song name.", p.lyricsLrclib && p.thirdPartyLookups) { on -> vm.update { it.copy(lyricsLrclib = on, thirdPartyLookups = on || it.thirdPartyLookups) } }
            }
        }
        "library" -> {
            Section("Gestures") {
                Choice("Tapping a song", p.tapAction, listOf(TapAction.PLAY_LIST to "Plays the list from there", TapAction.PLAY_ONE to "Plays only that song", TapAction.QUEUE to "Adds it to the queue", TapAction.PLAY_NEXT to "Plays it next")) { v -> vm.update { it.copy(tapAction = v) } }
                val swipes = listOf(SwipeAction.NONE to "Nothing", SwipeAction.QUEUE to "Add to queue", SwipeAction.PLAY_NEXT to "Play next", SwipeAction.FAVOURITE to "Favourite", SwipeAction.DOWNLOAD to "Download")
                Choice("Swipe right", p.swipeRight, swipes) { v -> vm.update { it.copy(swipeRight = v) } }
                Choice("Swipe left", p.swipeLeft, swipes) { v -> vm.update { it.copy(swipeLeft = v) } }
            }
            Section("Search") {
                ActionRowSetting(
                    "Offline search",
                    sync.error ?: "${sync.indexed.songs} songs · ${sync.indexed.albums} albums · ${sync.indexed.artists} artists on this phone",
                    if (sync.running) "Updating…" else "Update",
                    enabled = !sync.running,
                    detailColour = if (sync.error != null) MaterialTheme.colorScheme.error else null,
                    onClick = vm::syncLibrary,
                )
                Choice("Search delay", p.liveSearchDelayMs, listOf(150 to "150 ms", 250 to "250 ms", 350 to "350 ms", 500 to "500 ms", 800 to "800 ms")) { ms -> vm.update { it.copy(liveSearchDelayMs = ms) } }
            }
            Section("Listening history") {
                Toggle("Keep listening history", "Stored on this phone. Powers mixes and stats.", p.tasteModel) { on -> vm.update { it.copy(tasteModel = on) } }
                Toggle("Tell the server what you play", "Sends your plays to your server (scrobbling).", p.scrobble) { on -> vm.update { it.copy(scrobble = on) } }
                if (p.scrobble) Choice("Count a play after", p.scrobblePercent, listOf(25 to "25 %", 50 to "50 %", 75 to "75 %", 90 to "90 %", 100 to "the whole song")) { v -> vm.update { it.copy(scrobblePercent = v) } }
            }
            Section("Online") {
                Toggle("Look things up online", "Update checks and missing lyrics. Sends the artist and song name.", p.thirdPartyLookups) { on -> vm.update { it.copy(thirdPartyLookups = on, lyricsLrclib = on) } }
            }
        }
        "data" -> {
            Section("Streaming quality") {
                Choice("Quality on Wi-Fi", p.wifi, qualities) { q -> vm.update { it.copy(wifi = q) } }
                Choice("Quality on mobile data", p.mobile, qualities) { q -> vm.update { it.copy(mobile = q) } }
            }
            Section("Downloads") {
                Choice("Quality for downloads", p.download, qualities) { q -> vm.update { it.copy(download = q) } }
                Choice("Downloads at once", p.parallelDownloads, (1..10).map { it to "$it" }) { n -> vm.update { it.copy(parallelDownloads = n) } }
                ActionRowSetting("Download the whole library", "Every song, at the download quality.", "Download", enabled = sync.indexed.songs > 0u) { vm.downloadLibrary() }
            }
            Section("Loading ahead") {
                Choice("Load ahead on Wi-Fi", p.precacheWifi, listOf(1 to "Next song", 2 to "2 songs", 3 to "3 songs", 5 to "5 songs", 10 to "10 songs")) { v -> vm.update { it.copy(precacheWifi = v) } }
                Choice("Load ahead on mobile data", p.precacheMobile, listOf(1 to "Next song", 2 to "2 songs", 3 to "3 songs", 5 to "5 songs")) { v -> vm.update { it.copy(precacheMobile = v) } }
                Choice("Load covers ahead", p.coversAhead, listOf(0 to "Off", 1 to "1", 2 to "2", 3 to "3", 5 to "5", 8 to "8", 10 to "10")) { n -> vm.update { it.copy(coversAhead = n) } }
            }
            Section("Storage") {
                Choice("Space for streamed music", p.cacheMb, listOf(256 to "256 MB", 1024 to "1 GB", 4096 to "4 GB", 16384 to "16 GB")) { mb -> vm.update { it.copy(cacheMb = mb) }; vm.applyCacheLimit() }
                StorageRows(vm)
            }
        }
        "about" -> AboutContent({ title, content -> Section(title, content) }) { nav.settingsGroup("licences") }
        "licences" -> LicencesContent { title, content -> Section(title, content) }
        "servers" -> {
            Section("Accounts") {
                p.servers.forEach { server ->
                    val active = server.id == p.activeServerId
                    Row(Modifier.fillMaxWidth().clickable(enabled = !active) { vm.switchServer(server) }.padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                        Column(Modifier.weight(1f)) {
                            Text(server.label, color = if (active) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface)
                            Text(listOfNotNull(server.user.ifEmpty { "API key" }, if (active) "in use" else "tap to switch", "Wi-Fi only".takeIf { server.wifiOnly }, "second address".takeIf { server.altUrl.isNotBlank() }).joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                        TextButton({ editing = server }) { Text("Edit") }
                        TextButton({ vm.removeServer(server.id) }) { Text("Remove") }
                    }
                    Hairline(startIndent = 16.dp)
                }
                TextButton({ editing = vm.newProfile() }, Modifier.padding(horizontal = 8.dp)) { Text("Add server") }
            }
            val folderRow = folders.size > 1
            val altRow = p.server?.altUrl?.isNotBlank() == true
            if (folderRow || altRow) Section("This server") {
                if (folderRow) Choice("Music folder", p.server?.musicFolderId.orEmpty(), listOf("" to "All") + folders.map { it.id to it.name }) { id -> p.server?.let { vm.updateServer(it.copy(musicFolderId = id)) } }
                if (altRow) Choice("Bitrate limit on the second address", p.server?.altMaxBitRate ?: 0, listOf(0 to "No limit", 320 to "320 kbps", 192 to "192 kbps", 128 to "128 kbps", 96 to "96 kbps")) { v -> p.server?.let { vm.updateServer(it.copy(altMaxBitRate = v)) } }
            }
        }
    }
}
