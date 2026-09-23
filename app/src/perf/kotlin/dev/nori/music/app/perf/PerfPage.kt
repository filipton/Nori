package dev.nori.music.app.perf

import android.content.Intent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.fadeIn
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.Image
import androidx.compose.material.icons.outlined.Share
import androidx.compose.material.icons.outlined.Speed
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.dp
import dev.nori.music.app.ui.Hairline
import dev.nori.music.app.ui.LocalChromeInset
import dev.nori.music.app.ui.LocalNav
import dev.nori.music.app.ui.PillButton
import dev.nori.music.app.ui.SectionHeader
import dev.nori.music.app.ui.Space

/**
 * The Performance page: the states added up, the frames, the stretches themselves, and the buttons.
 * Opening it reads the counters once, so the stretch under way is on it too; nothing on it updates by itself.
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
            IconButton(nav::back) { Icon(Icons.AutoMirrored.Filled.ArrowBack, "Back") }
            Text("Performance", Modifier.weight(1f), style = MaterialTheme.typography.headlineSmall)
        }
        // The first reading takes a few milliseconds on the recorder's thread; the figures fade in once it is there.
        AnimatedVisibility(read != null, enter = fadeIn()) { read?.let { Recorded(it, calls, covers, recorder) } }
        Spacer(Modifier.height(Space.section + LocalChromeInset.current))
    }
}

@Composable
private fun Recorded(shown: Shown, calls: String, covers: String, recorder: Recorder) {
    val context = LocalContext.current
    val all = shown.kept + listOfNotNull(shown.live)
    val totals = Totals.of(all)
    Column {
        FlowRow(
            Modifier.padding(horizontal = Space.gutter, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            PillButton("Share report", Icons.Outlined.Share, {
                val text = report(shown, calls, covers)
                context.startActivity(Intent.createChooser(Intent(Intent.ACTION_SEND).setType("text/plain").putExtra(Intent.EXTRA_TEXT, text), null))
            }, prominent = true)
            PillButton("Start fresh", Icons.Outlined.Delete, recorder::clear)
        }

        SectionHeader("By state")
        if (totals.isEmpty()) Note("Nothing recorded yet. A stretch is kept when the app moves into another state: the screen goes off, music stops, the player opens.")
        totals.forEach { t -> Figures(stateName(t.state) + if (t.count > 1) " (${t.count})" else "", t.line()) }

        SectionHeader("Frames")
        val drawn = all.sumOf { it.frames }
        val janky = all.sumOf { it.janky }
        if (drawn == 0L) Note("No frames counted yet: they are counted while the app is on screen.")
        else Figures(f("%d frames, %d janky (%.1f %%)", drawn, janky, janky * 100.0 / drawn), f("The slowest took %.0f ms", all.maxOf { it.worstMs }))

        SectionHeader("Stretches")
        shown.live?.let { Figures("Now: " + stateName(it.state), it.line()) }
        shown.kept.asReversed().take(40).forEach { s -> Figures(stateName(s.state), s.line()) }

        SectionHeader("Benchmarks")
        FlowRow(
            Modifier.padding(horizontal = Space.gutter, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(8.dp), verticalArrangement = Arrangement.spacedBy(8.dp),
        ) {
            PillButton("Run call benchmark", Icons.Outlined.Speed, recorder::runCalls)
            PillButton("Run cover benchmark", Icons.Outlined.Image, recorder::runCovers)
        }
        if (calls.isNotEmpty()) Figures("Calls", calls)
        if (covers.isNotEmpty()) Figures("Covers", covers)
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

@Composable
private fun Note(text: String) = Text(
    text, Modifier.padding(horizontal = Space.gutter, vertical = 8.dp),
    style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant,
)
