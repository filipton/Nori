//! Which streamed songs leave the local cache when it is over its limit: least recently used first, and
//! before those, whatever an earlier run of the app cached and this one has not used since - untouched
//! since the restart is the stalest there is. The platform's cache holds the bytes; this knows the keys it
//! holds (told once what an earlier run left, then of each use) and names what goes, one key at a time,
//! so the platform never hands the whole list over.

use std::collections::HashMap;

use parking_lot::Mutex;

struct Order {
    /// When each key was last used, in touches since the process started; what an earlier run left and
    /// this one has not used counts down from -1, older than anything touched.
    used: HashMap<String, i64>,
    clock: i64,
    left: i64,
}

static ORDER: Mutex<Option<Order>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Order) -> R) -> R {
    f(ORDER.lock().get_or_insert_with(|| Order { used: HashMap::new(), clock: 0, left: 0 }))
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

/// What the cache held when this process first looked: the keys not known yet join as never used.
pub fn seed<'a>(held: impl IntoIterator<Item = &'a str>) {
    with(|o| {
        for k in held {
            if !o.used.contains_key(k) {
                o.left -= 1;
                o.used.insert(k.to_string(), o.left);
            }
        }
    });
}

/// The next key to drop: never used by this process first, then the least recently used. It is forgotten
/// here as it is handed out; if the cache still holds it after, its next use makes it known again.
pub fn next() -> Option<String> {
    with(|o| {
        let key = o.used.iter().min_by_key(|(_, t)| **t).map(|(k, _)| k.clone())?;
        o.used.remove(&key);
        Some(key)
    })
}

/// The streamed copies of `id` the cache holds, whatever quality they were fetched at, forgotten here as
/// they are handed out (the caller drops them).
pub fn copies(id: &str) -> Vec<String> {
    with(|o| {
        let keys: Vec<String> = o.used.keys().filter(|k| crate::stream::is_copy(id, k)).cloned().collect();
        for k in &keys {
            o.used.remove(k);
        }
        keys
    })
}

/// The cache was emptied.
pub fn clear() {
    with(|o| o.used.clear());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_this_run_never_used_goes_first_then_the_stalest() {
        // One test holds the whole order: tests run side by side and it is one per process.
        clear();
        touch("sc-a");
        touch("sc-b");
        seed(["sc-a", "sc-old", "sc-b"]);
        touch("sc-a");
        assert_eq!([next(), next(), next(), next()], [Some("sc-old".into()), Some("sc-b".into()), Some("sc-a".into()), None]);
        // A key handed out is used again: it is known again.
        touch("sc-b");
        assert_eq!(next().as_deref(), Some("sc-b"));

        touch("x:0");
        touch("x:192opus");
        touch("xy:0");
        seed(["x:320mp3"]);
        let mut c = copies("x");
        c.sort();
        assert_eq!(c, ["x:0", "x:192opus", "x:320mp3"]);
        assert_eq!(next().as_deref(), Some("xy:0"), "only the copies were dropped");
    }
}
