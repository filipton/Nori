package dev.nori.music.app.ui

import androidx.compose.animation.core.Animatable
import androidx.compose.animation.core.tween
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.PointerEventPass
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.viewinterop.AndroidView
import androidx.lifecycle.compose.LifecycleResumeEffect
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import dev.nori.music.app.vm.PlayerViewModel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.first

/**
 * The moving cover's state on screen. Made only while the switch is on: with it off there is no lookup,
 * no player, no surface and no touch watcher.
 */
@Stable
internal class SleeveMotion(
    /** Hands the video surface to the player as it arrives on screen (true) and as it leaves. */
    val onView: (android.view.TextureView, Boolean) -> Unit,
) {
    /** How much of the video is drawn over the still cover. Read in the draw phase. */
    val alpha = Animatable(0f)

    /** The surface is on screen: this album has a video, or one is still fading out. */
    var present by mutableStateOf(false)

    /** What the player was last asked to play. The same one carries on from where it stopped. */
    var playing: String? = null

    /** Written by the carousel: no record held, lifted, sliding or waiting to land. */
    var resting by mutableStateOf(true)

    /** A finger is on the player. Whatever it does may move the record, so the video steps back first. */
    var touching by mutableStateOf(false)

    /** Whether the sleeve was still (all but the finger) the last time the director looked. */
    var wasCalm = false

    /** When it last became still (uptimeMillis). */
    var calmAt = 0L
}

/** How long the music stays paused before the moving cover's decoder is let go (its frame stays). */
private const val MOTION_REST_MS = 5_000L

/** The video's fade back to the still cover: quick, since something is about to move. */
private const val MOTION_OUT_MS = 160

/** The still cover coming alive: slow enough to read as the picture starting to move, not a cut. */
private const val MOTION_IN_MS = 480

/**
 * How long the sleeve is still after a move before the video comes back, so it does not start in the
 * same breath as the move ends. Not after a tap that moved nothing: the video comes straight back.
 */
private const val MOTION_SETTLE_MS = 350L

/**
 * When the moving cover plays, and its fades. It plays only while all of this holds: the player is on
 * screen, fully open, with the artwork panel showing and no flight under way; the app is in front and
 * the screen is on; the record is at rest and no finger is on the player; and movement is not reduced.
 *
 * Nothing appears in one frame. The video fades in over the still cover only once its first frame is on
 * the surface; it fades back to the still cover as soon as any of the above stops holding and is paused
 * where it is (a finger going down is the first sign of a swipe, a skip, a pull on the sheet or a change
 * of panel, so it steps back before the record moves); and a new album fades the last one's out before
 * its own is loaded. The still cover stays what every flight draws (FlyingCover, PanelFlight), which is
 * why the video is gone before one starts.
 *
 * Put away, left, or with the screen off, the player is released outright: nobody can see a fade, and
 * nothing may go on decoding. Switched off, this leaves the composition and releases it too.
 */
@Composable
internal fun MotionDirector(vm: PlayerViewModel, motion: SleeveMotion, sheet: PlayerSheet, onArtwork: Boolean) {
    val shown = LocalPlayerShown.current
    var resumed by remember { mutableStateOf(false) }
    LifecycleResumeEffect(Unit) { resumed = true; onPauseOrDispose { resumed = false } }
    val live = shown && resumed
    // Collected only while someone could see it, so the lookup is never made for a song that changes
    // behind a closed player.
    val target = if (live) vm.motionVideo.collectAsStateWithLifecycle().value else null
    val open by remember(sheet) { derivedStateOf { sheet.progress.value >= 1f && sheet.isOpen } }
    val still = reduceMotion()
    // Everything but the finger: a tap that moved nothing does not count as a move ending.
    val calm = live && open && onArtwork && !still && motion.resting
    val want = calm && !motion.touching
    // The music paused, the picture stops where it is: the frame on screen stays, nothing fades, and
    // nothing decodes until the music goes on again. A cover looping over a paused song kept the screen
    // redrawing two dozen times a second for as long as it was left open.
    val sounding by vm.sounding.collectAsStateWithLifecycle()
    DisposableEffect(motion) { onDispose { vm.motionRelease() } }
    LaunchedEffect(motion, target, want, live, calm, sounding) {
        if (calm && !motion.wasCalm) motion.calmAt = android.os.SystemClock.uptimeMillis()
        motion.wasCalm = calm
        if (!live) {
            motion.alpha.snapTo(0f)
            motion.present = false
            motion.playing = null
            vm.motionRelease()
            return@LaunchedEffect
        }
        if (!want || target != motion.playing) {
            if (motion.alpha.value > 0f) motion.alpha.animateTo(0f, tween(MOTION_OUT_MS))
            vm.motionPause()
        }
        if (target == null) {
            motion.present = false
            return@LaunchedEffect
        }
        if (!want) return@LaunchedEffect
        if (!sounding) {
            // Held on the frame it is showing, or, with none showing yet, not started: the still cover.
            vm.motionPause()
            // A paused decoder still wakes to be polled; a pause that lasts lets it go, frame kept.
            delay(MOTION_REST_MS)
            vm.motionRest()
            return@LaunchedEffect
        }
        motion.present = true
        val settle = MOTION_SETTLE_MS - (android.os.SystemClock.uptimeMillis() - motion.calmAt)
        if (settle > 0) delay(settle)
        vm.motionPlay(target)
        motion.playing = target
        vm.motionReady.first { it == target }
        motion.alpha.animateTo(1f, tween(MOTION_IN_MS))
    }
}

/**
 * The moving cover's surface. A TextureView rather than a SurfaceView: the sleeve's soft bottom is a
 * blend in an offscreen layer (SoftSleeve), and only a texture is drawn into that layer like the rest of
 * the picture; a SurfaceView would punch a hole through the page with a hard bottom edge.
 */
@Composable
internal fun MotionCover(motion: SleeveMotion) {
    val context = LocalContext.current
    val view = remember(context) { android.view.TextureView(context).apply { isOpaque = false } }
    DisposableEffect(view) {
        motion.onView(view, true)
        onDispose { motion.onView(view, false) }
    }
    AndroidView(
        factory = {
            (view.parent as? android.view.ViewGroup)?.removeView(view)
            view
        },
        modifier = Modifier.fillMaxSize().graphicsLayer { alpha = motion.alpha.value },
    )
}

/**
 * [SleeveMotion.touching] from a finger's first touch on the player until the last one leaves. It only
 * watches, in the first pass, and consumes nothing.
 */
internal fun Modifier.watchTouches(motion: SleeveMotion): Modifier = pointerInput(motion) {
    awaitEachGesture {
        awaitFirstDown(requireUnconsumed = false, pass = PointerEventPass.Initial)
        motion.touching = true
        try {
            while (true) {
                val event = awaitPointerEvent(PointerEventPass.Initial)
                if (event.changes.none { it.pressed }) break
            }
        } finally {
            motion.touching = false
        }
    }
}
