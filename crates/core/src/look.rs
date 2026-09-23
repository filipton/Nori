//! How a page looks, as Kotlin reaches it: `nori_look` over JNI - a cover's Bitmap read where it lies,
//! its wash drawn straight into a Bitmap and its look into a small int array - so nothing is boxed,
//! serialised or copied twice,
//! and the UI only ever looks colours up. The lyrics page is prepared once per song through uniffi and
//! then asked every frame over JNI with primitives in and one packed `long` out, allocating nothing on
//! either side.

use jni::objects::{JClass, JIntArray};
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::JNIEnv;
#[cfg_attr(not(target_os = "android"), allow(unused_imports))]
use nori_look::cover::{derive, WASH_OUT};
use nori_look::dress;
use nori_look::lyrics::{Line, LyricClock, LyricTiming, Word};

/// A software ARGB_8888 `Bitmap`'s own memory, read and written in place through Android's bitmap API
/// (libjnigraphics): no copy of the picture into a Java array, and none across into Rust. Locked for as
/// long as the value lives. Anything else - another format, a hardware bitmap, a size that does not
/// add up - is refused, and the caller gets nothing rather than a guess.
#[cfg(target_os = "android")]
mod bitmap {
    use jni::objects::JObject;
    use jni::JNIEnv;
    use std::ffi::c_void;

    #[repr(C)]
    struct Info {
        width: u32,
        height: u32,
        stride: u32,
        format: i32,
        flags: u32,
    }
    const RGBA_8888: i32 = 1;

    #[link(name = "jnigraphics")]
    extern "C" {
        fn AndroidBitmap_getInfo(env: *mut jni::sys::JNIEnv, bitmap: jni::sys::jobject, info: *mut Info) -> i32;
        fn AndroidBitmap_lockPixels(env: *mut jni::sys::JNIEnv, bitmap: jni::sys::jobject, addr: *mut *mut c_void) -> i32;
        fn AndroidBitmap_unlockPixels(env: *mut jni::sys::JNIEnv, bitmap: jni::sys::jobject) -> i32;
    }

    pub struct Locked {
        env: *mut jni::sys::JNIEnv,
        obj: jni::sys::jobject,
        pub px: *mut u8,
        pub width: usize,
        pub height: usize,
        pub stride: usize,
    }

    impl Locked {
        pub fn new(env: &JNIEnv, bitmap: &JObject) -> Option<Locked> {
            let (e, o) = (env.get_raw(), bitmap.as_raw());
            if o.is_null() {
                return None;
            }
            let mut info = Info { width: 0, height: 0, stride: 0, format: 0, flags: 0 };
            if unsafe { AndroidBitmap_getInfo(e, o, &mut info) } != 0 || info.format != RGBA_8888 {
                return None;
            }
            let (width, height, stride) = (info.width as usize, info.height as usize, info.stride as usize);
            if width == 0 || height == 0 || stride < width * 4 {
                return None;
            }
            let mut addr: *mut c_void = std::ptr::null_mut();
            if unsafe { AndroidBitmap_lockPixels(e, o, &mut addr) } != 0 || addr.is_null() {
                return None;
            }
            Some(Locked { env: e, obj: o, px: addr as *mut u8, width, height, stride })
        }

        pub fn row(&self, y: usize) -> &[u8] {
            unsafe { std::slice::from_raw_parts(self.px.add(y * self.stride), self.width * 4) }
        }

        pub fn row_mut(&mut self, y: usize) -> &mut [u8] {
            unsafe { std::slice::from_raw_parts_mut(self.px.add(y * self.stride), self.width * 4) }
        }
    }

    impl Drop for Locked {
        fn drop(&mut self) {
            unsafe { AndroidBitmap_unlockPixels(self.env, self.obj) };
        }
    }
}

/// One pixel as a Bitmap keeps it (R, G, B, A, premultiplied) to ARGB as `Bitmap.getPixels` gives it.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn unpremultiplied(p: &[u8]) -> u32 {
    let a = p[3] as u32;
    let c = |v: u8| match a {
        255 => v as u32,
        0 => 0,
        _ => ((v as u32 * 255 + a / 2) / a).min(255),
    };
    (a << 24) | (c(p[0]) << 16) | (c(p[1]) << 8) | c(p[2])
}

/// ARGB to a Bitmap's own layout (R, G, B, A, premultiplied), as `Bitmap.setPixels` would store it.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
fn premultiplied(argb: u32, out: &mut [u8]) {
    let a = argb >> 24;
    let c = |v: u32| if a == 255 { v } else { (v * a + 127) / 255 };
    out[0] = c((argb >> 16) & 0xFF) as u8;
    out[1] = c((argb >> 8) & 0xFF) as u8;
    out[2] = c(argb & 0xFF) as u8;
    out[3] = a as u8;
}

