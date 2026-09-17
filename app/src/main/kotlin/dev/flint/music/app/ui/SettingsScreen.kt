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
import dev.flint.music.settings.ReplayGainMode

@Composable
private fun Toggle(title: String, detail: String, value: Boolean, enabled: Boolean = true, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(enabled) { onChange(!value) }.padding(horizontal = 16.dp, vertical = 10.dp), verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(value, onChange, enabled = enabled)
    }
}

@Composable
private fun <T> Choice(title: String, value: T, options: List<Pair<T, String>>, onChange: (T) -> Unit) {
    var open by remember { mutableStateOf(false) }
    Row(Modifier.fillMaxWidth().clickable { open = true }.padding(horizontal = 16.dp, vertical = 14.dp)) {
        Text(title, Modifier.weight(1f))
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
    Column(Modifier.verticalScroll(rememberScrollState())) {
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
                dac.bitPerfect -> "Active: ${dac.device} at ${dac.sampleRate / 1000.0} kHz / ${dac.bits} bit, no mixing or resampling"
                dac.device != null && dac.supported -> "${dac.device} connected; engages when playback starts"
                dac.device != null -> "${dac.device} connected, but this phone offers no bit-perfect mode for it"
                else -> "Android 14+: sends audio to a USB DAC untouched, at the track's own sample rate. Equalizer and ReplayGain are bypassed."
            },
            p.bitPerfect,
        ) { on -> vm.update { it.copy(bitPerfect = on) } }
        Toggle("Hi-res float output", "Keeps 24-bit files at full precision instead of 16-bit. The equalizer is unavailable in this mode. Applies the next time playback starts from cold.", p.hiRes) { on -> vm.update { it.copy(hiRes = on) } }
        Choice("ReplayGain", p.replayGain, listOf(ReplayGainMode.OFF to "Off", ReplayGainMode.TRACK to "Track", ReplayGainMode.ALBUM to "Album")) { m -> vm.update { it.copy(replayGain = m) } }
        if (p.replayGain != ReplayGainMode.OFF) {
            Text("Pre-amp ${"%+.1f".format(p.preampDb)} dB", Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall)
            Slider(p.preampDb, { v -> vm.update { it.copy(preampDb = v) } }, Modifier.padding(horizontal = 16.dp), valueRange = -12f..6f)
        }
        Row(Modifier.fillMaxWidth().clickable(onClick = nav::equalizer).padding(horizontal = 16.dp, vertical = 14.dp)) {
            Text("Equalizer", Modifier.weight(1f)); Text(if (p.eqEnabled) "On" else "Off", color = MaterialTheme.colorScheme.primary)
        }
        Text("System audio effects", Modifier.fillMaxWidth().clickable {
            runCatching { context.startActivity(Intent(AudioEffect.ACTION_DISPLAY_AUDIO_EFFECT_CONTROL_PANEL).putExtra(AudioEffect.EXTRA_PACKAGE_NAME, context.packageName).putExtra(AudioEffect.EXTRA_CONTENT_TYPE, AudioEffect.CONTENT_TYPE_MUSIC)) }
        }.padding(horizontal = 16.dp, vertical = 14.dp))

        SectionTitle("Library")
        Toggle("Scrobble", "Report plays and the play queue to the server", p.scrobble) { on -> vm.update { it.copy(scrobble = on) } }
        Toggle("Keep playing", "When the queue runs out, continue with similar songs from the library", p.autoFill) { on -> vm.update { it.copy(autoFill = on) } }
        Choice("Live search delay", p.liveSearchDelayMs, listOf(150 to "150 ms", 250 to "250 ms", 350 to "350 ms", 500 to "500 ms", 800 to "800 ms")) { ms -> vm.update { it.copy(liveSearchDelayMs = ms) } }
        Row(Modifier.fillMaxWidth().padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Offline search index")
                Text(sync.error ?: "${sync.indexed.songs} songs · ${sync.indexed.albums} albums · ${sync.indexed.artists} artists", style = MaterialTheme.typography.bodySmall, color = if (sync.error != null) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant)
            }
            TextButton(vm::syncLibrary, enabled = !sync.running) { Text(if (sync.running) "Syncing…" else "Sync all") }
        }

        SectionTitle("Server")
        Text("${p.user} @ ${p.serverUrl}", Modifier.padding(horizontal = 16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant)
        TextButton(vm::logout, Modifier.padding(horizontal = 8.dp)) { Text("Log out") }
    }
}

@Composable
fun EqualizerScreen(vm: SettingsViewModel) {
    val p by vm.prefs.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    Column(Modifier.verticalScroll(rememberScrollState())) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Equalizer", style = MaterialTheme.typography.titleLarge)
        }
        Toggle("Enabled", "Runs in the Rust core on the playback thread. Costs battery: offload is off while this is on.", p.eqEnabled) { on -> vm.update { it.copy(eqEnabled = on) } }
        Equalizer.FREQUENCIES.forEachIndexed { i, hz ->
            Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text(if (hz >= 1000) "${hz / 1000}k" else "$hz", Modifier.padding(end = 4.dp), style = MaterialTheme.typography.labelMedium)
                Slider(p.eqGains[i], { v -> vm.update { it.copy(eqGains = it.eqGains.toMutableList().also { g -> g[i] = v }) } }, Modifier.weight(1f), enabled = p.eqEnabled, valueRange = -12f..12f)
                Text("%+.0f".format(p.eqGains[i]), style = MaterialTheme.typography.labelMedium)
            }
        }
        TextButton({ vm.update { it.copy(eqGains = List(10) { 0f }) } }, Modifier.padding(8.dp)) { Text("Reset") }
    }
}
