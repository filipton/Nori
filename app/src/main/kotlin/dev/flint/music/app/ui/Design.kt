package dev.flint.music.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.gestures.detectHorizontalDragGestures
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.MoreHoriz
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.Typography
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.lifecycle.compose.collectAsStateWithLifecycle
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.FilterQuality
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.Font
import androidx.compose.ui.text.font.FontFamily
import androidx.compose.ui.text.font.FontVariation
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntSize
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import dev.flint.music.app.R

/**
 * The look of the app in one file: radii, spacing, type and the handful of shapes every screen is
 * built from. Apple Music is the reference for the feel - artwork that bleeds into the page, round
 * corners everywhere, hairlines instead of boxes, floating chrome - but the widgets underneath stay
 * ordinary Material 3, because a phone that looks like iOS on purpose ends up looking wrong.
 *
 * Everything here is static: colours and brushes are values, not animations, so a page that sits on
 * screen costs nothing between frames.
 */
object Radius {
    val cover = 8.dp
    val card = 12.dp
    val tile = 18.dp
    val sheet = 26.dp
    val pill = 100.dp
}

object Space {
    /** The side margin every screen shares. Text, rows and section titles all start here. */
    val gutter = 20.dp
    val tight = 8.dp
    val row = 12.dp
    val section = 26.dp
}

val CoverShape = RoundedCornerShape(Radius.cover)
val CardShape = RoundedCornerShape(Radius.card)
val TileShape = RoundedCornerShape(Radius.tile)
val PillShape = RoundedCornerShape(Radius.pill)

/**
 * Inter, as a variable font, standing in for the system face an iPhone would use. It is the single
 * biggest reason a screenshot reads as "a modern music app" rather than "an Android app": Roboto's
 * wide, loose letterforms are what makes stock Compose look like a settings screen. One 880 kB file
 * carries every weight, and each weight is a real instance of the variable font, never a synthetic
 * bold - synthesised weights are what make text look smeared.
 */
private fun inter(weight: FontWeight) = Font(
    R.font.inter, weight,
    variationSettings = FontVariation.Settings(FontVariation.weight(weight.weight)),
)

val Inter = FontFamily(inter(FontWeight.Normal), inter(FontWeight.Medium), inter(FontWeight.SemiBold), inter(FontWeight.Bold))

/**
 * Tighter and heavier than stock Material, which is what makes a music app read as a music app:
 * screen and album titles are display-weight with negative tracking, list rows are plain text, and
 * captions are small grey capitals.
 */
val FlintTypography = Typography().run {
    fun TextStyle.f(weight: FontWeight? = null, tracking: Float? = null) = copy(
        fontFamily = Inter,
        fontWeight = weight ?: fontWeight,
        letterSpacing = tracking?.sp ?: letterSpacing,
    )
    copy(
        displayLarge = displayLarge.f(FontWeight.Bold, -1.5f),
        displayMedium = displayMedium.f(FontWeight.Bold, -1.2f),
        displaySmall = displaySmall.f(FontWeight.Bold, -1f),
        headlineLarge = headlineLarge.f(FontWeight.Bold, -0.9f),
        headlineMedium = headlineMedium.f(FontWeight.Bold, -0.7f),
        headlineSmall = headlineSmall.f(FontWeight.Bold, -0.5f),
        titleLarge = titleLarge.f(FontWeight.Bold, -0.4f),
        titleMedium = titleMedium.f(FontWeight.SemiBold, -0.2f),
        titleSmall = titleSmall.f(FontWeight.SemiBold, -0.1f),
        bodyLarge = bodyLarge.f(FontWeight.Normal, -0.1f).copy(fontSize = 16.sp, lineHeight = 21.sp),
        bodyMedium = bodyMedium.f().copy(fontSize = 15.sp, lineHeight = 20.sp),
        bodySmall = bodySmall.f().copy(fontSize = 13.sp, lineHeight = 17.sp),
        labelLarge = labelLarge.f(FontWeight.SemiBold),
        labelMedium = labelMedium.f(FontWeight.Medium),
        labelSmall = labelSmall.f(FontWeight.SemiBold, 0.5f),
    )
}

