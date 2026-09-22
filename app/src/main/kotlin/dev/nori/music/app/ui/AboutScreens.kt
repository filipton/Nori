package dev.nori.music.app.ui

import android.os.Build
import androidx.compose.foundation.clickable
import androidx.compose.foundation.verticalScroll
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ColumnScope
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.remember
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.unit.dp
import dev.nori.music.app.BuildConfig

/** A row that says something: a title, a line under it, and optionally something at its end or a tap. */
@Composable
internal fun InfoRow(title: String, detail: String, end: String? = null, onClick: (() -> Unit)? = null) {
    Row(
        Modifier.fillMaxWidth().then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier)
            .padding(horizontal = 16.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f).padding(end = if (end != null) 12.dp else 0.dp)) {
            Text(title, style = MaterialTheme.typography.bodyLarge)
            if (detail.isNotEmpty()) Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        if (end != null) Text(end, style = MaterialTheme.typography.labelMedium, color = if (onClick != null) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
    }
    Hairline(startIndent = 16.dp)
}

/**
 * What this is and what it is built out of. Most of a bug report is answered here - which build, which
 * commit, which phone, which audio engine at which version - and a tap on the top row copies all of it.
 */
@Composable
internal fun AboutContent(section: @Composable (String, @Composable ColumnScope.() -> Unit) -> Unit, openLicences: () -> Unit) {
    val clipboard = LocalClipboardManager.current
    val facts = remember { BuildFacts.collect() }
    section("Nori") {
        InfoRow("Nori ${BuildConfig.VERSION_NAME}", facts.build, end = "Copy") { clipboard.setText(AnnotatedString(facts.report())) }
    }
    section("Under the hood") {
        InfoRow("Playback", facts.playback)
        InfoRow("Library and search", facts.library)
        InfoRow("AutoMix", facts.automix)
        InfoRow("Interface", facts.ui)
    }
    section("Open source") {
        InfoRow("Nori is free software", "MIT licence  ·  Copyright (c) 2026 filipton", end = "MIT")
        InfoRow("Licences", "The libraries, fonts and data this app is made of, and their terms", onClick = openLicences)
    }
}

/** The facts About shows, gathered once; none of them change while the app runs. */
private class BuildFacts(val build: String, val playback: String, val library: String, val automix: String, val ui: String) {
    fun report(): String = listOf(
        "Nori ${BuildConfig.VERSION_NAME} ($build)",
        "Playback: $playback",
        "Library: $library",
        "AutoMix: $automix",
        "Interface: $ui",
    ).joinToString("\n")

    companion object {
        fun collect(): BuildFacts {
            val versions = BuildConfig.CORE_VERSIONS.split(';')
                .mapNotNull { it.split('=').takeIf { p -> p.size == 2 && p[1].isNotBlank() }?.let { p -> p[0] to p[1] } }
                .toMap()
            fun v(name: String) = versions[name]?.let { " $it" }.orEmpty()
            val kind = if (BuildConfig.DEBUG) "debug" else "release"
            val sha = BuildConfig.GIT_SHA.ifBlank { "no commit" }
            val abi = Build.SUPPORTED_ABIS.firstOrNull() ?: "unknown ABI"
            return BuildFacts(
                build = "$kind $sha  ·  $abi  ·  Android ${Build.VERSION.RELEASE} (API ${Build.VERSION.SDK_INT})",
                playback = "Media3 ExoPlayer${v("media3")}  ·  OkHttp${v("okhttp")}  ·  equalizer, crossfeed and limiter in the Rust core",
                library = "SQLite with full-text search through rusqlite${v("rusqlite")}  ·  Rust core reached through uniffi${v("uniffi")}",
                automix = "Beat and key analysis on this phone with RustFFT${v("rustfft")}  ·  tempo changes by Signalsmith Stretch${v("signalsmith-stretch")}",
                ui = "Jetpack Compose and Material 3 (BOM${v("composeBom")})  ·  covers through Coil${v("coil")}",
            )
        }
    }
}

/**
 * One thing worth crediting: what it is, whose it is, under what terms, and which bundled text those
 * terms are (`assets/licences/<file>.txt`; null for a service with no licence to reproduce).
 */
private data class Credit(val name: String, val what: String, val copyright: String, val license: String, val file: String?)

/**
 * Everything the app is built from that is not ours. A static list, because the set changes with the
 * code and not with the user: a line here, and in NOTICE, is added in the same commit that adds the
 * dependency. Where a library offers MIT or Apache-2.0, the MIT text is the one shown.
 */
