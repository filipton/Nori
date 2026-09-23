package dev.nori.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.material3.Surface
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.lazy.LazyRow
import androidx.compose.foundation.lazy.items
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.filled.Add
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.nori.music.app.vm.SettingsViewModel
import dev.nori.music.settings.Band
import dev.nori.music.settings.BandChannel
import dev.nori.music.settings.BandKind

/** The limiter's gain reduction, sampled while this screen is resumed and dropped the moment it is not. */
@Composable
private fun limiterMeter(): Float {
    var value by remember { mutableStateOf(0f) }
    var resumed by remember { mutableStateOf(false) }
    androidx.lifecycle.compose.LifecycleResumeEffect(Unit) { resumed = true; onPauseOrDispose { resumed = false } }
    androidx.compose.runtime.LaunchedEffect(resumed) {
        while (resumed) {
            value = dev.nori.music.playback.Equalizer.active?.gainReductionDb ?: 0f
            kotlinx.coroutines.delay(120)
        }
    }
    return value
}

/** A band's label, its frequency and a mark for its kind (nori-core's `fmt::eq_band_label`). */
private fun bandLabel(b: Band): String = dev.nori.music.ffi.eqBandLabel(
    b.freq, b.channel == BandChannel.LEFT, b.channel == BandChannel.RIGHT,
    b.kind == BandKind.LOW_SHELF || b.kind == BandKind.LOW_SHELF_SLOPE,
    b.kind == BandKind.HIGH_SHELF || b.kind == BandKind.HIGH_SHELF_SLOPE, b.kind.usesGain,
)

