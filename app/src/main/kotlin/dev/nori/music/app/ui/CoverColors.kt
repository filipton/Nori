package dev.nori.music.app.ui

import android.graphics.Bitmap
import android.util.LruCache
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.luminance
import androidx.compose.ui.graphics.toArgb
import androidx.core.graphics.scale
import androidx.compose.ui.platform.LocalContext
import androidx.core.graphics.ColorUtils
import androidx.palette.graphics.Palette
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowHardware
import coil3.toBitmap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * Picked once per cover (a 160 px copy, which the list screens have usually loaded already) and kept
 * in memory, so reopening a page costs nothing. Palette runs off the main thread on ~25k pixels.
 */
private object CoverPalette {
    val cache = LruCache<String, PagePalette>(128)

    /**
     * One lock per cover, so a picture the bar, the player and the warm-up all ask for at once is
     * measured once and the others wait for it. Bounded: an evicted lock costs one repeated
     * measurement, which is the behaviour this replaces.
     */
    private val locks = LruCache<String, kotlinx.coroutines.sync.Mutex>(64)

    suspend fun <T> once(key: String, block: suspend () -> T): T =
        synchronized(locks) { locks.get(key) ?: kotlinx.coroutines.sync.Mutex().also { locks.put(key, it) } }.withLock { block() }
}

private fun paletteKey(url: String, dark: Boolean, amoled: Boolean) = "$url|$dark|$amoled"

/**
 * Fetches the cover and works its colours out, unless that has been done before. Slow the first time
 * (a decode the picture's own cache cannot serve, because the colours have to be read back off the
 * bitmap) and a map lookup every time after.
 */
private suspend fun paletteOf(context: android.content.Context, url: String, dark: Boolean, amoled: Boolean): PagePalette? {
    val key = paletteKey(url, dark, amoled)
    CoverPalette.cache.get(key)?.let { return it }
    // One cover is only ever worked out once, even when the bar, the player and the warm-up below all
    // ask for it in the same frame: the others wait here and then find it in the cache.
    return CoverPalette.once(key) {
        CoverPalette.cache.get(key) ?: withContext(Dispatchers.Default) {
            val result = SingletonImageLoader.get(context).execute(
                ImageRequest.Builder(context).data(url).size(CoverSize.ROW).allowHardware(false).build(),
            ) as? SuccessResult ?: return@withContext null
            derive(result.image.toBitmap(), dark, amoled)
        }?.also { CoverPalette.cache.put(key, it) }
    }
}

/**
 * Works a cover's colours out before anything asks for them. The covers either side of what is playing
 * are fetched ahead (PlayerViewModel) so a skip lands on a picture that is already there, but the page's
 * colour was still being worked out after the fact, which is the beat the page spent wearing the last
 * song's colour. Done here, the cross-fade starts with the song.
 */
suspend fun warmCoverPalette(context: android.content.Context, url: String?, dark: Boolean, amoled: Boolean) {
    if (url == null || CoverPalette.cache.get(paletteKey(url, dark, amoled)) != null) return
    runCatching { paletteOf(context, url, dark, amoled) }
}

/**
 * A cover's colours together with the cover they came from. The two are handed out as one because the
 * colours arrive a frame or two after the song does, and a caller that cannot tell whose colours it is
 * holding treats the last song's as the new one's.
 */
data class CoverTint(val url: String?, val palette: PagePalette?)

@Composable
fun rememberCoverTint(url: String?, dark: Boolean, amoled: Boolean): CoverTint {
    val context = LocalContext.current
    val key = url?.let { paletteKey(it, dark, amoled) }
    // Straight out of the cache, here in composition. It used to come from `produceState`, whose block
    // is a coroutine that runs *after* the frame the cover changed on: a cover measured long ago still
    // arrived a frame or two late, and until it did, everything drawn from it - the page, the soft
    // bottom under the sleeve, the text - was still the last record's. That is the lag that showed as
    // the old colours sitting under a cover that had already changed, most obviously when the record
    // coming in was black.
    val ready = key?.let(CoverPalette.cache::get)
    // Only for a cover nobody has measured yet; reset with the key, so the last song's colours are
    // never handed out for this one.
    var measured by remember(key) { mutableStateOf<PagePalette?>(null) }
    LaunchedEffect(key) {
        if (url == null || ready != null) return@LaunchedEffect
        measured = paletteOf(context, url, dark, amoled)
    }
    return CoverTint(url, ready ?: measured)
}

