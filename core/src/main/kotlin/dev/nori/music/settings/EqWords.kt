package dev.nori.music.settings

import dalvik.annotation.optimization.CriticalNative
import dalvik.annotation.optimization.FastNative

/**
 * The equalizer screen's figures (nori-core's `fmt::eq_*`), over plain JNI: each is asked on every step
 * of a slider's drag, and through uniffi each short string cost a call status, a buffer and a cleaner.
 * Primitives in, one Java string out.
 */
object EqWords {
    init { System.loadLibrary("norimusic") }

    /** "+3.5", "-1.0", "+0.0" (`fmt::signed_db`). */
    @JvmStatic @FastNative external fun signedDb(db: Float): String
    /** "Pre-amp -3.5 dB (automatic)". */
    @JvmStatic @FastNative external fun preamp(db: Float, automatic: Boolean): String
    /** "centre", "L 30%". */
    @JvmStatic @FastNative external fun balance(balance: Float): String
    /** "Ceiling -1.0 dB". */
    @JvmStatic @FastNative external fun ceiling(db: Float): String
    /** What the limiter is pulling back: "−2.3 dB", "not clipping". */
    @JvmStatic @FastNative external fun reduction(db: Float): String
    @JvmStatic @FastNative external fun crossfeed(db: Float): String
    /** The band dialog's title: "63 Hz", "1k Hz". */
    @JvmStatic @FastNative external fun hzTitle(freq: Float): String
    /** "Slope 0.71" or "Q 1.41". */
    @JvmStatic @FastNative external fun shape(slope: Boolean, q: Float): String
    /** A band's label, its frequency and a mark for its channel or kind (`settings::band_label`). */
    @JvmStatic @FastNative external fun bandName(kind: Int, freq: Float, channel: Int): String
    /** The logarithmic frequency slider: 20 Hz at 0 to 20 kHz at 1. */
    @JvmStatic @CriticalNative external fun freqToSlider(freq: Float): Float
    @JvmStatic @CriticalNative external fun sliderToFreq(x: Float): Float
}

/**
 * The equalizer's sliders, edited in the core where the settings are kept (settings_store.rs), so a
 * drag builds no settings record and sends none across. -1 means nothing changed.
 */
internal object SoundEdit {
    init { System.loadLibrary("norimusic") }

    /** [band] is `[kind, freq, gain, q, channel]` in, and the band as it was kept out; returns the effects. */
    @JvmStatic @FastNative external fun setBand(index: Int, band: FloatArray): Int
    /** [level] an `EqLevel` ordinal; the value as it was kept as float bits in the high 32, the effects in the low. */
    @JvmStatic @CriticalNative external fun setLevel(level: Int, value: Float): Long
}
