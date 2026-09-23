//! How a page looks, as Kotlin reaches it: `nori_look` over JNI with plain int arrays - a cover's
//! pixels in, its whole dressed look and its wash out - so nothing is boxed, serialised or copied twice,
//! and the UI only ever looks colours up. The lyrics page is prepared once per song through uniffi and
//! then asked every frame over JNI with primitives in and one packed `long` out, allocating nothing on
//! either side.

use jni::objects::{JClass, JIntArray};
use jni::sys::{jboolean, jfloat, jint, jlong};
use jni::JNIEnv;
use nori_look::cover::{derive, WASH_OUT};
use nori_look::dress;
use nori_look::lyrics::{Line, LyricClock, LyricTiming, Word};

/// The page for `w` x `h` ARGB `pixels`. `out` holds the page's look (`nori_look::dress`, `dress::LEN`
/// entries), then the wash (`WASH_OUT`² pixels). Returns whether there is a wash (not on AMOLED black).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_look_CoverLook_derive(
    env: JNIEnv, _: JClass, pixels: JIntArray, w: jint, h: jint, dark: jboolean, amoled: jboolean, out: JIntArray,
) -> jboolean {
    let (w, h) = (w.max(0) as usize, h.max(0) as usize);
    if w == 0 || h == 0 || env.get_array_length(&pixels).unwrap_or(0) as usize != w * h {
        return 0;
    }
    let mut px = vec![0i32; w * h];
    if env.get_int_array_region(&pixels, 0, &mut px).is_err() {
        return 0;
    }
    let px: Vec<u32> = px.into_iter().map(|p| p as u32).collect();
    let c = derive(&px, w, h, dark != 0, amoled != 0);
    let look = dress::page(c.edge, c.background, c.on, c.accent, c.wash_edge).map(|v| v as i32);
    if env.set_int_array_region(&out, 0, &look).is_err() {
        return 0;
    }
    match c.wash {
        Some(wash) if env.get_array_length(&out).unwrap_or(0) as usize >= dress::LEN + WASH_OUT * WASH_OUT => {
            let wash: Vec<i32> = wash.into_iter().map(|p| p as i32).collect();
            env.set_int_array_region(&out, dress::LEN as i32, &wash).is_ok() as jboolean
        }
        _ => 0,
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
