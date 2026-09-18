package dev.flint.music.app.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Clear
import androidx.compose.material.icons.filled.Search
import androidx.compose.material3.Icon
import androidx.compose.material3.LocalContentColor
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.Typography
import androidx.compose.runtime.Composable
import androidx.compose.runtime.staticCompositionLocalOf
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp

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
    val section = 28.dp
}

val CoverShape = RoundedCornerShape(Radius.cover)
val CardShape = RoundedCornerShape(Radius.card)
val TileShape = RoundedCornerShape(Radius.tile)
val PillShape = RoundedCornerShape(Radius.pill)

/**
 * Tighter and heavier than stock Material, which is what makes a music app read as a music app:
 * album and screen titles are display-weight, list rows are plain text, and captions are small caps
 * grey. Tracking is negative on the big sizes, the way a photographed cover needs.
 */
val FlintTypography = Typography().run {
    copy(
        displaySmall = displaySmall.copy(fontWeight = FontWeight.Bold, letterSpacing = (-1).sp),
        headlineLarge = headlineLarge.copy(fontWeight = FontWeight.Bold, letterSpacing = (-0.8).sp),
        headlineMedium = headlineMedium.copy(fontWeight = FontWeight.Bold, letterSpacing = (-0.6).sp),
        headlineSmall = headlineSmall.copy(fontWeight = FontWeight.Bold, letterSpacing = (-0.4).sp),
        titleLarge = titleLarge.copy(fontWeight = FontWeight.Bold, letterSpacing = (-0.3).sp),
        titleMedium = titleMedium.copy(fontWeight = FontWeight.SemiBold),
        titleSmall = titleSmall.copy(fontWeight = FontWeight.SemiBold),
        bodyLarge = bodyLarge.copy(fontSize = 16.sp, lineHeight = 21.sp),
        bodyMedium = bodyMedium.copy(fontSize = 15.sp, lineHeight = 20.sp),
        bodySmall = bodySmall.copy(fontSize = 13.sp, lineHeight = 17.sp),
        labelSmall = labelSmall.copy(fontWeight = FontWeight.Medium, letterSpacing = 0.6.sp),
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
)

val LocalPalette = staticCompositionLocalOf<PagePalette?> { null }

/** The page wash: the cover's own bottom colour at the top, easing into the page colour. */
fun pageBrush(palette: PagePalette, endY: Float): Brush = Brush.verticalGradient(
    0f to palette.edge,
    0.18f to blend(palette.edge, palette.background, 0.55f),
    0.42f to blend(palette.edge, palette.background, 0.88f),
    1f to palette.background,
    startY = 0f, endY = endY,
)

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
        Text(title, Modifier.weight(1f), style = MaterialTheme.typography.titleLarge, maxLines = 1, overflow = TextOverflow.Ellipsis)
        action?.invoke(this)
    }
}

/** The name of a whole screen, the size Apple sets it: big, bold, sitting on the gutter. */
@Composable
fun LargeTitle(text: String, modifier: Modifier = Modifier, trailing: @Composable (RowScope.() -> Unit)? = null) {
    Row(
        modifier.fillMaxWidth().padding(start = Space.gutter, end = Space.tight, top = 10.dp, bottom = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Text(text, Modifier.weight(1f), style = MaterialTheme.typography.headlineMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        trailing?.invoke(this)
    }
}

/** Small grey capitals under a title: "2019 · ALTERNATIVE · LOSSLESS". */
@Composable
fun Caption(text: String, modifier: Modifier = Modifier, align: TextAlign = TextAlign.Start) {
    if (text.isEmpty()) return
    Text(
        text.uppercase(), modifier, style = MaterialTheme.typography.labelSmall,
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
        modifier = modifier.heightIn(min = 46.dp),
    ) {
        Row(Modifier.padding(horizontal = 18.dp), Arrangement.Center, Alignment.CenterVertically) {
            if (icon != null) Icon(icon, null, Modifier.size(19.dp))
            Text(
                text, Modifier.padding(start = if (icon != null) 7.dp else 0.dp),
                style = MaterialTheme.typography.titleSmall, maxLines = 1,
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
        modifier.size(38.dp).background(Color.Black.copy(alpha = 0.32f), androidx.compose.foundation.shape.CircleShape),
    ) { Icon(icon, description, Modifier.size(21.dp), tint = Color.White) }
}

/**
 * A search or filter field as a soft rounded capsule rather than an outlined box: one tinted surface,
 * an icon, and a clear button that only exists when there is something to clear.
 */
@Composable
fun SearchField(
    value: String, onValue: (String) -> Unit, placeholder: String, modifier: Modifier = Modifier,
    testTag: String? = null,
) {
    val scheme = MaterialTheme.colorScheme
    Surface(shape = PillShape, color = scheme.onSurface.copy(alpha = 0.08f).over(scheme.background), modifier = modifier.fillMaxWidth()) {
        Row(Modifier.padding(horizontal = 14.dp, vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
            Icon(Icons.Filled.Search, null, Modifier.size(19.dp), tint = scheme.onSurfaceVariant)
            Box(Modifier.weight(1f).padding(horizontal = 8.dp)) {
                if (value.isEmpty()) Text(placeholder, style = MaterialTheme.typography.bodyLarge, color = scheme.onSurfaceVariant)
                androidx.compose.foundation.text.BasicTextField(
                    value, onValue, singleLine = true,
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = scheme.onSurface),
                    cursorBrush = androidx.compose.ui.graphics.SolidColor(scheme.primary),
                    modifier = Modifier.fillMaxWidth().padding(vertical = 12.dp)
                        .then(if (testTag != null) Modifier.testTag(testTag) else Modifier),
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
        Text(label, Modifier.padding(horizontal = 15.dp, vertical = 9.dp), style = MaterialTheme.typography.labelLarge, maxLines = 1)
    }
}