@Composable
fun rememberCoverPalette(url: String?, dark: Boolean, amoled: Boolean): PagePalette? =
    rememberCoverTint(url, dark, amoled).palette


/**
 * The dark page for a record's dominant colour. It used to be that colour's hue and saturation at a
 * flat lightness of 0.20 whatever the record was, which is why a sleeve that is a field of bright red
 * came out as a dark maroon: the page was always as dark as the darkest record's would have to be.
 *
 * What actually limits it is the writing on it, and different colours reach that limit at very
 * different lightnesses - a blue at 0.34 is dimmer to the eye than a yellow at 0.20. So the page keeps
 * the record's own lightness, and is only taken down as far as the text needs: white has at least
 * eight to one on it, which leaves the album line, the faintest thing on the page at 45 % white, with
 * about three. A record that is darker than that stays darker, so nothing is ever brightened to meet
 * a floor - the Black Album's page is still black.
 */
private const val PAGE_LIGHTEST = 0.34f
private const val PAGE_MAX_LUMA = 0.081f

private fun darkPage(hsl: FloatArray): Color {
    val sat = (hsl[1] * 0.95f).coerceAtMost(0.62f)
    var lo = 0.04f
    var hi = hsl[2].coerceIn(0.04f, PAGE_LIGHTEST)
    fun at(l: Float) = Color(ColorUtils.HSLToColor(floatArrayOf(hsl[0], sat, l)))
    if (at(hi).luminance() <= PAGE_MAX_LUMA) return at(hi)
    // Twelve halvings put it within a thousandth of the brightest this colour may be.
    repeat(12) {
        val mid = (lo + hi) / 2f
        if (at(mid).luminance() <= PAGE_MAX_LUMA) lo = mid else hi = mid
    }
    return at(lo)
}

/**
 * The seam is the whole trick. A page tinted with the cover's *dominant* colour still shows a line
 * where the picture ends, because the bottom of a picture is rarely its dominant colour. So the wash
 * starts from the average of the cover's bottom rows - the exact colour the last pixel row is - and
 * travels from there into a page colour deep or pale enough to carry text.
 */
private fun derive(bitmap: Bitmap, dark: Boolean, amoled: Boolean): PagePalette {
    val edgeRaw = Color(bottomAverage(bitmap))
    val p = Palette.from(bitmap).maximumColorCount(16).generate()
    val body = dominant(bitmap, edgeRaw.toArgb())
    val bodyHsl = FloatArray(3).also { ColorUtils.colorToHSL(body, it) }
    // Paper / ink: the sleeve is itself. Do not let Palette's "vibrant" JPEG fringe invent pink.
    val paper = bodyHsl[2] > 0.85f || (bodyHsl[1] < 0.10f && bodyHsl[2] > 0.72f)
    val ink = bodyHsl[2] < 0.10f && bodyHsl[1] < 0.18f
    val inkOrPaper = paper || ink || bodyHsl[1] < 0.12f
    val accentSeed = if (inkOrPaper) body
        else (p.vibrantSwatch ?: p.lightVibrantSwatch ?: p.lightMutedSwatch ?: p.dominantSwatch)?.rgb ?: body
    // White sleeves keep a white page even in dark mode: cover colours mean the page follows the
    // record, and paper is white. Charcoal-from-white was the coward's contrast fix and looked wrong.
    // AMOLED still stays black - lighting those pixels would break the promise.
    val edge = when {
        paper -> Color(ColorUtils.blendARGB(edgeRaw.toArgb(), 0xFFF7F7F7.toInt(), 0.75f))
        ink -> Color(ColorUtils.blendARGB(edgeRaw.toArgb(), 0xFF0A0A0A.toInt(), 0.70f))
        else -> edgeRaw
    }
    val hsl = bodyHsl
    val background = when {
        dark && amoled -> Color.Black
        paper -> Color(0xFFF7F7F7)
        dark && ink -> Color(0xFF0A0A0A)
        dark -> darkPage(hsl)
        ink -> Color(0xFFECECEC)
        else -> Color(ColorUtils.HSLToColor(floatArrayOf(hsl[0], (hsl[1] * 0.55f).coerceAtMost(0.4f), hsl[2].coerceIn(0.90f, 0.96f))))
    }
    val on = if (background.luminance() < 0.4f) Color.White else Color(0xFF0D0D0D)
    // On paper, a near-white accentSeed is useless: nudge to a readable ink grey rather than
    // saturating a phantom hue.
    val accent = when {
        paper -> Color(0xFF2A2A2A)
        ink && !dark -> Color(0xFF2A2A2A)
        else -> readable(Color(accentSeed), background, on)
    }
    val wash = if (amoled && dark) null else runCatching { washOf(bitmap, background, dark, paper = paper, ink = ink) }.getOrNull()
    return PagePalette(
        edge, background, on, on.copy(alpha = 0.66f), accent,
        wash = wash?.first, washEdge = wash?.second ?: edge,
    )
}

