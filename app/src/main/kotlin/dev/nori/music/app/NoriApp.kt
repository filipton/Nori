package dev.nori.music.app

import android.app.Application
import dev.nori.music.Nori

class NoriApp : Application() {
    override fun onCreate() {
        super.onCreate()
        // Loading the native core and opening SQLite overlaps with the activity being created instead of preceding it.
        val nori = Nori.get(this)
        Thread { numberStyle(); nori.warmUp(); forgetCoil() }.start()
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
        dev.nori.music.ffi.words.fmtSetLocale(symbols.decimalSeparator.toString(), symbols.groupingSeparator.toString())
    }

    /**
     * Covers were kept by Coil, in its own format, before the core fetched and kept them
     * (dev.nori.music.data.CoverLoader, in a directory of its own): nothing reads that directory now, so
     * it goes, once.
     */
    private fun forgetCoil() {
        val old = cacheDir.resolve("covers")
        if (old.exists()) old.deleteRecursively()
    }
}
