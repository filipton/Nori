package dev.nori.music.app.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import kotlinx.coroutines.flow.collectLatest

/**
 * How the pieces of a [FrameCrossfade] come and go, in milliseconds: the one arriving waits [delayMs],
 * then fades in over [inMs] and rises into place over [riseMs] (0: it does not move); the one leaving
 * fades out over [outMs] where it stands.
 */
internal class FadeTimes(val inMs: Float, val outMs: Float, val delayMs: Float = 0f, val riseMs: Float = 0f)

/**
 * The pieces of a cross-fade and how far each has come, moved on by [step] a frame at a time. Kept apart
 * from drawing so the timing can be tested.
 *
 * A piece's [Piece.level] is how visible it is, 0..1, going up while it arrives and down while it
 * leaves; a change in the middle of a fade turns the piece round from wherever it is, so nothing is ever
 * restarted from the wrong end. A piece that is asked for again while it is still leaving comes back from
 * where it had got to rather than being replaced by a new copy of itself.
 */
internal class FrameFades<T>(
    first: T, private val key: (T) -> Any?, private val times: FadeTimes,
    /** The times of the change from one target to the next, when they are not [times]: see [FrameCrossfade]. */
    private val timesFor: ((from: T, to: T) -> FadeTimes?)? = null,
) {
    inner class Piece(value: T, var times: FadeTimes) {
        var value by mutableStateOf(value)
        val key: Any? get() = this@FrameFades.key(value)
        var level by mutableFloatStateOf(0f)
        var rise by mutableFloatStateOf(0f)
        var wait = 0f
        var leaving by mutableStateOf(false)
    }

    val pieces = mutableStateListOf(Piece(first, times).apply { level = 1f; rise = 1f })

    /** The piece arriving or shown: the last one not leaving. */
    val current: Piece? get() = pieces.lastOrNull { !it.leaving }

    /** Whether anything is still moving. */
    val moving: Boolean get() = pieces.size != 1 || pieces[0].let { it.leaving || it.level < 1f || it.rise < 1f }

    /** Makes [target] the piece shown; the one shown before leaves. The same piece (by key) is only updated. */
    fun show(target: T) {
        val k = key(target)
        val now = current
        if (now != null && now.key == k) { now.value = target; return }
        // The change's own times: the one leaving goes as they say, and the one arriving comes by them.
        val change = now?.let { timesFor?.invoke(it.value, target) } ?: times
        now?.leaving = true
        now?.times = change
        val back = pieces.firstOrNull { it.key == k }
        if (back != null) {
            back.value = target
            back.times = change
            back.leaving = false
            // Drawn last, over the ones leaving, as a new arrival would be.
            pieces.remove(back)
            pieces.add(back)
        } else {
            pieces.add(Piece(target, change).apply { wait = change.delayMs; rise = if (change.riseMs > 0f) 0f else 1f })
        }
    }

    /** Moves every piece on by [ms]; ones that have faded out are dropped. */
    fun step(ms: Float) {
        val gone = ArrayList<Piece>(0)
        for (p in pieces) {
            if (p.leaving) {
                p.level = (p.level - ms / p.times.outMs).coerceAtLeast(0f)
                if (p.level == 0f) gone += p
                continue
            }
            var left = ms
            if (p.wait > 0f) {
                val w = minOf(p.wait, left)
                p.wait -= w
                left -= w
                if (left <= 0f) continue
            }
            p.level = (p.level + left / p.times.inMs).coerceAtMost(1f)
            if (p.times.riseMs > 0f) p.rise = (p.rise + left / p.times.riseMs).coerceAtMost(1f)
        }
        pieces.removeAll(gone)
    }

    /** Everything where it is going, at once (reduced motion). */
    fun finish() {
        pieces.removeAll { it.leaving }
        pieces.forEach { it.level = 1f; it.rise = 1f; it.wait = 0f }
    }
}

/** The longest a frame counts for in a [FrameCrossfade]: two frames at sixty a second. */
internal const val FADE_MAX_STEP_MS = 33f

/** One frame at sixty a second. */
private const val FRAME_MS = 16.7f

/**
 * [content] for [target], cross-faded into the next target's rather than cut: the one leaving fades out
 * where it stands while the one arriving fades in (and rises into place, with [FadeTimes.riseMs]).
 * Targets with the same [key] are the same piece. [timesFor] may give one change its own times (the
 * same song's words replaced by finer ones cross-fade in place, with no wait and no rise).
 *
 * Timed by frames, the way the sleeve's slide is ([settleByFrames]), and not by the clock: the frame a
 * song changes on composes the whole new song and can take a quarter of a second, and a clock-timed
 * fade was over by the next frame - the last song's words went in one frame with the page changing under
 * them. No frame here counts for more than [FADE_MAX_STEP_MS], so a slow frame slows the fade instead of
 * skipping it.
 */
@Composable
internal fun <T> FrameCrossfade(
    target: T,
    times: FadeTimes,
    modifier: Modifier = Modifier,
    key: (T) -> Any? = { it },
    timesFor: ((from: T, to: T) -> FadeTimes?)? = null,
    content: @Composable BoxScope.(T) -> Unit,
) {
    val fades = remember { FrameFades(target, key, times, timesFor) }
    // In this composition, before the pieces are read below: the new piece is composed on the frame the
    // target changed, together with whatever else changed with it (the song's title, the page).
    fades.show(target)
    LaunchedEffect(fades) {
        snapshotFlow { fades.moving }.collectLatest { moving ->
            if (!moving) return@collectLatest
            // The first frame counts as one frame, so the fade is under way on the next frame drawn
            // rather than the one after it.
            var last = -1L
            while (fades.moving) {
                val now = withFrameNanos { it }
                val ms = if (last < 0) FRAME_MS else ((now - last) / 1e6f).coerceIn(0f, FADE_MAX_STEP_MS)
                if (AppMotion.reduce) fades.finish() else fades.step(ms)
                last = now
            }
        }
    }
    Box(modifier) {
        for (piece in fades.pieces) key(piece) {
            Box(
                Modifier.graphicsLayer {
                    alpha = fadeEase(piece.level)
                    if (piece.times.riseMs > 0f) translationY = (1f - fadeEase(piece.rise)) * size.height / 40f
                },
            ) { content(piece.value) }
        }
    }
}

/** Soft at both ends and the same curve both ways, so a fade turned round in the middle does not jump. */
internal fun fadeEase(t: Float): Float = t.coerceIn(0f, 1f).let { it * it * (3f - 2f * it) }

/**
 * [animateTo] for a fade that must not be skipped: the time is counted a frame at a time, no frame for
 * more than [FADE_MAX_STEP_MS], so a slow frame (the one that composes a new song) slows it down rather
 * than carrying it most of the way in one step.
 */
internal suspend fun androidx.compose.animation.core.Animatable<Float, androidx.compose.animation.core.AnimationVector1D>.fadeByFrames(
    to: Float,
    ms: Float,
    easing: androidx.compose.animation.core.Easing = androidx.compose.animation.core.FastOutSlowInEasing,
) {
    val from = value
    var t = 0f
    var last = -1L
    while (t < 1f) {
        val now = withFrameNanos { it }
        t = (t + (if (last < 0) FRAME_MS else ((now - last) / 1e6f).coerceIn(0f, FADE_MAX_STEP_MS)) / ms).coerceAtMost(1f)
        last = now
        snapTo(from + (to - from) * easing.transform(t))
    }
}
