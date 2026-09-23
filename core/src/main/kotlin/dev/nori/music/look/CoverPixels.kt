package dev.nori.music.look

import android.graphics.Bitmap
import dalvik.annotation.optimization.FastNative
import java.nio.ByteBuffer

/**
 * Covers decoded in Rust (crates/covers) straight into a Bitmap's own pixels, at the Bitmap's size: a
 * JPEG, PNG or WebP file, its bytes, or its bytes in a direct buffer. The Bitmap must be a mutable
 * software ARGB_8888 one, or RGB_565 for an opaque picture (the core decodes to RGBA and packs it).
 *
 * The app's Coil decodes its covers through [header] and [decodeBuffer] (`RustCoverDecoder`), and the
 * debug build's `coverbench` measures [decodeFile] against BitmapFactory. Each answers [OK] or what went
 * wrong. The decodes do a decode's worth of work: never on the main thread.
 */
object CoverPixels {
    init { System.loadLibrary("norimusic") }

    const val OK = 0
    const val BAD_BITMAP = 1
    const val UNREADABLE = 2
    const val UNKNOWN = 3
    const val BROKEN = 4
    /** The file's EXIF turns or mirrors the picture; this decoder draws it as stored, Coil's own turns it. */
    const val ORIENTED = 5

    /** [idct]: a JPEG at least twice the size drawn is shrunk by the decoder's IDCT rather than decoded whole and averaged. */
    @JvmStatic external fun decodeFile(path: String, bitmap: Bitmap, idct: Boolean): Int

    /** The picture in the first [length] bytes of [bytes]. */
    @JvmStatic external fun decodeBytes(bytes: ByteArray, length: Int, bitmap: Bitmap, idct: Boolean): Int

    /**
     * The picture in the first [length] bytes of the direct [buffer], from its headers alone: its width
     * in the high 32 bits and its height in the low, or minus the reason it cannot be decoded here
     * ([UNREADABLE], [UNKNOWN], [BROKEN], [ORIENTED]).
     */
    @JvmStatic @FastNative external fun header(buffer: ByteBuffer, length: Int): Long

    /** The picture in the first [length] bytes of the direct [buffer], read where it lies. */
    @JvmStatic external fun decodeBuffer(buffer: ByteBuffer, length: Int, bitmap: Bitmap, idct: Boolean): Int
}
