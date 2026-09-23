package dev.nori.music.app

import android.app.Application
import coil3.ImageLoader
import coil3.PlatformContext
import coil3.SingletonImageLoader
import coil3.disk.DiskCache
import coil3.disk.directory
import coil3.memory.MemoryCache
import coil3.network.okhttp.OkHttpNetworkFetcherFactory
import coil3.request.crossfade
import coil3.request.allowRgb565
import dev.nori.music.Nori
import okio.Path.Companion.toOkioPath

class NoriApp : Application(), SingletonImageLoader.Factory {
    override fun onCreate() {
        super.onCreate()
        // Loading the native core and opening SQLite overlaps with the activity being created instead of preceding it.
        val nori = Nori.get(this)
        Thread { numberStyle(); nori.warmUp() }.start()
    }

    /** A new locale changes how the core writes fractions ("12,4 MB"), so it is told again. */
    override fun onConfigurationChanged(newConfig: android.content.res.Configuration) {
        super.onConfigurationChanged(newConfig)
        numberStyle()
    }

    /**
     * The core writes numbers the way `String.format` does in the default locale; this tells it which
     * separators that locale uses. On the warm-up thread at start, so loading the core stays off the
     * main thread.
     */
    private fun numberStyle() {
        val symbols = java.text.DecimalFormatSymbols.getInstance()
        dev.nori.music.ffi.fmtSetLocale(symbols.decimalSeparator.toString(), symbols.groupingSeparator.toString())
    }

    /** Cover art shares the API's connection pool; its URLs are stable, so the disk cache needs no custom keys. How much it keeps is the core's (`cover_rules`). */
    override fun newImageLoader(context: PlatformContext): ImageLoader = ImageLoader.Builder(context)
        .components { add(OkHttpNetworkFetcherFactory(callFactory = { Nori.get(this@NoriApp).http.callFactory })) }
        .memoryCache { MemoryCache.Builder().maxSizePercent(context, dev.nori.music.data.Covers.rules.memoryShare).build() }
        .diskCache { DiskCache.Builder().directory(cacheDir.resolve("covers").toOkioPath()).maxSizeBytes(dev.nori.music.data.Covers.rules.diskBytes.toLong()).build() }
        .crossfade(false)
        // Covers have no alpha: 16-bit bitmaps halve decode memory, so twice as many stay in the memory cache.
        .allowRgb565(true)
        .build()
}
