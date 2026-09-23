package dev.nori.music.app

import android.content.Context
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.os.Debug
import coil3.request.allowHardware
import coil3.request.allowRgb565
import dev.nori.music.look.CoverPixels
import java.io.File
import java.util.Locale
import kotlin.math.abs
import kotlin.math.roundToInt

/**
 * The benchmarks the debug build's TestBridge runs over adb and the perf build's Performance page runs
 * from a button (src/bench is a source set of both builds, and of neither release). Each answers with
 * one line of text.
 */
object Bench {
    /** What a crossing into the core costs, by kind, against the same work done in Kotlin: one line. */
    fun calls(): String {
        val out = StringBuilder()
        var sink = 0L
        run(out, "empty loop", 200_000) { i -> sink += i }
        run(out, "kotlin seekStep", 200_000) { i -> sink += kotlinSeekStep(0.3f + i * 1e-7f, 0.31f, 0.016f, 900f, 0.003f) }
        run(out, "jni seekStep", 200_000) { i -> sink += dev.nori.music.look.CoverLook.seekStep(0.3f + i * 1e-7f, 0.31f, 0.016f, 900f, 0.003f) }
        run(out, "uniffi swipeTurn", 20_000) { i -> sink += dev.nori.music.ffi.swipeTurn(-20f - i % 7, -950f, 1000f, true, true, true) }
        val a = IntArray(dev.nori.music.look.CoverLook.LEN) { 0xFF102030.toInt() + it * 997 }
        val b = IntArray(dev.nori.music.look.CoverLook.LEN) { 0xFFF0E0D0.toInt() - it * 991 }
        val o = IntArray(a.size)
        run(out, "jni mix (all colours)", 50_000) { i -> dev.nori.music.look.CoverLook.mix(a, b, (i % 100) / 100f, o); sink += o[3] }
        run(out, "kotlin compose lerp (all colours)", 50_000) { i ->
            val t = (i % 100) / 100f
            for (k in a.indices) o[k] = androidx.compose.ui.graphics.lerp(androidx.compose.ui.graphics.Color(a[k]), androidx.compose.ui.graphics.Color(b[k]), t).value.toInt()
            sink += o[3]
        }
        run(out, "jni duration string", 50_000) { i -> sink += dev.nori.music.look.CoverLook.duration((i % 7000).toLong(), false).length }
        run(out, "uniffi duration string", 20_000) { i -> sink += dev.nori.music.ffi.duration((i % 7000).toLong()).length }
        return out.append("sink $sink").toString()
    }

    private fun allocated() = android.os.Debug.getRuntimeStat("art.gc.bytes-allocated")?.toLongOrNull() ?: 0L

    /** Inline, so the loop itself boxes nothing and costs next to nothing: what is measured is the body. */
    private inline fun run(out: StringBuilder, name: String, n: Int, body: (Int) -> Unit) {
        repeat(3) { for (i in 0 until n / 10) body(i) } // warm up the JIT
        val a0 = allocated(); val t0 = System.nanoTime()
        for (i in 0 until n) body(i)
        val ns = (System.nanoTime() - t0).toDouble() / n; val bytes = (allocated() - a0).toDouble() / n
        out.append(String.format(java.util.Locale.ROOT, "%s: %.0f ns, %.0f B per call; ", name, ns, bytes))
    }