/**
 * How many pixels a side the wash is kept at. Sixteen held the colour but no shape at all, so the page
 * read as a plain field and the sleeve looked like it stopped dead; Apple's blur still has the record's
 * forms in it, which is what makes the picture seem to carry on behind the words. Thirty-two costs
 * 4 kB and exactly the same one quad per frame.
 */
private const val WASH = 32

/**
 * How far each pixel of the wash is pulled back towards the flat page colour. Two thirds of the way
 * left the page reading as one flat tint with a hint of movement in it - the owner's words were that
 * the colours "aren't good" and that the page should look like a blurred mirror of the record. It is
 * pulled back much less now, and the extra blur below is what keeps that from turning into patches.
 */
private const val MUTE = 0.38f

/**
 * Apple's player is not painted one flat colour. Measure across their screenshot and the page varies
 * both ways: the background *is* the artwork, enormously enlarged and blurred, which is why it matches
 * the cover so exactly. One average of the cover's bottom rows cannot do that - a sleeve that is black
 * at the bottom and vivid everywhere else turns the whole page to mud.
 *
 * This is the same thing done cheaply: the cover shrunk to [WASH] pixels a side and smoothed once,
 * here, off the main thread, and then drawn stretched over the page, where the GPU's own bilinear
 * filter does the enlarging for free. It costs one 1 kB texture per cover and one quad per frame -
 * no blur shader, no `RenderEffect`, nothing that changes between frames.
 *
 * Every pixel is then pulled to within a hair of the page colour's own lightness and its saturation
 * held back, so the hues vary across the page but the contrast the text needs does not.
 */
private fun washOf(bitmap: Bitmap, background: Color, dark: Boolean, paper: Boolean = false, ink: Boolean = false): Pair<ImageBitmap, Color> {
    val inkOrPaper = paper || ink
    val small = bitmap.scale(WASH, WASH)
    val px = IntArray(WASH * WASH)
    small.getPixels(px, 0, WASH, 0, 0, WASH, WASH)
    if (small !== bitmap) small.recycle()
    // Five passes rather than three. The cover has to become colour and light with no forms left in
    // it at all: at three, a strong shape near the middle of a record still arrived as a shape on the
    // page, and with the colours no longer muted down it would arrive as a bright one.
    repeat(5) { blur(px) }

    val pageHsl = FloatArray(3).also { ColorUtils.colorToHSL(background.toArgb(), it) }
    val hsl = FloatArray(3)
    var meanL = 0f
    for (p in px) { ColorUtils.colorToHSL(p, hsl); meanL += hsl[2] }
    meanL /= px.size
    // How far from the page colour a pixel may stray. Wide enough for the record's own light and dark
    // to show through, narrow enough that white text never lands on a pale patch: at 0.11 the page's
    // own 0.20 lightness reaches 0.31 at its brightest, where white still reads at about five to one.
    // Paper: keep the wash bright and grey - soft white variation, no chromatic bloom.
    val spread = when {
        paper -> 0.04f
        dark -> 0.11f
        else -> 0.055f
    }
    val pull = if (dark && !paper) 1f else 0.85f
    val maxSat = when {
        inkOrPaper -> 0.04f
        dark -> 0.62f
        else -> 0.40f
    }
    val mute = if (inkOrPaper) 0.78f else MUTE
    for (i in px.indices) {
        ColorUtils.colorToHSL(px[i], hsl)
        if (inkOrPaper) {
            hsl[0] = pageHsl[0]
            hsl[1] = 0f
        } else {
            hsl[1] = (hsl[1] * pull).coerceAtMost(maxSat)
        }
        hsl[2] = (pageHsl[2] + (hsl[2] - meanL) * spread * 2.5f).coerceIn(pageHsl[2] - spread, pageHsl[2] + spread).coerceIn(0f, 1f)
        // Then most of the way back to the flat page colour. Clamping the lightness alone was not
        // enough on a record that is many colours at once: one that is teal down one side and warm
        // down the other gave the page teal and warm patches, and a patch reads as a fault where a
        // glow does not. Apple's pages look calm because their covers are mostly a single hue, not
        // because their wash does less - measured, their colour actually varies rather more than
        // ours did. Pulling each pixel back towards the page colour keeps the drift and takes the
        // shouting out of it.
        px[i] = ColorUtils.blendARGB(ColorUtils.HSLToColor(hsl), background.toArgb(), mute)
    }
    // What the soft bottom of the sleeve averages out to, kept with the picture: these are the rows
    // of the page's wash that show through where the records are rubbed out (`rubOutBottom`), so this
    // is the one colour the band as a whole wears. See `PagePalette.meltColour`.
    val first = (WASH * (1f - MELT)).toInt().coerceIn(0, WASH - 1)
    var r = 0f; var g = 0f; var b = 0f
    for (y in first until WASH) for (x in 0 until WASH) {
        val p = px[y * WASH + x]
        r += (p shr 16) and 0xFF; g += (p shr 8) and 0xFF; b += p and 0xFF
    }
    val n = ((WASH - first) * WASH * 255).toFloat()
    return Bitmap.createBitmap(smooth(px), WASH_OUT, WASH_OUT, Bitmap.Config.ARGB_8888).asImageBitmap() to
        Color(r / n, g / n, b / n)
}

