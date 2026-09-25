package dev.nori.music.app.perf

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Test

class SelfTestLogicTest {
    /** Readings every 250 ms from [from] ms on: the place moving at [speed], the output's counts as given. */
    private fun run(
        n: Int, from: Long = 0, pos: Long = 10_000, speed: Double = 1.0, index: Int = 0,
        head: (Int) -> Long? = { it * 11_025L }, stamp: (Int) -> Long? = { it * 11_025L }, written: (Int) -> Long? = { null },
        playing: Boolean = true, offloaded: Boolean = false, track: Int = 1,
    ) = List(n) { i ->
        Reading(from + i * 250L, pos + (i * 250 * speed).toLong(), index, "s$index", "s$index", playing, head(i), stamp(i), written(i), offloaded, track)
    }

    // ---- the plan ----

    @Test fun the_suite_sets_up_first_puts_everything_back_last_and_tests_both_players() {
        val p = plan(Options())
        assertEquals("prepare", p.first().id)
        assertEquals("restore", p.last().id)
        assertEquals(listOf(EXO, RUST), p.filter { it.id == "engine" }.map { it.section })
        // Offload is the Rust player's alone, after its controls; every step may take a while, never for ever.
        assertEquals(listOf(RUST, RUST), p.filter { it.id.startsWith("offload") }.map { it.section })
        assertTrue(p.indexOfFirst { it.id == "offload" } > p.indexOfLast { it.id == "rapid" })
        assertTrue(p.all { it.timeoutMs in 1_000..120_000 })
        assertFalse("downloads are asked for", p.any { it.id == "downloads" })
        assertTrue(plan(Options(downloads = true)).let { q -> q[q.size - 2].id == "downloads" })
        assertEquals(p.size, p.map { it.section to it.id }.distinct().size)
    }

    // ---- the songs ----

    @Test fun songs_are_the_library_s_own_on_the_phone_first_one_per_album_first() {
        val all = listOf(
            Candidate("ext-deezer-1", "mp3", 200, local = true),
            Candidate("x", "mp3", 200, local = false, external = true),
            Candidate("radio:1", "mp3", 200, local = true),
            Candidate("short", "mp3", 20, local = true),
            Candidate("a1", "flac", 200, local = false, album = "A"),
            Candidate("b1", "mp3", 200, local = true, album = "B"),
            Candidate("b2", "mp3", 200, local = true, album = "B"),
            Candidate("c1", "mp3", 200, local = true, album = "C"),
        )
        assertEquals(listOf("b1", "c1", "b2", "a1"), pickSongs(all, 6).map { it.id })
        assertEquals(listOf("b1", "c1"), pickSongs(all, 2).map { it.id })
        assertEquals(listOf("a1"), pickSongs(all, 3, suffix = "FLAC").map { it.id })
    }

    // ---- playback ----

    @Test fun music_that_moves_everywhere_passes() {
        val j = judgeProgress(run(41, written = { 20_000L + it * 11_025L }), written = true)
        assertTrue(j.problems.toString(), j.ok)
        assertEquals("played 10.0 s in 10.0 s, play head +441000, timestamp +441000 frames, written +441000 frames", j.measured)
    }

    @Test fun a_play_head_stuck_at_nought_is_fine_while_the_timestamp_moves_as_on_a_galaxy_s22() {
        val j = judgeProgress(run(41, head = { 0L }), written = false)
        assertTrue(j.problems.toString(), j.ok)
    }

    @Test fun silence_while_the_player_says_it_plays_fails_and_says_where() {
        // The place runs on from the clock; the output stops presenting after a second.
        val j = judgeProgress(run(41, head = { minOf(it, 4) * 11_025L }, stamp = { minOf(it, 4) * 11_025L }), written = false)
        assertFalse(j.ok)
        assertEquals(listOf("the output presented nothing new for 9.0 s"), j.problems)
        val stuck = judgeProgress(run(41, speed = 0.0), written = false)
        assertTrue(stuck.problems.toString(), stuck.problems.any { it.startsWith("the place moved 0.0 s") })
        assertTrue(stuck.problems.any { it == "the place stood still for 10.0 s" })
    }

