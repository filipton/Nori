package dev.nori.music.app.ui

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
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
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
import dev.nori.music.app.vm.SettingsViewModel
import dev.nori.music.settings.ServerProfile
import dev.nori.music.settings.profile
import dev.nori.music.settings.stored

@Composable
private fun Check(title: String, detail: String, value: Boolean, onChange: (Boolean) -> Unit) {
    Row(Modifier.fillMaxWidth().clickable { onChange(!value) }, verticalAlignment = Alignment.CenterVertically) {
        Column(Modifier.weight(1f).padding(end = 12.dp)) {
            Text(title)
            Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        NoriSwitch(value, onChange)
    }
}

/** First run, and "add / edit server" from settings ([initial] and [onClose] set). */
@Composable
fun LoginScreen(vm: SettingsViewModel, initial: ServerProfile? = null, onClose: (() -> Unit)? = null) {
    val ui by vm.login.collectAsStateWithLifecycle()
    var p by remember { mutableStateOf(initial ?: vm.newProfile()) }
    var advanced by remember { mutableStateOf(initial != null) }
    var headers by remember { mutableStateOf(dev.nori.music.ffi.settings.serverHeadersText(p.headers)) }
    var certPassword by remember { mutableStateOf("") }
    val pickCert = rememberLauncherForActivityResult(ActivityResultContracts.OpenDocument()) { uri -> if (uri != null) p = vm.importClientCert(p, uri, certPassword) }
    LaunchedEffect(ui.done) { if (ui.done) { vm.clearLoginResult(); onClose?.invoke() } }

    // Reading the form - the headers typed, the addresses trimmed, whether it can be sent, the schemes
    // offered for an address typed without one - is the core's (settings.rs).
    val ready = remember(p.url, p.user, p.apiKey) { dev.nori.music.ffi.settings.serverReady(p.stored()) }
    val schemes = remember(p.url) { dev.nori.music.ffi.settings.serverUrlSchemes(p.url) }

    Surface(Modifier.fillMaxSize()) {
        Column(Modifier.systemBarsPadding().imePadding().verticalScroll(rememberScrollState()).padding(24.dp), Arrangement.spacedBy(12.dp)) {
            Text(
                if (initial == null) "nori" else say.server,
                style = MaterialTheme.typography.displaySmall, modifier = Modifier.padding(top = 16.dp, bottom = 2.dp),
            )
            if (initial == null) Text(say.serverHint, color = MaterialTheme.colorScheme.onSurfaceVariant)
            FormField(p.url, { p = p.copy(url = it) }, label = { Text(say.serverUrl) }, singleLine = true, keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Uri), modifier = Modifier.fillMaxWidth().testTag("url"))
            if (schemes.isNotEmpty()) Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                schemes.forEach { s -> Chip(s, false) { p = p.copy(url = s + p.url) } }
            }
            if (p.apiKey.isEmpty()) {
                FormField(p.user, { p = p.copy(user = it) }, label = { Text(say.user) }, singleLine = true, modifier = Modifier.fillMaxWidth().testTag("user"))
                FormField(p.password, { p = p.copy(password = it) }, label = { Text(say.password) }, singleLine = true, visualTransformation = PasswordVisualTransformation(), keyboardOptions = KeyboardOptions(keyboardType = KeyboardType.Password), modifier = Modifier.fillMaxWidth().testTag("password"))
            }
            ui.error?.let { Text(it, color = MaterialTheme.colorScheme.error) }
            PillButton(
                if (ui.busy) say.connecting else say.connect, null,
                { vm.login(dev.nori.music.ffi.settings.serverFromForm(p.stored(), headers).profile()) },
                Modifier.fillMaxWidth().padding(top = 4.dp), prominent = true,
                enabled = !ui.busy && ready,
            )
            Row {
                TextButton({ advanced = !advanced }) { Text(if (advanced) say.hideAdvanced else say.advanced) }
                onClose?.let { TextButton(it) { Text(say.cancel) } }
            }
            if (advanced) {
                FormField(p.name, { p = p.copy(name = it) }, label = { Text(say.nameOptional) }, singleLine = true, modifier = Modifier.fillMaxWidth())
                FormField(p.altUrl, { p = p.copy(altUrl = it) }, label = { Text(say.secondAddress) }, supportingText = { Text(say.secondAddressHint) }, singleLine = true, modifier = Modifier.fillMaxWidth())
                FormField(p.apiKey, { p = p.copy(apiKey = it) }, label = { Text(say.apiKey) }, singleLine = true, modifier = Modifier.fillMaxWidth())
                FormField(headers, { headers = it }, label = { Text(say.extraHeaders) }, supportingText = { Text(say.extraHeadersHint) }, minLines = 2, modifier = Modifier.fillMaxWidth())
                Check(say.legacyAuth, say.legacyAuthHint, p.legacyAuth) { p = p.copy(legacyAuth = it) }
                Check(say.selfSigned, say.selfSignedHint, p.allowSelfSigned) { p = p.copy(allowSelfSigned = it) }
                Check(say.wifiOnly, say.wifiOnlyHint, p.wifiOnly) { p = p.copy(wifiOnly = it) }
                FormField(certPassword, { certPassword = it }, label = { Text(say.clientCertPassword) }, singleLine = true, visualTransformation = PasswordVisualTransformation(), modifier = Modifier.fillMaxWidth())
                Row(verticalAlignment = Alignment.CenterVertically) {
                    TextButton({ pickCert.launch(arrayOf("application/x-pkcs12", "application/octet-stream", "*/*")) }) { Text(if (p.clientCert.isEmpty()) say.importClientCert else say.replaceClientCert) }
                    if (p.clientCert.isNotEmpty()) TextButton({ p = p.copy(clientCert = "", clientCertPassword = "") }) { Text(say.remove) }
                }
            }
        }
    }
}