/// The page for the cover `bitmap` (software ARGB_8888), read where it lies. `out` gets the page's look
/// (`dress::LEN` entries); the wash is drawn straight into `wash` (a mutable `WASH_OUT`² ARGB_8888
/// bitmap). Returns 0 for no page, 1 for a page, 2 for a page with its wash (not on AMOLED black).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_CoverLook_deriveBitmap(
    env: JNIEnv, _: JClass, bitmap: jni::objects::JObject, dark: jboolean, amoled: jboolean, out: JIntArray, wash: jni::objects::JObject,
) -> jint {
    #[cfg(target_os = "android")]
    {
        let px = {
            let Some(b) = bitmap::Locked::new(&env, &bitmap) else { return 0 };
            let mut px = Vec::with_capacity(b.width * b.height);
            for y in 0..b.height {
                px.extend(b.row(y).chunks_exact(4).map(unpremultiplied));
            }
            (px, b.width, b.height)
        };
        let c = derive(&px.0, px.1, px.2, dark != 0, amoled != 0);
        let look = dress::page(c.edge, c.background, c.on, c.accent, c.wash_edge).map(|v| v as i32);
        if env.set_int_array_region(&out, 0, &look).is_err() {
            return 0;
        }
        let Some(pixels) = c.wash else { return 1 };
        let Some(mut w) = bitmap::Locked::new(&env, &wash) else { return 1 };
        if w.width != WASH_OUT || w.height != WASH_OUT {
            return 1;
        }
        for y in 0..WASH_OUT {
            let row = w.row_mut(y);
            for (x, px) in row.chunks_exact_mut(4).enumerate() {
                premultiplied(pixels[y * WASH_OUT + x], px);
            }
        }
        2
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (env, bitmap, dark, amoled, out, wash);
        0
    }
}

/// Two looks mixed at `t` into `out` (`dress::mix`): a frame of the player's page cross-fading, so one
/// crossing and no allocation, three arrays copied on the stack.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_CoverLook_mix(
    env: JNIEnv, _: JClass, from: JIntArray, to: JIntArray, t: jfloat, out: JIntArray,
) {
    let (mut a, mut b) = ([0i32; dress::LEN], [0i32; dress::LEN]);
    if env.get_int_array_region(&from, 0, &mut a).is_err() || env.get_int_array_region(&to, 0, &mut b).is_err() {
        return;
    }
    let mut mixed = [0u32; dress::LEN];
    dress::mix(&a.map(|v| v as u32), &b.map(|v| v as u32), t, &mut mixed);
    let _ = env.set_int_array_region(&out, 0, &mixed.map(|v| v as i32));
}

/// The look of a page in the theme's own colours. `roles` is background, on surface, on surface variant,
/// primary, on primary, surface variant, surface container, surface container high, secondary container,
/// outline variant; `out` gets `dress::LEN` entries.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_CoverLook_plain(env: JNIEnv, _: JClass, roles: JIntArray, out: JIntArray) {
    let mut r = [0i32; 10];
    if env.get_int_array_region(&roles, 0, &mut r).is_err() {
        return;
    }
    let r = r.map(|v| v as u32);
    let s = dress::Scheme {
        background: r[0], on: r[1], on_variant: r[2], primary: r[3], on_primary: r[4], surface_variant: r[5],
        surface_container: r[6], surface_container_high: r[7], secondary_container: r[8], outline_variant: r[9],
    };
    let _ = env.set_int_array_region(&out, 0, &dress::plain(&s).map(|v| v as i32));
}

/// A theme's tones from one seed colour, into `out` (11 colours; see `nori_look::theme::seeded`).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_CoverLook_tones(env: JNIEnv, _: JClass, seed: jint, dark: jboolean, out: JIntArray) {
    let tones = nori_look::theme::seeded(seed as u32, dark != 0).map(|v| v as i32);
    let _ = env.set_int_array_region(&out, 0, &tones);
}

/// The surfaces AMOLED black puts in a dark scheme (`dress::AMOLED`, 8 colours), into `out`.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_CoverLook_amoled(env: JNIEnv, _: JClass, out: JIntArray) {
    let _ = env.set_int_array_region(&out, 0, &dress::AMOLED.map(|v| v as i32));
}

// ---- lyrics -------------------------------------------------------------------------------------------------

