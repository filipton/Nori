//! Covers decoded by `nori_covers` straight into an Android Bitmap: a file (Coil's disk cache holds the
//! server's bytes as they came), bytes or a direct buffer, decoded to the Bitmap's own size and written
//! into its locked pixels, so nothing is copied into a Java array. The app's Coil decodes its covers
//! through `header` and `decodeBuffer` (the app's `RustCoverDecoder`); the debug build's `coverbench`
//! measures the file door against BitmapFactory.
// Bitmaps are only there on Android; elsewhere the doors build and refuse every Bitmap.
#![cfg_attr(not(target_os = "android"), allow(dead_code, unused_variables))]

use std::cell::RefCell;
use std::io::Read;
use std::sync::Mutex;

use jni::objects::{JByteArray, JByteBuffer, JClass, JObject, JString};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use nori_covers::Decoder;

use crate::{native, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/look/CoverPixels",
    methods: &[
        native!(c"decodeFile", c"(Ljava/lang/String;Landroid/graphics/Bitmap;Z)I", decode_file),
        native!(c"decodeBytes", c"([BILandroid/graphics/Bitmap;Z)I", decode_bytes),
        native!(c"header", c"(Ljava/nio/ByteBuffer;I)J", header),
        native!(c"decodeBuffer", c"(Ljava/nio/ByteBuffer;ILandroid/graphics/Bitmap;Z)I", decode_buffer),
    ],
};

/// What a door answers: 0 for a cover drawn, or what stopped it.
const OK: jint = 0;
/// Not a mutable software ARGB_8888 or RGB_565 Bitmap.
const BAD_BITMAP: jint = 1;
/// The file could not be read, or the byte range is not in the array.
const UNREADABLE: jint = 2;
/// Not a JPEG, PNG or WebP picture.
const UNKNOWN: jint = 3;
/// A picture the decoder refused: broken, or larger than any cover.
const BROKEN: jint = 4;
/// The file's EXIF turns or mirrors the picture, which this decoder does not do and Coil's own does.
const ORIENTED: jint = 5;

/// A decoder, and the RGBA rows an RGB_565 Bitmap's picture is decoded into before it is packed.
#[derive(Default)]
struct Drawer {
    decoder: Decoder,
    rgba: Vec<u8>,
}

/// RGBA above this many bytes (a 512 x 512 cover) is given back after its picture is packed: the player's
/// cover should not hold its memory until the next one.
const KEEP_RGBA: usize = 512 * 512 * 4;

/// What a thread that decodes keeps between covers: the decoder's buffers and the file's bytes. Covers
/// are decoded on the caller's thread, so each thread has its own.
struct Kept {
    drawer: Drawer,
    bytes: Vec<u8>,
}

thread_local! {
    static KEPT: RefCell<Kept> = RefCell::new(Kept { drawer: Drawer::default(), bytes: Vec::new() });
}

/// Decoders lent to `decode_buffer`, one per cover being decoded at that moment. Coil decodes on whichever
/// of its I/O threads is free, so a decoder per thread would leave its buffers on every thread that ever
/// decoded a cover; lent per cover, there are only as many as decode at once (the app lets four).
static IDLE: Mutex<Vec<Drawer>> = Mutex::new(Vec::new());

/// RGBA rows (`width` x `height`, tight) packed into RGB_565 rows `stride` bytes apart, each channel's
/// top bits, as Android's own conversion keeps them. Alpha is dropped: only an opaque cover is asked for
/// in RGB_565.
fn pack_565(rgba: &[u8], width: usize, height: usize, out: &mut [u8], stride: usize) {
    for y in 0..height {
        let from = &rgba[y * width * 4..(y + 1) * width * 4];
        let to = &mut out[y * stride..y * stride + width * 2];
        for (p, q) in from.chunks_exact(4).zip(to.chunks_exact_mut(2)) {
            let v = (u16::from(p[0]) >> 3) << 11 | (u16::from(p[1]) >> 2) << 5 | u16::from(p[2]) >> 3;
            q.copy_from_slice(&v.to_le_bytes());
        }
    }
}

/// Decodes `bytes` into `bitmap`, filling it; the IDCT shrinks big JPEGs when `idct` (see
/// `Decoder::set_idct_scaling`). An RGB_565 Bitmap's picture is decoded to RGBA first and packed into it.
fn draw(env: &JNIEnv, drawer: &mut Drawer, bytes: &[u8], bitmap: &JObject, idct: bool) -> jint {
    #[cfg(target_os = "android")]
    {
        let Some(mut b) = crate::look::bitmap::Locked::new_or_565(env, bitmap) else { return BAD_BITMAP };
        let (width, height, stride) = (b.width, b.height, b.stride);
        drawer.decoder.set_idct_scaling(idct);
        let decoded = if b.rgb565 {
            drawer.rgba.resize(width * height * 4, 0);
            let target = nori_covers::Target { px: &mut drawer.rgba, width, height, stride: width * 4 };
            let r = drawer.decoder.decode_into(bytes, target, nori_covers::Alpha::Premultiplied);
            if r.is_ok() {
                pack_565(&drawer.rgba, width, height, b.pixels_mut(), stride);
            }
            if drawer.rgba.capacity() > KEEP_RGBA {
                drawer.rgba = Vec::new();
            }
            r
        } else {
            let target = nori_covers::Target { px: b.pixels_mut(), width, height, stride };
            drawer.decoder.decode_into(bytes, target, nori_covers::Alpha::Premultiplied)
        };
        match decoded {
            Ok(()) => OK,
            Err(nori_covers::DecodeError::Unknown) => UNKNOWN,
            Err(nori_covers::DecodeError::Target) => BAD_BITMAP,
            Err(_) => BROKEN,
        }
    }
    #[cfg(not(target_os = "android"))]
    BAD_BITMAP
}

