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

        /** The lyrics of song [songId] went up on the player's lyrics panel: for the invariant watch. */
        fun lyricsShown(songId: String) {}

        /**
         * A cover at [url] did not load, with [status] (CoverPixels' codes); [again]: it is asked again in a
         * moment. For the log: a sleeve left on its placeholder says why.
         */
        fun coverFailed(url: String, status: Int, again: Boolean) {}
    }

    @Volatile var recorder: Recorder? = null
}
