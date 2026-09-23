//! Decoded covers kept in memory, by address and size, under a limit in bytes: the least recently drawn
//! go first. A list scrolled back and forth draws from here without decoding again.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use parking_lot::Mutex;

use crate::disk::Key;

/// A decoded cover: tight rows of `width` x `height` RGBA pixels.
#[derive(Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub pixels: Box<[u8]>,
}

/// A cover at one size.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Sized {
    pub key: Key,
    pub width: u32,
    pub height: u32,
}

#[derive(Default)]
struct Kept {
    images: HashMap<Sized, (Arc<Image>, u64)>,
    order: BTreeMap<u64, Sized>,
    bytes: usize,
    clock: u64,
}

pub struct MemoryCache {
    limit: usize,
    kept: Mutex<Kept>,
}

impl MemoryCache {
    pub fn new(limit: usize) -> MemoryCache {
        MemoryCache { limit, kept: Mutex::new(Kept::default()) }
    }

    /// The cover, if kept; a hit counts as a use.
    pub fn get(&self, key: &Sized) -> Option<Arc<Image>> {
        let mut k = self.kept.lock();
        k.clock += 1;
        let clock = k.clock;
        let (image, used) = k.images.get_mut(key)?;
        let (image, was) = (image.clone(), std::mem::replace(used, clock));
        k.order.remove(&was);
        k.order.insert(clock, *key);
        Some(image)
    }

    /// Keeps `image`, letting the least recently used go until it fits. An image larger than the whole
    /// limit is not kept.
    pub fn put(&self, key: Sized, image: Arc<Image>) {
        let size = image.pixels.len();
        if size > self.limit {
            return;
        }
        let mut k = self.kept.lock();
        if let Some((old, used)) = k.images.remove(&key) {
            k.order.remove(&used);
            k.bytes -= old.pixels.len();
        }
        while k.bytes + size > self.limit {
            let Some((_, gone)) = k.order.pop_first() else { break };
            let (old, _) = k.images.remove(&gone).expect("every image in the order is kept");
            k.bytes -= old.pixels.len();
        }
        k.clock += 1;
        let clock = k.clock;
        k.images.insert(key, (image, clock));
        k.order.insert(clock, key);
        k.bytes += size;
    }

    /// Lets everything go, for a client told memory is short.
    pub fn clear(&self) {
        *self.kept.lock() = Kept::default();
    }

    pub fn bytes(&self) -> usize {
        self.kept.lock().bytes
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(side: u32) -> Arc<Image> {
        Arc::new(Image { width: side, height: side, pixels: vec![0; (side * side * 4) as usize].into() })
    }

    fn at(url: &str, side: u32) -> Sized {
        Sized { key: Key::of(url), width: side, height: side }
    }

    #[test]
    fn the_least_recently_drawn_go_first_under_the_byte_limit() {
        // Room for three 4x4 covers (64 bytes each).
        let m = MemoryCache::new(200);
        m.put(at("a", 4), image(4));
        m.put(at("b", 4), image(4));
        m.put(at("c", 4), image(4));
        assert!(m.get(&at("a", 4)).is_some());
        m.put(at("d", 4), image(4));
        assert!(m.get(&at("b", 4)).is_none());
        assert!(m.get(&at("a", 4)).is_some() && m.get(&at("c", 4)).is_some() && m.get(&at("d", 4)).is_some());
        assert_eq!(m.bytes(), 192);
        // One size is not another.
        assert!(m.get(&at("a", 2)).is_none());
        // A bigger cover pushes out as many as it needs.
        m.put(at("e", 6), image(6));
        assert_eq!(m.bytes(), 144);
        assert!(m.get(&at("e", 6)).is_some());
        // Larger than the whole limit: not kept, nothing lost.
        m.put(at("f", 8), image(8));
        assert!(m.get(&at("f", 8)).is_none() && m.get(&at("e", 6)).is_some());
        m.clear();
        assert_eq!(m.bytes(), 0);
    }
}
