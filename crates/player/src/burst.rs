//! Playing in bursts. Left alone, a player tops the output up one decoder buffer at a time, which wakes
//! several threads dozens of times a second for as long as music plays. [`Fed`] lets the output's
//! (deliberately deep, [`BUFFER_US`]) buffer drain to [`LOW_US`] before offering it audio again, then
//! offers everything until it is full: a fraction of a second of work every few seconds, and real sleep
//! in between. A refused offer does not even reach the output.
//!
//! Output handed to a chip that decodes by itself (offload) already sleeps on its own; it is passed
//! straight through, and only counted.

use crate::engine::{Downstream, POSITION_NOT_SET};
use crate::pcm::Format;

/// How deep the output buffer is made for bursts.
pub const BUFFER_US: i64 = 10_000_000;
/// Audio is offered again once the output holds less than this.
pub const LOW_US: i64 = 2_000_000;
/// Slack for the clock moving a little further than the count between two readings.
const JUMP_US: i64 = 250_000;

/// The bookkeeping, kept between calls.
#[derive(Debug, Clone)]
pub struct Burst {
    /// False while the output decodes by itself, or something needs low latency (the equalizer
    /// being tuned): every offer goes straight through.
    pub enabled: bool,
    filling: bool,
    /// How much audio is in the output, counted from what was handed to it and how far its clock has
    /// moved - never from timestamps. It once was the last buffer's timestamp less the clock, which is
    /// wrong across a jump: a mix is stamped in the next song's time, so the moment its first chunk
    /// went in the timestamps leapt ahead by the length of the mix while the clock only follows once
    /// that chunk is heard. The buffer looked twelve seconds deeper than it was, feeding stopped for
    /// twice as long as it should, and the output ran dry near the end of the mix.
    written_us: i64,
    played_us: i64,
    last_position: i64,
    last_read_ms: i64,
    format: Option<Format>,
    /// Bytes handed to the output since this was made: the honest answer to "is audio flowing?".
    pub bytes_written: u64,
}

impl Default for Burst {
    fn default() -> Self {
        Burst { enabled: true, filling: true, written_us: 0, played_us: 0, last_position: POSITION_NOT_SET, last_read_ms: 0, format: None, bytes_written: 0 }
    }
}

impl Burst {
    /// Anything that makes the count unsure starts it again at nothing, which can only make feeding
    /// start a little early, never late: playing, pausing, a flush, a new output.
    pub fn restart(&mut self) {
        self.filling = true;
        self.written_us = 0;
        self.played_us = 0;
        self.last_position = POSITION_NOT_SET;
    }

    /// How much audio the output holds, given its clock now. A jump of the clock is not playing, so a
    /// move larger than what could have been queued counts as at most the time that passed.
    fn queued_us(&mut self, position: i64, now_ms: i64) -> Option<i64> {
        if position == POSITION_NOT_SET {
            return None;
        }
        let (last, wall) = (self.last_position, (now_ms - self.last_read_ms) * 1000);
        self.last_position = position;
        self.last_read_ms = now_ms;
        if last != POSITION_NOT_SET {
            let moved = position - last;
            let queued = self.written_us - self.played_us;
            self.played_us += match moved {
                m if m <= 0 => 0,
                m if m <= queued + JUMP_US => m,
                _ => wall.min(queued),
            };
        }
        Some((self.written_us - self.played_us).max(0))
    }
}

/// The output below, fed in bursts. Made for each call into the engine: `now_ms` is that call's clock.
pub struct Fed<'a, D> {
    pub down: &'a mut D,
    pub burst: &'a mut Burst,
    pub now_ms: i64,
    /// The output's clock, read once per call: it does not move in the microseconds a call takes.
    position: Option<i64>,
}

impl<'a, D: Downstream> Fed<'a, D> {
    pub fn new(down: &'a mut D, burst: &'a mut Burst, now_ms: i64) -> Self {
        Fed { down, burst, now_ms, position: None }
    }

    fn clock(&mut self) -> i64 {
        *self.position.get_or_insert_with(|| self.down.position_us(false))
    }
}

