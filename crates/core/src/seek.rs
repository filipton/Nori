//! Making a seek stick (`nori_player::seek`), as Kotlin reaches it: primitives in, one number out.

use jni::objects::JClass;
use jni::sys::{jboolean, jlong};
use jni::JNIEnv;
use nori_player::seek::{SeekKeeper, Verdict};
use parking_lot::Mutex;

fn keeper<'a>(h: jlong) -> Option<&'a Mutex<SeekKeeper>> {
    (h != 0).then(|| unsafe { &*(h as *const Mutex<SeekKeeper>) })
}

/// What a look answers: keep watching, forget it, or (zero or more) the place to ask for again.
const WATCH: jlong = -1;
const FORGET: jlong = -2;

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_SeekJni_create(_: JNIEnv, _: JClass) -> jlong {
    Box::into_raw(Box::new(Mutex::new(SeekKeeper::new()))) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_SeekJni_ask(_: JNIEnv, _: JClass, h: jlong, target: jlong, now: jlong, ready: jboolean, pos: jlong) {
    if let Some(k) = keeper(h) {
        k.lock().ask(target, now, ready != 0, pos);
    }
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_SeekJni_forget(_: JNIEnv, _: JClass, h: jlong) {
    if let Some(k) = keeper(h) {
        k.lock().forget();
    }
}

/// -1: keep watching; -2: done with it; otherwise the place to ask the player for again.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_SeekJni_look(
    _: JNIEnv, _: JClass, h: jlong, now: jlong, same_song: jboolean, ready: jboolean, pos: jlong, playing: jboolean,
) -> jlong {
    let Some(k) = keeper(h) else { return FORGET };
    match k.lock().look(now, same_song != 0, ready != 0, pos, playing != 0) {
        Verdict::Watch => WATCH,
        Verdict::Forget => FORGET,
        Verdict::SeekAgain(at) => at.max(0),
    }
}
