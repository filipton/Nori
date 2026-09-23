package dev.nori.music.app

import androidx.compose.runtime.Composable

/**
 * Where the perf build's recorder plugs in (src/perf, docs/perf-build.md). Only that build installs
 * one, at start; every other build has no recorder class at all, and what is left here is one null
 * field read when the app draws its first screen and a settings row that is never shown.
 */
object PerfHooks {
    interface Recorder {
        /** The player was put up or away: a stretch with the screen on is told apart by it. */
        fun playerOpen(open: Boolean)

        /** The Performance page: what was recorded, the report to share and the benchmarks. */
        @Composable fun Page()
    }

    @Volatile var recorder: Recorder? = null
}
