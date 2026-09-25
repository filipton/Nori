package dev.nori.music.app.perf

import java.util.Locale

/*
 * The self test's plain part: which checks run in which order, how each reading is judged, what is put
 * back afterwards and how the result reads. Nothing here touches Android, the player or the core, so all
 * of it is tested on the JVM (src/testPerf); SelfTest.kt drives the phone and hands its readings here.
 */

enum class Verdict { PASS, FAIL, SKIP }

/** One check's outcome: what was measured, and on a failure (or a skip) why, with the lines that show it. */
data class Outcome(
    val section: String,
    val name: String,
    val verdict: Verdict,
    val measured: String,
    val detail: List<String> = emptyList(),
    val tookMs: Long = 0,
)

/** A judgement of readings, before it is named: [problems] empty is a pass. */
data class Judged(val measured: String, val problems: List<String> = emptyList()) {
    val ok: Boolean get() = problems.isEmpty()
    fun outcome(section: String, name: String, tookMs: Long, extra: List<String> = emptyList()): Outcome =
        Outcome(section, name, if (ok) Verdict.PASS else Verdict.FAIL, measured, if (ok) emptyList() else problems + extra, tookMs)
}

/**
 * One look at the player while a check runs, every quarter second or so. [tMs] is the phone's uptime
 * clock. The output's own counts are the AudioTrack's: its play head and its timestamp (frames), and for
 * the Rust player the frames written into it; null where they could not be read. [track] tells one
 * AudioTrack from the next, whose counts start again.
 */
data class Reading(
    val tMs: Long,
    val positionMs: Long,
    val index: Int,
    val shownId: String?,
    val heardId: String?,
    val playing: Boolean,
    val head: Long? = null,
    val stamp: Long? = null,
    val written: Long? = null,
    val offloaded: Boolean = false,
    val track: Int = 0,
)

/** The longest the Rust player may go between two writes into its output (its deep buffer, less the low mark). */
const val BURST_MS = 9_000L

/** How long a count may stand still while music plays before it is a stall. */
const val STALL_MS = 1_500L

/** The longest the value [of] stood still over [r] (readings of one song and one track only), ms. */
fun longestStill(r: List<Reading>, of: (Reading) -> Long?): Long {
    var longest = 0L
    var since: Reading? = null
    var last: Long? = null
    for (x in r) {
        val v = of(x) ?: continue
        val s = since
        if (s == null || v != last || s.track != x.track || s.index != x.index) {
            since = x
            last = v
            continue
        }
        longest = maxOf(longest, x.tMs - s.tMs)
    }
    return longest
}

/** How much [of] grew over [r], counting only steps within one track (a new track starts from nought). */
fun grew(r: List<Reading>, of: (Reading) -> Long?): Long {
    var sum = 0L
    var prev: Reading? = null
    for (x in r) {
        val v = of(x) ?: continue
        val p = prev
        val pv = p?.let(of)
        if (p != null && pv != null && p.track == x.track && v > pv) sum += v - pv
        prev = x
    }
    return sum
}

/** The place in the song moved on, summed over steps within one song: a seek or a new song is not playing. */
fun played(r: List<Reading>): Long {
    var sum = 0L
    for ((a, b) in r.zipWithNext()) {
        if (a.index != b.index) continue
        val d = b.positionMs - a.positionMs
        // A jump forward much faster than time is a seek, not music.
        if (d > 0 && d <= (b.tMs - a.tMs) * 2 + 500) sum += d
    }
    return sum
}

fun secs(ms: Long): String = String.format(Locale.ROOT, "%.1f s", ms / 1000.0)

/**
 * Music really plays over [r]: the place moves on at about the speed of time, the output's play head or
 * timestamp moves on (either will do: an offloaded track on some phones keeps its play head at nought),
 * and with [written] the frames written to the output move on too and stay ahead of what it presented.
 */
