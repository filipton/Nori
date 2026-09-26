//! How much the Rust heap holds, for the perf report's memory line: counted by [`Counting`], which a client
//! installs as its global allocator (Android's library does), so the report can tell Rust's share of the
//! native heap from the platform's own (Skia, the codecs, the graphics driver). One relaxed atomic add per
//! allocation and one per free, no lock and no allocation of its own; where nobody installs it the count
//! stays nought and [`live_bytes`] says None.

use std::alloc::{GlobalAlloc, Layout, System};
use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};

/// The system allocator, counting the bytes it has handed out and not had back.
pub struct Counting;

static LIVE: AtomicIsize = AtomicIsize::new(0);
static COUNTING: AtomicBool = AtomicBool::new(false);

// SAFETY: every call is the system allocator's own; only a count is kept beside it.
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(l) };
        if !p.is_null() {
            LIVE.fetch_add(l.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let p = unsafe { System.alloc_zeroed(l) };
        if !p.is_null() {
            LIVE.fetch_add(l.size() as isize, Ordering::Relaxed);
        }
        p
    }

    unsafe fn realloc(&self, p: *mut u8, l: Layout, size: usize) -> *mut u8 {
        let q = unsafe { System.realloc(p, l, size) };
        if !q.is_null() {
            LIVE.fetch_add(size as isize - l.size() as isize, Ordering::Relaxed);
        }
        q
    }

    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) };
        LIVE.fetch_sub(l.size() as isize, Ordering::Relaxed);
    }
}

impl Counting {
    /// Says the count is kept: called once by the client that installed it, as it starts.
    pub fn installed() {
        COUNTING.store(true, Ordering::Relaxed);
    }
}

/// Bytes the Rust heap holds now, as asked for (not what the allocator rounds them up to); None where
/// [`Counting`] is not the global allocator.
pub fn live_bytes() -> Option<i64> {
    COUNTING.load(Ordering::Relaxed).then(|| LIVE.load(Ordering::Relaxed).max(0) as i64)
}
