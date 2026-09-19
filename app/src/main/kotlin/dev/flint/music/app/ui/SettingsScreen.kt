package dev.flint.music.app.ui

import android.content.Intent
import android.media.audiofx.AudioEffect
import androidx.compose.foundation.clickable
import androidx.compose.ui.layout.positionInWindow
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.graphics.vector.ImageVector
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
import dev.flint.music.app.vm.SettingsViewModel
import dev.flint.music.playback.Equalizer
import dev.flint.music.settings.Quality
import dev.flint.music.settings.HomeRow
import dev.flint.music.settings.ThemeMode
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.size
import dev.flint.music.settings.SwipeAction
import dev.flint.music.settings.TapAction
import dev.flint.music.settings.ServerProfile
import androidx.compose.runtime.LaunchedEffect
import dev.flint.music.settings.ReplayGainMode

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
        Column(Modifier.weight(1f).padding(end = 14.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        FlintSwitch(value, onChange, enabled = enabled)
    }
    Hairline(startIndent = 16.dp)
    }
}

@Composable
private fun <T> Choice(title: String, value: T, options: List<Pair<T, String>>, onChange: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Column {
    Row(Modifier.fillMaxWidth().spotlight(title).clickable { open = true }.padding(horizontal = 16.dp, vertical = 15.dp)) {
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
        Text(options.firstOrNull { it.first == value }?.second ?: "$value", color = MaterialTheme.colorScheme.primary)
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

/** A settings group: its own page, so the root of Settings is nine rows instead of eighty. */
private data class Group(val id: String, val title: String, val icon: ImageVector, val summary: String)

private val groups = listOf(

    Group("look", "Look", Icons.Outlined.Palette, "Theme, cover colours, AMOLED"),

    Group("quality", "Streaming quality", Icons.Outlined.CloudDownload, "Bitrate per network, cache size"),

    Group("audio", "Audio", Icons.Outlined.GraphicEq, "Offload, USB DAC, ReplayGain, AutoMix, equalizer"),

    Group("features", "Features", Icons.Outlined.AutoAwesome, "History, mixes, lookups"),

    Group("playback", "Playback behaviour", Icons.Outlined.PlayCircle, "Fades, gapless, pitch, fetch ahead"),

    Group("lyrics", "Lyrics", Icons.Outlined.Lyrics, "Sweep, size, translations"),

    Group("lists", "Lists", Icons.Outlined.List, "Tap and swipe actions, home rows"),

    Group("library", "Library", Icons.Outlined.LibraryMusic, "Scrobbling, offline index, downloads"),

    Group("servers", "Servers", Icons.Outlined.Dns, "Accounts, music folder, bitrate limits"),

)

/** One searchable row: which page it lives on, its title, and the words under it. */
private data class Entry(val group: String, val title: String, val hint: String)

/**
 * What the search can find. A row is matched by its own title, so the entry here and the row on the
 * page cannot drift apart in wording - only in existence, which a missing result makes obvious.
 */
private val index = listOf(
    Entry("look", "Theme", ""),
    Entry("look", "Interface size", "Automatic keeps the layout's proportions on a phone set to a large display size"),
    Entry("look", "AMOLED black", "True black in dark mode: those pixels are switched off, which also saves power on OLED screens"),
    Entry("look", "Reduce motion", "Shorter, plainer movement throughout"),
    Entry("look", "Animate even when Android's are off", "Lyrics glide to the next line even with system animations turned off"),
    Entry("look", "Colours from the cover", "Album, artist and playlist pages and the player take their colour from the artwork, which runs edge to edge"),
    Entry("look", "Wallpaper colours", "Material You: take the colours from your wallpaper"),
    Entry("quality", "On Wi-Fi", ""),
    Entry("quality", "On mobile data", ""),
    Entry("quality", "Downloads", ""),
    Entry("quality", "Stream cache", ""),
    Entry("audio", "Hardware offload", "Saves battery by decoding on a dedicated audio chip; turned off automatically while the equalizer is on."),
    Entry("audio", "Bit-perfect USB DAC", ""),
    Entry("audio", "Hi-res output", "Plays 24-bit files at full quality instead of 16-bit; the equalizer isn't available in this mode, and it takes effect next time you start playback."),
    Entry("audio", "ReplayGain", ""),
    Entry("audio", "AutoMix", "Blends the next track in like a DJ set: matching tempo, aligning beats, swapping the bass and filtering out the ending."),
    Entry("audio", "Longest transition", ""),
    Entry("audio", "Match tempo and beats", "Speeds the next song up or down a little so the beats line up"),
    Entry("audio", "Largest tempo change", ""),
    Entry("audio", "Keep pitch", "Off changes speed and pitch together, like a turntable, and limits the change to 2 %"),
    Entry("audio", "Bass swap", "The next song's bass comes in on a bar line as the old one's goes out, so they never clash"),
    Entry("audio", "Filter sweep", "The outgoing song fades out as it's gradually muffled"),
    Entry("audio", "Crossfade", ""),
    Entry("audio", "Playback speed", ""),
    Entry("audio", "Skip silence", "Cuts silent stretches inside and between tracks"),
    Entry("audio", "Equalizer and crossfeed", ""),
    Entry("audio", "Apply AutoEQ automatically", "Headphones with a known AutoEQ curve get it as soon as they connect, instead of being asked"),
    Entry("features", "Listening history and taste model", "Kept on this device only; powers mixes, smart playlists and your listening stats."),
    Entry("features", "Spread artists when shuffling", "Shuffle avoids two songs by the same artist or album in a row"),
    Entry("features", "Apply a profile per output", "When headphones or a DAC are connected, load the sound profile bound to them"),
    Entry("features", "Third-party lookups", "Looks up missing lyrics and checks for app updates on its own, sending the artist and title of what's playing"),
    Entry("playback", "Fade on play, pause, seek and skip", ""),
    Entry("playback", "No crossfade inside an album", "Tracks that follow each other on the same album stay gapless"),
    Entry("playback", "Pitch", ""),
    Entry("playback", "Previous always goes back a track", "Instead of first rewinding the current one"),
    Entry("playback", "Skip tracks that fail to load", "Up to three in a row, then playback stops"),
    Entry("playback", "Fetch ahead on Wi-Fi", ""),
    Entry("playback", "Fetch ahead on mobile data", ""),
    Entry("playback", "Gain for files without ReplayGain tags", ""),
    Entry("lyrics", "Word-by-word sweep", "Fills in each word as it's sung, for lyrics with word-by-word timing; other lyrics light up a line at a time"),
    Entry("lyrics", "Keep the screen on", "While lyrics are showing and music is playing"),
    Entry("lyrics", "Show translations", "When the server has a translation layer"),
    Entry("lyrics", "Text size", ""),
    Entry("lists", "Tapping a song", ""),
    Entry("lists", "Swipe right", ""),
    Entry("lists", "Swipe left", ""),
    Entry("lists", "Skip explicit songs", "Songs the server marks explicit are skipped during playback"),
    Entry("library", "Scrobble", "Report plays and the play queue to the server"),
    Entry("library", "Count a play after", ""),
    Entry("library", "Keep playing", "When the queue runs out, continue with similar songs from the library"),
    Entry("library", "Live search delay", ""),
    Entry("library", "Downloads at once", "How many songs download at the same time; the rest wait their turn"),
    Entry("servers", "Music folder", ""),
    Entry("servers", "Max bitrate on the second address", ""),)


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
    val group = groups.firstOrNull { it.id == id } ?: return
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
    when (id) {
        "look" -> SettingsCard {
            Choice("Theme", p.theme, listOf(ThemeMode.SYSTEM to "Follow system", ThemeMode.LIGHT to "Light", ThemeMode.DARK to "Dark")) { v -> vm.update { it.copy(theme = v) } }
            Toggle("AMOLED black", "True black in dark mode: those pixels are switched off, which also saves power on OLED screens", p.amoled) { on -> vm.update { it.copy(amoled = on) } }
            if (p.amoled) Toggle(
                "Player in the cover's colours",
                "The full-screen player keeps the record's colours, as Apple Music's does. Off makes that screen black too.",
                p.playerColours,
            ) { on -> vm.update { it.copy(playerColours = on) } }
            Toggle(
                "Reduce motion",
                "Shorter, plainer movement throughout. Follows the system setting when animations are turned off there.",
                p.reduceMotion,
            ) { on -> vm.update { it.copy(reduceMotion = on) } }
            if (!p.reduceMotion) Toggle(
                "Animate even when Android's are off",
                "Android's animation setting is often turned off just for speed, and with it off every movement here - the lyrics gliding to the next line, above all - becomes a jump. On, this app moves anyway.",
                p.ignoreSystemMotion,
            ) { on -> vm.update { it.copy(ignoreSystemMotion = on) } }
            Choice(
                "Interface size", p.uiScale,
                listOf(0f to "Automatic", 0.9f to "Smaller", 1f to "As the system", 1.1f to "Larger"),
            ) { v -> vm.update { it.copy(uiScale = v) } }
            Toggle("Colours from the cover", "Album, artist and playlist pages and the player take their colour from the artwork, which runs edge to edge", p.coverColors) { on -> vm.update { it.copy(coverColors = on) } }
            if (android.os.Build.VERSION.SDK_INT >= 31) Toggle("Wallpaper colours", "Material You: take the colours from your wallpaper", p.dynamicColor) { on -> vm.update { it.copy(dynamicColor = on) } }
            if (!p.dynamicColor || android.os.Build.VERSION.SDK_INT < 31) Row(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                listOf(0xFF6750A4, 0xFF1E88E5, 0xFF00897B, 0xFF43A047, 0xFFF4511E, 0xFFE53935, 0xFFD81B60, 0xFF8E24AA).forEach { c ->
                    androidx.compose.foundation.layout.Box(
                        Modifier.size(if (p.accent == c) 36.dp else 30.dp).background(androidx.compose.ui.graphics.Color(c), androidx.compose.foundation.shape.CircleShape).clickable { vm.update { it.copy(accent = c) } },
                    )
                }
            }

        }
        "quality" -> SettingsCard {
            Choice("On Wi-Fi", p.wifi, qualities) { q -> vm.update { it.copy(wifi = q) } }
            Choice("On mobile data", p.mobile, qualities) { q -> vm.update { it.copy(mobile = q) } }
            Choice("Downloads", p.download, qualities) { q -> vm.update { it.copy(download = q) } }
            Choice("Stream cache", p.cacheMb, listOf(256 to "256 MB", 1024 to "1 GB", 4096 to "4 GB", 16384 to "16 GB")) { mb -> vm.update { it.copy(cacheMb = mb) } }

        }
        "audio" -> SettingsCard {
            Toggle(
                "Hardware offload",
                if (dac.device != null) "Stands down while a USB DAC is connected: the audio chip has no path to it. Saves battery on the phone's own outputs."
                else "Saves battery by decoding on a dedicated audio chip; turned off automatically while the equalizer is on.",
                p.offload,
            ) { on -> vm.update { it.copy(offload = on) } }
            Toggle(
                "Bit-perfect USB DAC",
                when {
                    dac.bitPerfect -> "Active: ${dac.device} at ${dac.sampleRate / 1000.0} kHz / ${dac.bits} bit, no mixing, resampling or volume scaling"
                    dac.blockedBy != null -> "${dac.device ?: "USB DAC"}: ${dac.blockedBy}"
                    dac.device != null && dac.supported -> "${dac.device} connected; engages when playback starts"
                    dac.device != null -> "${dac.device} connected, but this phone offers no bit-perfect mode for it"
                    else -> "Android 14+: sends audio to a USB DAC untouched, at the track's own sample rate. Equalizer and ReplayGain are bypassed."
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
            if (detail.isNotEmpty()) Text(detail.joinToString("  ·  "), Modifier.padding(horizontal = Space.gutter), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Toggle("Hi-res output", "Plays 24-bit files at full quality instead of 16-bit; the equalizer isn't available in this mode, and it takes effect next time you start playback.", p.hiRes) { on -> vm.update { it.copy(hiRes = on) } }
            Choice("ReplayGain", p.replayGain, listOf(ReplayGainMode.OFF to "Off", ReplayGainMode.TRACK to "Track", ReplayGainMode.ALBUM to "Album", ReplayGainMode.AUTO to "Automatic")) { m -> vm.update { it.copy(replayGain = m) } }
            if (p.replayGain != ReplayGainMode.OFF) {
                Text("Pre-amp ${signedDb(p.preampDb)} dB", Modifier.padding(horizontal = Space.gutter), style = MaterialTheme.typography.bodySmall)
                FlintSlider(p.preampDb, -12f..6f, { v -> vm.update { it.copy(preampDb = v) } }, Modifier.padding(horizontal = 16.dp), centred = true)
            }
            Row(Modifier.fillMaxWidth().clickable(onClick = nav::equalizer).padding(horizontal = 16.dp, vertical = 15.dp)) {
                Text("Equalizer and crossfeed", Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
                Text(if (p.dsp) "On" else "Off", color = MaterialTheme.colorScheme.primary)
            }
            Hairline(startIndent = 16.dp)
            Toggle(
                "Apply AutoEQ automatically",
                "Headphones with a known AutoEQ curve get it as soon as they connect, instead of being asked. Choose a curve for a single device, like a DAC, under Equalizer → Devices.",
                p.autoEqAuto,
            ) { on -> vm.update { it.copy(autoEqAuto = on) } }
            Toggle(
                "AutoMix",
                "Blends the next track in like a DJ set: matching tempo, aligning beats, swapping the bass and filtering out the ending.",
                p.autoMix,
            ) { on -> vm.update { it.copy(autoMix = on) } }
            if (p.autoMix) {
                Choice("Longest transition", p.autoMixMaxS, listOf(6 to "6 s", 8 to "8 s", 12 to "12 s", 16 to "16 s", 24 to "24 s")) { v -> vm.update { it.copy(autoMixMaxS = v) } }
                Toggle("Match tempo and beats", "Speeds the next song up or down a little so the beats line up", p.autoMixBeatMatch) { on -> vm.update { it.copy(autoMixBeatMatch = on) } }
                if (p.autoMixBeatMatch) {
                    Choice("Largest tempo change", p.autoMixMaxTempoPct, listOf(2f to "2 %", 4f to "4 %", 6f to "6 %", 8f to "8 %")) { v -> vm.update { it.copy(autoMixMaxTempoPct = v) } }
                    Toggle("Keep pitch", "Off changes speed and pitch together, like a turntable, and limits the change to 2 %", p.autoMixKeepPitch) { on -> vm.update { it.copy(autoMixKeepPitch = on) } }
                }
                Toggle("Bass swap", "The next song's bass comes in on a bar line as the old one's goes out, so they never clash", p.autoMixBassSwap) { on -> vm.update { it.copy(autoMixBassSwap = on) } }
                Toggle("Filter sweep", "The outgoing song fades out as it's gradually muffled", p.autoMixFilters) { on -> vm.update { it.copy(autoMixFilters = on) } }
                val analysed by vm.analysed.collectAsStateWithLifecycle()
                LaunchedEffect(Unit) { vm.refreshAnalysed() }
                Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text("Measured tracks")
                        Text("$analysed analysed while playing. Tempo, beats and cue points, kept on this device.", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    TextButton(vm::clearAnalyses, enabled = analysed > 0) { Text("Measure again") }
                }
            }
            if (!p.autoMix) Choice("Crossfade", p.crossfadeSec, listOf(0 to "Off", 2 to "2 s", 4 to "4 s", 6 to "6 s", 8 to "8 s", 12 to "12 s")) { v -> vm.update { it.copy(crossfadeSec = v) } }
            Choice("Playback speed", p.speed, listOf(0.75f to "0.75×", 1f to "1×", 1.25f to "1.25×", 1.5f to "1.5×", 2f to "2×")) { v -> vm.update { it.copy(speed = v) } }
            Toggle("Skip silence", "Cuts silent stretches inside and between tracks", p.skipSilence) { on -> vm.update { it.copy(skipSilence = on) } }
            if (p.offload && (p.dsp || p.crossfadeSec > 0 || p.skipSilence || p.speed != 1f)) {
                Text("Hardware offload is paused while the equalizer, crossfade or another audio effect is switched on.", Modifier.padding(horizontal = Space.gutter), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Text("System audio effects", Modifier.fillMaxWidth().clickable {
                runCatching { context.startActivity(Intent(AudioEffect.ACTION_DISPLAY_AUDIO_EFFECT_CONTROL_PANEL).putExtra(AudioEffect.EXTRA_PACKAGE_NAME, context.packageName).putExtra(AudioEffect.EXTRA_CONTENT_TYPE, AudioEffect.CONTENT_TYPE_MUSIC)) }
            }.padding(horizontal = Space.gutter, vertical = 15.dp))

        }
        "features" -> SettingsCard {
            Toggle("Listening history and taste model", "Kept on this device only; powers mixes, smart playlists and your listening stats.", p.tasteModel) { on -> vm.update { it.copy(tasteModel = on) } }
            Toggle("Spread artists when shuffling", "Shuffle avoids two songs by the same artist or album in a row", p.weightedShuffle) { on -> vm.update { it.copy(weightedShuffle = on) } }
            Toggle("Apply a profile per output", "When headphones or a DAC are connected, load the sound profile bound to them", p.profilePerOutput) { on -> vm.update { it.copy(profilePerOutput = on) } }
            Toggle("Third-party lookups", "Looks up missing lyrics and checks for app updates on its own, sending the artist and title of what's playing", p.thirdPartyLookups) { on -> vm.update { it.copy(thirdPartyLookups = on, lyricsLrclib = on) } }

        }
        "playback" -> SettingsCard {
            Choice("Fade on play, pause, seek and skip", p.fadeMs, listOf(0 to "Off", 150 to "150 ms", 300 to "300 ms", 500 to "500 ms", 1000 to "1 s")) { v -> vm.update { it.copy(fadeMs = v) } }
            Toggle("No crossfade inside an album", "Tracks that follow each other on the same album stay gapless", p.crossfadeKeepAlbums) { on -> vm.update { it.copy(crossfadeKeepAlbums = on) } }
            Choice("Pitch", p.pitch, listOf(0.9f to "−10 %", 0.95f to "−5 %", 1f to "Normal", 1.05f to "+5 %", 1.1f to "+10 %")) { v -> vm.update { it.copy(pitch = v) } }
            Toggle("Previous always goes back a track", "Instead of first rewinding the current one", p.previousAlwaysSkips) { on -> vm.update { it.copy(previousAlwaysSkips = on) } }
            Toggle("Skip tracks that fail to load", "Up to three in a row, then playback stops", p.skipOnError) { on -> vm.update { it.copy(skipOnError = on) } }
            Choice("Fetch ahead on Wi-Fi", p.precacheWifi, listOf(1 to "Next track", 2 to "2 tracks", 3 to "3 tracks", 5 to "5 tracks", 10 to "10 tracks")) { v -> vm.update { it.copy(precacheWifi = v) } }
            Choice("Fetch ahead on mobile data", p.precacheMobile, listOf(1 to "Next track", 2 to "2 tracks", 3 to "3 tracks", 5 to "5 tracks")) { v -> vm.update { it.copy(precacheMobile = v) } }
            if (p.replayGain != ReplayGainMode.OFF) Choice("Gain for files without ReplayGain tags", p.untaggedGainDb, listOf(0f to "0 dB", -3f to "−3 dB", -6f to "−6 dB", -9f to "−9 dB", -12f to "−12 dB")) { v -> vm.update { it.copy(untaggedGainDb = v) } }

        }
        "lyrics" -> SettingsCard {
            Toggle("Word-by-word sweep", "Fills in each word as it's sung, for lyrics with word-by-word timing; other lyrics light up a line at a time", p.lyricsSweep) { on -> vm.update { it.copy(lyricsSweep = on) } }
            Toggle("Keep the screen on", "While lyrics are showing and music is playing", p.lyricsKeepScreenOn) { on -> vm.update { it.copy(lyricsKeepScreenOn = on) } }
            Toggle("Show translations", "When the server has a translation layer", p.lyricsTranslation) { on -> vm.update { it.copy(lyricsTranslation = on) } }
            Choice("Text size", p.lyricsSize, listOf(0 to "Small", 1 to "Medium", 2 to "Large")) { v -> vm.update { it.copy(lyricsSize = v) } }

        }
        "lists" -> SettingsCard {
            Choice("Tapping a song", p.tapAction, listOf(TapAction.PLAY_LIST to "Plays the list from there", TapAction.PLAY_ONE to "Plays only that song", TapAction.QUEUE to "Adds it to the queue", TapAction.PLAY_NEXT to "Plays it next")) { v -> vm.update { it.copy(tapAction = v) } }
            val swipes = listOf(SwipeAction.NONE to "Nothing", SwipeAction.QUEUE to "Add to queue", SwipeAction.PLAY_NEXT to "Play next", SwipeAction.FAVOURITE to "Favourite", SwipeAction.DOWNLOAD to "Download")
            Choice("Swipe right", p.swipeRight, swipes) { v -> vm.update { it.copy(swipeRight = v) } }
            Choice("Swipe left", p.swipeLeft, swipes) { v -> vm.update { it.copy(swipeLeft = v) } }
            Toggle("Skip explicit songs", "Songs the server marks explicit are skipped during playback", p.skipExplicit) { on -> vm.update { it.copy(skipExplicit = on) } }
            Column(Modifier.padding(horizontal = 16.dp, vertical = 8.dp)) {
                Text("Home shelves", style = MaterialTheme.typography.titleSmall)
                Text(
                    "Choose which shelves appear. To reorder them, drag on the home page.",
                    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            HomeRow.entries.forEach { row ->
                val on = row in p.homeRows
                Column {
                    Row(Modifier.fillMaxWidth().padding(start = 16.dp, end = 16.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(row.title, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
                        FlintSwitch(on, { show -> vm.update { s -> s.copy(homeRows = if (show) s.homeRows + row else s.homeRows - row) } })
                    }
                    Hairline(startIndent = 16.dp)
                }
            }

        }
        "library" -> SettingsCard {
            Toggle("Scrobble", "Report plays and the play queue to the server", p.scrobble) { on -> vm.update { it.copy(scrobble = on) } }
            if (p.scrobble) Choice("Count a play after", p.scrobblePercent, listOf(25 to "25 %", 50 to "50 %", 75 to "75 %", 90 to "90 %", 100 to "the whole track")) { v -> vm.update { it.copy(scrobblePercent = v) } }
            Toggle("Keep playing", "When the queue runs out, continue with similar songs from the library", p.autoFill) { on -> vm.update { it.copy(autoFill = on) } }
            Choice("Live search delay", p.liveSearchDelayMs, listOf(150 to "150 ms", 250 to "250 ms", 350 to "350 ms", 500 to "500 ms", 800 to "800 ms")) { ms -> vm.update { it.copy(liveSearchDelayMs = ms) } }
            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Offline search index")
                    Text(sync.error ?: "${sync.indexed.songs} songs · ${sync.indexed.albums} albums · ${sync.indexed.artists} artists", style = MaterialTheme.typography.bodySmall, color = if (sync.error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
                }
                TextButton(vm::syncLibrary, enabled = !sync.running) { Text(if (sync.running) "Syncing…" else "Sync all") }
            }

            Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text("Download whole library")
                    Text("Every song in the offline index, at the download quality above", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                TextButton({ vm.downloadLibrary() }, enabled = sync.indexed.songs > 0u) { Text("Download") }
            }
            Choice("Downloads at once", p.parallelDownloads, (1..10).map { it to "$it" }) { n -> vm.update { it.copy(parallelDownloads = n) } }

        }
        "servers" -> SettingsCard {
            p.servers.forEach { server ->
                val active = server.id == p.activeServerId
                Row(Modifier.fillMaxWidth().clickable(enabled = !active) { vm.switchServer(server) }.padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Column(Modifier.weight(1f)) {
                        Text(server.label, color = if (active) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurface)
                        Text(listOfNotNull(server.user.ifEmpty { "API key" }, if (active) "active" else "tap to switch", "Wi-Fi only".takeIf { server.wifiOnly }, "second address".takeIf { server.altUrl.isNotBlank() }).joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    TextButton({ editing = server }) { Text("Edit") }
                    TextButton({ vm.removeServer(server.id) }) { Text("Remove") }
                }
            }
            TextButton({ editing = vm.newProfile() }, Modifier.padding(horizontal = 8.dp)) { Text("Add server") }
            if (folders.size > 1) Choice("Music folder", p.server?.musicFolderId.orEmpty(), listOf("" to "All") + folders.map { it.id to it.name }) { id -> p.server?.let { vm.updateServer(it.copy(musicFolderId = id)) } }
            if (p.server?.altUrl?.isNotBlank() == true) Choice("Max bitrate on the second address", p.server?.altMaxBitRate ?: 0, listOf(0 to "No limit", 320 to "320 kbps", 192 to "192 kbps", 128 to "128 kbps", 96 to "96 kbps")) { v -> p.server?.let { vm.updateServer(it.copy(altMaxBitRate = v)) } }
        }
    }
}