/**
 * The colours a page wears, taken from its artwork. [edge] is what the bottom of the cover actually
 * is, so the picture can dissolve into the page without a seam; [background] is the page under it.
 */
data class PagePalette(
    val edge: Color,
    val background: Color,
    val onBackground: Color,
    val onBackgroundVariant: Color,
    val accent: Color,
    val tinted: Boolean = true,
    /**
     * The cover at sixteen pixels a side, to be drawn stretched over the page: the colours vary the
     * way the artwork's do instead of settling into one average. See `CoverColors.washOf`.
     */
    val wash: androidx.compose.ui.graphics.ImageBitmap? = null,
)

val LocalPalette = staticCompositionLocalOf<PagePalette?> { null }

/** Star changes made this session, keyed `"${kind.param}:$id"` (see Library.starMarks). */
val LocalStarMarks = staticCompositionLocalOf<Map<String, Boolean>> { emptyMap() }

/**
 * Star state as the screen should show it: this session's change wins over the snapshot the list was
 * painted with. Every toggle must also act on this, not on the snapshot, or the second tap undoes
 * the first one's server call instead of flipping what is on screen.
 */
fun Map<String, Boolean>.effectiveStar(kind: dev.flint.music.data.StarKind, id: String, snapshot: Boolean): Boolean =
    get("${kind.param}:$id") ?: snapshot

/** The page wash: the cover's own bottom colour at the top, easing into the page colour. */
fun pageBrush(palette: PagePalette, endY: Float, startY: Float = 0f): Brush = Brush.verticalGradient(
    0f to palette.edge,
    0.18f to blend(palette.edge, palette.background, 0.55f),
    0.42f to blend(palette.edge, palette.background, 0.88f),
    1f to palette.background,
    startY = startY, endY = endY,
)

/**
 * The player's page: the cover itself, sixteen-times enlarged and smoothed, so the colours vary the way
 * the record's do instead of settling into one average - which is what Apple Music does and why their
 * page matches the sleeve so exactly. The seam goes over the top of it, so the artwork still runs out
 * rather than stopping.
 *
 * The player only. An album page has a list scrolling over it and an artwork that fades out under the
 * parallax, and every edge those give the wash is one more thing for it to disagree with: the cover
 * squashed into the header, a line where the block ended, a slab of seam colour left behind when the
 * picture above it faded. It keeps `pageBrush`, which has none of those problems.
 *
 * Static: one 4 kB texture per cover, uploaded once, drawn as one quad. Neither fill covers the whole
 * page - the seam is opaque down to [SEAM_OPAQUE] and gone by [SEAM_END], so each is clipped to where
 * it shows.
 */
/**
 * The player's page, aligned to its sleeve. The cover is drawn at the sleeve's own scale behind it, so
 * the blurred copy and the sharp one are the same picture at the same size; above and below, the
 * texture's first and last rows carry on. That last part is the whole point: the sleeve's bottom edge
 * then meets a wash made of the sleeve's own bottom rows, in the same colours, and the picture runs out
 * into the page instead of ending on one. Stretched over the screen instead, the wash showed the middle
 * of the cover where the sleeve ended, and the hue jumped across a line.
 */
fun DrawScope.drawSleeveWash(palette: PagePalette, sleeveBottom: Float, sleeveHeight: Float, endY: Float) {
    val wash = palette.wash
    if (wash == null) {
        drawRect(palette.background, size = Size(size.width, endY))
        return
    }
    val w = size.width.toInt().coerceAtLeast(1)
    val bottom = sleeveBottom.coerceIn(1f, endY)
    val top = (bottom - sleeveHeight).coerceAtLeast(0f)
    // Rounded edges rather than rounded heights, so the three bands abut exactly with no row of page
    // colour showing between them.
    fun band(srcY: Int, srcH: Int, y0: Int, y1: Int) {
        if (y1 <= y0) return
        drawImage(
            wash,
            srcOffset = IntOffset(0, srcY), srcSize = IntSize(WASH_ROWS, srcH),
            dstOffset = IntOffset(0, y0), dstSize = IntSize(w, y1 - y0),
            filterQuality = FilterQuality.Low,
        )
    }
    band(0, 1, 0, top.toInt())
    band(0, WASH_ROWS, top.toInt(), bottom.toInt())
    band(WASH_ROWS - 1, 1, bottom.toInt(), endY.toInt())
}

