package dev.nori.music.app.perf

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.Info
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.Speed
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.unit.dp
import dev.nori.music.app.ui.Hairline
import dev.nori.music.app.ui.LocalChromeInset
import dev.nori.music.app.ui.LocalNav
import dev.nori.music.app.ui.PillButton
import dev.nori.music.app.ui.SectionHeader
import dev.nori.music.app.ui.Space
import dev.nori.music.app.ui.say
import dev.nori.music.ffi.perf.PerfFigures
import dev.nori.music.ffi.words.PerfWords

/**
 * The Performance page: the states added up, the frames, the stretches themselves with their events
 * folded under them, the buttons, and the app's log folded away at the end. Opening it reads the counters
 * once, so the stretch under way is on it too; nothing on it updates by itself, and the log is read only
 * when it is unfolded. What each row says is the core's (perf_log.rs `perf_page`, `perf_log_text`, `words_perf`).
 */
@Composable
internal fun PerfPage(recorder: Recorder) {
    val nav = LocalNav.current
    LaunchedEffect(Unit) { recorder.refresh() }
    val read by recorder.shown
    val calls by recorder.callBench
    val covers by recorder.coverBench
    Column(Modifier.verticalScroll(rememberScrollState()).animateContentSize()) {
        Row(Modifier.padding(start = 4.dp, end = Space.gutter), verticalAlignment = Alignment.CenterVertically) {
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, say.back) }
            Text(say.performance, Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
        }
        // The first reading takes a few milliseconds on the recorder's thread; the figures fade in once it is there.
        AnimatedVisibility(read != null, enter = fadeIn()) { read?.let { Recorded(it, calls, covers, recorder) } }
        Spacer(Modifier.height(Space.section + LocalChromeInset.current))
    }
}