fun judgeProgress(r: List<Reading>, written: Boolean): Judged {
    if (r.size < 2) return Judged("no readings", listOf("the player could not be read"))
    val span = r.last().tMs - r.first().tMs
    val moved = played(r)
    val still = longestStill(r) { it.positionMs }
    val head = grew(r) { it.head }
    val stamp = grew(r) { it.stamp }
    val w = grew(r) { it.written }
    val out = mutableListOf<String>()
    if (r.any { !it.playing }) out += "the player said it was not playing at ${r.count { !it.playing }} of ${r.size} readings"
    if (moved < span * 8 / 10) out += "the place moved ${secs(moved)} in ${secs(span)}"
    if (still > STALL_MS) out += "the place stood still for ${secs(still)}"
    val counted = r.any { it.head != null || it.stamp != null }
    if (counted && head <= 0 && stamp <= 0) out += "the output presented nothing: play head and timestamp stood still"
    // Either count moving will do: an offloaded track on some phones keeps its play head at nought.
    val outStill = listOfNotNull(
        if (r.any { it.head != null }) longestStill(r) { it.head } else null,
        if (r.any { it.stamp != null }) longestStill(r) { it.stamp } else null,
    ).minOrNull()
    if (outStill != null && outStill > STALL_MS) out += "the output presented nothing new for ${secs(outStill)}"
    if (written) {
        // The Rust player writes in bursts into seconds of buffer: a short look may fall between two.
        if (r.none { it.written != null }) out += "the frames written could not be read"
        else if (w <= 0 && span >= BURST_MS) out += "nothing was written to the output in ${secs(span)}"
        val behind = r.filter { it.written != null && it.head != null && it.written < it.head }
        if (behind.isNotEmpty()) out += "the output presented more than was written (${behind.first().head} of ${behind.first().written} frames)"
    }
    val m = buildString {
        append("played ${secs(moved)} in ${secs(span)}")
        if (counted) append(", play head +$head, timestamp +$stamp frames")
        if (written) append(", written +$w frames")
    }
    return Judged(m, out)
}

/** Paused over [r]: the place and the output stand still (within [slackMs] of music). */
fun judgePaused(r: List<Reading>, slackMs: Long = 300, rate: Int = 48_000): Judged {
    if (r.size < 2) return Judged("no readings", listOf("the player could not be read"))
    val moved = r.last().positionMs - r.first().positionMs
    val head = grew(r) { it.head }
    val stamp = grew(r) { it.stamp }
    val frames = slackMs * rate / 1000
    val out = mutableListOf<String>()
    if (r.last().playing) out += "the player still said it was playing"
    if (moved > slackMs) out += "the place moved ${secs(moved)} while paused"
    if (head > frames) out += "the play head moved $head frames while paused"
    if (stamp > frames) out += "the timestamp moved $stamp frames while paused"
    return Judged("paused for ${secs(r.last().tMs - r.first().tMs)}: the place moved ${moved} ms, play head +$head, timestamp +$stamp", out)
}

/** A seek to [targetMs] landed: within [slackMs] of it within the readings, and played on from there. */
fun judgeSeek(targetMs: Long, r: List<Reading>, slackMs: Long = 1_500): Judged {
    if (r.isEmpty()) return Judged("no readings", listOf("the player could not be read"))
    val start = r.first().tMs
    val landed = r.firstOrNull { kotlin.math.abs(it.positionMs - targetMs) <= slackMs }
    val out = mutableListOf<String>()
    if (landed == null) out += "the place never came within ${secs(slackMs)} of ${secs(targetMs)}: it read ${r.map { it.positionMs }.distinct().take(6).joinToString()}"
    val after = landed?.let { l -> r.filter { it.tMs >= l.tMs } }.orEmpty()
    if (landed != null && after.last().positionMs <= landed.positionMs) out += "it did not play on from there"
    if (r.map { it.index }.distinct().size > 1) out += "the song changed: ${r.map { it.index }.distinct()}"
    val m = landed?.let { "at ${secs(it.positionMs)} ${it.tMs - start} ms after asking for ${secs(targetMs)}" } ?: "asked for ${secs(targetMs)}"
    return Judged(m, out)
}

