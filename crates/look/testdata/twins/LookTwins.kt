// The Kotlin originals of nori-look's twins, run on the host JVM by tools/twins.sh to write
// look_twins.tsv, which crates/look/tests/twins.rs holds the Rust to. Each function is the app's own code,
// copied as it is (BandEffect.of reduced to the key it keeps its effects under). Copy a function again
// when its original changes, and run tools/twins.sh.

fun row(vararg f: Any?) = println(f.joinToString("\t"))

// ---- app/.../ui/CoverColors.kt ----

fun paletteKey(url: String, dark: Boolean, amoled: Boolean) = "$url|$dark|$amoled"

// ---- app/.../ui/PlayerScreen.kt BandEffect.of: null is the plain blur ----

fun bandKey(s: Float, kr: Float, kg: Float, kb: Float): Long? {
    if (s <= 0f) return null
    // Kept by the tint to a 1/256th, which is finer than the blurred band can show.
    fun q(x: Float) = (x * 256f).toLong().coerceIn(0, 1023)
    return (q(s) shl 30) or (q(kr) shl 20) or (q(kg) shl 10) or q(kb)
}

fun bits(f: Float) = "%08x".format(f.toRawBits())

fun main() {
    for (url in listOf("https://m.example/rest/getCoverArt.view?u=a&id=al-1&size=320", "", "a|b")) {
        for (dark in listOf(false, true)) for (amoled in listOf(false, true)) {
            row("palette_key", url.ifEmpty { "_" }, dark, amoled, paletteKey(url, dark, amoled).ifEmpty { "_" })
        }
    }

    val values = listOf(-1f, 0f, -0f, 1e-9f, 1f / 256f, 0.5f, 0.2126f, 0.7152f, 0.0722f, 0.9f, 1f, 3.99f, 4f, 5f, Float.NaN, Float.POSITIVE_INFINITY, Float.NEGATIVE_INFINITY)
    for (s in values) for (k in values) {
        row("band_key", bits(s), bits(k), bits(1f - k), bits(k * 0.5f), bandKey(s, k, 1f - k, k * 0.5f) ?: "-")
    }
}
