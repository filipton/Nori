//! Steady playback must not allocate: not on the engine's thread for each buffer that goes through the
//! sink into the ring, and never on the device's thread. The test binary counts every allocation made
//! on the calling thread; each path runs past its warm-up and must then make none.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::sync::Arc;

use nori_player::dsp::{Band, PEAKING};
use nori_player::engine::Downstream;
use nori_player::pcm::{Encoding, Format};
use nori_player::pipeline::{Sink, Sound};

use crate::output::{AudioOutput, Feed, OutputFormat, RingTrack};

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

fn allocations(f: impl FnOnce()) -> u64 {
    let before = ALLOCS.with(Cell::get);
    f();
    ALLOCS.with(Cell::get) - before
}

/// A device at `rate` whose feed the test pulls by hand.
struct Hand(Arc<parking_lot::Mutex<Option<Feed>>>, u32);

impl AudioOutput for Hand {
    fn open(&mut self, want: OutputFormat) -> Result<OutputFormat, String> {
        Ok(OutputFormat { rate: self.1, channels: want.channels })
    }
    fn start(&mut self, feed: Feed) -> Result<(), String> {
        *self.0.lock() = Some(feed);
        Ok(())
    }
    fn pause(&mut self) {}
    fn resume(&mut self) {}
    fn latency_us(&self) -> u64 {
        20_000
    }
    fn close(&mut self) {}
}

const FMT: Format = Format { rate: 44_100, channels: 2, encoding: Encoding::Pcm16 };

fn tone(frames: usize) -> Vec<u8> {
    (0..frames).flat_map(|i| {
        let v = ((i as f64 / 44_100.0 * 440.0 * std::f64::consts::TAU).sin() * 9000.0) as i16;
        [v.to_le_bytes(), v.to_le_bytes()].concat()
    })
    .collect()
}

/// Buffers through the sink into the ring and out of the device, as playback runs them.
fn steady(device_rate: u32, sound: Sound, speed: f32, skip_silence: bool) -> u64 {
    let feed = Arc::new(parking_lot::Mutex::new(None));
    let mut sink = Sink::new(nori_player::burst::BUFFER_US, sound.on(), sound, RingTrack::new(Box::new(Hand(feed.clone(), device_rate))));
    sink.set_stages(speed, 1.0, skip_silence);
    sink.configure(&1, Some(FMT));
    sink.play();
    let data = tone(1152);
    let mut out = vec![0f32; 2048 * 2];
    let mut feed = feed.lock().take().expect("the device was started");
    let mut pts = 0i64;
    let mut turn = |sink: &mut Sink<RingTrack>, feed: &mut Feed| {
        sink.handle_buffer(&data, 0, pts);
        pts += FMT.us(data.len());
        feed.pull(&mut out);
        sink.position_us(false);
    };
    for _ in 0..400 {
        turn(&mut sink, &mut feed);
    }
    allocations(|| {
        for _ in 0..400 {
            turn(&mut sink, &mut feed);
        }
    })
}

#[test]
fn a_buffer_through_the_sink_and_the_ring_allocates_nothing() {
    assert_eq!(steady(44_100, Sound::default(), 1.0, false), 0, "straight through");
    let eq = Sound { bands: vec![Band { kind: PEAKING, freq: 1000.0, gain_db: 4.0, q: 1.0, channel: 0 }], limiter: true, ..Sound::default() };
    assert_eq!(steady(44_100, eq, 1.25, true), 0, "equalizer, limiter, silence skipping and speed");
    assert_eq!(steady(48_000, Sound::default(), 1.0, false), 0, "resampled for a device at another rate");
}