private val CREDITS = listOf(
    "Rust core" to listOf(
        Credit("uniffi", "Generates the Kotlin bindings to the core", "Mozilla Foundation", "MPL-2.0", "MPL-2.0"),
        Credit("rusqlite", "The library index, full-text search and caches", "Copyright (c) 2014 The rusqlite developers", "MIT", "MIT"),
        Credit("SQLite", "The database itself, bundled into the core", "D. Richard Hipp and the SQLite developers, dedicated to the public domain", "Public domain", null),
        Credit("RustFFT", "The spectrum analysis behind tempo, beats and key", "Copyright (c) 2015 The RustFFT Developers", "MIT or Apache-2.0", "MIT"),
        Credit("Signalsmith Stretch", "Time-stretching for beat-matched mixes", "Copyright (c) 2022 Geraint Luff / Signalsmith Audio Ltd.; Rust binding Copyright 2024 Colin Marc", "MIT", "MIT"),
        Credit("serde and serde_json", "Reading the server's answers", "Copyright (c) David Tolnay and the Serde developers", "MIT or Apache-2.0", "MIT"),
        Credit("jni", "The core's direct calls from the audio path", "Copyright (c) 2016 Prevoty, Inc. and jni-rs contributors", "MIT or Apache-2.0", "MIT"),
        Credit("md-5", "Signing requests the way the Subsonic API asks", "Copyright (c) RustCrypto Developers", "MIT or Apache-2.0", "MIT"),
        Credit("parking_lot", "Locks inside the core", "Copyright (c) 2016 The Rust Project Developers (Amanieu d'Antras)", "MIT or Apache-2.0", "MIT"),
        Credit("thiserror", "Errors inside the core", "Copyright (c) David Tolnay", "MIT or Apache-2.0", "MIT"),
        Credit("Media3 Sonic and silence skipping, ported", "Speed, pitch and shortened silences, ported line for line into the core", "Copyright The Android Open Source Project", "Apache-2.0", "Apache-2.0"),
        Credit("AndroidX Palette, ported", "The colour quantiser behind a page's accent, ported line for line into the core", "Copyright The Android Open Source Project", "Apache-2.0", "Apache-2.0"),
    ),
    "Android" to listOf(
        Credit("AndroidX Media3", "Playback, the media session and the notification", "Copyright The Android Open Source Project", "Apache-2.0", "Apache-2.0"),
        Credit("Jetpack Compose and Material 3", "The user interface toolkit", "Copyright The Android Open Source Project", "Apache-2.0", "Apache-2.0"),
        Credit("Material Icons", "The icons", "Copyright Google LLC", "Apache-2.0", "Apache-2.0"),
        Credit("AndroidX Navigation, Lifecycle, Activity, Core", "The app's plumbing", "Copyright The Android Open Source Project", "Apache-2.0", "Apache-2.0"),
        Credit("OkHttp", "Every network request", "Copyright Square, Inc.", "Apache-2.0", "Apache-2.0"),
        Credit("Coil", "Loading and caching covers", "Copyright Coil Contributors", "Apache-2.0", "Apache-2.0"),
        Credit("kotlinx.coroutines", "The concurrency the app is written in", "Copyright JetBrains s.r.o. and Kotlin Programming Language contributors", "Apache-2.0", "Apache-2.0"),
        Credit("JNA", "How Kotlin reaches the Rust core, taken under the Apache half of its dual licence", "Copyright (c) 2007 Timothy Wall and the JNA contributors", "Apache-2.0", "Apache-2.0"),
    ),
    "Fonts and data" to listOf(
        Credit("Inter", "The typeface", "Copyright (c) 2016 The Inter Project Authors (Rasmus Andersson)", "OFL-1.1", "OFL-1.1"),
        Credit("AutoEQ", "Headphone correction curves, fetched when you ask for them", "Copyright (c) 2018 Jaakko Pasanen", "MIT", "MIT"),
        Credit("LRCLIB", "Synced lyrics for songs your server has none for, asked only when switched on", "lrclib.net; lyrics belong to their authors and contributors", "Service", null),
    ),
)

/**
 * The licences page: every credit, grouped, with its licence at the end of the row. A tap shows who it
 * belongs to and the licence's full text, as the licences themselves ask to be shipped with the app.
 */
@Composable
internal fun LicencesContent(section: @Composable (String, @Composable ColumnScope.() -> Unit) -> Unit) {
    var open by androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf<Credit?>(null) }
    CREDITS.forEach { (heading, credits) ->
        section(heading) {
            credits.forEach { c -> InfoRow(c.name, c.what, end = c.license) { open = c } }
        }
    }
    open?.let { c -> LicenceText(c) { open = null } }
}

@Composable
private fun LicenceText(c: Credit, dismiss: () -> Unit) {
    val context = androidx.compose.ui.platform.LocalContext.current
    val text = androidx.compose.runtime.remember(c) {
        c.file?.let { f -> runCatching { context.assets.open("licences/$f.txt").bufferedReader().use { it.readText() }.trim() }.getOrNull() }
    }
    androidx.compose.material3.AlertDialog(
        onDismissRequest = dismiss,
        confirmButton = { androidx.compose.material3.TextButton(dismiss) { Text("Close") } },
        title = { Text(c.name) },
        text = {
            Column(Modifier.verticalScroll(androidx.compose.foundation.rememberScrollState())) {
                Text(c.copyright, style = MaterialTheme.typography.bodyMedium)
                Text(c.license, Modifier.padding(top = 4.dp, bottom = 12.dp), style = MaterialTheme.typography.labelMedium, color = MaterialTheme.colorScheme.primary)
                Text(
                    text ?: if (c.file == null) "No licence text to reproduce." else "The licence text could not be read.",
                    style = MaterialTheme.typography.bodySmall.copy(fontFamily = androidx.compose.ui.text.font.FontFamily.Monospace),
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        },
    )
}
