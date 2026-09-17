package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Close
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Slider
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.flint.music.app.vm.SettingsViewModel
import dev.flint.music.settings.Band
import dev.flint.music.settings.BandKind

private fun hz(f: Float) = if (f >= 1000) "%.4gk".format(f / 1000).replace(".000k", "k").replace(".00k", "k") else "%.0f".format(f)

@Composable
fun EqualizerScreen(vm: SettingsViewModel) {
    val p by vm.prefs.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    var importing by remember { mutableStateOf(false) }
    var editing by remember { mutableStateOf(-1) }
    // Only while this screen is open does the player give up its deep buffer for instant response.
    DisposableEffect(Unit) { vm.setTuning(true); onDispose { vm.setTuning(false) } }

    if (importing) ImportDialog(vm) { importing = false }
    p.eqBands.getOrNull(editing)?.let { BandDialog(it, { b -> vm.setBand(editing, b) }, { vm.removeBand(editing); editing = -1 }) { editing = -1 } }

    Column(Modifier.verticalScroll(rememberScrollState())) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Equalizer", Modifier.weight(1f), style = MaterialTheme.typography.titleLarge)
            Switch(p.eqEnabled, { on -> vm.update { it.copy(eqEnabled = on) } }, Modifier.padding(end = 16.dp))
        }
        Text("Parametric, runs in the Rust core. Tap a band's label to change its frequency, width or type.", Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)

        p.eqBands.forEachIndexed { i, b ->
            Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                Text("${hz(b.freq)}${if (b.kind == BandKind.LOW_SHELF) " ↙" else if (b.kind == BandKind.HIGH_SHELF) " ↗" else ""}", Modifier.width(56.dp).clickable { editing = i }, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                Slider(b.gainDb, { v -> vm.setBand(i, b.copy(gainDb = v)) }, Modifier.weight(1f), enabled = p.eqEnabled, valueRange = -12f..12f)
                Text("%+.1f".format(b.gainDb), Modifier.width(40.dp), style = MaterialTheme.typography.labelMedium)
            }
        }
        Row(Modifier.padding(horizontal = 8.dp)) {
            TextButton(vm::addBand) { Text("Add band") }
            TextButton({ importing = true }) { Text("Import AutoEQ / APO") }
            TextButton(vm::resetBands) { Text("Reset") }
        }

        Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Pre-amp ${"%+.1f".format(p.effectivePreampDb)} dB${if (p.eqPreampDb == null) " (automatic)" else ""}")
                Text("Automatic pulls the level down by the largest boost so the curve cannot clip", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Switch(p.eqPreampDb == null, { auto -> vm.update { it.copy(eqPreampDb = if (auto) null else it.effectivePreampDb) } })
        }
        p.eqPreampDb?.let { v -> Slider(v, { x -> vm.update { it.copy(eqPreampDb = x) } }, Modifier.padding(horizontal = 16.dp), enabled = p.eqEnabled, valueRange = -20f..6f) }

        SectionTitle("Crossfeed")
        Text(if (p.crossfeedDb > 0f) "%.1f dB: each ear also hears a little of the other channel, like loudspeakers. For headphones.".format(p.crossfeedDb) else "Off", Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Slider(p.crossfeedDb, { v -> vm.update { it.copy(crossfeedDb = if (v < 1f) 0f else v) } }, Modifier.padding(horizontal = 16.dp), valueRange = 0f..9f)
    }
}

@Composable
private fun ImportDialog(vm: SettingsViewModel, onDone: () -> Unit) {
    var text by remember { mutableStateOf("") }
    var error by remember { mutableStateOf(false) }
    AlertDialog(
        onDismissRequest = onDone, title = { Text("Import preset") },
        text = {
            Column {
                Text("Paste an AutoEQ ParametricEQ.txt or an Equalizer APO config.", style = MaterialTheme.typography.bodySmall)
                OutlinedTextField(text, { text = it; error = false }, Modifier.fillMaxWidth().padding(top = 8.dp), minLines = 5, maxLines = 10, isError = error, placeholder = { Text("Preamp: -6.2 dB\nFilter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70") })
                if (error) Text("No filters found in that text", color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall)
            }
        },
        confirmButton = { TextButton({ if (vm.importPreset(text) > 0) onDone() else error = true }) { Text("Import") } },
        dismissButton = { TextButton(onDone) { Text("Cancel") } },
    )
}

@Composable
private fun BandDialog(band: Band, onChange: (Band) -> Unit, onRemove: () -> Unit, onDone: () -> Unit) {
    AlertDialog(
        onDismissRequest = onDone, title = { Text("${hz(band.freq)} Hz") },
        text = {
            Column {
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    listOf(BandKind.PEAKING to "Peak", BandKind.LOW_SHELF to "Low shelf", BandKind.HIGH_SHELF to "High shelf").forEach { (k, label) ->
                        TextButton({ onChange(band.copy(kind = k)) }) { Text(label, color = if (band.kind == k) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant) }
                    }
                }
                Text("Frequency", style = MaterialTheme.typography.labelMedium)
                // Logarithmic: the slider position is the exponent, 20 Hz to 20 kHz.
                Slider(kotlin.math.log10(band.freq / 20f) / 3f, { x -> onChange(band.copy(freq = (20f * Math.pow(10.0, x * 3.0).toFloat()))) })
                Text("Q %.2f".format(band.q), style = MaterialTheme.typography.labelMedium)
                Slider(band.q, { q -> onChange(band.copy(q = q)) }, valueRange = 0.2f..8f)
            }
        },
        confirmButton = { TextButton(onDone) { Text("Done") } },
        dismissButton = { TextButton(onRemove) { Icon(Icons.Filled.Close, null); Text("Remove band") } },
    )
}