@Composable
private fun Recorded(shown: Shown, calls: String, covers: String, recorder: Recorder) {
    val context = LocalContext.current
    val page = shown.page
    val w = recorder.words
    Column {
        FlowRow(
            Modifier.padding(horizontal = Space.gutter, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            PillButton(w.shareReport, Icons.Outlined.Share, { recorder.share(context, calls, covers) }, prominent = true)
            PillButton(w.startFresh, Icons.Outlined.Delete, recorder::clear)
        }

        SelfTestSection(recorder.selfTest)

        SectionHeader(w.byState)
        if (page.totals.isEmpty()) Note(w.nothingRecorded)
        page.totals.forEach { Figures(it.title, it.detail) }

        SectionHeader(w.frames)
        page.frames?.let { Figures(it.title, it.detail) } ?: Note(w.noFrames)

        SectionHeader(w.stretches)
        page.live?.let { Stretch(it, w) }
        page.stretches.forEach { key(it.title, it.detail) { Stretch(it, w) } }

        SectionHeader(w.benchmarks)
        FlowRow(
            Modifier.padding(horizontal = Space.gutter, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            PillButton(w.runCalls, Icons.Outlined.Speed, recorder::runCalls)
            PillButton(w.runCovers, Icons.Outlined.Image, recorder::runCovers)
        }
        if (calls.isNotEmpty()) Figures(w.calls, calls)
        if (covers.isNotEmpty()) Figures(w.covers, covers)

        Log(recorder)
    }
}

@Composable
private fun Figures(title: String, detail: String) {
    Column(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 8.dp)) {
        Text(title, style = MaterialTheme.typography.bodyLarge)
        Text(detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
    }
    Hairline()
}

/** A stretch's figures, and its events folded under them until asked for. */
@Composable
private fun Stretch(f: PerfFigures, w: PerfWords) {
    var open by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 8.dp)) {
        Text(f.title, style = MaterialTheme.typography.bodyLarge)
        Text(f.detail, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        if (f.events.isNotEmpty()) {
            Text(
                if (open) w.hideEvents else dev.nori.music.ffi.words.wordsPerfEvents(f.events.size.toUInt()),
                Modifier.clickable { open = !open }.padding(top = 6.dp),
                style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary,
            )
            AnimatedVisibility(open, enter = expandVertically() + fadeIn(), exit = shrinkVertically() + fadeOut()) {
                SelectionContainer {
                    Text(
                        f.events.joinToString("\n"), Modifier.padding(top = 4.dp),
                        style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace), color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
    Hairline()
}

/** The app's own log and any crash, folded away: read from logcat only when unfolded. */
@Composable
private fun Log(recorder: Recorder) {
    val w = recorder.words
    val text by recorder.log
    val open by recorder.logOpen
    SectionHeader(w.log)
    Note(w.logNote)
    Row(Modifier.padding(horizontal = Space.gutter, vertical = 6.dp)) {
        PillButton(if (open) w.hideLog else w.showLog, Icons.Outlined.Info, { if (open) recorder.hideLog() else recorder.showLog() })
    }
    AnimatedVisibility(open, enter = expandVertically() + fadeIn(), exit = shrinkVertically() + fadeOut()) {
        SelectionContainer {
            Text(
                text, Modifier.padding(horizontal = Space.gutter, vertical = 8.dp),
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace), color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
}

/**
 * The self test: its button, the switches it runs with, and while it runs where it is, each check's
 * verdict as it comes, and a way to stop; afterwards the result, which Share carries too.
 */
@Composable
private fun SelfTestSection(test: SelfTest) {
    val p = test.progress
    val running = p?.running == true
    val activity = LocalContext.current as? android.app.Activity
    SectionHeader("Self test")
    Note(
        "Plays songs of your library (those on the phone first) through both players, quietly unless you listen, and checks " +
            "playback, the controls, offload, settings applied live, lyrics and covers. Takes about six minutes; your settings, " +
            "queue, place and player are put back afterwards.",
    )
    FlowRow(
        Modifier.padding(horizontal = Space.gutter, vertical = 6.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        PillButton(if (running) "Cancel" else "Run self test", Icons.Outlined.Speed, { if (running) test.cancel() else test.start(activity) }, prominent = !running)
        dev.nori.music.app.ui.Chip("Listen", test.listen) { if (!running) test.listen = !test.listen }
        dev.nori.music.app.ui.Chip("Downloads", test.downloads) { if (!running) test.downloads = !test.downloads }
    }
    AnimatedVisibility(p != null, enter = expandVertically() + fadeIn(), exit = shrinkVertically() + fadeOut()) {
        p?.let { SelfTestProgress(it) }
    }
}

@Composable
private fun SelfTestProgress(p: SelfTest.Progress) {
    Column(Modifier.fillMaxWidth().padding(horizontal = Space.gutter, vertical = 8.dp).animateContentSize()) {
        if (p.running) {
            Text("Step ${p.step} of ${p.total}: ${p.current}", style = MaterialTheme.typography.bodyLarge)
            androidx.compose.material3.LinearProgressIndicator(
                progress = { (p.step - 1).coerceAtLeast(0) / p.total.coerceAtLeast(1).toFloat() },
                modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
            )
        }
        val failed = p.outcomes.count { it.verdict == Verdict.FAIL }
        if (!p.running) {
            Text(
                if (failed == 0) "All checks passed" else "$failed checks failed",
                style = MaterialTheme.typography.bodyLarge,
                color = if (failed == 0) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.error,
            )
        }
        SelectionContainer {
            Text(
                p.report ?: p.outcomes.joinToString("\n") { "${it.verdict.name}  ${it.section}: ${it.name}" + if (it.measured.isNotEmpty()) " - ${it.measured}" else "" },
                Modifier.padding(top = 4.dp),
                style = MaterialTheme.typography.bodySmall.copy(fontFamily = FontFamily.Monospace), color = MaterialTheme.colorScheme.onSurfaceVariant,
            )
        }
    }
    Hairline()
}

@Composable
private fun Note(text: String) = Text(
    text, Modifier.padding(horizontal = Space.gutter, vertical = 8.dp),
    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
)