    @Test fun a_rust_output_whose_writes_stop_fails() {
        val j = judgeProgress(run(41, written = { 500_000L }), written = true)
        assertEquals(listOf("nothing was written to the output in 10.0 s"), j.problems)
        assertTrue("a short look may fall between two bursts", judgeProgress(run(13, written = { 500_000L }), written = true).ok)
    }

    @Test fun paused_means_still() {
        assertTrue(judgePaused(run(9, speed = 0.0, head = { 44_100L }, stamp = { 44_100L }, playing = false)).ok)
        val moving = judgePaused(run(9, playing = false))
        assertFalse(moving.ok)
        assertTrue(moving.problems.toString(), moving.problems.any { it.startsWith("the place moved") })
    }

    @Test fun a_seek_lands_near_where_it_was_asked_and_plays_on() {
        val r = run(4, pos = 2_000) + run(8, from = 1_000, pos = 30_100)
        val j = judgeSeek(30_000, r)
        assertTrue(j.problems.toString(), j.ok)
        assertEquals("at 30.1 s 1000 ms after asking for 30.0 s", j.measured)
        assertFalse(judgeSeek(30_000, run(12, pos = 2_000)).ok)
    }

    @Test fun three_presses_that_moved_four_songs_fail() {
        val end = Reading(0, 1_000, 4, "s4", "s4", true)
        val j = judgeMove(1, 3, end, "s4", listOf(2, 3, 4))
        assertTrue(j.problems.toString(), j.ok)
        val over = judgeMove(1, 3, end.copy(index = 5, shownId = "s5", heardId = "s5"), "s4", listOf(2, 3, 4, 5))
        assertEquals(
            listOf(
                "ended on queue place 5, expected 4 (4 songs for 3 presses)",
                "the screen shows s5, expected s4",
                "the player service is on s5, expected s4",
                "went past it on the way: arrived at [2, 3, 4, 5]",
            ),
            over.problems,
        )
        val split = judgeMove(1, 3, end.copy(shownId = "s3"), "s4", listOf(4), engineIndex = 5)
        assertEquals(listOf("the screen shows s3, expected s4", "the engine plays queue place 5, expected 4"), split.problems)
    }

    // ---- offload ----

    @Test fun offload_that_goes_silent_after_a_second_fails_as_the_s22_did() {
        val r = run(81, offloaded = true, head = { 0L }, stamp = { minOf(it, 4) * 11_025L })
        val j = judgeOffload(r, jumps = emptyList())
        assertFalse(j.ok)
        assertTrue(j.problems.toString(), j.problems.any { it.startsWith("the chip presented nothing for 19.0 s") })
    }

    @Test fun offload_across_a_skip_and_a_seek_passes() {
        val a = run(32, offloaded = true, index = 0)
        val b = run(24, from = 8_000, pos = 0, index = 1, offloaded = true, track = 1, head = { 400_000L + it * 11_025L }, stamp = { 400_000L + it * 11_025L })
        // A seek: the place jumps, the chip's count starts again, and it takes a moment to move.
        val c = run(24, from = 14_000, pos = 60_000, index = 1, offloaded = true, track = 1, head = { if (it < 3) 0L else it * 11_025L }, stamp = { if (it < 3) 0L else it * 11_025L })
        val j = judgeOffload(a + b + c, jumps = listOf(8_000, 14_000))
        assertTrue(j.problems.toString(), j.ok)
        val left = judgeOffload(run(20, offloaded = true) + run(20, from = 5_000, pos = 15_000, offloaded = false), jumps = emptyList())
        assertTrue(left.problems.toString(), left.problems.first().startsWith("left offload at 20 of 40 readings (first at 5.0 s)"))
    }

    // ---- settings ----

