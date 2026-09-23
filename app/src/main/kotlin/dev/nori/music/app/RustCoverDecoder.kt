package dev.nori.music.app

import android.graphics.Bitmap
import coil3.ImageLoader
import coil3.annotation.ExperimentalCoilApi
import coil3.asImage
import coil3.decode.DecodeResult
import coil3.decode.DecodeUtils
import coil3.decode.Decoder
import coil3.decode.ImageSource
import coil3.fetch.SourceFetchResult
import coil3.request.Options
import coil3.request.allowRgb565
import coil3.request.bitmapConfig
import coil3.request.colorSpace
import coil3.request.maxBitmapSize
import coil3.request.premultipliedAlpha
import coil3.size.Precision
import dev.nori.music.look.CoverPixels
import java.io.IOException
import java.nio.ByteBuffer
import java.nio.channels.FileChannel
import java.nio.channels.ReadableByteChannel
import java.nio.file.StandardOpenOption
import java.util.concurrent.ConcurrentLinkedQueue
import kotlin.math.roundToInt
import kotlinx.coroutines.runInterruptible
import kotlinx.coroutines.sync.Semaphore
import kotlinx.coroutines.sync.withPermit
import okio.FileSystem

/**
 * Covers decoded by the core (crates/covers, through [CoverPixels]) inside Coil, which keeps everything
 * else: fetching, both caches, requests and drawing. On the owner's phone it decodes a cover to 300 px in
 * about half of BitmapFactory's time and to 1080 px in under a third, with no native heap growth
 * (docs/clients.md, "Measured").
 *
 * It makes the Bitmap Coil's own decoders would: the size is worked out by Coil's own [DecodeUtils] the
 * way its ImageDecoder path does (the picture's shape kept, shrunk to fit or fill, grown only for an exact
 * size), so nothing is laid out differently, and the config is the one Android's decoders would pick:
 * RGB_565 for a JPEG where Coil allows it, ARGB_8888 otherwise (the core packs 565 itself). A software
 * Bitmap is decoded into directly; a hardware one is decoded into a software Bitmap kept for the next
 * cover and copied to the GPU, the upload Android's own decoders make too.
 *
 * Anything else goes to Coil's decoders, by answering null: GIF, HEIF or anything not JPEG, PNG or WebP,
 * a broken file, a picture its EXIF turns (they honour the turn, this does not), another colour space or
 * config, and every cover while the "Decode covers in the core" setting is off.
 */
class RustCoverDecoder private constructor(private val source: ImageSource, private val options: Options) : Decoder {

    override suspend fun decode(): DecodeResult? = permits.withPermit { runInterruptible { decodeHere() } }

    private fun decodeHere(): DecodeResult? {
        val scratch = idle.poll() ?: Scratch()
        try {
            val length = try { scratch.read(source) } catch (_: IOException) { null } ?: return null
            val buffer = scratch.buffer
            val size = CoverPixels.header(buffer, length)
            if (size < 0) return null
            val (width, height, sampled) = sized((size ushr 32).toInt(), size.toInt())
            val jpeg = buffer.get(0) == 0xFF.toByte()
            // A JPEG has no alpha, so where Coil allows it, it is RGB_565: half the memory, and twice the
            // covers in the memory cache. BitmapFactory does that for software Bitmaps, ImageDecoder (what
            // Coil uses from Android 10) for hardware ones as well.
            val pixels = if (options.allowRgb565 && jpeg) Bitmap.Config.RGB_565 else Bitmap.Config.ARGB_8888
            // Straight into the Bitmap Coil gets when it asks for a software one; otherwise into the kept
            // one and copied to the GPU. Saying a JPEG has no alpha (as Android's decoders do) lets
            // drawing skip blending it; the copy takes it along.
            val bitmap = if (options.bitmapConfig != Bitmap.Config.HARDWARE) {
                val b = Bitmap.createBitmap(width, height, pixels)
                if (CoverPixels.decodeBuffer(buffer, length, b, IDCT) != CoverPixels.OK) { b.recycle(); return null }
                b.also { it.setHasAlpha(!jpeg) }
            } else {
                val drawn = scratch.bitmap(width, height, pixels)
                if (CoverPixels.decodeBuffer(buffer, length, drawn, IDCT) != CoverPixels.OK) return null
                drawn.setHasAlpha(!jpeg)
                drawn.copy(Bitmap.Config.HARDWARE, false) ?: return null
            }
            return DecodeResult(bitmap.asImage(), sampled)
        } finally {
            scratch.letGo()
            idle.offer(scratch)
        }
    }