/**
 * The size the wash is handed to the GPU at. The colours and shapes are worked out at [WASH], which is
 * plenty for what the page shows; but stretched straight from there over a whole screen, each of its
 * pixels became a visible step - the owner's friend called the blur "stairs" - and the melt at the
 * bottom of the sleeve, which reads the texture a row at a time, stepped as well. Four times finer,
 * filled in smoothly here once per cover, the steps are a few screen pixels tall and gone. It costs a
 * 64 kB texture instead of 4 kB, and still one quad per frame.
 */
private const val WASH_OUT = WASH * 4

/**
 * [WASH] pixels a side up to [WASH_OUT]: bilinear, then two light box passes so the corners the
 * bilinear leaves between samples round off, then a dither of one level either way per channel.
 * The dither is what hides the banding that eight bits give a slow dark gradient - a page that goes
 * from one deep colour to another over a whole screen has only a handful of levels to do it in, and
 * without noise each level is a band with an edge.
 */
private fun smooth(px: IntArray): IntArray {
    val n = WASH_OUT
    val r = FloatArray(n * n); val g = FloatArray(n * n); val b = FloatArray(n * n)
    val scale = (WASH - 1).toFloat() / (n - 1)
    for (y in 0 until n) {
        val fy = y * scale; val y0 = fy.toInt().coerceAtMost(WASH - 2); val ty = fy - y0
        for (x in 0 until n) {
            val fx = x * scale; val x0 = fx.toInt().coerceAtMost(WASH - 2); val tx = fx - x0
            val a = px[y0 * WASH + x0]; val bb = px[y0 * WASH + x0 + 1]
            val c = px[(y0 + 1) * WASH + x0]; val d = px[(y0 + 1) * WASH + x0 + 1]
            fun ch(v: Int, s: Int) = ((v shr s) and 0xFF).toFloat()
            fun lerp(s: Int) = (ch(a, s) * (1 - tx) + ch(bb, s) * tx) * (1 - ty) + (ch(c, s) * (1 - tx) + ch(d, s) * tx) * ty
            val i = y * n + x
            r[i] = lerp(16); g[i] = lerp(8); b[i] = lerp(0)
        }
    }
    repeat(2) { boxBlur(r, n); boxBlur(g, n); boxBlur(b, n) }
    // Fixed seed: the same cover gives the same texture every time, so reopening a page shows exactly
    // what it showed before.
    val noise = java.util.Random(0x5EED)
    return IntArray(n * n) { i ->
        fun q(v: Float) = (v + noise.nextFloat() - 0.5f).toInt().coerceIn(0, 255)
        (0xFF shl 24) or (q(r[i]) shl 16) or (q(g[i]) shl 8) or q(b[i])
    }
}