    /**
     * The covers in Coil's disk cache (at most [limit] of them) decoded to 300x300 and
     * 1080x1080 by BitmapFactory as Coil decodes them (bounds first, the largest power-of-two
     * `inSampleSize` that still fills the size, the rest by density scaling; ARGB_8888, and RGB_565 as
     * the app's Coil asks for opaque covers) and by the Rust door (crates/covers) into one reused Bitmap,
     * with the IDCT shrinking big JPEGs and without, and into a reused RGB_565 Bitmap (what packing adds).
     * Then the first five decoded both ways, compared
     * pixel by pixel. Takes seconds: call it off the main thread.
     */
    fun covers(context: Context, limit: Int = 40): String {
        val files = coverFiles(context, limit)
        if (files.isEmpty()) return "coverbench: no covers in ${context.cacheDir}/covers"
        val out = StringBuilder("coverbench ${files.size} covers (")
        files.groupingBy { f -> BitmapFactory.Options().also { it.inJustDecodeBounds = true; BitmapFactory.decodeFile(f.path, it) }.let { "${it.outWidth}x${it.outHeight}" } }
            .eachCount().entries.sortedByDescending { it.value }.joinTo(out) { "${it.key}:${it.value}" }
        out.append(')')
        for (side in intArrayOf(300, 1080)) {
            val reused = Bitmap.createBitmap(side, side, Bitmap.Config.ARGB_8888)
            val reused565 = Bitmap.createBitmap(side, side, Bitmap.Config.RGB_565)
            val paths = listOf<Pair<String, (File) -> Bitmap?>>(
                "bitmapfactory-8888" to { f -> bitmapFactory(f, side, Bitmap.Config.ARGB_8888) },
                "bitmapfactory-565" to { f -> bitmapFactory(f, side, Bitmap.Config.RGB_565) },
                "rust-idct" to { f -> reused.takeIf { CoverPixels.decodeFile(f.path, it, true) == CoverPixels.OK } },
                "rust-whole" to { f -> reused.takeIf { CoverPixels.decodeFile(f.path, it, false) == CoverPixels.OK } },
                // What packing into RGB_565 adds to rust-whole.
                "rust-565" to { f -> reused565.takeIf { CoverPixels.decodeFile(f.path, it, false) == CoverPixels.OK } },
            )
            for ((name, decode) in paths) out.append(" | ").append(side).append(' ').append(measure(name, files, decode))
            out.append(" | ").append(side).append(" vs bitmapfactory-8888, mean abs diff a/r/g/b: ").append(quality(files.take(5), side, reused))
            reused.recycle()
            reused565.recycle()
        }
        return out.toString()
    }

    /** At most [limit] covers from Coil's disk cache, the same ones each time. */
    private fun coverFiles(context: Context, limit: Int): List<File> =
        File(context.cacheDir, "covers").walkTopDown().filter { it.isFile && isPicture(it) }.sortedBy { it.name }.take(limit).toList()

    /**
     * The covers in Coil's disk cache (at most [limit]) loaded through Coil the way the app loads them,
     * once with the app's [RustCoverDecoder] ahead of Coil's decoders and once with Coil's alone: to
     * 300 px and 1080 px, as a hardware Bitmap (what the screens get), software ARGB_8888 (what the
     * cover colours read) and RGB_565 (allowed, for a JPEG, when hardware is not), each from the file
     * (Coil's disk cache hands over files) and from bytes in memory. For each: ms per cover both ways,
     * and how many covers the Rust decoder drew, handed on to Coil's own, or failed. Then the cover
     * colours (`CoverLook.derive`) of the first five, worked out from either decoder's Bitmap: how many
     * came out identical, and the largest difference in a colour channel. Takes seconds: off the main thread.
     */
    fun coverDecode(context: Context, limit: Int = 40): String {
        val files = coverFiles(context, limit)
        if (files.isEmpty()) return "coverdecode: no covers in ${context.cacheDir}/covers"
        val bytes = files.map { it.readBytes() }
        val counts = RustCounts()
        val rust = coil3.ImageLoader.Builder(context).components { add(RustCoverDecoder.Factory { true }) }
            .eventListener(counts).memoryCache(null).diskCache(null).build()
        val coil = coil3.ImageLoader.Builder(context).memoryCache(null).diskCache(null).build()
        fun request(data: Any, side: Int, kind: String) = coil3.request.ImageRequest.Builder(context).data(data).size(side)
            .allowHardware(kind == "hardware").allowRgb565(kind == "565")
            .memoryCachePolicy(coil3.request.CachePolicy.DISABLED).diskCachePolicy(coil3.request.CachePolicy.DISABLED).build()
        fun load(loader: coil3.ImageLoader, data: Any, side: Int, kind: String): Bitmap? =
            (kotlinx.coroutines.runBlocking { loader.execute(request(data, side, kind)) } as? coil3.request.SuccessResult)
                ?.let { (it.image as? coil3.BitmapImage)?.bitmap }
        val out = StringBuilder("coverdecode ${files.size} covers")
        for (side in intArrayOf(300, 1080)) for (kind in listOf("hardware", "8888", "565")) for ((from, data) in listOf("file" to files, "bytes" to bytes)) {
            fun time(loader: coil3.ImageLoader): Double {
                // A few first, to warm the JIT and the page cache; each Bitmap goes as soon as it is made.
                data.take(3).forEach { load(loader, it, side, kind)?.recycle() }
                counts.reset()
                val t0 = System.nanoTime()
                for (d in data) load(loader, d, side, kind)?.recycle() ?: counts.failed.incrementAndGet()
                return (System.nanoTime() - t0) / 1e6 / data.size
            }
            val coilMs = time(coil)
            val rustMs = time(rust)
            out.append(String.format(Locale.ROOT, " | %d %s %s: rust %.2f ms/cover (%d drawn, %d to coil, %d failed), coil %.2f ms/cover",
                side, kind, from, rustMs, counts.drawn.get(), counts.handedOn.get(), counts.failed.get(), coilMs))
        }
        var same = 0; var largest = 0
        for (f in files.take(5)) {
            val a = load(rust, f, 320, "8888"); val b = load(coil, f, 320, "8888")
            val la = a?.let { dev.nori.music.look.CoverLook.derive(it, true, false) }?.look
            val lb = b?.let { dev.nori.music.look.CoverLook.derive(it, true, false) }?.look
            a?.recycle(); b?.recycle()
            if (la == null || lb == null) continue
            if (la.contentEquals(lb)) same++
            for (i in la.indices) for (s in 0..24 step 8) largest = maxOf(largest, abs(((la[i] ushr s) and 0xFF) - ((lb[i] ushr s) and 0xFF)))
        }
        return out.append(" | colours: $same of ${minOf(5, files.size)} identical, largest channel difference $largest").toString()
    }