@Composable
fun EqualizerScreen(vm: SettingsViewModel) {
    val p by vm.prefs.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    var importing by remember { mutableStateOf(false) }
    var editing by remember { mutableStateOf(-1) }
    // Only while this screen is open does the player give up its deep buffer for instant response.
    // Low-latency mode costs a rebuild of the audio output, which is a small drop in the sound. Opening
    // this screen to look is not a reason to pay it - the first change to a band is. That used to happen
    // on entry, and on a DAC it was a noticeable break in the music just for opening the page.
    val tuned = remember { mutableStateOf(false) }
    val settled = remember { mutableStateOf(false) }
    LaunchedEffect(p.eqBands, p.eqPreampDb, p.crossfeedDb, p.balance) {
        if (!settled.value) { settled.value = true; return@LaunchedEffect }
        if (!tuned.value && p.eqEnabled) { tuned.value = true; vm.setTuning(true) }
    }
    DisposableEffect(Unit) { onDispose { if (tuned.value) vm.setTuning(false) } }

    if (importing) ImportDialog(vm) { importing = false }
    p.eqBands.getOrNull(editing)?.let { BandDialog(it, { b -> vm.setBand(editing, b) }, { vm.removeBand(editing); editing = -1 }) { editing = -1 } }

    Column(Modifier.verticalScroll(rememberScrollState()).padding(bottom = LocalChromeInset.current)) {
        Row(Modifier.padding(start = 4.dp, end = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Equalizer", Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
            NoriSwitch(p.eqEnabled, { on -> vm.update { it.copy(eqEnabled = on) } })
        }
        Text(
            "Tap a band's label to change its frequency, width or type.",
            Modifier.padding(horizontal = Space.gutter, vertical = 2.dp),
            style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
        // Two settings switch the whole sample chain off. Without this the screen looks broken: bands
        // move, the limiter says it is on, and nothing whatsoever happens to the sound.
        val dac by vm.dac.collectAsStateWithLifecycle()
        val bypass = when {
            dac.bitPerfect -> "Bit-perfect USB output is active, so nothing here touches the audio."
            p.hiRes -> "High quality output is on, so nothing here changes the sound. Turn it off in Settings, under Sound."
            else -> null
        }
        if (bypass != null) Surface(
            shape = CardShape, color = MaterialTheme.colorScheme.errorContainer,
            modifier = Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 8.dp),
        ) {
            Text(bypass, Modifier.padding(12.dp), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onErrorContainer)
        }

        p.eqBands.forEachIndexed { i, b ->
            Row(Modifier.padding(horizontal = Space.gutter), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(10.dp)) {
                // Once per band shape, not on every frame of a gain drag.
                val label = remember(b.freq, b.channel, b.kind) { bandLabel(b) }
                Text(label, Modifier.width(56.dp).clickable { editing = i }, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
                if (b.kind.usesGain) {
                    NoriSlider(b.gainDb, -12f..12f, { v -> vm.setBand(i, b.copy(gainDb = v)) }, Modifier.weight(1f), enabled = p.eqEnabled, centred = true)
                    Text(
                        signedDb(b.gainDb), Modifier.width(42.dp),
                        style = MaterialTheme.typography.labelMedium, textAlign = androidx.compose.ui.text.style.TextAlign.End,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                } else {
                    Text(b.kind.label, Modifier.weight(1f).clickable { editing = i }, style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
        }
        LazyRow(contentPadding = PaddingValues(horizontal = Space.gutter, vertical = 10.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            item { Chip("Add band", false, onClick = vm::addBand) }
            item { Chip("Paste a preset", false) { importing = true } }
            item { Chip("Headphone presets", false, onClick = nav::autoEq) }
            item { Chip("Reset", false, onClick = vm::resetBands) }
        }
        SectionTitle("Presets")
        LazyRow(contentPadding = PaddingValues(horizontal = 16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            items(vm.presets) { preset -> Chip(preset.name, false) { vm.applyPreset(preset) } }
        }

        Row(Modifier.padding(horizontal = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(dev.nori.music.ffi.eqPreamp(p.effectivePreampDb, p.eqPreampDb == null))
                Text("Automatic pulls the level down by the largest boost so the curve cannot clip", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            NoriSwitch(p.eqPreampDb == null, { auto -> vm.update { it.copy(eqPreampDb = if (auto) null else it.effectivePreampDb) } })
        }
        p.eqPreampDb?.let { v -> NoriSlider(v, -20f..6f, { x -> vm.update { it.copy(eqPreampDb = x) } }, Modifier.padding(horizontal = Space.gutter), enabled = p.eqEnabled) }

        SectionTitle("Output")
        Row(Modifier.padding(horizontal = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
            Text("Balance", Modifier.width(80.dp))
            NoriSlider(p.balance, -1f..1f, { v -> vm.update { it.copy(balance = dev.nori.music.ffi.eqBalanceSnap(v)) } }, Modifier.weight(1f), centred = true)
            Text(dev.nori.music.ffi.eqBalance(p.balance), Modifier.width(72.dp), style = MaterialTheme.typography.labelMedium)
        }
        Toggle("Mono", "Both channels summed, for one-earbud listening", p.mono) { on -> vm.update { it.copy(mono = on) } }
        Toggle("Limiter", "Catches what a boost or a positive ReplayGain would clip. Adds 5 ms of delay; below the ceiling the audio passes through untouched.", p.limiter) { on -> vm.update { it.copy(limiter = on) } }
        if (p.limiter) {
            Row(Modifier.padding(horizontal = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
                Text(dev.nori.music.ffi.eqCeiling(p.limiterThresholdDb), Modifier.weight(1f), style = MaterialTheme.typography.bodySmall)
                // Proof that it is working: what it is pulling back, right now. Polled only while this
                // screen is on top, so it costs nothing the rest of the time.
                val reduction = limiterMeter()
                Text(
                    dev.nori.music.ffi.eqReduction(reduction),
                    style = MaterialTheme.typography.labelMedium,
                    color = if (reduction > 0.05f) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
            NoriSlider(p.limiterThresholdDb, -12f..0f, { v -> vm.update { it.copy(limiterThresholdDb = v) } }, Modifier.padding(horizontal = Space.gutter))
        }

        DevicesSection(vm)

        SectionTitle("Profiles")
        val profiles by vm.profiles.collectAsStateWithLifecycle()
        var naming by remember { mutableStateOf(false) }
        var newName by remember { mutableStateOf("") }
        if (naming) AlertDialog(
            onDismissRequest = { naming = false }, title = { Text("Save these settings") },
            text = { OutlinedTextField(newName, { newName = it }, singleLine = true, label = { Text("Name") }) },
            confirmButton = { TextButton({ vm.saveProfile(newName); newName = ""; naming = false }, enabled = newName.isNotBlank()) { Text("Save") } },
            dismissButton = { TextButton({ naming = false }) { Text("Cancel") } },
        )
        val rows by vm.deviceRows.collectAsStateWithLifecycle()
        AnimatedRows(profiles, { it.name }) { profile ->
            val used = rows.filter { it.output in profile.outputs }.joinToString(", ") { it.name }
            NavRow(
                profile.name, { vm.applyProfile(profile) },
                subtitle = if (used.isEmpty()) "Tap to load" else "Used for $used",
                action = { IconButton({ vm.deleteProfile(profile.name) }) { Icon(Icons.Outlined.Delete, "Delete ${profile.name}", tint = MaterialTheme.colorScheme.onSurfaceVariant) } },
            )
        }
        ActionRow("Save current settings as a profile", Icons.Filled.Add, { naming = true }, divider = false)

        SectionTitle("Crossfeed")
        Text(dev.nori.music.ffi.eqCrossfeed(p.crossfeedDb), Modifier.padding(horizontal = Space.gutter), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        NoriSlider(p.crossfeedDb, 0f..9f, { v -> vm.update { it.copy(crossfeedDb = dev.nori.music.ffi.eqCrossfeedSnap(v)) } }, Modifier.padding(horizontal = Space.gutter))
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
        onDismissRequest = onDone, title = { Text("${dev.nori.music.ffi.eqHz(band.freq)} Hz") },
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
                NoriSlider(dev.nori.music.ffi.eqFreqToSlider(band.freq), 0f..1f, { x -> onChange(band.copy(freq = dev.nori.music.ffi.eqSliderToFreq(x))) })
                Text(dev.nori.music.ffi.eqShape(band.kind == BandKind.LOW_SHELF_SLOPE || band.kind == BandKind.HIGH_SHELF_SLOPE, band.q), style = MaterialTheme.typography.labelMedium)
                NoriSlider(band.q, 0.2f..8f, { q -> onChange(band.copy(q = q)) })
            }
        },
        confirmButton = { TextButton(onDone) { Text("Done") } },
        dismissButton = { TextButton(onRemove) { Icon(Icons.Filled.Close, null); Text("Remove band") } },
    )
}
