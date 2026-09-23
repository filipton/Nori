package dev.nori.music.app.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.text.BasicText
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.graphics.isUnspecified
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.ColorProducer
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.graphics.drawscope.translate
import androidx.compose.ui.graphics.vector.rememberVectorPainter
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.TextLayoutResult
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import dev.nori.music.look.CoverLook

/**
 * What a page is dressed in: one table worked out in Rust (`nori_look::dress`, through [CoverLook]) and
 * only ever looked up here - the theme's roles, the plates behind the buttons, the chrome, the status
 * bar, the gradients' stops. Indices are [CoverLook]'s.
 *
 * A page that sits still has a [FixedLook]. The player's page cross-fades between records, and its
 * [LiveLook] mixes two tables as it goes; read from the draw phase, that mix redraws what shows it and
 * recomposes nothing, which is what the fade used to do to the whole player on every frame.
 */
@Stable
interface Look {
    fun argb(entry: Int): Int
    fun color(entry: Int): Color = Color(argb(entry))

    /** 0 on a dark page, 1 on paper. */
    val paper: Float get() = Float.fromBits(argb(CoverLook.PAPER))
}

/** A look that does not move: a page's own table, looked up. */
@androidx.compose.runtime.Immutable
class FixedLook(val table: IntArray) : Look {
    override fun argb(entry: Int): Int = table[entry]
}

/** The page's look. [NoriTheme] provides the theme's own; [TintedTheme] a cover's; the player a [LiveLook]. */
val LocalLook = staticCompositionLocalOf<Look> { FixedLook(IntArray(CoverLook.LEN)) }

/** How far along a [LiveLook] is, by clock [mode]; a primitive in and out, so reading it boxes nothing. */
fun interface Progress { fun at(mode: Int): Float }

/** A Float read where it is used (the draw phase), without boxing it on every frame as `() -> Float` does. */
fun interface FloatReader { fun read(): Float }

/**
 * Two looks and how far the page is from one to the other, mixed at most once per frame and only while
 * something reads it. [progress] is read where the colour is, so a reader in the draw phase follows the
 * fade by redrawing.
 */
@Stable
class LiveLook(private val progress: Progress) : Look {
    private var from by mutableStateOf<IntArray?>(null)
    private var to by mutableStateOf(IntArray(CoverLook.LEN))
    private var mode by mutableIntStateOf(0)

    /** Where the page is: [to] alone, or on its way to it from [from], which of [progress]'s clocks says how far. */
    fun set(from: IntArray?, to: IntArray, mode: Int) {
        if (this.to !== to) this.to = to
        if (this.from !== from) this.from = from
        if (this.mode != mode) this.mode = mode
    }

    /** How far along, 0 at [from] and 1 at [to]; 1 when there is nothing to mix. */
    fun t(): Float {
        if (from == null) return 1f
        return progress.at(mode).coerceIn(0f, 1f)
    }

    // The whole mix for one point of the fade, made by nori-look in one crossing the first time
    // anything reads it at that point, then looked up by everything else drawn in that frame.
    private val cache = IntArray(CoverLook.LEN)
    private var cachedAt = Float.NaN
    private var cacheFrom: IntArray? = null
    private var cacheTo: IntArray? = null

    override fun argb(entry: Int): Int {
        val b = to
        val a = from ?: return b[entry]
        val t = progress.at(mode)
        if (t >= 1f) return b[entry]
        if (t <= 0f) return a[entry]
        if (cachedAt != t || cacheFrom !== a || cacheTo !== b) {
            CoverLook.mix(a, b, t, cache)
            cacheFrom = a; cacheTo = b; cachedAt = t
        }
        return cache[entry]
    }
}

/**
 * Text whose colour is looked up while it is drawn, not while it is composed: a page cross-fading
 * redraws it and recomposes nothing. Otherwise it is Material's Text with [style] as given.
 */
@Composable
fun LookText(
    text: String, color: ColorProducer, modifier: Modifier = Modifier, style: TextStyle,
    textAlign: TextAlign? = null, maxLines: Int = Int.MAX_VALUE, softWrap: Boolean = true,
    overflow: TextOverflow = TextOverflow.Clip, onTextLayout: ((TextLayoutResult) -> Unit)? = null,
) {
    BasicText(
        text, modifier,
        style = if (textAlign != null) style.merge(TextStyle(textAlign = textAlign)) else style,
        onTextLayout = onTextLayout, overflow = overflow, softWrap = softWrap, maxLines = maxLines, color = color,
    )
}

/**
 * Material's Icon, but tinted while it is drawn: the tint is read in the draw phase, and the colour
 * filter is only made again when the colour really changes.
 */
@Composable
fun LookIcon(icon: ImageVector, description: String?, modifier: Modifier, tint: ColorProducer) {
    val painter = rememberVectorPainter(icon)
    val filter = remember { TintCache() }
    Box(
        modifier
            .then(if (description != null) Modifier.semantics { contentDescription = description; role = Role.Image } else Modifier)
            .drawBehind {
                    val c = tint()
                    // Fit, as Material's Icon draws: a square glyph in its box, scaled to it.
                    val scale = ContentScale.Fit.computeScaleFactor(painter.intrinsicSize, size)
                    val w = painter.intrinsicSize.width * scale.scaleX
                    val h = painter.intrinsicSize.height * scale.scaleY
                    translate((size.width - w) / 2f, (size.height - h) / 2f) {
                        with(painter) { draw(androidx.compose.ui.geometry.Size(w, h), colorFilter = filter.of(c)) }
                    }
            },
    )
}

/**
 * Round-cornered shapes by radius, made once each and kept: a layer whose corners change as it moves
 * (a cover flying between two places) used to make a new shape object on every frame. Radii are kept to
 * a quarter pixel, which no eye can tell from exact.
 */
class CornerShapes(private val topOnly: Boolean = false) {
    private val made = androidx.collection.MutableIntObjectMap<androidx.compose.ui.graphics.Shape>()
    fun of(radiusPx: Float): androidx.compose.ui.graphics.Shape {
        val key = (radiusPx.coerceAtLeast(0f) * 4f).toInt()
        return made[key] ?: (key / 4f).let { r ->
            if (topOnly) androidx.compose.foundation.shape.RoundedCornerShape(topStart = r, topEnd = r)
            else androidx.compose.foundation.shape.RoundedCornerShape(r)
        }.also { made[key] = it }
    }
}

/** One tint filter, made again only when the colour changes. */
private class TintCache {
    private var colour = Color.Unspecified
    private var filter: ColorFilter? = null
    fun of(c: Color): ColorFilter? {
        if (c.isUnspecified) return null
        if (c != colour || filter == null) { colour = c; filter = ColorFilter.tint(c) }
        return filter
    }
}
