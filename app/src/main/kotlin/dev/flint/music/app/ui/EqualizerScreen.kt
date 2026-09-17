package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.FilterChip
import androidx.lifecycle.compose.collectAsStateWithLifecycle
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
import dev.flint.music.settings.BandChannel
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
                val mark = when {
                    b.channel == BandChannel.LEFT -> " L"
                    b.channel == BandChannel.RIGHT -> " R"
                    b.kind == BandKind.LOW_SHELF || b.kind == BandKind.LOW_SHELF_SLOPE -> " ↙"
                    b.kind == BandKind.HIGH_SHELF || b.kind == BandKind.HIGH_SHELF_SLOPE -> " ↗"
                    !b.kind.usesGain -> " ∿"
                    else -> ""
                }
                Text(hz(b.freq) + mark, Modifier.width(64.dp).clickable { editing = i }, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                if (b.kind.usesGain) {
                    Slider(b.gainDb, { v -> vm.setBand(i, b.copy(gainDb = v)) }, Modifier.weight(1f), enabled = p.eqEnabled, valueRange = -12f..12f)
                    Text("%+.1f".format(b.gainDb), Modifier.width(40.dp), style = MaterialTheme.typography.labelMedium)
                } else {
                    Text(b.kind.label, Modifier.weight(1f).clickable { editing = i }, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
        Row(Modifier.padding(horizontal = 8.dp)) {
            TextButton(vm::addBand) { Text("Add band") }
            TextButton({ importing = true }) { Text("Paste a preset") }
            TextButton(LocalNav.current::autoEq) { Text("Headphone presets") }
            TextButton(vm::resetBands) { Text("Reset") }
        }
        SectionTitle("Presets")
        LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            items(vm.presets) { preset -> FilledTonalButton({ vm.applyPreset(preset) }) { Text(preset.name) } }
        }

        Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text("Pre-amp ${"%+.1f".format(p.effectivePreampDb)} dB${if (p.eqPreampDb == null) " (automatic)" else ""}")
                Text("Automatic pulls the level down by the largest boost so the curve cannot clip", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            Switch(p.eqPreampDb == null, { auto -> vm.update { it.copy(eqPreampDb = if (auto) null else it.effectivePreampDb) } })
        }
        p.eqPreampDb?.let { v -> Slider(v, { x -> vm.update { it.copy(eqPreampDb = x) } }, Modifier.padding(horizontal = 16.dp), enabled = p.eqEnabled, valueRange = -20f..6f) }

        SectionTitle("Output")
        Row(Modifier.padding(horizontal = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Balance", Modifier.width(80.dp))
            Slider(p.balance, { v -> vm.update { it.copy(balance = if (kotlin.math.abs(v) < 0.04f) 0f else v) } }, Modifier.weight(1f), valueRange = -1f..1f)
            Text(if (p.balance == 0f) "centre" else "%s %.0f%%".format(if (p.balance < 0) "L" else "R", kotlin.math.abs(p.balance) * 100), Modifier.width(72.dp), style = MaterialTheme.typography.labelMedium)
        }
        Toggle("Mono", "Both channels summed, for one-earbud listening", p.mono) { on -> vm.update { it.copy(mono = on) } }
        Toggle("Limiter", "Catches what a boost or a positive ReplayGain would clip. Adds 5 ms of delay; below the ceiling the audio passes through untouched.", p.limiter) { on -> vm.update { it.copy(limiter = on) } }
        if (p.limiter) {
            Text("Ceiling %.1f dB".format(p.limiterThresholdDb), Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall)
            Slider(p.limiterThresholdDb, { v -> vm.update { it.copy(limiterThresholdDb = v) } }, Modifier.padding(horizontal = 16.dp), valueRange = -12f..0f)
        }

        SectionTitle("Profiles")
        val profiles by vm.profiles.collectAsStateWithLifecycle()
        val outputs by vm.outputs.collectAsStateWithLifecycle()
        val output by vm.currentOutput.collectAsStateWithLifecycle()
        var naming by remember { mutableStateOf(false) }
        var newName by remember { mutableStateOf("") }
        if (naming) AlertDialog(
            onDismissRequest = { naming = false }, title = { Text("Save these settings") },
            text = { OutlinedTextField(newName, { newName = it }, singleLine = true, label = { Text("Name") }) },
            confirmButton = { TextButton({ vm.saveProfile(newName); newName = ""; naming = false }, enabled = newName.isNotBlank()) { Text("Save") } },
            dismissButton = { TextButton({ naming = false }) { Text("Cancel") } },
        )
        Text("Playing through: $output", Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        profiles.forEach { profile ->
            Column(Modifier.padding(horizontal = 16.dp, vertical = 4.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(profile.name, Modifier.weight(1f).clickable { vm.applyProfile(profile) })
                    TextButton({ vm.applyProfile(profile) }) { Text("Apply") }
                    TextButton({ vm.deleteProfile(profile.name) }) { Text("Delete") }
                }
                LazyRow(horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                    items(outputs) { o ->
                        FilterChip(o in profile.outputs, { vm.bindProfile(profile, o, o !in profile.outputs) }, { Text(o, style = MaterialTheme.typography.labelSmall) })
                    }
                }
            }
        }
        TextButton({ naming = true }, Modifier.padding(horizontal = 8.dp)) { Text("Save current settings as a profile") }
        Text("A profile bound to an output is applied when that output becomes active; switch that off in Settings → Features.", Modifier.padding(horizontal = 16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)

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
                LazyRow(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    items(BandKind.entries) { k ->
                        TextButton({ onChange(band.copy(kind = k)) }) { Text(k.label, color = if (band.kind == k) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant) }
                    }
                }
                Row(horizontalArrangement = Arrangement.spacedBy(4.dp)) {
                    BandChannel.entries.forEach { c ->
                        TextButton({ onChange(band.copy(channel = c)) }) { Text(c.label, color = if (band.channel == c) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant) }
                    }
                }
                Text("Frequency", style = MaterialTheme.typography.labelMedium)
                // Logarithmic: the slider position is the exponent, 20 Hz to 20 kHz.
                Slider(kotlin.math.log10(band.freq / 20f) / 3f, { x -> onChange(band.copy(freq = (20f * Math.pow(10.0, x * 3.0).toFloat()))) })
                Text(if (band.kind == BandKind.LOW_SHELF_SLOPE || band.kind == BandKind.HIGH_SHELF_SLOPE) "Slope %.2f".format(band.q) else "Q %.2f".format(band.q), style = MaterialTheme.typography.labelMedium)
                Slider(band.q, { q -> onChange(band.copy(q = q)) }, valueRange = 0.2f..8f)
            }
        },
        confirmButton = { TextButton(onDone) { Text("Done") } },
        dismissButton = { TextButton(onRemove) { Icon(Icons.Filled.Close, null); Text("Remove band") } },
    )
}
