package dev.flint.music.app.ui

import android.content.Intent
import android.media.audiofx.AudioEffect
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
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
import androidx.compose.material3.Switch
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

@Composable
fun Toggle(title: String, detail: String, value: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(enabled) { onChange(!value) }.padding(horizontal = Space.gutter, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 14.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(value, onChange, enabled = enabled)
    }
}

@Composable
private fun <T> Choice(title: String, value: T, options: List<Pair<T, String>>, onChange: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().clickable { open = true }.padding(horizontal = Space.gutter, vertical = 15.dp)) {
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.bodyLarge)
        Text(options.firstOrNull { it.first == value }?.second ?: "$value", color = MaterialTheme.colorScheme.primary)
        DropdownMenu(open, { open = false }) { options.forEach { (v, label) -> DropdownMenuItem({ Text(label) }, { onChange(v); open = false }) } }
    }
}

private val qualities = listOf(Quality() to "Original", Quality(320, "mp3") to "MP3 320", Quality(192, "opus") to "Opus 192", Quality(128, "opus") to "Opus 128", Quality(96, "opus") to "Opus 96", Quality(64, "opus") to "Opus 64")

@Composable
fun SettingsScreen(vm: SettingsViewModel) {
    val p by vm.prefs.collectAsStateWithLifecycle()
    val dac by vm.dac.collectAsStateWithLifecycle()
    val sync by vm.sync.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    val context = LocalContext.current
    val folders by vm.musicFolders.collectAsStateWithLifecycle()
    var editing by remember { mutableStateOf<ServerProfile?>(null) }
    LaunchedEffect(p.activeServerId) { vm.loadMusicFolders() }
    editing?.let { e -> androidx.compose.ui.window.Dialog({ editing = null }, androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false)) { LoginScreen(vm, e) { editing = null } }; }
    Column(Modifier.verticalScroll(rememberScrollState())) {
        LargeTitle("Settings")
        SectionTitle("Look")
        Choice("Theme", p.theme, listOf(ThemeMode.SYSTEM to "Follow system", ThemeMode.LIGHT to "Light", ThemeMode.DARK to "Dark")) { v -> vm.update { it.copy(theme = v) } }
        Toggle("AMOLED black", "True black in dark mode: those pixels are switched off, which also saves power on OLED screens", p.amoled) { on -> vm.update { it.copy(amoled = on) } }
        Toggle("Colours from the cover", "Album, artist and playlist pages and the player take their colour from the artwork, which runs edge to edge", p.coverColors) { on -> vm.update { it.copy(coverColors = on) } }
        if (android.os.Build.VERSION.SDK_INT >= 31) Toggle("Wallpaper colours", "Material You: take the colours from your wallpaper", p.dynamicColor) { on -> vm.update { it.copy(dynamicColor = on) } }
        if (!p.dynamicColor || android.os.Build.VERSION.SDK_INT < 31) Row(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 8.dp), horizontalArrangement = Arrangement.spacedBy(10.dp)) {
            listOf(0xFF6750A4, 0xFF1E88E5, 0xFF00897B, 0xFF43A047, 0xFFF4511E, 0xFFE53935, 0xFFD81B60, 0xFF8E24AA).forEach { c ->
                androidx.compose.foundation.layout.Box(
                    Modifier.size(if (p.accent == c) 36.dp else 30.dp).background(androidx.compose.ui.graphics.Color(c), androidx.compose.foundation.shape.CircleShape).clickable { vm.update { it.copy(accent = c) } },
                )
            }
        }

        SectionTitle("Streaming quality")
        Choice("On Wi-Fi", p.wifi, qualities) { q -> vm.update { it.copy(wifi = q) } }
        Choice("On mobile data", p.mobile, qualities) { q -> vm.update { it.copy(mobile = q) } }
        Choice("Downloads", p.download, qualities) { q -> vm.update { it.copy(download = q) } }
        Choice("Stream cache", p.cacheMb, listOf(256 to "256 MB", 1024 to "1 GB", 4096 to "4 GB", 16384 to "16 GB")) { mb -> vm.update { it.copy(cacheMb = mb) } }

        SectionTitle("Audio")
        Toggle("Hardware offload", "Decode on the audio chip so the CPU can sleep. The biggest battery saver; turned off while the equalizer is on.", p.offload) { on -> vm.update { it.copy(offload = on) } }
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
        if (dac.modes.isNotEmpty()) Text("This DAC offers: " + dac.modes.joinToString(", ") + (dac.playing?.let { "  ·  now playing $it" } ?: ""), Modifier.padding(horizontal = Space.gutter), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Toggle("Hi-res float output", "Keeps 24-bit files at full precision instead of 16-bit. The equalizer is unavailable in this mode. Applies the next time playback starts from cold.", p.hiRes) { on -> vm.update { it.copy(hiRes = on) } }
        Choice("ReplayGain", p.replayGain, listOf(ReplayGainMode.OFF to "Off", ReplayGainMode.TRACK to "Track", ReplayGainMode.ALBUM to "Album", ReplayGainMode.AUTO to "Automatic")) { m -> vm.update { it.copy(replayGain = m) } }
        if (p.replayGain != ReplayGainMode.OFF) {
            Text("Pre-amp ${"%+.1f".format(p.preampDb)} dB", Modifier.padding(horizontal = Space.gutter), style = MaterialTheme.typography.bodySmall)
            Slider(p.preampDb, { v -> vm.update { it.copy(preampDb = v) } }, Modifier.padding(horizontal = Space.gutter), valueRange = -12f..6f)
        }
        Row(Modifier.fillMaxWidth().clickable(onClick = nav::equalizer).padding(horizontal = Space.gutter, vertical = 15.dp)) {
            Text("Equalizer and crossfeed", Modifier.weight(1f)); Text(if (p.dsp) "On" else "Off", color = MaterialTheme.colorScheme.primary)
        }
        Toggle(
            "AutoMix",
            "Transitions like a DJ set: tempo matched, beats aligned, bass swapped, the ending filtered out. Each track is analysed once while it plays; a transition costs a few percent of a core for its few seconds.",
            p.autoMix,
        ) { on -> vm.update { it.copy(autoMix = on) } }
        if (p.autoMix) {
            Choice("Longest transition", p.autoMixMaxS, listOf(6 to "6 s", 8 to "8 s", 12 to "12 s", 16 to "16 s", 24 to "24 s")) { v -> vm.update { it.copy(autoMixMaxS = v) } }
            Toggle("Match tempo and beats", "Speeds the next song up or down a little so the beats line up", p.autoMixBeatMatch) { on -> vm.update { it.copy(autoMixBeatMatch = on) } }
            if (p.autoMixBeatMatch) {
                Choice("Largest tempo change", p.autoMixMaxTempoPct, listOf(2f to "2 %", 4f to "4 %", 6f to "6 %", 8f to "8 %")) { v -> vm.update { it.copy(autoMixMaxTempoPct = v) } }
                Toggle("Keep pitch", "Off changes speed and pitch together, like a turntable: cheaper, and limited to 2 %", p.autoMixKeepPitch) { on -> vm.update { it.copy(autoMixKeepPitch = on) } }
            }
            Toggle("Bass swap", "The next song's bass comes in on a bar line as the old one's goes out, so they never clash", p.autoMixBassSwap) { on -> vm.update { it.copy(autoMixBassSwap = on) } }
            Toggle("Filter sweep", "The outgoing song fades through a closing low-pass filter", p.autoMixFilters) { on -> vm.update { it.copy(autoMixFilters = on) } }
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
            Text("Something above needs the decoded audio, so hardware offload is paused. Playback still runs in bursts from a deep buffer.", Modifier.padding(horizontal = Space.gutter), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Text("System audio effects", Modifier.fillMaxWidth().clickable {
            runCatching { context.startActivity(Intent(AudioEffect.ACTION_DISPLAY_AUDIO_EFFECT_CONTROL_PANEL).putExtra(AudioEffect.EXTRA_PACKAGE_NAME, context.packageName).putExtra(AudioEffect.EXTRA_CONTENT_TYPE, AudioEffect.CONTENT_TYPE_MUSIC)) }
        }.padding(horizontal = Space.gutter, vertical = 15.dp))

        SectionTitle("Features")
        Text("Anything switched off here is not even started: no listener, no socket, no audio processing.", Modifier.padding(horizontal = Space.gutter), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Toggle("Listening history and taste model", "Kept on this device only. Feeds mixes, smart playlists and the listening stats. One small write when a track ends.", p.tasteModel) { on -> vm.update { it.copy(tasteModel = on) } }
        Toggle("Spread artists when shuffling", "Shuffle avoids two songs by the same artist or album in a row", p.weightedShuffle) { on -> vm.update { it.copy(weightedShuffle = on) } }
        Toggle("Apply a profile per output", "When headphones or a DAC are connected, load the sound profile bound to them", p.profilePerOutput) { on -> vm.update { it.copy(profilePerOutput = on) } }
        Toggle("Third-party lookups", "Lyrics from lrclib.net when the server has none, the AutoEQ headphone list, update checks. Sends artist and title to those services.", p.thirdPartyLookups) { on -> vm.update { it.copy(thirdPartyLookups = on) } }

        SectionTitle("Playback behaviour")
        Choice("Fade on play, pause, seek and skip", p.fadeMs, listOf(0 to "Off", 150 to "150 ms", 300 to "300 ms", 500 to "500 ms", 1000 to "1 s")) { v -> vm.update { it.copy(fadeMs = v) } }
        Toggle("No crossfade inside an album", "Tracks that follow each other on the same album stay gapless", p.crossfadeKeepAlbums) { on -> vm.update { it.copy(crossfadeKeepAlbums = on) } }
        Choice("Pitch", p.pitch, listOf(0.9f to "−10 %", 0.95f to "−5 %", 1f to "Normal", 1.05f to "+5 %", 1.1f to "+10 %")) { v -> vm.update { it.copy(pitch = v) } }
        Toggle("Previous always goes back a track", "Instead of first rewinding the current one", p.previousAlwaysSkips) { on -> vm.update { it.copy(previousAlwaysSkips = on) } }
        Toggle("Skip tracks that fail to load", "Up to three in a row, then playback stops", p.skipOnError) { on -> vm.update { it.copy(skipOnError = on) } }
        Choice("Fetch ahead on Wi-Fi", p.precacheWifi, listOf(1 to "Next track", 2 to "2 tracks", 3 to "3 tracks", 5 to "5 tracks", 10 to "10 tracks")) { v -> vm.update { it.copy(precacheWifi = v) } }
        Choice("Fetch ahead on mobile data", p.precacheMobile, listOf(1 to "Next track", 2 to "2 tracks", 3 to "3 tracks", 5 to "5 tracks")) { v -> vm.update { it.copy(precacheMobile = v) } }
        if (p.replayGain != ReplayGainMode.OFF) Choice("Gain for files without ReplayGain tags", p.untaggedGainDb, listOf(0f to "0 dB", -3f to "−3 dB", -6f to "−6 dB", -9f to "−9 dB", -12f to "−12 dB")) { v -> vm.update { it.copy(untaggedGainDb = v) } }

        SectionTitle("Lyrics")
        Toggle("Word-by-word sweep", "The line being sung fills in word by word. Redraws one line of text per frame, only while the lyrics are on screen; off means the line just lights up.", p.lyricsSweep) { on -> vm.update { it.copy(lyricsSweep = on) } }
        Toggle("Keep the screen on", "While lyrics are showing and music is playing", p.lyricsKeepScreenOn) { on -> vm.update { it.copy(lyricsKeepScreenOn = on) } }
        Toggle(
            "Fetch missing lyrics from LRCLIB", if (p.thirdPartyLookups) "When the server has no synced lyrics, ask lrclib.net (sends artist, title and length)" else "Needs \"Third-party lookups\" in Features",
            p.lyricsLrclib && p.thirdPartyLookups, enabled = p.thirdPartyLookups,
        ) { on -> vm.update { it.copy(lyricsLrclib = on) } }
        Toggle("Show translations", "When the server has a translation layer", p.lyricsTranslation) { on -> vm.update { it.copy(lyricsTranslation = on) } }
        Choice("Text size", p.lyricsSize, listOf(0 to "Small", 1 to "Medium", 2 to "Large")) { v -> vm.update { it.copy(lyricsSize = v) } }

        SectionTitle("Lists")
        Choice("Tapping a song", p.tapAction, listOf(TapAction.PLAY_LIST to "Plays the list from there", TapAction.PLAY_ONE to "Plays only that song", TapAction.QUEUE to "Adds it to the queue", TapAction.PLAY_NEXT to "Plays it next")) { v -> vm.update { it.copy(tapAction = v) } }
        val swipes = listOf(SwipeAction.NONE to "Nothing", SwipeAction.QUEUE to "Add to queue", SwipeAction.PLAY_NEXT to "Play next", SwipeAction.FAVOURITE to "Favourite", SwipeAction.DOWNLOAD to "Download")
        Choice("Swipe right", p.swipeRight, swipes) { v -> vm.update { it.copy(swipeRight = v) } }
        Choice("Swipe left", p.swipeLeft, swipes) { v -> vm.update { it.copy(swipeLeft = v) } }
        Toggle("Skip explicit songs", "Songs the server marks explicit are skipped during playback", p.skipExplicit) { on -> vm.update { it.copy(skipExplicit = on) } }
        Text("Home shelves", Modifier.padding(horizontal = 16.dp, vertical = 6.dp), style = MaterialTheme.typography.titleSmall)
        HomeRow.entries.forEach { row ->
            val on = row in p.homeRows
            Row(Modifier.fillMaxWidth().padding(start = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                Text(row.title, Modifier.weight(1f))
                if (on) TextButton({ vm.update { s -> s.copy(homeRows = s.homeRows.toMutableList().also { l -> val i = l.indexOf(row); if (i > 0) { l.removeAt(i); l.add(i - 1, row) } }) } }) { Text("Up") }
                Switch(on, { show -> vm.update { s -> s.copy(homeRows = if (show) s.homeRows + row else s.homeRows - row) } }, Modifier.padding(end = 16.dp))
            }
        }

        SectionTitle("Library")
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

        SectionTitle("Servers")
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