/**
 * The bottom of the player's sleeve, cross-faded into its own blur. Drawn inside the sleeve's own
 * bounds, in slices: slice by slice this is the same picture at the same scale as [drawSleeveWash]
 * draws behind and below it, so the sharp cover simply goes soft and then carries on down the page.
 *
 * Nowhere in it is there a flat colour. That is the point - every version of this that faded the
 * picture onto some computed colour left that colour meeting the page along a dead straight line,
 * which is the one thing the eye always finds.
 */
fun DrawScope.drawSleeveMelt(palette: PagePalette, fraction: Float) {
    val top = size.height * (1f - fraction)
    val wash = palette.wash
    if (wash == null) {
        drawRect(
            Brush.verticalGradient(
                0f to Color.Transparent, 1f to palette.background,
                startY = top, endY = size.height,
            ),
            topLeft = Offset(0f, top), size = Size(size.width, size.height - top),
        )
        return
    }
    val steps = 28
    val band = (size.height - top) / steps
    val w = size.width.toInt().coerceAtLeast(1)
    val last = size.height.toInt()
    for (i in 0 until steps) {
        // Rounded edges, not a rounded height: a band of 14.2 px drawn as 14 leaves a fifth of a pixel
        // behind every time, and by the last slice that is six rows of raw, unmelted cover sitting
        // across the bottom of the sleeve - a bright hairline, which is exactly the edge this is here
        // to remove. The final slice is pinned to the sleeve's own bottom.
        val y0 = (top + band * i).toInt()
        val y1 = if (i == steps - 1) last else (top + band * (i + 1)).toInt()
        if (y1 <= y0) continue
        val v0 = (y0 / size.height * WASH_ROWS).toInt().coerceIn(0, WASH_ROWS - 1)
        val v1 = ((y1 / size.height) * WASH_ROWS).toInt().coerceIn(v0 + 1, WASH_ROWS)
        val t = (i + 1f) / steps
        drawImage(
            wash,
            srcOffset = IntOffset(0, v0), srcSize = IntSize(WASH_ROWS, v1 - v0),
            dstOffset = IntOffset(0, y0), dstSize = IntSize(w, y1 - y0),
            alpha = t * t,
            filterQuality = FilterQuality.Low,
        )
    }
}

fun DrawScope.drawPageWash(palette: PagePalette, endY: Float, seam: Boolean = true) {
    val wash = palette.wash
    if (wash == null) {
        drawRect(pageBrush(palette, endY))
        return
    }
    // Without a seam over it the whole texture is drawn, top rows included: the player's sleeve starts
    // a little below the top of the screen, and what shows above it is the same cover, softened.
    val from = if (seam) (WASH_ROWS * SEAM_OPAQUE).toInt() else 0
    val top = (endY * from / WASH_ROWS).toInt()
    drawImage(
        wash,
        srcOffset = IntOffset(0, from), srcSize = IntSize(WASH_ROWS, WASH_ROWS - from),
        dstOffset = IntOffset(0, top),
        dstSize = IntSize(size.width.toInt().coerceAtLeast(1), (endY.toInt() - top).coerceAtLeast(1)),
        filterQuality = FilterQuality.Low,
    )
    if (!seam) return
    // Same stops as pageBrush, but ending transparent: what is under it is the wash, not a flat colour.
    drawRect(
        Brush.verticalGradient(
            0f to palette.edge,
            0.18f to blend(palette.edge, palette.background, 0.55f),
            0.42f to blend(palette.edge, palette.background, 0.88f),
            0.66f to palette.background.copy(alpha = 0f),
            1f to palette.background.copy(alpha = 0f),
            startY = 0f, endY = endY,
        ),
        size = Size(size.width, endY * SEAM_END),
    )
}