    @Test fun automix_needs_a_planned_transition_with_a_length_songs_measured_and_a_mix_heard() {
        val log = listOf(
            "09-25 21:05:10.000  1234  1300 I nori    : planFor: gapless (no analysis of the next song)",
            "09-25 21:05:40.000  1234  1300 I nori    : transition Song A -> Song B: BEAT_MIX 8200 ms at 181000, tempo x1.012 (beats match)",
        )
        assertEquals(listOf(Transition("Song A", "Song B", "BEAT_MIX", 8200, log[1].trim())), parseTransitions(log))
        val ok = judgeAutoMix("Song A", log, mapOf("a" to true, "b" to true), mixed = true)
        assertTrue(ok.problems.toString(), ok.ok)
        assertEquals("BEAT_MIX 8200 ms into \"Song B\"; measured ahead: 2 of 2; mix heard: true", ok.measured)
        val none = judgeAutoMix("Song A", log.take(1), mapOf("a" to true, "b" to false), mixed = false)
        assertEquals(
            listOf("no transition out of \"Song A\": gapless (no analysis of the next song)", "not measured ahead: b", "no mix was heard at the boundary"),
            none.problems,
        )
        assertFalse(judgeAutoMix("Song A", listOf("transition Song A -> Song B: CROSSFADE 0 ms at 1"), mapOf(), true).ok)
    }

    @Test fun the_crossfade_heard_is_the_length_just_set() {
        val log = listOf("transition X -> Y: CROSSFADE 6000 ms at 1000, tempo x1.000 (crossfade)")
        assertTrue(judgeCrossfade(6, "X", log, mixed = true).ok)
        assertEquals(listOf("planned 6000 ms, not the 3000 ms set: ${log[0]}"), judgeCrossfade(3, "X", log, mixed = true).problems)
        assertEquals(listOf("no mix was heard at the boundary"), judgeCrossfade(6, "X", emptyList(), mixed = false).problems)
    }

    // ---- lyrics ----

    @Test fun lyrics_belong_to_the_song_stay_put_and_are_not_fetched_again() {
        val good = listOf(
            LyricsSeen(0, "a", "a", loading = true, rank = 0),
            LyricsSeen(500, "a", "a", loading = false, rank = 2, key = "A"),
            LyricsSeen(5_000, "b", "b", loading = true, rank = 0),
            LyricsSeen(5_600, "b", "b", loading = false, rank = 1, key = "B1"),
            LyricsSeen(6_000, "b", "b", loading = false, rank = 3, key = "B2"),
            LyricsSeen(10_000, "a", "a", loading = false, rank = 2, key = "A"),
        )
        val j = judgeLyrics(good, returnedTo = "a", returnedAtMs = 9_000)
        assertTrue(j.problems.toString(), j.ok)
        assertEquals("2 songs, best answers by line, by word", j.measured)
        val bad = listOf(
            LyricsSeen(500, "a", "a", loading = false, rank = 2, key = "A"),
            LyricsSeen(700, "a", "b", loading = false, rank = 2, key = "A"),
            LyricsSeen(900, "a", "a", loading = false, rank = 1, key = "A2"),
            LyricsSeen(10_000, "a", "a", loading = true, rank = 0),
            LyricsSeen(10_500, "a", "a", loading = false, rank = 2, key = "A3"),
        )
        assertEquals(
            listOf(
                "lyrics of a were handed over while b played",
                "the words of a changed after they were shown, for no better ones (rank 2 to 1)",
                "coming back to a showed the loader again",
                "coming back to a showed other lyrics than before",
            ),
            judgeLyrics(bad, returnedTo = "a", returnedAtMs = 9_000).problems,
        )
        val none = listOf(
            LyricsSeen(500, "c", "c", loading = false, rank = 0, key = "none"),
            LyricsSeen(10_000, "c", "c", loading = true, rank = 0),
            LyricsSeen(10_500, "c", "c", loading = false, rank = 0, key = "none"),
        )
        assertTrue("a song without words may be asked again", judgeLyrics(none, returnedTo = "c", returnedAtMs = 9_000).ok)
    }

    // ---- putting things back ----

    private val user = Knobs(
        engine = 0, eq = true, crossfeedDb = -3f, balance = 0f, mono = false, limiter = true, speed = 1f, pitch = 1f,
        skipSilence = false, offload = true, crossfadeSec = 4, autoMix = true, replayGain = 3, scrobble = true,
        autoFill = true, skipExplicit = false, previousAlwaysSkips = true, fadeMs = 200,
    )

