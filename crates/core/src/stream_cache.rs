//! Which streamed songs leave the local cache when it is over its limit: least recently used first, and
//! before those, whatever an earlier run of the app cached and this one has not used since - untouched
//! since the restart is the stalest there is. The platform's cache holds the bytes and says what it
//! holds; this keeps the order they were used in and says what goes.

use std::collections::HashMap;

use jni::objects::{JClass, JObjectArray, JString};
use jni::JNIEnv;
use parking_lot::Mutex;

struct Order {
    /// When each key was last used, in touches since the process started.
    used: HashMap<String, u64>,
    clock: u64,
}

static ORDER: Mutex<Option<Order>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Order) -> R) -> R {
    f(ORDER.lock().get_or_insert_with(|| Order { used: HashMap::new(), clock: 0 }))
}

/// `key` was read or written just now.
pub fn touch(key: &str) {
    with(|o| {
        o.clock += 1;
        let t = o.clock;
        match o.used.get_mut(key) {
            Some(u) => *u = t,
            None => {
                o.used.insert(key.to_string(), t);
            }
        }
    });
}

/// Of `held` (what the cache holds now), the order to drop them in until it fits: never used by this
/// process first, then the least recently used.
pub fn eviction_order(held: &[String]) -> Vec<String> {
    with(|o| {
        o.used.retain(|k, _| held.contains(k));
        let mut keys: Vec<(u64, &String)> = held.iter().map(|k| (o.used.get(k).copied().unwrap_or(0), k)).collect();
        keys.sort_by_key(|(t, _)| *t);
        keys.into_iter().map(|(_, k)| k.clone()).collect()
    })
}

/// A cache span was read or written. Called from the cache's own callbacks, a few times a song; a key
/// already known is found without allocating.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_StreamCacheJni_touch(mut env: JNIEnv, _: JClass, key: JString) {
    let Ok(k) = env.get_string(&key) else { return };
    let k: std::borrow::Cow<str> = (&k).into();
    touch(&k);
}

/// The keys the cache holds, reordered in place into the order they should go in.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_playback_StreamCacheJni_order(mut env: JNIEnv, _: JClass, keys: JObjectArray) {
    let n = env.get_array_length(&keys).unwrap_or(0);
    let mut held = Vec::with_capacity(n.max(0) as usize);
    for i in 0..n {
        let Ok(o) = env.get_object_array_element(&keys, i) else { return };
        let s = JString::from(o);
        let Ok(v) = env.get_string(&s) else { return };
        held.push(String::from(v));
    }
    for (i, k) in eviction_order(&held).iter().enumerate() {
        let Ok(s) = env.new_string(k) else { return };
        if env.set_object_array_element(&keys, i as i32, s).is_err() {
            return;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn what_this_run_never_used_goes_first_then_the_stalest() {
        touch("sc-a");
        touch("sc-b");
        touch("sc-a");
        let held = s(&["sc-a", "sc-old", "sc-b"]);
        assert_eq!(eviction_order(&held), s(&["sc-old", "sc-b", "sc-a"]));
        // A key the cache no longer holds is forgotten.
        eviction_order(&s(&["sc-a"]));
        assert!(!with(|o| o.used.contains_key("sc-b")));
    }
}