/** Matches `CoverColors.WASH`: the wash texture is this many pixels a side. */
private const val WASH_ROWS = 32
/** Where the seam stops being opaque: the player's artwork covers everything above this. */
private const val SEAM_OPAQUE = 0.42f
private const val SEAM_END = 0.68f

/**
 * The player's wash: the same colours, but dimmed at the top, because there the artwork is a card in
 * the middle of the screen rather than the ceiling of the page.
 */
fun playerBrush(palette: PagePalette, endY: Float): Brush = Brush.verticalGradient(
    0f to blend(palette.edge, palette.background, 0.30f),
    0.45f to blend(palette.edge, palette.background, 0.72f),
    1f to palette.background,
    startY = 0f, endY = endY,
)

/** Mixes two opaque colours; cheaper and clearer at call sites than compositing an alpha layer. */
fun blend(a: Color, b: Color, t: Float): Color = Color(
    a.red + (b.red - a.red) * t, a.green + (b.green - a.green) * t, a.blue + (b.blue - a.blue) * t, 1f,
)

/** An opaque version of a translucent colour over a known background, so no layer is needed to draw it. */
fun Color.over(background: Color): Color = blend(background, copy(alpha = 1f), alpha)

/**
 * A hairline in the Apple sense: a dim line that starts where the text starts and never reaches the
 * right edge of the screen. Drawn, not laid out, so a long list does not pay for a divider composable.
 */
@Composable
fun Hairline(startIndent: Dp = Space.gutter) {
    val color = LocalContentColor.current.copy(alpha = 0.10f)
    Box(Modifier.fillMaxWidth().padding(start = startIndent).height(1.dp).background(color))
}

/** A section heading: big, bold, sitting on the gutter, with an optional action on the right. */
@Composable
fun SectionHeader(title: String, modifier: Modifier = Modifier, action: @Composable (RowScope.() -> Unit)? = null) {
    Row(
        modifier.fillMaxWidth().padding(start = Space.gutter, end = Space.tight, top = Space.section - 12.dp, bottom = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            title, Modifier.weight(1f),
            style = MaterialTheme.typography.titleLarge.copy(fontSize = 20.sp, fontWeight = FontWeight.SemiBold),
            maxLines = 1, overflow = TextOverflow.Ellipsis,
        )
        action?.invoke(this)
    }
}