    @Test fun the_test_starts_from_a_plain_path_and_puts_every_setting_back() {
        val t = testKnobs(user, engine = 1)
        assertEquals(
            listOf("playbackEngine", "eqEnabled", "crossfeedDb", "limiter", "offload", "crossfadeSec", "autoMix", "scrobble", "autoFill", "previousAlwaysSkips", "fadeMs"),
            knobsDiffer(user, t),
        )
        assertEquals(3, t.replayGain)
        assertEquals(emptyList<String>(), knobsDiffer(user, user.copy()))
    }

    @Test fun the_queue_goes_back_as_the_app_starts_it_and_plays_only_if_it_played() {
        val before = Snapshot(user, listOf("a", "b", "c"), 1, 42_000, playing = false, shuffle = false, repeat = 0, serviceRunning = true)
        val now = before.copy(knobs = testKnobs(user, 1), ids = listOf("x", "y"), index = 0, positionMs = 3_000, playing = true, repeat = 0)
        assertEquals(listOf(RestoreStep.STOP_PLAYER, RestoreStep.SETTINGS, RestoreStep.START_PLAYER, RestoreStep.QUEUE), restoreSteps(before, now))
        val playing = before.copy(playing = true, shuffle = true, repeat = 1)
        assertEquals(
            listOf(RestoreStep.STOP_PLAYER, RestoreStep.SETTINGS, RestoreStep.START_PLAYER, RestoreStep.QUEUE, RestoreStep.SHUFFLE, RestoreStep.REPEAT, RestoreStep.RESUME),
            restoreSteps(playing, now),
        )
        assertEquals(listOf(RestoreStep.STOP_PLAYER, RestoreStep.SETTINGS), restoreSteps(before.copy(ids = emptyList(), serviceRunning = false), now))
        assertEquals(emptyList<String>(), restoreProblems(before, before.copy(positionMs = 44_000)))
        assertEquals(
            listOf(
                "settings not as they were: playbackEngine, eqEnabled, crossfeedDb, limiter, offload, crossfadeSec, autoMix, scrobble, autoFill, previousAlwaysSkips, fadeMs",
                "the queue is not the same: 3 songs before, 2 after",
                "on queue place 0, not 1",
                "at 3.0 s, not 42.0 s",
                "playing, though it was paused",
            ),
            restoreProblems(before, now),
        )
    }

    // ---- the result ----

    @Test fun the_report_says_pass_or_fail_first_and_every_check_after() {
        val outcomes = listOf(
            Outcome(EXO, "Seek", Verdict.PASS, "at 30.1 s", tookMs = 1_200),
            Outcome(RUST, "Offload", Verdict.FAIL, "20.0 s", listOf("the chip presented nothing for 19.0 s: silence", "21:05:12 offload: the play head read 0")),
            Outcome(RUST, "Offload EQ", Verdict.SKIP, "not available on this device"),
        )
        val text = reportText(outcomes, "2026-09-25 21:05", 252_000, listOf("21:05:13 invariant: offload-starved: ..."))
        assertEquals(
            """
            Self test, 2026-09-25 21:05, took 4 min 12 s: 1 FAILED (1 passed, 1 failed, 1 skipped)
            Failed: Rust: Offload
            PASS  ExoPlayer: Seek - at 30.1 s [1 s]
            FAIL  Rust: Offload - 20.0 s
                    the chip presented nothing for 19.0 s: silence
                    21:05:12 offload: the play head read 0
            SKIP  Rust: Offload EQ - not available on this device
            Invariant breaks during the test:
                    21:05:13 invariant: offload-starved: ...

            """.trimIndent(),
            text,
        )
        assertTrue(reportText(outcomes.take(1), "x", 38_000, emptyList()).startsWith("Self test, x, took 38 s: ALL PASSED (1 passed, 0 failed, 0 skipped)\n"))
        assertTrue(reportText(outcomes.take(1), "x", 38_000, emptyList(), cancelled = true).startsWith("Self test, x, took 38 s, cancelled: CANCELLED, nothing failed before (1 passed"))
    }
}
