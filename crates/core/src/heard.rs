//! Which song the ear is on and where, for the app's seek bar and now-playing page:
//! `nori_player::heard` fed with the transition engine's latest reading. Asked every frame the bar is
//! drawn, so the question is one JNI call with primitives in and one packed `long` out - no strings,
//! records or buffers cross, and nothing is allocated on either side.

use jni::objects::JClass;
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use nori_player::heard::HeardTracker;
use parking_lot::Mutex;

use crate::automix::engine_jni::HEARD;

const MS_BITS: u32 = 43;

/// The tracker, and the revision of the core's queue it was last given (crates/core/src/playlist.rs).
struct Clock {
    t: HeardTracker,
    rev: u64,
}

fn tracker<'a>(h: jlong) -> Option<&'a Mutex<Clock>> {
    (h != 0).then(|| unsafe { &*(h as *const Mutex<Clock>) })
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_HeardJni_create(_: JNIEnv, _: JClass) -> jlong {
    Box::into_raw(Box::new(Mutex::new(Clock { t: HeardTracker::new(), rev: u64::MAX }))) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_HeardJni_destroy(_: JNIEnv, _: JClass, h: jlong) {
    if h != 0 {
        drop(unsafe { Box::from_raw(h as *mut Mutex<Clock>) });
    }
}

/// Asked with what the player says now: `on` is its current index in the queue and `next` the one it
/// goes to next (-1 for none). The queue is the core's own; it is read again only when it has changed.
/// Returns `(index + 1) << 44 | changed << 43 | ms`, index -1 meaning the player's own word stands
/// (and ms is then `position_ms`).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_HeardJni_at(
    _: JNIEnv, _: JClass, h: jlong, now_ms: jlong, playing: jboolean, on: jint, next: jint, position_ms: jlong,
) -> jlong {
    let Some(t) = tracker(h) else { return position_ms.max(0) };
    let heard = HEARD.lock();
    let mut c = t.lock();
    let rev = crate::playlist::playlist_rev();
    if c.rev != rev {
        c.rev = rev;
        c.t.set_queue(crate::playlist::with(|p| crate::queue::durations(p.ids())));
    }
    let s = c.t.at_index(&heard, now_ms, playing != 0, usize::try_from(on).ok(), usize::try_from(next).ok(), position_ms);
    let index = s.index.map_or(0, |i| i as i64 + 1);
    (index << (MS_BITS + 1)) | ((s.changed as i64) << MS_BITS) | s.ms.clamp(0, (1 << MS_BITS) - 1)
}