/**
 * After skipping from queue place [from] by [by] (negative: back), the player ended on [expected]; and
 * the song shown, the song heard (the player service's) and what the engine plays are all that song.
 * [arrivals] are the places the service arrived at on the way, in order.
 */
fun judgeMove(from: Int, by: Int, end: Reading, expectedId: String?, arrivals: List<Int>, engineIndex: Int? = null): Judged {
    val want = from + by
    val out = mutableListOf<String>()
    if (end.index != want) out += "ended on queue place ${end.index}, expected $want (${kotlin.math.abs(end.index - from)} songs for ${kotlin.math.abs(by)} presses)"
    if (end.shownId != expectedId) out += "the screen shows ${end.shownId}, expected $expectedId"
    if (end.heardId != expectedId) out += "the player service is on ${end.heardId}, expected $expectedId"
    if (engineIndex != null && engineIndex >= 0 && engineIndex != want) out += "the engine plays queue place $engineIndex, expected $want"
    val beyond = arrivals.filter { if (by >= 0) it > want else it < want }
    if (beyond.isNotEmpty()) out += "went past it on the way: arrived at $arrivals"
    return Judged("from $from to ${end.index} (${arrivals.size} arrivals: ${arrivals.joinToString()})", out)
}

/**
 * The offloaded stretch [r] played without silence: stayed on the chip, and the chip's counts (play head
 * or timestamp) never stood still longer than [STALL_MS] except within [graceMs] after a jump at [jumps].
 */
fun judgeOffload(r: List<Reading>, jumps: List<Long>, graceMs: Long = 1_500): Judged {
    if (r.size < 2) return Judged("no readings", listOf("the player could not be read"))
    val out = mutableListOf<String>()
    val span = r.last().tMs - r.first().tMs
    val off = r.count { it.offloaded }
    if (off < r.size) out += "left offload at ${r.count { !it.offloaded }} of ${r.size} readings (first at ${secs(r.first { !it.offloaded }.tMs - r.first().tMs)})"
    // Silence: a stretch where neither count moved, outside the moments just after a skip or a seek.
    var silent = 0L
    var from: Reading? = null
    for ((a, b) in r.zipWithNext()) {
        val moved = (b.track != a.track) || ((b.head ?: 0) > (a.head ?: 0)) || ((b.stamp ?: 0) > (a.stamp ?: 0)) || b.index != a.index
        val nearJump = jumps.any { b.tMs >= it && b.tMs - it <= graceMs }
        if (moved || nearJump) { from = null; continue }
        val f = from ?: a.also { from = it }
        silent = maxOf(silent, b.tMs - f.tMs)
    }
    if (silent > STALL_MS) out += "the chip presented nothing for ${secs(silent)}: silence"
    val moved = played(r)
    if (moved < (span - jumps.size * graceMs) * 7 / 10) out += "the place moved ${secs(moved)} in ${secs(span)}"
    val head = grew(r) { it.head }
    val stamp = grew(r) { it.stamp }
    return Judged("${secs(span)} with ${jumps.size} jumps: played ${secs(moved)}, play head +$head, timestamp +$stamp frames, longest silence ${secs(silent)}, offloaded at $off of ${r.size} readings", out)
}

/** A transition the planner logged: "transition A -> B: CROSSFADE 6000 ms at 180000, tempo x1.000 (why)". */
data class Transition(val from: String, val to: String, val kind: String, val durationMs: Long, val line: String)

private val TRANSITION = Regex("""transition (.+) -> (.+): ([A-Z_]+) (-?\d+) ms at""")

fun parseTransitions(lines: List<String>): List<Transition> = lines.mapNotNull { l ->
    TRANSITION.find(l)?.let { m -> Transition(m.groupValues[1], m.groupValues[2], m.groupValues[3], m.groupValues[4].toLongOrNull() ?: 0, l.trim()) }
}

