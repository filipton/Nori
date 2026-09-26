package dev.nori.music.app.ui

import android.graphics.Bitmap
import android.os.Handler
import android.os.Looper
import dev.nori.music.app.PerfHooks
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.painter.BitmapPainter
import androidx.compose.ui.graphics.painter.Painter
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.IntSize
import dev.nori.music.data.CoverLoader
import kotlin.math.roundToInt

/**
 * One cover as a view draws it: the picture kept in memory at once when there is one, otherwise the one
 * the core's loader decodes for the view's size (CoverLoader). Composables only read [image] and
 * [state]; nothing here runs per frame.
 */
@Stable
class CoverImage internal constructor(val url: String?, loader: CoverLoader?) {
    private val kept = url?.let { loader?.kept(it) }

    /** The picture, once there is one. A smaller one kept from another view may stand in until it comes. */
    var image by mutableStateOf(kept?.bitmap?.asImageBitmap())
        private set

    /** It was in memory when the view was composed: drawn at once, not faded in. */
    val fromMemory: Boolean = kept != null

    var state by mutableStateOf(if (url == null) MISSING else if (kept != null) READY else LOADING)
        private set

    private val fetch = CoverFetch(
        url, if (loader != null) Loads(loader) else Nowhere, kept?.bitmap,
        shown = { b -> if (image?.asAndroidBitmap() !== b) image = b.asImageBitmap(); state = READY },
        missing = { state = MISSING },
    )
    private var painted: Pair<ImageBitmap, Painter>? = null

    /** [image] as a painter, made once per picture, for the places that take one. */
    val painter: Painter?
        get() {
            val i = image ?: return null
            painted?.let { (b, p) -> if (b === i) return p }
            return BitmapPainter(i, filterQuality = FilterQuality.Low).also { painted = i to it }
        }

    /**
     * Asks for the picture a view [width] x [height] pixels draws, unless what is kept will do; a load
     * that fails is asked again once, a moment later (see [CoverFetch]). Main thread.
     */
    internal fun want(width: Int, height: Int) = fetch.want(width, height)

    /** The view has gone, or wants another size: whatever is on its way is let go. */
    internal fun stop() = fetch.stop()

    /** The app's loader, as a [CoverFetch] asks it. */
    private class Loads(private val loader: CoverLoader) : CoverFetch.Source<Bitmap> {
        override fun kept(url: String, width: Int, height: Int): Pair<Bitmap, Boolean>? =
            loader.kept(url)?.let { it.bitmap to it.fits(width, height) }

        override fun load(url: String, width: Int, height: Int, done: (Bitmap?, Int) -> Unit): CoverFetch.Handle {
            val r = loader.request(url, width, height, done)
            return CoverFetch.Handle { r.cancel() }
        }

        override fun later(ms: Long, run: () -> Unit): CoverFetch.Handle {
            val r = Runnable(run)
            main.postDelayed(r, ms)
            return CoverFetch.Handle { main.removeCallbacks(r) }
        }

        override fun failed(url: String, status: Int, again: Boolean) {
            PerfHooks.recorder?.coverFailed(url, status, again)
        }
    }

    /** No loader (a preview): nothing is ever asked for. */
    private object Nowhere : CoverFetch.Source<Bitmap> {
        override fun kept(url: String, width: Int, height: Int): Pair<Bitmap, Boolean>? = null
        override fun load(url: String, width: Int, height: Int, done: (Bitmap?, Int) -> Unit) = CoverFetch.Handle {}
        override fun later(ms: Long, run: () -> Unit) = CoverFetch.Handle {}
        override fun failed(url: String, status: Int, again: Boolean) {}
    }

    companion object {
        private val main = Handler(Looper.getMainLooper())

        const val LOADING = 0
        const val READY = 1
        /** No cover, or none that can be drawn: the plate stays. */
        const val MISSING = 2
    }
}

/**
 * The cover at [url] for a view [width] x [height] pixels. Asked for once the size is known (0 until the
 * view is measured) and let go when the view leaves composition, so a row scrolled past before its cover
 * came never decodes it.
 */
@Composable
fun rememberCover(url: String?, width: Int, height: Int = width): CoverImage {
    val context = LocalContext.current
    val cover = remember(url) { CoverImage(url, url?.let { CoverLoader.get(context) }) }
    DisposableEffect(cover, width, height) {
        if (width > 0 && height > 0) cover.want(width, height)
        onDispose { cover.stop() }
    }
    return cover
}

/** [image] filling this area the way ContentScale.Crop draws it: the middle of the picture in the area's shape. */
internal fun DrawScope.drawCover(image: ImageBitmap, alpha: Float) {
    val w = size.width
    val h = size.height
    if (w <= 0f || h <= 0f) return
    val scale = maxOf(w / image.width, h / image.height)
    val sw = (w / scale).roundToInt().coerceIn(1, image.width)
    val sh = (h / scale).roundToInt().coerceIn(1, image.height)
    drawImage(
        image, IntOffset((image.width - sw) / 2, (image.height - sh) / 2), IntSize(sw, sh),
        dstSize = IntSize(w.roundToInt(), h.roundToInt()), alpha = alpha, filterQuality = FilterQuality.Low,
    )
}