/** The name of a whole screen, the size Apple sets it: big, bold, sitting on the gutter. */
@Composable
fun LargeTitle(text: String, modifier: Modifier = Modifier, trailing: @Composable (RowScope.() -> Unit)? = null) {
    Row(
        modifier.fillMaxWidth().padding(start = Space.gutter, end = Space.tight, top = 8.dp, bottom = 2.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(
            text, Modifier.weight(1f),
            style = MaterialTheme.typography.headlineMedium.copy(fontSize = 30.sp),
            maxLines = 1, overflow = TextOverflow.Ellipsis,
        )
        trailing?.invoke(this)
    }
}

/** Small grey capitals under a title: "2019 · ALTERNATIVE · LOSSLESS". */
@Composable
fun Caption(text: String, modifier: Modifier = Modifier, align: TextAlign = TextAlign.Start, caps: Boolean = true) {
    if (text.isEmpty()) return
    Text(
        if (caps) text.uppercase() else text, modifier, style = MaterialTheme.typography.labelSmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = align, maxLines = 2, overflow = TextOverflow.Ellipsis,
    )
}

/**
 * The button the page is built around: a full pill, tinted with the page's accent. [prominent] is the
 * one the eye should land on (Play); the others sit on a translucent version of the same colour.
 */
@Composable
fun PillButton(
    text: String, icon: ImageVector?, onClick: () -> Unit, modifier: Modifier = Modifier,
    prominent: Boolean = false, enabled: Boolean = true,
) {
    val scheme = MaterialTheme.colorScheme
    val container = if (prominent) scheme.primary else scheme.onSurface.copy(alpha = 0.12f).over(scheme.background)
    val content = if (prominent) scheme.onPrimary else scheme.primary
    Surface(
        onClick = onClick, enabled = enabled, shape = PillShape, color = container, contentColor = content,
        modifier = modifier.heightIn(min = 42.dp),
    ) {
        Row(Modifier.padding(horizontal = 16.dp), Arrangement.Center, Alignment.CenterVertically) {
            if (icon != null) Icon(icon, null, Modifier.size(18.dp))
            Text(
                text, Modifier.padding(start = if (icon != null) 7.dp else 0.dp),
                style = MaterialTheme.typography.titleSmall.copy(fontSize = 15.sp), maxLines = 1,
            )
        }
    }
}


/**
 * Dresses everything inside in the colours of one cover: the page colour becomes the surface, the
 * cover's accent becomes the primary, and text colours are chosen to read on it. Screens keep using
 * `MaterialTheme.colorScheme`, so nothing below needs to know where the colours came from.
 */
@Composable
fun TintedTheme(palette: PagePalette?, content: @Composable () -> Unit) {
    val base = MaterialTheme.colorScheme
    val scheme = androidx.compose.runtime.remember(palette, base) {
        palette?.let {
            base.copy(
                background = it.background, surface = it.background,
                onBackground = it.onBackground, onSurface = it.onBackground, onSurfaceVariant = it.onBackgroundVariant,
                surfaceVariant = it.onBackground.copy(alpha = 0.10f).over(it.background),
                surfaceContainer = it.onBackground.copy(alpha = 0.07f).over(it.background),
                surfaceContainerHigh = it.onBackground.copy(alpha = 0.11f).over(it.background),
                primary = it.accent, onPrimary = if (it.accent.luminance() < 0.5f) Color.White else Color(0xFF0D0D0D),
                secondaryContainer = it.onBackground.copy(alpha = 0.14f).over(it.background), onSecondaryContainer = it.onBackground,
                outlineVariant = it.onBackground.copy(alpha = 0.14f).over(it.background),
            )
        } ?: base
    }
    MaterialTheme(colorScheme = scheme) {
        androidx.compose.runtime.CompositionLocalProvider(
            LocalContentColor provides scheme.onSurface,
            LocalPalette provides palette,
        ) { content() }
    }
}

/** Status bar icons follow the colour of the page they sit on, and go back to the app's when it leaves. */
@Composable
fun SystemBarIcons(background: Color) {
    val view = androidx.compose.ui.platform.LocalView.current
    androidx.compose.runtime.DisposableEffect(background) {
        val window = (view.context as? android.app.Activity)?.window
        val controller = window?.let { androidx.core.view.WindowCompat.getInsetsController(it, view) }
        val before = controller?.isAppearanceLightStatusBars
        controller?.isAppearanceLightStatusBars = background.luminance() > 0.5f
        onDispose { if (before != null) controller.isAppearanceLightStatusBars = before }
    }
}

/** A round, dimmed button that stays legible on top of artwork: back, close, more. */
@Composable
fun ScrimIconButton(icon: ImageVector, description: String, onClick: () -> Unit, modifier: Modifier = Modifier) {
    androidx.compose.material3.IconButton(
        onClick,
        modifier.size(40.dp).background(Color.Black.copy(alpha = 0.35f), androidx.compose.foundation.shape.CircleShape),
    ) { Icon(icon, description, Modifier.size(22.dp), tint = Color.White) }
}

/**
 * A search or filter field as a soft rounded capsule rather than an outlined box: one tinted surface,
 * an icon, and a clear button that only exists when there is something to clear.
 */
@Composable
fun SearchField(
    value: String, onValue: (String) -> Unit, placeholder: String, modifier: Modifier = Modifier,
    testTag: String? = null, autofocus: Boolean = false, focusKey: Any = Unit,
) {
    val scheme = MaterialTheme.colorScheme
    // Asked for by hand: opening search focuses the field so the keyboard is already there.
    val focus = remember(autofocus) { if (autofocus) FocusRequester() else null }
    val keyboard = androidx.compose.ui.platform.LocalSoftwareKeyboardController.current
    LaunchedEffect(focus, focusKey) {
        if (focus == null) return@LaunchedEffect
        // The field's node is not attached on the frame this first runs, and requestFocus on an
        // unattached one throws and leaves the screen with no keyboard at all - which is what tapping
        // Search used to do about half the time. Ask again for a few frames until it takes.
        repeat(12) {
            androidx.compose.runtime.withFrameNanos {}
            if (runCatching { focus.requestFocus() }.isSuccess) {
                // Focus alone does not always raise the keyboard when it lands before the window is
                // ready; ask for it explicitly, which is the whole point of autofocus.
                keyboard?.show()
                return@LaunchedEffect
            }
        }
    }
    Surface(
        shape = PillShape, color = scheme.onSurface.copy(alpha = 0.08f).over(scheme.background),
        contentColor = scheme.onSurface, modifier = modifier.fillMaxWidth(),
    ) {
        Row(Modifier.padding(horizontal = 14.dp, vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Filled.Search, null, Modifier.size(19.dp), tint = scheme.onSurfaceVariant)
            // The padding belongs to the box, not to the field: with it on the field the placeholder sat
            // at the top of the capsule while the typed text sat in the middle of it.
            Box(Modifier.weight(1f).padding(horizontal = 8.dp, vertical = 11.dp)) {
                if (value.isEmpty()) Text(placeholder, style = MaterialTheme.typography.bodyLarge, color = scheme.onSurfaceVariant)
                androidx.compose.foundation.text.BasicTextField(
                    value, onValue, singleLine = true,
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = scheme.onSurface),
                    cursorBrush = androidx.compose.ui.graphics.SolidColor(scheme.primary),
                    modifier = Modifier.fillMaxWidth()
                        .then(if (testTag != null) Modifier.testTag(testTag) else Modifier)
                        .then(if (focus != null) Modifier.focusRequester(focus) else Modifier),
                )
            }
            if (value.isNotEmpty()) androidx.compose.material3.IconButton({ onValue("") }, Modifier.size(28.dp)) {
                Icon(Icons.Filled.Clear, "Clear", Modifier.size(17.dp), tint = scheme.onSurfaceVariant)
            }
        }
    }
}

