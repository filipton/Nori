//! The lyrics are asked where they are once every second display frame for as long as they are on screen,
//! so that question must not allocate: an allocation per frame is a lock and a search on the UI thread,
//! thirty times a second. The test binary counts every allocation made on the calling thread.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;

use crate::lyrics::{Line, LyricClock, LyricTiming, Step, Word};

struct Counting;

thread_local! {
    static ALLOCS: Cell<u64> = const { Cell::new(0) };
}

unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc(l) }
    }
    unsafe fn alloc_zeroed(&self, l: Layout) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.alloc_zeroed(l) }
    }
    unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
        let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
        unsafe { System.realloc(p, l, n) }
    }
    unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
        unsafe { System.dealloc(p, l) }
    }
}

#[global_allocator]
static GLOBAL: Counting = Counting;

/// Allocations `f` made on this thread.
fn allocations(f: impl FnOnce()) -> u64 {
    let before = ALLOCS.with(Cell::get);
    f();
    ALLOCS.with(Cell::get) - before
}

#[test]
fn asking_the_lyrics_where_they_are_allocates_nothing() {
    // A song's worth: 80 lines of five timed words, 2.5 s apart.
    let lines = (0..80i64).map(|i| {
        let start = 5_000 + i * 2_500;
        let words = (0..5u32).map(|k| Word { start_ms: start + k as i64 * 400, end_ms: start + k as i64 * 400 + 350, start: k * 6, end: k * 6 + 5 }).collect();
        Line { start_ms: start, len: 29, words }
    });
    let clock = LyricClock::new(LyricTiming::new(true, true, lines), 0);
    let mut seen = 0i64;
    let n = allocations(|| {
        // Every second frame of the whole song with the sweep, then again a line at a time, with a nudge,
        // a forced look and a tap in between: everything a platform does while the lyrics are open.
        for sweep in [true, false] {
            let mut t = 0;
            while t < 210_000 {
                seen ^= clock.advance(t, sweep, t % 50_000 == 0).pack();
                seen ^= std::hint::black_box(Step::unpack(seen)).frame.active as i64;
                t += 33;
            }
            clock.nudge(1);
            seen ^= clock.tap(3) ^ clock.shown().pack();
            clock.nudge(0);
        }
    });
    std::hint::black_box(seen);
    assert_eq!(n, 0);
}