/// The picture in the file at `path`, into `bitmap`. Long work (a read and a decode), so a plain JNI
/// call: never on the UI thread.
extern "system" fn decode_file(mut env: JNIEnv, _: JClass, path: JString, bitmap: JObject, idct: jboolean) -> jint {
    KEPT.with_borrow_mut(|kept| {
        let read = crate::with_str(&mut env, &path, |p| {
            kept.bytes.clear();
            std::fs::File::open(p).and_then(|mut f| f.read_to_end(&mut kept.bytes)).is_ok()
        });
        if read != Some(true) {
            return UNREADABLE;
        }
        draw(&env, &mut kept.drawer, &kept.bytes, &bitmap, idct != 0)
    })
}

/// The picture in the first `len` bytes of `bytes`, into `bitmap`. The compressed bytes are copied once,
/// into a buffer this thread keeps.
extern "system" fn decode_bytes(env: JNIEnv, _: JClass, bytes: JByteArray, len: jint, bitmap: JObject, idct: jboolean) -> jint {
    KEPT.with_borrow_mut(|kept| {
        let Ok(have) = env.get_array_length(&bytes) else { return UNREADABLE };
        let Ok(len) = usize::try_from(len) else { return UNREADABLE };
        if len > have as usize {
            return UNREADABLE;
        }
        kept.bytes.resize(len, 0);
        // SAFETY: i8 and u8 have one size and alignment, and `kept.bytes` holds `len` initialised bytes.
        let into = unsafe { std::slice::from_raw_parts_mut(kept.bytes.as_mut_ptr().cast::<i8>(), len) };
        if env.get_byte_array_region(&bytes, 0, into).is_err() {
            return UNREADABLE;
        }
        draw(&env, &mut kept.drawer, &kept.bytes, &bitmap, idct != 0)
    })
}

/// The picture in the first `len` bytes of the direct buffer `buf`, from its headers alone: its width in
/// the high 32 bits and its height in the low, or minus what stops it being decoded here (`UNREADABLE`,
/// `UNKNOWN`, `BROKEN`, `ORIENTED`). Microseconds of work, so `@FastNative`.
extern "system" fn header(env: JNIEnv, _: JClass, buf: JByteBuffer, len: jint) -> jlong {
    let Some(bytes) = crate::region(&env, &buf, 0, len) else { return -jlong::from(UNREADABLE) };
    match nori_covers::header(bytes) {
        Ok(h) if h.oriented => -jlong::from(ORIENTED),
        Ok(h) => (h.width as jlong) << 32 | h.height as jlong,
        Err(nori_covers::DecodeError::Unknown) => -jlong::from(UNKNOWN),
        Err(_) => -jlong::from(BROKEN),
    }
}

/// The picture in the first `len` bytes of the direct buffer `buf`, into `bitmap`: read where it lies,
/// with no copy. A decode's work, so a plain JNI call, never on the UI thread.
extern "system" fn decode_buffer(env: JNIEnv, _: JClass, buf: JByteBuffer, len: jint, bitmap: JObject, idct: jboolean) -> jint {
    let Some(bytes) = crate::region(&env, &buf, 0, len) else { return UNREADABLE };
    let mut drawer = IDLE.lock().ok().and_then(|mut idle| idle.pop()).unwrap_or_default();
    let r = draw(&env, &mut drawer, bytes, &bitmap, idct != 0);
    if let Ok(mut idle) = IDLE.lock() {
        idle.push(drawer);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgba_is_packed_into_565_rows_by_each_channels_top_bits() {
        let rgba = [255, 255, 255, 255, 0, 0, 0, 255, 0xF8, 0x04, 0x08, 255, 0x07, 0xFC, 0xF7, 255];
        // Two rows of two, into rows padded to three pixels; the padding is left alone.
        let mut out = [0xAAu8; 12];
        pack_565(&rgba, 2, 2, &mut out, 6);
        let px = |i: usize| u16::from_le_bytes([out[i], out[i + 1]]);
        assert_eq!((px(0), px(2), px(4)), (0xFFFF, 0x0000, 0xAAAA));
        assert_eq!(px(6), 0b11111_000001_00001, "red's top five, green's top six, blue's top five");
        assert_eq!(px(8), 0b00000_111111_11110);
        assert_eq!(px(10), 0xAAAA);
    }
}