/** A choice chip: a pill that fills with the accent when it is the one selected. */
@Composable
fun Chip(label: String, selected: Boolean, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(
        onClick = onClick, shape = PillShape,
        color = if (selected) scheme.primary else scheme.onSurface.copy(alpha = 0.08f).over(scheme.background),
        contentColor = if (selected) scheme.onPrimary else scheme.onSurface,
        modifier = modifier,
    ) {
        Text(
            label, Modifier.padding(horizontal = 14.dp, vertical = 8.dp),
            style = MaterialTheme.typography.labelLarge.copy(fontSize = 13.5f.sp), maxLines = 1,
        )
    }
}

/**
 * A slider drawn rather than assembled: a rounded track, a fill and a dot. Material's Slider brings a
 * ripple, a state layer and a value label, which on a screen of ten equalizer bands reads as ten
 * widgets instead of one curve - and costs a layer each. [centred] fills outwards from zero, which is
 * what a gain control should look like.
 */
@Composable
fun FlintSlider(
    value: Float,
    range: ClosedFloatingPointRange<Float>,
    onChange: (Float) -> Unit,
    modifier: Modifier = Modifier,
    enabled: Boolean = true,
    centred: Boolean = false,
) {
    val scheme = MaterialTheme.colorScheme
    val track = scheme.onSurface.copy(alpha = if (enabled) 0.16f else 0.07f)
    val fill = if (enabled) scheme.primary else scheme.onSurface.copy(alpha = 0.25f)
    val knob = if (enabled) scheme.onSurface else scheme.onSurface.copy(alpha = 0.4f)
    val span = (range.endInclusive - range.start).takeIf { it > 0f } ?: 1f
    val fraction = ((value - range.start) / span).coerceIn(0f, 1f)
    val pick: (Float, Float) -> Unit = { x, w -> onChange(range.start + (x / w).coerceIn(0f, 1f) * span) }
    Box(
        modifier.fillMaxWidth().height(34.dp)
            .pointerInput(enabled, range) {
                if (!enabled) return@pointerInput
                detectHorizontalDragGestures(
                    onDragStart = { pick(it.x, size.width.toFloat()) },
                ) { change, _ -> pick(change.position.x, size.width.toFloat()) }
            }
            .pointerInput(enabled, range) {
                if (!enabled) return@pointerInput
                detectTapGestures { pick(it.x, size.width.toFloat()) }
            }
            .drawBehind {
                val h = 6.dp.toPx()
                val y = (size.height - h) / 2f
                val radius = androidx.compose.ui.geometry.CornerRadius(h / 2f, h / 2f)
                drawRoundRect(track, androidx.compose.ui.geometry.Offset(0f, y), androidx.compose.ui.geometry.Size(size.width, h), radius)
                val from = if (centred) size.width * ((0f - range.start) / span).coerceIn(0f, 1f) else 0f
                val to = size.width * fraction
                drawRoundRect(
                    fill, androidx.compose.ui.geometry.Offset(minOf(from, to), y),
                    androidx.compose.ui.geometry.Size(kotlin.math.abs(to - from), h), radius,
                )
                drawCircle(knob, h * 1.45f, androidx.compose.ui.geometry.Offset(to, size.height / 2f))
            },
    )
}