/** The planner's "planFor: ..." lines: why a boundary got no transition. */
fun planRefusals(lines: List<String>): List<String> = lines.filter { "planFor:" in it }.map { it.substringAfter("planFor:").trim() }

/**
 * AutoMix, switched on mid-song, planned the next boundary for real: a transition out of [fromTitle] was
 * logged with a length, the songs either side were measured ahead, and a mix was heard.
 */
fun judgeAutoMix(fromTitle: String, lines: List<String>, measured: Map<String, Boolean>, mixed: Boolean): Judged {
    val ts = parseTransitions(lines).filter { it.from == fromTitle }
    val out = mutableListOf<String>()
    val t = ts.lastOrNull()
    if (t == null) {
        val why = planRefusals(lines)
        out += if (why.isEmpty()) "no transition was planned out of \"$fromTitle\"" else "no transition out of \"$fromTitle\": ${why.last()}"
    } else if (t.durationMs <= 0) out += "the transition has no length: ${t.line}"
    val unmeasured = measured.filterValues { !it }.keys
    if (unmeasured.isNotEmpty()) out += "not measured ahead: ${unmeasured.joinToString()}"
    if (!mixed) out += "no mix was heard at the boundary"
    val m = t?.let { "${it.kind} ${it.durationMs} ms into \"${it.to}\"" } ?: "no plan"
    return Judged("$m; measured ahead: ${measured.count { it.value }} of ${measured.size}; mix heard: $mixed", out)
}

/** A crossfade of [seconds] set near a song's end was the one heard at that boundary. */
fun judgeCrossfade(seconds: Int, fromTitle: String, lines: List<String>, mixed: Boolean): Judged {
    val t = parseTransitions(lines).lastOrNull { it.from == fromTitle }
    val out = mutableListOf<String>()
    if (!mixed) out += "no mix was heard at the boundary"
    if (t != null && kotlin.math.abs(t.durationMs - seconds * 1000L) > 1_000) out += "planned ${t.durationMs} ms, not the ${seconds * 1000} ms set: ${t.line}"
    if (t == null && lines.isNotEmpty() && planRefusals(lines).isNotEmpty()) out += "no transition: ${planRefusals(lines).last()}"
    return Judged(t?.let { "${it.kind} ${it.durationMs} ms" } ?: if (mixed) "a mix was heard (no plan line in the log)" else "no mix", out)
}

// ---- lyrics ----

/** How good a lyrics answer is: none, plain, timed by line, timed by word. */
fun lyricsRank(lines: Int, synced: Boolean, wordTimed: Boolean): Int = when {
    lines == 0 -> 0
    wordTimed -> 3
    synced -> 2
    else -> 1
}

/** What the lyrics panel was handed, as it was handed: for song [songId], loading, or an answer of [rank]. */
data class LyricsSeen(val tMs: Long, val songId: String?, val playingId: String?, val loading: Boolean, val rank: Int, val failed: Boolean = false, val key: String = "")

/**
 * The lyrics shown belong to the song playing; once words are shown for a song they change only for
 * better ones; and a song come back to ([returnedTo], from [returnedAtMs]) starts from its words, with no
 * loader and no second lookup.
 */