/// Prepares a song's lyrics for timing (`nori_look::lyrics`), showing the moment `position_ms`. Once per
/// set of lyrics, so it takes the record as it is; the answer is a handle for `LyricsJni`, which the
/// platform frees with `LyricsJni.destroy`.
#[uniffi::export]
pub fn lyrics_clock(lyrics: crate::Lyrics, position_ms: i64) -> i64 {
    let lines = lyrics.lines.into_iter().map(|l| Line {
        start_ms: l.start_ms,
        len: l.text.encode_utf16().count() as u32,
        words: l.words.into_iter().map(|w| Word { start_ms: w.start_ms, end_ms: w.end_ms, start: w.start, end: w.end }).collect(),
    });
    Box::into_raw(Box::new(LyricClock::new(LyricTiming::new(lyrics.synced, lyrics.word_timed, lines), position_ms))) as i64
}

fn clock<'a>(h: jlong) -> Option<&'a LyricClock> {
    (h != 0).then(|| unsafe { &*(h as *const LyricClock) })
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_LyricsJni_destroy(_: JNIEnv, _: JClass, h: jlong) {
    if h != 0 {
        drop(unsafe { Box::from_raw(h as *mut LyricClock) });
    }
}

/// Whether the active line can fill in word by word at all (the listener's switch aside).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_LyricsJni_sweeps(_: JNIEnv, _: JClass, h: jlong) -> jboolean {
    clock(h).is_some_and(|c| c.timing().sweeps()) as jboolean
}

/// The per-frame question, `LyricClock::advance` packed by `Step::pack`. A freed handle answers
/// "nothing lit, never ask again".
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_LyricsJni_at(_: JNIEnv, _: JClass, h: jlong, position_ms: jlong, sweep: jboolean, force: jboolean) -> jlong {
    clock(h).map_or(0, |c| c.advance(position_ms, sweep != 0, force != 0).pack())
}

/// What is on screen now, packed by `Frame::pack`.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_LyricsJni_shown(_: JNIEnv, _: JClass, h: jlong) -> jlong {
    clock(h).map_or(0, |c| c.shown().pack())
}

/// A tap on `line`: shows it and returns where to seek the player to.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_LyricsJni_tap(_: JNIEnv, _: JClass, h: jlong, line: jint) -> jlong {
    clock(h).map_or(0, |c| c.tap(line.max(0) as usize))
}

/// Sooner (> 0), later (< 0) or back to none (0); returns the nudge in ms.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_LyricsJni_nudge(_: JNIEnv, _: JClass, h: jlong, dir: jint) -> jlong {
    clock(h).map_or(0, |c| c.nudge(dir))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LyricLine, Lyrics};
    use nori_look::lyrics::Step;

    #[test]
    fn a_clock_from_the_cores_lyrics_counts_utf16_and_frees() {
        let line = |start_ms, text: &str| LyricLine { start_ms, end_ms: start_ms + 2000, text: text.into(), words: vec![], translation: None, background: false };
        let h = lyrics_clock(Lyrics { synced: true, word_timed: false, lines: vec![line(1000, "Żółć 🎵"), line(4000, "x")] }, 0);
        let c = clock(h).unwrap();
        assert!(!c.timing().sweeps());
        // A line without words is all lit once reached: seven UTF-16 units, the note being two.
        let s = Step::unpack(c.advance(1500, true, true).pack());
        assert_eq!((s.frame.active, s.frame.sung, s.redraw), (0, 7.0, true));
        drop(unsafe { Box::from_raw(h as *mut LyricClock) });
        assert!(clock(0).is_none());
    }
}

#[cfg(test)]
mod bitmap_tests {
    use super::*;

    #[test]
    fn a_bitmaps_pixels_read_and_write_like_get_and_set_pixels() {
        assert_eq!(unpremultiplied(&[0x12, 0x34, 0x56, 0xFF]), 0xFF123456);
        assert_eq!(unpremultiplied(&[0, 0, 0, 0]), 0);
        // Half transparent white is stored as half-bright grey and comes back white.
        assert_eq!(unpremultiplied(&[0x80, 0x80, 0x80, 0x80]), 0x80FFFFFF);
        let mut px = [0u8; 4];
        premultiplied(0xFF123456, &mut px);
        assert_eq!(px, [0x12, 0x34, 0x56, 0xFF]);
        premultiplied(0x80FFFFFF, &mut px);
        assert_eq!(px, [0x80, 0x80, 0x80, 0x80]);
    }
}

/// One step of the seek bar (`nori_look::motion::seek_step`), asked each frame it moves: primitives
/// in, `bar bits << 32 | wait` out, nothing allocated.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_CoverLook_seekStep(
    _: JNIEnv, _: JClass, bar: jfloat, target: jfloat, dt_s: jfloat, width_px: jfloat, speed: jfloat,
) -> jlong {
    let (b, wait) = nori_look::motion::seek_step(bar, target, dt_s, width_px, speed);
    ((b.to_bits() as i64) << 32) | (wait as u32 as i64)
}