/**
 * A form field with the app's corners and no hard outline: a soft filled capsule-ish box, the way a
 * settings form looks on iOS. It keeps Material's text field underneath, so labels, password masking
 * and keyboard options all behave exactly as before - only the frame changes.
 */
@Composable
fun FormField(
    value: String,
    onValueChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    label: @Composable (() -> Unit)? = null,
    placeholder: @Composable (() -> Unit)? = null,
    supportingText: @Composable (() -> Unit)? = null,
    singleLine: Boolean = false,
    minLines: Int = 1,
    visualTransformation: androidx.compose.ui.text.input.VisualTransformation = androidx.compose.ui.text.input.VisualTransformation.None,
    keyboardOptions: androidx.compose.foundation.text.KeyboardOptions = androidx.compose.foundation.text.KeyboardOptions.Default,
) {
    val scheme = MaterialTheme.colorScheme
    val filled = scheme.onSurface.copy(alpha = 0.07f).over(scheme.background)
    androidx.compose.material3.OutlinedTextField(
        value, onValueChange, modifier, label = label, placeholder = placeholder, supportingText = supportingText,
        singleLine = singleLine, minLines = minLines,
        visualTransformation = visualTransformation, keyboardOptions = keyboardOptions,
        shape = CardShape,
        colors = androidx.compose.material3.OutlinedTextFieldDefaults.colors(
            focusedContainerColor = filled, unfocusedContainerColor = filled,
            focusedBorderColor = scheme.primary.copy(alpha = 0.6f),
            unfocusedBorderColor = Color.Transparent,
        ),
    )
}

/**
 * The list row every browsing screen shares: something on the left, a title (and maybe a second line),
 * a value or a chevron on the right, and a hairline that starts where the text does. Genres, decades,
 * folders, playlists and stations all used to draw their own row, each with its own padding, which is
 * what made the library feel like several apps stitched together.
 */
