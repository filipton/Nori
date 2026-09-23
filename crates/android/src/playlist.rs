//! What the player page asks of the core's queue (`nori_core::playlist`) on every player event:
//! primitives only, and the play order written straight into the array the player takes.

use jni::objects::{JClass, JIntArray};
use jni::sys::{jboolean, jint, jlong};
use jni::JNIEnv;
use nori_core::playlist;

use crate::{native, Class};

pub(crate) static CLASS: Class = Class {
    name: c"dev/nori/music/playback/PlaylistJni",
    methods: &[
        native!(c"rev", c"()J", rev),
        native!(c"shuffleShown", c"()Z", shuffle_shown),
        native!(c"order", c"([I)I", order),
        native!(c"same", c"(IIZII)Z", same),
    ],
};

extern "system" fn shuffle_shown() -> jboolean {
    playlist::playlist_shuffle_shown() as jboolean
}

/// The play order while shuffling, written into `out` (the array the player's shuffle order is made
/// from) when it is exactly that long. Returns its length; -1 when not shuffling.
extern "system" fn order(env: JNIEnv, _: JClass, out: JIntArray) -> jint {
    let Ok(cap) = env.get_array_length(&out) else { return -1 };
    playlist::playlist_shuffle_order(|o| {
        let Some(o) = o else { return -1 };
        if o.len() == cap as usize {
            // One copy into the array, a page of the order at a time: nothing allocated.
            let mut page = [0 as jint; 256];
            for (k, chunk) in o.chunks(page.len()).enumerate() {
                for (d, &i) in page.iter_mut().zip(chunk) {
                    *d = i as jint;
                }
                if env.set_int_array_region(&out, (k * 256) as jint, &page[..chunk.len()]).is_err() {
                    return -1;
                }
            }
        }
        o.len() as jint
    })
}

extern "system" fn same(count: jint, current: jint, shuffling: jboolean, ids_hash: jint, order_hash: jint) -> jboolean {
    playlist::playlist_same(count.max(0) as usize, current, shuffling != 0, ids_hash, order_hash) as jboolean
}

/// What the list looks like now, cheaply, so an unchanged queue is not copied over again.
extern "system" fn rev() -> jlong {
    playlist::playlist_rev() as jlong
}
