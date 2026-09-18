package dev.flint.music.app.ui

import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material3.Button
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.LinearProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.flint.music.app.vm.SettingsViewModel

/**
 * The AutoEQ headphone database. The 850 kB index is downloaded once on request; after that searching
 * 8000+ measurements is a local query, and only the chosen preset is fetched.
 */
@Composable
fun AutoEqScreen(vm: SettingsViewModel) {
    val ui by vm.autoEq.collectAsStateWithLifecycle()
    val prefs by vm.prefs.collectAsStateWithLifecycle()
    val nav = LocalNav.current
    Column {
        Row(verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Headphone presets", style = MaterialTheme.typography.headlineSmall)
        }
        if (ui.busy) LinearProgressIndicator(Modifier.fillMaxWidth())
        ui.error?.let { Text(it, Modifier.padding(16.dp), color = MaterialTheme.colorScheme.error) }
        ui.applied?.let { Text("Applied $it. The equalizer screen now holds that curve.", Modifier.padding(16.dp), color = MaterialTheme.colorScheme.primary) }

        if (ui.count == 0) {
            Text(
                "AutoEQ measures headphones and publishes a correction curve for each. Downloading the list is one 850 kB request to github.com; after that, searching happens on this device." +
                    if (prefs.thirdPartyLookups) "" else "\n\nIt needs \"Third-party lookups\" in Settings → Features.",
                Modifier.padding(16.dp), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
            PillButton("Download the list", null, vm::downloadAutoEqIndex, Modifier.padding(horizontal = Space.gutter), prominent = true, enabled = !ui.busy)
            return@Column
        }

        Row(Modifier.padding(horizontal = 8.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("${ui.count} headphones", Modifier.weight(1f).padding(start = 8.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
            TextButton(vm::downloadAutoEqIndex, enabled = !ui.busy) { Text("Refresh list") }
        }
        SearchField(ui.query, vm::searchAutoEq, "Search ${ui.count} headphones", Modifier.padding(horizontal = Space.gutter, vertical = 8.dp))
        LazyColumn(contentPadding = PaddingValues(bottom = LocalChromeInset.current)) {
            items(ui.hits, key = { it.path }) { e ->
                Column(Modifier.fillMaxWidth().clickable { vm.applyAutoEq(e) }.padding(horizontal = Space.gutter, vertical = 11.dp)) {
                    Text(e.name)
                    Text(listOfNotNull(e.source.ifEmpty { null }, e.form.ifEmpty { null }, e.target.ifEmpty { null }).joinToString(" · "), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            if (ui.query.length >= 2 && ui.hits.isEmpty()) item { Text("Nothing matches", Modifier.padding(16.dp), color = MaterialTheme.colorScheme.onSurfaceVariant) }
            item { Text("Curves by the AutoEQ project (jaakkopasanen/AutoEq). Tapping one replaces the equalizer's bands.", Modifier.padding(16.dp), style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant) }
        }
    }
}