fun judgeLyrics(seen: List<LyricsSeen>, returnedTo: String?, returnedAtMs: Long): Judged {
    val out = mutableListOf<String>()
    val answers = seen.filter { !it.loading }
    answers.filter { it.songId != null && it.playingId != null && it.songId != it.playingId }.forEach {
        out += "lyrics of ${it.songId} were handed over while ${it.playingId} played"
    }
    for ((id, list) in answers.groupBy { it.songId }) {
        val shown = list.filter { it.rank > 0 }
        for ((a, b) in shown.zipWithNext()) {
            if (b.key != a.key && b.rank <= a.rank) out += "the words of $id changed after they were shown, for no better ones (rank ${a.rank} to ${b.rank})"
        }
    }
    if (returnedTo != null) {
        val back = seen.filter { it.tMs >= returnedAtMs && it.songId == returnedTo }
        val before = answers.lastOrNull { it.songId == returnedTo && it.tMs < returnedAtMs }
        // Words once shown come back at once; a song that had none may be asked again.
        if (back.any { it.loading } && (before == null || before.rank > 0)) out += "coming back to $returnedTo showed the loader again"
        val again = back.filter { !it.loading }
        if (before != null && again.size > 1) out += "coming back to $returnedTo handed its lyrics over ${again.size} times"
        if (before != null && again.isNotEmpty() && again.first().key != before.key) out += "coming back to $returnedTo showed other lyrics than before"
    }
    val songs = answers.map { it.songId }.distinct().size
    val ranks = answers.groupBy { it.songId }.map { (_, l) -> l.maxOf { it.rank } }
    return Judged("$songs songs, best answers ${ranks.joinToString { listOf("none", "plain", "by line", "by word")[it.coerceIn(0, 3)] }}", out)
}

// ---- songs ----

/** A song the test could play: from the user's library on their server, and whether it is on the phone already. */
data class Candidate(val id: String, val suffix: String, val durationS: Int, val local: Boolean, val album: String = "", val replayGain: Boolean = false, val external: Boolean = false)

/** Never a provider's song (octo-fiesta downloads it when asked for), a radio stream, or one too short to seek in. */
fun playable(c: Candidate): Boolean =
    !c.external && !c.id.startsWith("ext-") && !c.id.startsWith("radio:") && c.durationS in 45..1200

/**
 * [n] songs for a queue: the ones on the phone first (no network), then one of each album before a
 * second, and songs of [suffix] only when it is given.
 */
fun pickSongs(all: List<Candidate>, n: Int, suffix: String? = null): List<Candidate> {
    val ok = all.filter(::playable).filter { suffix == null || it.suffix.equals(suffix, ignoreCase = true) }.distinctBy { it.id }
    val (local, remote) = ok.partition { it.local }
    val ordered = spread(local) + spread(remote)
    return ordered.take(n)
}

/** One song of each album first, then the rest, keeping the order otherwise. */
private fun spread(list: List<Candidate>): List<Candidate> {
    val seen = HashSet<String>()
    val (first, rest) = list.partition { it.album.isEmpty() || seen.add(it.album) }
    return first + rest
}

// ---- the plan ----

/** A check the suite makes, with how long it may take before it is a failure. */
data class Step(val id: String, val section: String, val name: String, val timeoutMs: Long)

/** What the plan depends on: what this phone and the user's choices allow. */
data class Options(val downloads: Boolean = false)

/** The section the player's checks are reported under. */
const val PLAYER = "Player"

/**
 * The suite, in order: the player's playback and controls, the settings applied live on it and its
 * offload, then lyrics, covers and (asked for) downloads; setting up first and putting everything back
 * last, always.
 */
fun plan(o: Options): List<Step> {
    val out = mutableListOf(Step("prepare", "Setup", "Songs and settings for the test", 30_000))
    val e = PLAYER
    out += Step("start", e, "The test's songs start playing", 20_000)
    out += Step("play", e, "Playback moves for 10 s", 25_000)
    out += Step("pause", e, "Pause and resume", 15_000)
    out += Step("seek", e, "Seek", 12_000)
    out += Step("nextprev", e, "Next and previous", 20_000)
    out += Step("rapid", e, "3 quick nexts move 3 songs", 20_000)
    out += Step("endskip", e, "Skip just before a song ends", 20_000)
    out += Step("auto", e, "A song ends into the next by itself", 25_000)
    out += Step("eq", e, "Equalizer on and off while playing", 20_000)
    out += Step("automix", e, "AutoMix switched on mid-song mixes the next boundary", 75_000)
    out += Step("crossfade", e, "Crossfade length changed near a song's end", 40_000)
    out += Step("replaygain", e, "ReplayGain mode changed while playing", 15_000)
    out += Step("offload", e, "Offload: entered, 20 s without silence across a skip and a seek", 60_000)
    out += Step("offloadeq", e, "Offload: equalizer on leaves it, off takes it back", 40_000)
    out += Step("lyrics", "Lyrics", "Lyrics of 3 songs: the right ones, stable, not fetched again", 90_000)
    out += Step("covers", "Covers", "Covers decode and give colours", 30_000)
    if (o.downloads) out += Step("downloads", "Downloads", "A downloaded song opens", 15_000)
    out += Step("restore", "Setup", "Everything put back", 40_000)
    return out
}