    /**
     * The Bitmap's size for a [srcWidth] x [srcHeight] picture, and whether that is smaller: Coil's
     * StaticImageDecoder's rule, which BitmapFactory's sampling and density scaling come to as well.
     */
    @OptIn(ExperimentalCoilApi::class)
    private fun sized(srcWidth: Int, srcHeight: Int): Triple<Int, Int, Boolean> {
        val dst = DecodeUtils.computeDstSize(srcWidth, srcHeight, options.size, options.scale, options.maxBitmapSize)
        if (dst.first == srcWidth && dst.second == srcHeight) return Triple(srcWidth, srcHeight, false)
        val m = DecodeUtils.computeSizeMultiplier(srcWidth, srcHeight, dst.first, dst.second, options.scale, options.maxBitmapSize)
        if (m >= 1 && options.precision != Precision.EXACT) return Triple(srcWidth, srcHeight, false)
        return Triple((m * srcWidth).roundToInt().coerceAtLeast(1), (m * srcHeight).roundToInt().coerceAtLeast(1), m < 1)
    }

    /**
     * What one decode borrows: the file's bytes in a direct buffer the core reads where they lie, and the
     * software Bitmap a hardware or RGB_565 cover is decoded into before its copy. Kept between covers,
     * one per cover being decoded at once, so neither is made again per cover.
     */
    private class Scratch {
        var buffer: ByteBuffer = ByteBuffer.allocateDirect(START)
            private set
        private var drawn: Bitmap? = null

        /** The source's bytes into [buffer], read once: the file itself when Coil has one, otherwise without consuming the source, so Coil's decoders can still read it. Null when there are too many. */
        fun read(source: ImageSource): Int? {
            val file = if (source.fileSystem === FileSystem.SYSTEM) source.fileOrNull() else null
            if (file != null) {
                return FileChannel.open(file.toNioPath(), StandardOpenOption.READ).use { ch -> fill(ch, ch.size()) }
            }
            return source.source().peek().use { fill(it, 0) }
        }

        private fun fill(ch: ReadableByteChannel, size: Long): Int? {
            if (size > MAX_BYTES) return null
            if (buffer.capacity() < size) buffer = ByteBuffer.allocateDirect(Integer.highestOneBit(size.toInt() - 1) shl 1)
            buffer.clear()
            while (true) {
                if (!buffer.hasRemaining()) {
                    // A stream that did not say how long it is, longer than the buffer: grown by doubling.
                    if (buffer.capacity() >= MAX_BYTES) return null
                    buffer.flip()
                    buffer = ByteBuffer.allocateDirect(buffer.capacity() * 2).put(buffer)
                }
                if (ch.read(buffer) < 0) return buffer.position()
            }
        }

        /** The kept software Bitmap made [width] x [height] in [config], in the memory it already has when that is enough. */
        fun bitmap(width: Int, height: Int, config: Bitmap.Config): Bitmap {
            val b = drawn
            val bytes = width * height * if (config == Bitmap.Config.RGB_565) 2 else 4
            if (b != null && b.allocationByteCount >= bytes) {
                b.reconfigure(width, height, config)
                return b
            }
            b?.recycle()
            return Bitmap.createBitmap(width, height, config).also { drawn = it }
        }

        /** A player-sized Bitmap is not kept: that many bytes idle for the next list cover is a poor trade. */
        fun letGo() {
            val b = drawn ?: return
            if (b.allocationByteCount > KEEP_BYTES) { b.recycle(); drawn = null }
        }
    }

    /** [on] is read per cover (the "Decode covers in the core" setting), so switching it takes effect at once. */
    class Factory(private val on: () -> Boolean) : Decoder.Factory {
        override fun create(result: SourceFetchResult, options: Options, imageLoader: ImageLoader): Decoder? {
            if (!on()) return null
            val config = options.bitmapConfig
            if (config != Bitmap.Config.ARGB_8888 && config != Bitmap.Config.HARDWARE) return null
            if (options.colorSpace != null || !options.premultipliedAlpha) return null
            return RustCoverDecoder(result.source, options)
        }
    }

    private companion object {
        /**
         * Whether a big JPEG is shrunk by the IDCT. Decoded whole and averaged measured as fast on the
         * phone (0.93 against 0.98 ms a cover at 300 px, 2.80 ms both at 1080) and its average is exact.
         */
        const val IDCT = false
        /** As many decodes at once as Coil's own decoders allow themselves. */
        val permits = Semaphore(4)
        val idle = ConcurrentLinkedQueue<Scratch>()
        const val START = 256 * 1024
        /** Larger than any cover a server sends; such a file is Coil's to decode. */
        const val MAX_BYTES = 16 shl 20
        /** A 512 x 512 ARGB_8888 Bitmap: every list and grid cover fits. */
        const val KEEP_BYTES = 512 * 512 * 4
    }
}
