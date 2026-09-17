package dev.flint.music.app.ui

import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.systemBarsPadding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AssistChip
import androidx.compose.material3.Button
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.KeyboardType
import androidx.compose.ui.text.input.PasswordVisualTransformation
import androidx.compose.ui.unit.dp
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.flint.music.app.vm.SettingsViewModel
import dev.flint.music.settings.ServerProfile

@Composable
private fun Check(title: String, detail: String, value: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().clickable { onChange(!value) }, verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        Switch(value, onChange)
    }
}

/** First run, and "add / edit server" from settings ([initial] and [onClose] set). */
@Composable
fun LoginScreen(vm: SettingsViewModel, initial: ServerProfile? = null, onClose: (() -> Unit)? = null) {
    val ui by vm.login.collectAsStateWithLifecycle()
    var p by remember { mutableStateOf(initial ?: vm.newProfile()) }
    var advanced by remember { mutableStateOf(initial != null) }
    var headers by remember { mutableStateOf(p.headers.entries.joinToString("\n") { "${it.key}: ${it.value}" }) }
    var certPassword by remember { mutableStateOf("") }
    val pickCert = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri -> if (uri != null) p = vm.importClientCert(p, uri, certPassword) }
    LaunchedEffect(ui.done) { if (ui.done) { vm.clearLoginResult(); onClose?.invoke() } }

    fun parsedHeaders() = headers.lines().mapNotNull { l -> l.split(':', limit = 2).takeIf { it.size == 2 && it[0].isNotBlank() }?.let { it[0].trim() to it[1].trim() } }.toMap()

    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.systemBarsPadding().imePadding().verticalScroll(rememberScrollState()).padding(24.dp), Arrangement.spacedBy(12.dp)) {
            Text(if (initial == null) "flint music" else "Server", style = MaterialTheme.typography.headlineMedium)
            if (initial == null) Text("Navidrome, octo-fiesta or any Subsonic server", color = MaterialTheme.colorScheme.onSurfaceVariant)
            OutlinedTextField(p.url, { p = p.copy(url = it) }, label = { Text("Server URL") }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri), modifier = Modifier.fillMaxWidth().testTag("url"))
            if (p.url.isNotEmpty() && "://" !in p.url) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                AssistChip({ p = p.copy(url = "https://${p.url}") }, { Text("https://") })
                AssistChip({ p = p.copy(url = "http://${p.url}") }, { Text("http://") })
            }
            if (p.apiKey.isEmpty()) {
                OutlinedTextField(p.user, { p = p.copy(user = it) }, label = { Text("User") }, singleLine = true, modifier = Modifier.fillMaxWidth().testTag("user"))
                OutlinedTextField(p.password, { p = p.copy(password = it) }, label = { Text("Password") }, singleLine = true, visualTransformation = PasswordVisualTransformation(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password), modifier = Modifier.fillMaxWidth().testTag("password"))
            }
            ui.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            Button({ vm.login(p.copy(url = p.url.trim(), altUrl = p.altUrl.trim(), headers = parsedHeaders())) }, enabled = !ui.busy && p.url.isNotBlank() && (p.user.isNotBlank() || p.apiKey.isNotBlank()), modifier = Modifier.fillMaxWidth()) { Text(if (ui.busy) "Connecting…" else "Connect") }
            Row {
                TextButton({ advanced = !advanced }) { Text(if (advanced) "Hide advanced" else "Advanced") }
                onClose?.let { TextButton(it) { Text("Cancel") } }
            }
            if (advanced) {
                OutlinedTextField(p.name, { p = p.copy(name = it) }, label = { Text("Name (optional)") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(p.altUrl, { p = p.copy(altUrl = it) }, label = { Text("Second address (e.g. public URL)") }, supportingText = { Text("Used when the first one does not answer, for example away from home") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(p.apiKey, { p = p.copy(apiKey = it) }, label = { Text("API key instead of password (OpenSubsonic)") }, singleLine = true, modifier = Modifier.fillMaxWidth())
                OutlinedTextField(headers, { headers = it }, label = { Text("Extra HTTP headers") }, supportingText = { Text("One per line, Name: value. For reverse proxies, basic auth, Cloudflare Access.") }, minLines = 2, modifier = Modifier.fillMaxWidth())
                Check("Legacy authentication", "For old servers without token auth. Detected automatically when the server says so.", p.legacyAuth) { p = p.copy(legacyAuth = it) }
                Check("Accept self-signed certificate", "Only for your own server. Certificates you installed in Android are accepted without this.", p.allowSelfSigned) { p = p.copy(allowSelfSigned = it) }
                Check("Wi-Fi only", "Never contact this server over mobile data", p.wifiOnly) { p = p.copy(wifiOnly = it) }
                OutlinedTextField(certPassword, { certPassword = it }, label = { Text("Client certificate password") }, singleLine = true, visualTransformation = PasswordVisualTransformation(), modifier = Modifier.fillMaxWidth())
                Row(verticalAlignment = Alignment.CenterVertically) {
                    TextButton({ pickCert.launch(arrayOf("application/x-pkcs12", "application/octet-stream", "*/*")) }) { Text(if (p.clientCert.isEmpty()) "Import client certificate (.p12)" else "Replace client certificate") }
                    if (p.clientCert.isNotEmpty()) TextButton({ p = p.copy(clientCert = "", clientCertPassword = "") }) { Text("Remove") }
                }
            }
        }
    }
}