    /** Which of the Rust decoder's covers it drew and which it handed on to Coil's own decoders. */
    private class RustCounts : coil3.EventListener() {
        val drawn = java.util.concurrent.atomic.AtomicInteger()
        val handedOn = java.util.concurrent.atomic.AtomicInteger()
        val failed = java.util.concurrent.atomic.AtomicInteger()

        fun reset() { drawn.set(0); handedOn.set(0); failed.set(0) }

        override fun decodeEnd(request: coil3.request.ImageRequest, decoder: coil3.decode.Decoder, options: coil3.request.Options, result: coil3.decode.DecodeResult?) {
            if (decoder is RustCoverDecoder) (if (result != null) drawn else handedOn).incrementAndGet()
        }
    }

    /** Whether the file starts as a JPEG, PNG or WebP does (Coil's journal and metadata do not). */
    private fun isPicture(f: File): Boolean {
        val head = ByteArray(12)
        val n = runCatching { f.inputStream().use { it.read(head) } }.getOrDefault(0)
        if (n < 12) return false
        fun at(i: Int) = head[i].toInt() and 0xFF
        return (at(0) == 0xFF && at(1) == 0xD8 && at(2) == 0xFF) ||
            (at(0) == 0x89 && at(1) == 'P'.code && at(2) == 'N'.code && at(3) == 'G'.code) ||
            (String(head, 0, 4, Charsets.US_ASCII) == "RIFF" && String(head, 8, 4, Charsets.US_ASCII) == "WEBP")
    }

    /** BitmapFactory as Coil 3's BitmapFactoryDecoder drives it for a `side`-pixel square filled (Scale.FILL). */
    private fun bitmapFactory(f: File, side: Int, config: Bitmap.Config): Bitmap? {
        val o = BitmapFactory.Options()
        o.inJustDecodeBounds = true
        BitmapFactory.decodeFile(f.path, o)
        val (w, h) = o.outWidth to o.outHeight
        if (w <= 0 || h <= 0) return null
        o.inJustDecodeBounds = false
        o.inSampleSize = minOf(Integer.highestOneBit(w / side), Integer.highestOneBit(h / side)).coerceAtLeast(1)
        val scale = maxOf(side / (w / o.inSampleSize.toDouble()), side / (h / o.inSampleSize.toDouble()))
        o.inScaled = scale != 1.0
        if (o.inScaled) {
            if (scale > 1) {
                o.inDensity = (Int.MAX_VALUE / scale).roundToInt()
                o.inTargetDensity = Int.MAX_VALUE
            } else {
                o.inDensity = Int.MAX_VALUE
                o.inTargetDensity = (Int.MAX_VALUE * scale).roundToInt()
            }
        }
        o.inPreferredConfig = config
        return BitmapFactory.decodeFile(f.path, o)
    }