@Composable
fun NavRow(
    title: String,
    onClick: () -> Unit,
    modifier: Modifier = Modifier,
    subtitle: String? = null,
    trailing: String? = null,
    leading: (@Composable () -> Unit)? = null,
    chevron: Boolean = false,
    divider: Boolean = true,
    action: (@Composable RowScope.() -> Unit)? = null,
) {
    val scheme = MaterialTheme.colorScheme
    Column(modifier.fillMaxWidth()) {
        Row(
            Modifier.fillMaxWidth().clickable(onClick = onClick)
                .padding(start = Space.gutter, end = if (action != null) 4.dp else Space.gutter, top = 11.dp, bottom = 11.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (leading != null) { leading(); Spacer(Modifier.size(12.dp)) }
            Column(Modifier.weight(1f)) {
                Text(title, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
                if (!subtitle.isNullOrEmpty()) Text(
                    subtitle, style = MaterialTheme.typography.bodySmall, color = scheme.onSurfaceVariant,
                    maxLines = 1, overflow = TextOverflow.Ellipsis,
                )
            }
            if (!trailing.isNullOrEmpty()) Text(trailing, style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
            action?.invoke(this)
            if (chevron) Icon(
                Icons.AutoMirrored.Filled.KeyboardArrowRight, null,
                Modifier.padding(start = 6.dp).size(19.dp), tint = scheme.onSurfaceVariant.copy(alpha = 0.7f),
            )
        }
        if (divider) Hairline(startIndent = Space.gutter + (if (leading != null) 60.dp else 0.dp))
    }
}

/** A row that does something rather than going somewhere: "New playlist", "Import M3U…". */
@Composable
fun ActionRow(title: String, icon: ImageVector, onClick: () -> Unit, divider: Boolean = true) {
    Column {
        Row(
            Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = Space.gutter, vertical = 13.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Icon(icon, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.primary)
            Text(
                title, Modifier.padding(start = 12.dp),
                style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.primary,
            )
        }
        if (divider) Hairline(startIndent = Space.gutter + 32.dp)
    }
}

/** A round, softly filled button: the small actions either side of a page's Play pill. */
@Composable
fun CircleButton(icon: ImageVector, description: String, modifier: Modifier = Modifier, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(
        onClick = onClick, shape = androidx.compose.foundation.shape.CircleShape,
        color = scheme.onSurface.copy(alpha = 0.12f).over(scheme.background), contentColor = scheme.primary,
        modifier = modifier.size(46.dp),
    ) { Box(Modifier.fillMaxSize(), Alignment.Center) { Icon(icon, description, Modifier.size(20.dp)) } }
}

/** The circle that holds whatever did not fit beside a page's Play button. */
@Composable
fun MoreCircle(items: List<Pair<String, () -> Unit>>, modifier: Modifier = Modifier) {
    val open = androidx.compose.runtime.remember { androidx.compose.runtime.mutableStateOf(false) }
    Box(modifier) {
        CircleButton(Icons.Filled.MoreHoriz, "More") { open.value = true }
        androidx.compose.material3.DropdownMenu(open.value, { open.value = false }) {
            items.forEach { (label, action) ->
                androidx.compose.material3.DropdownMenuItem({ Text(label) }, { action(); open.value = false })
            }
        }
    }
}


/**
 * Whether movement should be kept to a minimum: the app's own switch, or the system's animations being
 * turned off (Developer options, or the accessibility setting some people rely on). Read it rather than
 * hard-coding durations, so "reduce motion" means the same thing everywhere.
 */
@Composable
fun reduceMotion(): Boolean {
    val prefs by (androidx.lifecycle.viewmodel.compose.viewModel<dev.flint.music.app.vm.SettingsViewModel>()).prefs.collectAsStateWithLifecycle()
    val context = androidx.compose.ui.platform.LocalContext.current
    val systemOff = androidx.compose.runtime.remember {
        android.provider.Settings.Global.getFloat(context.contentResolver, android.provider.Settings.Global.ANIMATOR_DURATION_SCALE, 1f) == 0f
    }
    return prefs.reduceMotion || systemOff
}