impl<D: Downstream> Downstream for Fed<'_, D> {
    type Config = D::Config;

    fn configure(&mut self, config: &D::Config, format: Option<Format>) {
        self.burst.restart();
        self.burst.format = format;
        self.position = None;
        self.down.configure(config, format);
    }

    fn handle_buffer(&mut self, data: &[u8], from: usize, pts_us: i64) -> (bool, usize) {
        if !self.burst.enabled {
            let r = self.down.handle_buffer(data, from, pts_us);
            self.burst.bytes_written += r.1 as u64;
            return r;
        }
        if !self.burst.filling {
            // The output has no clock while it is stopped, paused before it ever played, or freshly
            // restarted; then nothing is known and it is fed rather than left waiting for ever.
            let (position, now) = (self.clock(), self.now_ms);
            if self.burst.queued_us(position, now).is_some_and(|q| q > LOW_US) {
                return (false, 0);
            }
            self.burst.filling = true;
        }
        let (taken, used) = self.down.handle_buffer(data, from, pts_us);
        self.burst.bytes_written += used as u64;
        if let Some(f) = self.burst.format.filter(|f| f.frame_bytes() > 0 && f.rate > 0) {
            self.burst.written_us += (used / f.frame_bytes()) as i64 * 1_000_000 / f.rate as i64;
        }
        if !taken {
            // Full: nothing more until it has drained to the low mark.
            self.burst.filling = false;
        }
        (taken, used)
    }

    fn handle_discontinuity(&mut self) {
        // The audio already written stays in the output, so the count stands; the clock's jump when it
        // is reached is left out by `queued_us`.
        self.burst.filling = true;
        self.down.handle_discontinuity();
    }

    fn position_us(&mut self, source_ended: bool) -> i64 {
        if source_ended {
            self.down.position_us(true)
        } else {
            self.clock()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pcm::Encoding;

    const FMT: Format = Format { rate: 44_100, channels: 2, encoding: Encoding::Pcm16 };

    /// An output with a buffer of `cap_us` and a playhead the test moves.
    struct Track {
        cap_us: i64,
        written_us: i64,
        played_us: i64,
        offers: usize,
    }

    impl Downstream for Track {
        type Config = ();
        fn configure(&mut self, _: &(), _: Option<Format>) {}
        fn handle_buffer(&mut self, data: &[u8], from: usize, _: i64) -> (bool, usize) {
            self.offers += 1;
            let room = FMT.bytes(self.cap_us - (self.written_us - self.played_us)).min(data.len() - from);
            let room = room / FMT.frame_bytes() * FMT.frame_bytes();
            self.written_us += FMT.us(room);
            (room == data.len() - from, room)
        }
        fn handle_discontinuity(&mut self) {}
        fn position_us(&mut self, _: bool) -> i64 {
            self.played_us
        }
    }

    fn offer(t: &mut Track, b: &mut Burst, now_ms: i64) -> bool {
        let chunk = vec![0u8; FMT.bytes(26_000)];
        let mut f = Fed::new(t, b, now_ms);
        f.handle_buffer(&chunk, 0, 0).0
    }

    #[test]
    fn it_fills_to_the_top_then_leaves_the_output_alone_until_it_runs_low() {
        let (mut t, mut b) = (Track { cap_us: BUFFER_US, written_us: 0, played_us: 0, offers: 0 }, Burst::default());
        Fed::new(&mut t, &mut b, 0).configure(&(), Some(FMT));
        let mut now = 0;
        while offer(&mut t, &mut b, now) {}
        assert!(t.written_us >= BUFFER_US - 30_000, "filled: {}", t.written_us);
        let offers = t.offers;
        // Seven seconds of playing, asked every 10 ms as a player does: not one offer reaches the output.
        for _ in 0..700 {
            now += 10;
            t.played_us += 10_000;
            assert!(!offer(&mut t, &mut b, now));
        }
        assert_eq!(t.offers, offers, "refused without touching the output");
        // Below two seconds left: it is fed again, and filled (a player offers until it is refused).
        while t.offers == offers {
            now += 10;
            t.played_us += 10_000;
            offer(&mut t, &mut b, now);
        }
        assert!(t.written_us - t.played_us <= LOW_US + 30_000, "fed only once it ran low: {}", t.written_us - t.played_us);
        while offer(&mut t, &mut b, now) {}
        assert!(t.written_us - t.played_us >= BUFFER_US - 60_000, "and filled again");
    }

    #[test]
    fn a_clock_jump_is_not_counted_as_playing() {
        let (mut t, mut b) = (Track { cap_us: BUFFER_US, written_us: 0, played_us: 0, offers: 0 }, Burst::default());
        Fed::new(&mut t, &mut b, 0).configure(&(), Some(FMT));
        while offer(&mut t, &mut b, 0) {}
        offer(&mut t, &mut b, 10);
        // The clock leaps 20 s in 100 ms (a mix stamped in the next song's time is reached): at most the
        // 100 ms that passed has played, so the output is still nearly full and is left alone.
        t.played_us += 20_000_000;
        let before = t.offers;
        assert!(!offer(&mut t, &mut b, 110));
        assert_eq!(t.offers, before);
    }

    #[test]
    fn switched_off_it_passes_everything_through_and_counts_it() {
        let (mut t, mut b) = (Track { cap_us: 3_600_000_000, written_us: 0, played_us: 0, offers: 0 }, Burst { enabled: false, ..Burst::default() });
        for i in 0..50 {
            assert!(offer(&mut t, &mut b, i));
        }
        assert_eq!(t.offers, 50);
        assert_eq!(b.bytes_written as usize, 50 * FMT.bytes(26_000));
    }
}
