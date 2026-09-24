package dev.nori.music.app.ui

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
import dev.nori.music.data.CoverLoader
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext

/**
 * Picked once per cover (from the list rendition, which is usually on the disk already) and kept in
 * memory, so reopening a page costs nothing. The core works it out off the main thread.
 */
private object CoverPalette {
    val cache = LruCache<String, PagePalette>(128)

    /**
     * One lock per cover and theme (both its pages, plain and on black, are measured together), so a
     * picture the bar, the player and the warm-up all ask for at once is measured once and the others
     * wait for it. Bounded: an evicted lock costs one repeated measurement, which is the behaviour this
     * replaces.
     */
    private val locks = LruCache<String, kotlinx.coroutines.sync.Mutex>(64)

    suspend fun <T> once(key: String, block: suspend () -> T): T =
        synchronized(locks) { locks.get(key) ?: kotlinx.coroutines.sync.Mutex().also { locks.put(key, it) } }.withLock { block() }
}

private fun paletteKey(url: String, dark: Boolean, amoled: Boolean) = "$url|$dark|$amoled"

/**
 * Fetches the cover and works its colours out, unless that has been done before. Slow the first time
 * (a read of the disk, or the network, and a decode) and a map lookup every time after.
 */
private suspend fun paletteOf(context: android.content.Context, url: String, dark: Boolean, amoled: Boolean): PagePalette? {
    val key = paletteKey(url, dark, amoled)
    CoverPalette.cache.get(key)?.let { return it }
    measure(context, url, dark, black = amoled, plain = !amoled)
    return CoverPalette.cache.get(key)
}

/**
 * Works out the cover's page on AMOLED [black] and in the [plain] theme, whichever is asked for and not
 * known yet, from one decode: the bar and the player can want both for the same record.
 */
private suspend fun measure(context: android.content.Context, url: String, dark: Boolean, black: Boolean, plain: Boolean) {
    fun wanted(amoled: Boolean) = (if (amoled) black else plain) && CoverPalette.cache.get(paletteKey(url, dark, amoled)) == null
    if (!wanted(true) && !wanted(false)) return
    // One cover is only ever worked out once, even when the bar, the player and the warm-up below all
    // ask for it in the same frame: the others wait here and then find it in the cache.
    CoverPalette.once("$url|$dark") {
        val onBlack = wanted(true)
        val onPlain = wanted(false)
        if (!onBlack && !onPlain) return@once
        // Full eight bits a channel, decoded from the file by the core rather than read off a Bitmap
        // drawn on screen (which is 5-6-5 to save memory): the colours are worked out from the same
        // pixels any other app on nori-look would hand in, so the same record gets the same page
        // everywhere.
        val pages = withContext(Dispatchers.IO) { CoverLoader.get(context).colours(url, CoverSize.ROW, dark, onPlain, onBlack) } ?: return@once
        pages.plain?.let { CoverPalette.cache.put(paletteKey(url, dark, false), PagePalette(it.look, wash = it.wash?.asImageBitmap())) }
        pages.black?.let { CoverPalette.cache.put(paletteKey(url, dark, true), PagePalette(it.look, wash = null)) }
    }
}

/**
 * Works a cover's colours out before anything asks for them. The covers either side of what is playing
 * are fetched ahead (PlayerViewModel) so a skip lands on a picture that is already there, but the page's
 * colour was still being worked out after the fact, which is the beat the page spent wearing the last
 * song's colour. Done here, the cross-fade starts with the song. [andPlain]: its page in the plain
 * theme as well, from the same decode.
 */
suspend fun warmCoverPalette(context: android.content.Context, url: String?, dark: Boolean, amoled: Boolean, andPlain: Boolean = false) {
    if (url == null) return
    runCatching { measure(context, url, dark, black = amoled, plain = !amoled || andPlain) }
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

