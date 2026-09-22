//! Which song the ear is on and where, for the app's seek bar and now-playing page:
//! `nori_player::heard` fed with the transition engine's latest reading. Asked every frame the bar is
//! drawn, so the question is one JNI call with primitives in and one packed `long` out - no strings,
//! records or buffers cross, and nothing is allocated on either side.

use jni::objects::{JClass, JLongArray, JObjectArray, JString};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use nori_player::heard::HeardTracker;
use parking_lot::Mutex;

use crate::automix::engine_jni::HEARD;

const MS_BITS: u32 = 43;

fn tracker<'a>(h: jlong) -> Option<&'a Mutex<HeardTracker>> {
    (h != 0).then(|| unsafe { &*(h as *const Mutex<HeardTracker>) })
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_HeardJni_create(_: JNIEnv, _: JClass) -> jlong {
    Box::into_raw(Box::new(Mutex::new(HeardTracker::new()))) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_HeardJni_destroy(_: JNIEnv, _: JClass, h: jlong) {
    if h != 0 {
        drop(unsafe { Box::from_raw(h as *mut Mutex<HeardTracker>) });
    }
}

/// The queue in the player's order and each song's length in ms; answers are indexes into it.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_HeardJni_setQueue(mut env: JNIEnv, _: JClass, h: jlong, ids: JObjectArray, durations: JLongArray) {
    let Some(t) = tracker(h) else { return };
    let n = env.get_array_length(&ids).unwrap_or(0).max(0) as usize;
    let mut ms = vec![0i64; n];
    if env.get_long_array_region(&durations, 0, &mut ms).is_err() {
        return;
    }
    let mut songs = Vec::with_capacity(n);
    for (i, d) in ms.into_iter().enumerate() {
        let id = env.get_object_array_element(&ids, i as jint).ok().map(JString::from);
        let id: String = id.and_then(|s| env.get_string(&s).ok().map(Into::into)).unwrap_or_default();
        songs.push((id, d));
    }
    t.lock().set_queue(songs);
}

/// Asked with what the player says now: `on` is its current index in the queue (-1 for none).
/// Returns `(index + 1) << 44 | changed << 43 | ms`, index -1 meaning the player's own word stands
/// (and ms is then `position_ms`).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_HeardJni_at(
    _: JNIEnv, _: JClass, h: jlong, now_ms: jlong, playing: jboolean, on: jint, position_ms: jlong,
) -> jlong {
    let Some(t) = tracker(h) else { return position_ms.max(0) };
    let heard = HEARD.lock();
    let s = t.lock().at_index(&heard, now_ms, playing != 0, usize::try_from(on).ok(), position_ms);
    let index = s.index.map_or(0, |i| i as i64 + 1);
    (index << (MS_BITS + 1)) | ((s.changed as i64) << MS_BITS) | s.ms.clamp(0, (1 << MS_BITS) - 1)
}