    private fun gcCount() = Debug.getRuntimeStat("art.gc.gc-count")?.toLongOrNull() ?: 0L

    private fun javaHeap() = Runtime.getRuntime().let { it.totalMemory() - it.freeMemory() }

    /** Every cover decoded once by [decode], after a few to warm the JIT and the page cache. */
    private fun measure(name: String, files: List<File>, decode: (File) -> Bitmap?): String {
        files.take(5).forEach { decode(it) }
        System.gc(); System.runFinalization(); System.gc()
        val java0 = javaHeap(); val native0 = Debug.getNativeHeapAllocatedSize()
        val a0 = allocated(); val gc0 = gcCount()
        var failed = 0
        val t0 = System.nanoTime()
        for (f in files) if (decode(f) == null) failed++
        val ms = (System.nanoTime() - t0) / 1e6
        val bytes = (allocated() - a0).toDouble() / files.size; val gcs = gcCount() - gc0
        val java1 = javaHeap(); val native1 = Debug.getNativeHeapAllocatedSize()
        val mb = 1024.0 * 1024.0
        return String.format(Locale.ROOT, "%s: %.0f ms, %.2f ms/cover, %.0f B/cover allocated, %d GCs, java %.1f->%.1f MB, native %.1f->%.1f MB%s",
            name, ms, ms / files.size, bytes, gcs, java0 / mb, java1 / mb, native0 / mb, native1 / mb, if (failed > 0) ", $failed failed" else "")
    }

    /**
     * How far the Rust door's pixels are from BitmapFactory's ARGB_8888 ones, as a mean absolute
     * difference per channel over [files], with the IDCT and without: the middle `side` square of
     * BitmapFactory's (it scales to fill, and the view crops), against the Rust door's, which fills and crops.
     */
    private fun quality(files: List<File>, side: Int, reused: Bitmap): String {
        val sums = LongArray(8)
        val counted = LongArray(2)
        val a = IntArray(side * side); val b = IntArray(side * side)
        for (f in files) {
            val bf = bitmapFactory(f, side, Bitmap.Config.ARGB_8888) ?: continue
            val cw = minOf(bf.width, side); val ch = minOf(bf.height, side)
            bf.getPixels(a, 0, cw, (bf.width - cw) / 2, (bf.height - ch) / 2, cw, ch)
            bf.recycle()
            for ((k, idct) in listOf(0 to true, 1 to false)) {
                if (CoverPixels.decodeFile(f.path, reused, idct) != CoverPixels.OK) continue
                reused.getPixels(b, 0, cw, (side - cw) / 2, (side - ch) / 2, cw, ch)
                for (i in 0 until cw * ch) for (c in 0..3) {
                    val shift = 24 - 8 * c
                    sums[4 * k + c] += abs(((a[i] ushr shift) and 0xFF) - ((b[i] ushr shift) and 0xFF)).toLong()
                }
                counted[k] += (cw * ch).toLong()
            }
        }
        fun mad(k: Int) = (0..3).joinToString("/") { c -> String.format(Locale.ROOT, "%.2f", sums[4 * k + c].toDouble() / counted[k].coerceAtLeast(1)) }
        return "rust-idct ${mad(0)}, rust-whole ${mad(1)}"
    }

    /** nori_look::motion::seek_step, written out in Kotlin for the comparison. */
    private fun kotlinSeekStep(bar: Float, target: Float, dt: Float, widthPx: Float, speed: Float): Long {
        val width = widthPx.coerceAtLeast(1f)
        val gap = (target - bar) * width
        val next = if (kotlin.math.abs(gap) < 0.5f) bar else if (kotlin.math.abs(gap) < 2f) target else {
            val eased = bar + (target - bar) * (1f - kotlin.math.exp(-dt / 0.14f))
            if (kotlin.math.abs((target - eased) * width) < 2f) target else return (eased.toRawBits().toLong() shl 32)
        }
        if (speed <= 0f) return (next.toRawBits().toLong() shl 32) or 0xFFFF_FFFFL
        val ahead = ((target - next) * width).coerceIn(0f, 1f)
        val wait = ((1f - ahead) / (width * speed) * 1000f).toInt().coerceIn(16, 1000)
        return (next.toRawBits().toLong() shl 32) or wait.toLong()
    }
}