/** A separable box blur of radius 2 over an n by n channel, in place. */
private fun boxBlur(c: FloatArray, n: Int) {
    val tmp = FloatArray(c.size)
    for (y in 0 until n) for (x in 0 until n) {
        var s = 0f
        for (d in -2..2) s += c[y * n + (x + d).coerceIn(0, n - 1)]
        tmp[y * n + x] = s / 5f
    }
    for (y in 0 until n) for (x in 0 until n) {
        var s = 0f
        for (d in -2..2) s += tmp[(y + d).coerceIn(0, n - 1) * n + x]
        c[y * n + x] = s / 5f
    }
}

/** One separable 3-tap box pass over the tiny wash, so bilinear enlargement has no creases to show. */
private fun blur(px: IntArray) {
    val out = IntArray(px.size)
    for (pass in 0..1) {
        val src = if (pass == 0) px else out
        val dst = if (pass == 0) out else px
        for (y in 0 until WASH) for (x in 0 until WASH) {
            var r = 0; var g = 0; var b = 0
            for (d in -1..1) {
                val i = if (pass == 0) y * WASH + (x + d).coerceIn(0, WASH - 1)
                else (y + d).coerceIn(0, WASH - 1) * WASH + x
                val p = src[i]
                r += (p shr 16) and 0xFF; g += (p shr 8) and 0xFF; b += p and 0xFF
            }
            dst[y * WASH + x] = (0xFF shl 24) or ((r / 3) shl 16) or ((g / 3) shl 8) or (b / 3)
        }
    }
}

/**
 * Hue buckets the histogram counts into, then one for pale greys and whites, then one for black and
 * near-black. Black gets a bucket of its own at full weight: a sleeve that is mostly black is a black
 * record, and its page should be black too. White and pale grey stay discounted - a page that pale
 * would take the controls on it with it.
 */
private const val HUES = 18
private const val NEUTRAL = HUES
private const val DARK = HUES + 1

/**
 * The colour there is most of, which is not the question `Palette.dominantSwatch` answers. Palette
 * quantises to sixteen clusters and discards whole families of colour on the way in: its default
 * filter drops anything within a few points of white or black, and anything in the 10-37 degree band
 * unless it is strongly saturated. On a sleeve that is a field of pale dusty pink with a dark head of
 * hair down one side, the pink is thinned across several clusters and the hair comes through whole, so
 * the "dominant" swatch was the hair and the page took a hue the sleeve barely contains.
 *
 * So count pixels instead. Each sampled pixel votes for its hue in twenty-degree buckets - wide enough
 * that a field of one colour does not split across two of them - the heaviest bucket wins, and the
 * answer is that bucket's own weighted mean, so the page keeps the character of the field and not only
 * its hue. A pixel with almost no saturation votes at a quarter weight, and one so dark or so pale that
 * its hue is guesswork at less again, both into a bucket of their own: a sleeve that really is mostly
 * ink or mostly paper still gets a neutral page, but a white paper flower does not outvote the pink it
 * lies on. Nothing here rewards a colour for standing out - a small vivid mark loses to a large dull
 * field, which is the whole point.
 *
 * Every second row and column of a 160 px copy is ~6k pixels, on the same background pass as Palette
 * and cached with it, so this is paid once per cover.
 */
