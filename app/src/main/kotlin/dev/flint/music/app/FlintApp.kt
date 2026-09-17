package dev.flint.music.app

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
import dev.flint.music.Flint
import okio.Path.Companion.toOkioPath

class FlintApp : Application(), SingletonImageLoader.Factory {
    override fun onCreate() {
        super.onCreate()
        // Loading the native core and opening SQLite overlaps with the activity being created instead of preceding it.
        val flint = Flint.get(this)
        Thread { flint.warmUp() }.start()
    }

    /** Cover art shares the API's connection pool; its URLs are stable, so the disk cache needs no custom keys. */
    override fun newImageLoader(context: PlatformContext): ImageLoader = ImageLoader.Builder(context)
        .components { add(OkHttpNetworkFetcherFactory(callFactory = { Flint.get(this@FlintApp).http.api })) }
        .memoryCache { MemoryCache.Builder().maxSizePercent(context, 0.15).build() }
        .diskCache { DiskCache.Builder().directory(cacheDir.resolve("covers").toOkioPath()).maxSizeBytes(256L * 1024 * 1024).build() }
        .crossfade(false)
        // Covers have no alpha: 16-bit bitmaps halve decode memory, so twice as many stay in the memory cache.
        .allowRgb565(true)
        .build()
}