// ---- putting things back ----

/**
 * The settings the test changes, as the user had them: each is set to what a check needs and put back
 * afterwards. Everything that stands between a song and the audio chip is here, so the offload check
 * can clear the way; and scrobbling, autofill and explicit skipping, so the test neither reports its
 * plays nor adds songs nor skips any by itself.
 */
data class Knobs(
    val eq: Boolean, val crossfeedDb: Float, val balance: Float, val mono: Boolean, val limiter: Boolean,
    val speed: Float, val pitch: Float, val skipSilence: Boolean, val offload: Boolean, val crossfadeSec: Int, val autoMix: Boolean,
    val replayGain: Int, val scrobble: Boolean, val autoFill: Boolean, val skipExplicit: Boolean, val previousAlwaysSkips: Boolean,
    val fadeMs: Int,
)

/** What the test starts from: the plain CPU path, nothing mixing, nothing reported, nothing added. */
fun testKnobs(user: Knobs): Knobs = user.copy(
    eq = false, crossfeedDb = 0f, balance = 0f, mono = false, limiter = false, speed = 1f, pitch = 1f,
    skipSilence = false, offload = false, crossfadeSec = 0, autoMix = false, scrobble = false, autoFill = false,
    skipExplicit = false, previousAlwaysSkips = false, fadeMs = 0,
)

/** Names of the settings that differ between [a] and [b]: what putting [a] back changes. */
fun knobsDiffer(a: Knobs, b: Knobs): List<String> {
    val out = mutableListOf<String>()
    fun d(name: String, x: Any, y: Any) { if (x != y) out += name }
    d("eqEnabled", a.eq, b.eq); d("crossfeedDb", a.crossfeedDb, b.crossfeedDb)
    d("balance", a.balance, b.balance); d("mono", a.mono, b.mono); d("limiter", a.limiter, b.limiter); d("speed", a.speed, b.speed)
    d("pitch", a.pitch, b.pitch); d("skipSilence", a.skipSilence, b.skipSilence); d("offload", a.offload, b.offload)
    d("crossfadeSec", a.crossfadeSec, b.crossfadeSec); d("autoMix", a.autoMix, b.autoMix); d("replayGain", a.replayGain, b.replayGain)
    d("scrobble", a.scrobble, b.scrobble); d("autoFill", a.autoFill, b.autoFill); d("skipExplicit", a.skipExplicit, b.skipExplicit)
    d("previousAlwaysSkips", a.previousAlwaysSkips, b.previousAlwaysSkips); d("fadeMs", a.fadeMs, b.fadeMs)
    return out
}

/** Where the user was: the queue (ids), the song and the place in it, playing or not, shuffle and repeat, the player service. */
data class Snapshot(
    val knobs: Knobs, val ids: List<String>, val index: Int, val positionMs: Long, val playing: Boolean,
    val shuffle: Boolean, val repeat: Int, val serviceRunning: Boolean,
)

/** What putting [s] back takes, in order, given where the test left things ([now]). */
enum class RestoreStep { STOP_PLAYER, SETTINGS, START_PLAYER, QUEUE, SHUFFLE, REPEAT, RESUME }

