package dev.nori.music.app.perf

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
import dev.nori.music.app.ui.say

/**
 * The Performance page: the states added up, the frames, the stretches themselves, and the buttons.
 * Opening it reads the counters once, so the stretch under way is on it too; nothing on it updates by itself.
 * What each row says is the core's (perf_log.rs `perf_page`, `words_perf`).
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

        SectionHeader(w.byState)
        if (page.totals.isEmpty()) Note(w.nothingRecorded)
        page.totals.forEach { Figures(it.title, it.detail) }

        SectionHeader(w.frames)
        page.frames?.let { Figures(it.title, it.detail) } ?: Note(w.noFrames)

        SectionHeader(w.stretches)
        page.live?.let { Figures(it.title, it.detail) }
        page.stretches.forEach { Figures(it.title, it.detail) }

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