private fun dominant(bitmap: Bitmap, fallback: Int): Int {
    val w = bitmap.width
    val h = bitmap.height
    if (w <= 0 || h <= 0) return fallback
    val rowStep = (h / 64).coerceAtLeast(1)
    val colStep = (w / 64).coerceAtLeast(1)
    val row = IntArray(w)
    val hsl = FloatArray(3)
    val weight = FloatArray(HUES + 2)
    val sumR = FloatArray(HUES + 2)
    val sumG = FloatArray(HUES + 2)
    val sumB = FloatArray(HUES + 2)
    var paper = 0f
    var ink = 0f
    var total = 0f
    var y = 0
    while (y < h) {
        bitmap.getPixels(row, 0, w, 0, y, w, 1)
        var x = 0
        while (x < w) {
            val px = row[x]
            ColorUtils.colorToHSL(px, hsl)
            total += 1f
            // Dark and colourless - or so dark that any hue it has is noise - is black.
            val black = hsl[2] < 0.06f || (hsl[2] < 0.18f && hsl[1] < 0.25f)
            val white = hsl[2] > 0.88f && hsl[1] < 0.18f
            if (black) ink += 1f
            if (white) paper += 1f
            val bucket = when {
                black -> DARK
                white || hsl[1] < 0.10f -> NEUTRAL
                else -> (hsl[0] / (360f / HUES)).toInt().coerceIn(0, HUES - 1)
            }
            val wt = when (bucket) {
                // Paper and ink are the field on a white or black sleeve: count them at full weight so
                // a speck of JPEG pink cannot outvote the page.
                DARK -> 1f
                NEUTRAL -> if (white) 1f else 0.35f
                else -> 0.25f + 0.75f * (hsl[1] / 0.25f).coerceAtMost(1f)
            }
            weight[bucket] += wt
            sumR[bucket] += wt * ((px shr 16) and 0xFF)
            sumG[bucket] += wt * ((px shr 8) and 0xFF)
            sumB[bucket] += wt * (px and 0xFF)
            x += colStep
        }
        y += rowStep
    }
    // One colour to the eye is several buckets here: a record that is red from crimson through to
    // orange lands in three of them, and each one on its own can lose to the black around it - which is
    // how the Amnesiac sleeve, which is half a field of red, ended up with a black page. A hue is
    // therefore worth what its whole family covers: itself and the twenty degrees either side, at full
    // weight, so what is compared is simply how much of the record each colour takes up. Black and the
    // pale greys stand alone - they are not a hue with a spread, and lending them their neighbours
    // would hand every dark record a black page. A sleeve that really is black still gets one: the
    // Black Album is 98 % black and nothing else comes close.
    val score = FloatArray(HUES + 2)
    for (i in 0 until HUES) {
        score[i] = weight[i] + weight[(i + HUES - 1) % HUES] + weight[(i + 1) % HUES]
    }
    score[NEUTRAL] = weight[NEUTRAL]
    score[DARK] = weight[DARK]
    var best = 0
    for (i in score.indices) if (score[i] > score[best]) best = i
    // Majority paper or ink wins outright: a white cover with a tiny coloured mark is still white.
    if (total > 0f) {
        if (paper / total >= 0.45f) best = NEUTRAL
        if (ink / total >= 0.45f) best = DARK
    }
    val run = if (best < HUES) intArrayOf((best + HUES - 1) % HUES, best, (best + 1) % HUES) else intArrayOf(best)
    val n = run.sumOf { weight[it].toDouble() }.toFloat()
    if (n <= 0f) return fallback
    return (0xFF shl 24) or
        ((run.sumOf { sumR[it].toDouble() } / n).toInt().coerceIn(0, 255) shl 16) or
        ((run.sumOf { sumG[it].toDouble() } / n).toInt().coerceIn(0, 255) shl 8) or
        (run.sumOf { sumB[it].toDouble() } / n).toInt().coerceIn(0, 255)
}

/** The colour of the cover's last rows: what the page has to start from for the picture to melt into it. */
private fun bottomAverage(bitmap: Bitmap): Int {
    val h = bitmap.height
    val w = bitmap.width
    val rows = (h / 12).coerceIn(1, 12)
    val row = IntArray(w)
    var r = 0L; var g = 0L; var b = 0L
    for (y in h - rows until h) {
        bitmap.getPixels(row, 0, w, 0, y, w, 1)
        for (px in row) { r += (px shr 16) and 0xFF; g += (px shr 8) and 0xFF; b += px and 0xFF }
    }
    val n = (w * rows).coerceAtLeast(1)
    return (0xFF shl 24) or ((r / n).toInt() shl 16) or ((g / n).toInt() shl 8) or (b / n).toInt()
}

/** Pushes a colour lighter or darker in its own hue until it has contrast against the page. */
private fun readable(color: Color, background: Color, fallback: Color): Color {
    if (ColorUtils.calculateContrast(color.toArgb(), background.toArgb()) >= 3.2) return color
    val hsl = FloatArray(3).also { ColorUtils.colorToHSL(color.toArgb(), it) }
    val towardsLight = background.luminance() < 0.4f
    for (step in 1..8) {
        hsl[2] = if (towardsLight) (hsl[2] + 0.07f).coerceAtMost(0.92f) else (hsl[2] - 0.07f).coerceAtLeast(0.15f)
        hsl[1] = (hsl[1] * 1.05f).coerceAtMost(1f)
        val candidate = Color(ColorUtils.HSLToColor(hsl))
        if (ColorUtils.calculateContrast(candidate.toArgb(), background.toArgb()) >= 3.2) return candidate
    }
    return fallback
}