fun restoreSteps(s: Snapshot, now: Snapshot): List<RestoreStep> {
    val out = mutableListOf(RestoreStep.STOP_PLAYER, RestoreStep.SETTINGS)
    // The player is stopped for the queue to be put back as a start of the app puts it back (unprepared,
    // at its place: nothing fetched).
    if (s.serviceRunning || s.ids.isNotEmpty()) out += RestoreStep.START_PLAYER
    if (s.ids.isNotEmpty()) out += RestoreStep.QUEUE
    if (s.shuffle != now.shuffle || s.shuffle) out += RestoreStep.SHUFFLE
    if (s.repeat != now.repeat) out += RestoreStep.REPEAT
    if (s.playing) out += RestoreStep.RESUME
    return out
}

/** What is not as it was after putting [before] back: [after] read back from the phone. */
fun restoreProblems(before: Snapshot, after: Snapshot): List<String> {
    val out = mutableListOf<String>()
    knobsDiffer(before.knobs, after.knobs).takeIf { it.isNotEmpty() }?.let { out += "settings not as they were: ${it.joinToString()}" }
    if (before.ids != after.ids) out += "the queue is not the same: ${before.ids.size} songs before, ${after.ids.size} after"
    if (before.ids.isNotEmpty() && before.index != after.index) out += "on queue place ${after.index}, not ${before.index}"
    if (before.ids.isNotEmpty() && kotlin.math.abs(before.positionMs - after.positionMs) > 5_000) out += "at ${secs(after.positionMs)}, not ${secs(before.positionMs)}"
    if (before.playing != after.playing) out += if (before.playing) "not playing again" else "playing, though it was paused"
    if (before.shuffle != after.shuffle) out += "shuffle ${if (after.shuffle) "on" else "off"}, it was ${if (before.shuffle) "on" else "off"}"
    if (before.repeat != after.repeat) out += "repeat mode ${after.repeat}, it was ${before.repeat}"
    return out
}

// ---- the result ----

/** "4 min 12 s", "38 s". */
fun took(ms: Long): String {
    val s = (ms + 500) / 1000
    return if (s >= 60) "${s / 60} min ${s % 60} s" else "$s s"
}

/**
 * The result as the report carries it: a line of counts and the failures named, then every check in the
 * order it ran with its verdict and what it measured, a failure's lines under it, and at the end what
 * the invariant watch said while the test ran.
 */
fun reportText(outcomes: List<Outcome>, started: String, tookMs: Long, breaks: List<String>, notes: List<String> = emptyList(), cancelled: Boolean = false): String {
    val pass = outcomes.count { it.verdict == Verdict.PASS }
    val fail = outcomes.count { it.verdict == Verdict.FAIL }
    val skip = outcomes.count { it.verdict == Verdict.SKIP }
    val sb = StringBuilder()
    sb.append("Self test, $started, took ${took(tookMs)}${if (cancelled) ", cancelled" else ""}: ")
    sb.append(if (fail > 0) "$fail FAILED" else if (cancelled) "CANCELLED, nothing failed before" else "ALL PASSED").append(" ($pass passed, $fail failed, $skip skipped)\n")
    if (fail > 0) sb.append("Failed: ").append(outcomes.filter { it.verdict == Verdict.FAIL }.joinToString("; ") { "${it.section}: ${it.name}" }).append('\n')
    for (n in notes) sb.append("Note: ").append(n).append('\n')
    for (o in outcomes) {
        sb.append(String.format(Locale.ROOT, "%-4s  %s: %s", o.verdict.name, o.section, o.name))
        if (o.measured.isNotEmpty()) sb.append(" - ").append(o.measured)
        if (o.tookMs > 0) sb.append(" [").append(took(o.tookMs)).append(']')
        sb.append('\n')
        for (d in o.detail.take(40)) sb.append("        ").append(d).append('\n')
    }
    if (breaks.isNotEmpty()) {
        sb.append("Invariant breaks during the test:\n")
        breaks.forEach { sb.append("        ").append(it).append('\n') }
    }
    return sb.toString()
}
