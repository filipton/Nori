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
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.platform.LocalContext
import coil3.SingletonImageLoader
import coil3.request.ImageRequest
import coil3.request.SuccessResult
import coil3.request.allowRgb565
import coil3.request.allowHardware
import coil3.toBitmap
import dev.nori.music.look.CoverLook
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
            // Full eight bits a channel, read from the disk cache rather than the list's memory copy (which
            // is 5-6-5 to save memory): the colours are worked out from the same pixels any other app on
            // nori-look would hand in, so the same record gets the same page everywhere.
            val result = SingletonImageLoader.get(context).execute(
                ImageRequest.Builder(context).data(url).size(CoverSize.ROW).allowHardware(false).allowRgb565(false)
                    .memoryCachePolicy(coil3.request.CachePolicy.DISABLED).build(),
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
    // Built once per cover and theme, not on every recomposition.
    val key = remember(url, dark, amoled) { url?.let { paletteKey(it, dark, amoled) } }
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
 * A cover's page, worked out in Rust (crates/look, `nori_look::cover::derive`) from its pixels: every
 * rule about which colour a record's page is lives there, so any app built on it dresses the same
 * record the same way. This only reads the pixels out and wraps the answer for Compose.
 */
private fun derive(bitmap: Bitmap, dark: Boolean, amoled: Boolean): PagePalette? {
    val px = IntArray(bitmap.width * bitmap.height).also { bitmap.getPixels(it, 0, bitmap.width, 0, 0, bitmap.width, bitmap.height) }
    val c = CoverLook.derive(px, bitmap.width, bitmap.height, dark, amoled) ?: return null
    return PagePalette(
        c.look,
        wash = c.wash?.let { Bitmap.createBitmap(it, CoverLook.WASH, CoverLook.WASH, Bitmap.Config.ARGB_8888).asImageBitmap() },
    )
}
