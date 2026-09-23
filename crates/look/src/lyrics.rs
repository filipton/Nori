//! How a page of lyrics moves with the song: when each line takes over and how long its change lasts,
//! how far into the active line the singing is, when a platform has to look again, and whether what it
//! would draw has changed at all. Lyrics are shown, not heard - nothing here touches what plays - so this
//! sits with the rest of how a page looks rather than in the player.
//!
//! A platform prepares a [`LyricClock`] once per set of lyrics and then asks it one question per frame
//! (or per wake-up) with the playhead: [`LyricClock::advance`]. The answer fits in one `i64`
//! ([`Step::pack`]), so it can cross any FFI as a primitive, and asking allocates nothing.
//!
//! Text offsets are UTF-16 units, as the core hands them out and as Java and JavaScript strings index.

use core::sync::atomic::{AtomicI64, Ordering::Relaxed};

/// How long a line change takes at most, scroll and colour together so they arrive at the same moment.
/// Long enough that the eye follows the words up rather than losing its place; short enough that a fast
/// verse, a line every second or so, is never still catching up.
pub const GLIDE_MS: i32 = 620;
/// The shortest a line change gets, on the fastest verse. Below this it stops reading as movement.
pub const MIN_GLIDE_MS: i32 = 160;
/// A change is this share of the gap to the next line, so it finishes before the next one starts.
const GLIDE_SHARE: f64 = 0.85;
/// Gaps longer than this count as this long; they all end at [`GLIDE_MS`] anyway.
const LONGEST_GAP_MS: i64 = 10_000;

/// Without the sweep the lyrics sleep until the next line takes over and wake once, rather than looking
/// three times a second and changing up to 300 ms late. Never sooner than this...
const WAKE_MIN_MS: i64 = 8;
/// ...and never later, so a seek is picked up within half a second.
const WAKE_MAX_MS: i64 = 500;
/// With the sweep on, every second display frame is plenty for a text fill, and half the redraws.
pub const SWEEP_FRAMES: u32 = 2;
/// A sweep that moved less than this many characters is not redrawn: between words, and while a held
/// note keeps the boundary still, nothing on screen changes.
const SWEEP_STEP: f32 = 0.04;

/// One press of "Sooner" or "Later", for the few songs whose timings are wrong.
pub const NUDGE_STEP_MS: i64 = 250;

/// Lines past this are never lit: the index has to fit its field in [`Step::pack`]. A song has a few
/// hundred at most.
pub const MAX_LINES: usize = (1 << ACTIVE_BITS) - 2;

/// One word (or syllable) of a line and when it is sung; `start`/`end` index the line's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Word {
    pub start_ms: i64,
    pub end_ms: i64,
    pub start: u32,
    pub end: u32,
}

/// A line as the timing needs it: when it is sung, its length in UTF-16 units and its words.
#[derive(Debug, Clone, Default)]
pub struct Line {
    pub start_ms: i64,
    pub len: u32,
    pub words: Vec<Word>,
}

/// What the lyrics look like at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Frame {
    /// The line lit and scrolled to, or -1: before the first line, or lyrics that are not timed.
    pub active: i32,
    /// How long the change into [`Frame::active`] takes; the scroll and every line's fade use it.
    pub glide_ms: i32,
    /// How far into the active line the singing is, in UTF-16 units with a fraction: 7.5 is half of the
    /// character at index 7.
    pub sung: f32,
}

/// One answer to [`LyricClock::advance`].
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Step {
    /// What to draw: the new moment when `redraw`, otherwise what is already on screen.
    pub frame: Frame,
    /// When to ask again: display frames while sweeping, milliseconds otherwise; 0 for never (the lyrics
    /// are not timed, so nothing moves).
    pub wait: u32,
    /// Whether anything on screen changed; when not, the platform should not invalidate.
    pub redraw: bool,
}

/// When each line takes over, how long its change lasts, and where its words are sung.
pub struct LyricTiming {
    synced: bool,
    word_timed: bool,
    starts: Vec<i64>,
    /// Starts never go backwards, so the line being sung can be found by halving.
    sorted: bool,
    switch_at: Vec<i64>,
    glide: Vec<i32>,
    lens: Vec<u32>,
    /// Each line's words, as a range of `words`: one allocation for the whole song.
    spans: Vec<(u32, u32)>,
    words: Vec<Word>,
}

impl LyricTiming {
    /// Each line's change is as long as the line allows and is centred on the moment it is sung, so the
    /// line is fully lit as the singing starts rather than a third of a second after. A fixed 620 ms
    /// glide that began on the timestamp both lagged every line and, on a verse faster than that, never
    /// finished - the next change always arrived first.
    pub fn new(synced: bool, word_timed: bool, lines: impl IntoIterator<Item = Line>) -> Self {
        let mut starts = Vec::new();
        let mut lens = Vec::new();
        let mut spans = Vec::new();
        let mut words = Vec::new();
        for l in lines.into_iter().take(MAX_LINES) {
            starts.push(l.start_ms);
            lens.push(l.len);
            let from = words.len() as u32;
            words.extend_from_slice(&l.words);
            spans.push((from, words.len() as u32));
        }
        let n = starts.len();
        let glide: Vec<i32> = (0..n)
            .map(|i| {
                let gap = if i + 1 < n { starts[i + 1].wrapping_sub(starts[i]) } else { i64::MAX };
                ((gap.min(LONGEST_GAP_MS) as f64 * GLIDE_SHARE) as i32).clamp(MIN_GLIDE_MS, GLIDE_MS)
            })
            .collect();
        // A line may not take over before the one ahead of it has, however short its own gap.
        let mut switch_at: Vec<i64> = Vec::with_capacity(n);
        for i in 0..n {
            let at = starts[i] - (glide[i] / 2) as i64;
            switch_at.push(if i == 0 { at } else { at.max(switch_at[i - 1] + 1) });
        }
        let sorted = starts.windows(2).all(|w| w[0] <= w[1]);
        LyricTiming { synced, word_timed, starts, sorted, switch_at, glide, lens, spans, words }
    }

    pub fn synced(&self) -> bool {
        self.synced
    }

    /// Whether the active line may fill in word by word. Only when the lyrics carry real per-word times
    /// (enhanced LRC, or a server's structured cues): spreading a line's duration across its words by
    /// length looks right for a beat and then drifts badly on a held note or a fast line, which reads as
    /// broken sync - a line at a time is honest and stays in step.
    pub fn sweeps(&self) -> bool {
        self.synced && self.word_timed
    }

    /// The line whose change has begun by `t`, or -1 before the first, whether or not the lyrics are timed.
    pub fn line_at(&self, t: i64) -> i32 {
        self.switch_at.partition_point(|&s| s <= t) as i32 - 1
    }

    /// When the line after the one at `t` takes over.
    pub fn next_switch_after(&self, t: i64) -> Option<i64> {
        self.switch_at.get((self.line_at(t) + 1) as usize).copied()
    }

    /// How long the change into `line` takes; [`GLIDE_MS`] for no line.
    pub fn glide_ms(&self, line: i32) -> i32 {
        usize::try_from(line).ok().and_then(|i| self.glide.get(i)).copied().unwrap_or(GLIDE_MS)
    }

    /// The line whose timestamp is the last one reached by `t` - the one being sung, as opposed to the
    /// one lit, which takes over half a change earlier.
    pub fn sung_line(&self, t: i64) -> Option<usize> {
        if self.sorted {
            self.starts.partition_point(|&s| s <= t).checked_sub(1)
        } else {
            self.starts.iter().rposition(|&s| s <= t)
        }
    }

    /// How far into `line` the singing is at `ms`. Inside a word the sweep is linear; between words it
    /// rests at the word's end. A line without words is all or nothing. In `f32`, step for step as the
    /// app drew it, so the boundary lands on the same pixel.
    pub fn sung_offset(&self, line: usize, ms: i64) -> f32 {
        let (Some(&(from, to)), Some(&len), Some(&start)) = (self.spans.get(line), self.lens.get(line), self.starts.get(line)) else {
            return 0.0;
        };
        if from == to {
            return if ms >= start { len as f32 } else { 0.0 };
        }
        let mut at = 0f32;
        for w in &self.words[from as usize..to as usize] {
            if ms >= w.end_ms {
                at = w.end as f32;
                continue;
            }
            if ms > w.start_ms {
                let through = (ms - w.start_ms) as f32 / (w.end_ms - w.start_ms).max(1) as f32;
                at = w.start as f32 + (w.end as i32 - w.start as i32) as f32 * through;
            }
            break;
        }
        at
    }

    /// The lyrics as they look at `t`.
    pub fn frame(&self, t: i64) -> Frame {
        let active = if self.synced { self.line_at(t) } else { -1 };
        let sung = if active >= 0 { self.sung_offset(active as usize, t) } else { 0.0 };
        Frame { active, glide_ms: self.glide_ms(active), sung }
    }

    /// Whether moving what is drawn from `shown` to `t` changes anything on screen. Without the sweep a
    /// wake-up only happens when a line is due, so it always does.
    ///
    /// The test follows the line being sung, not the one lit. Where a line's last word ends well before
    /// the next line's timestamp, the sweep therefore holds the page still until that timestamp, and the
    /// next change starts on it rather than half a change ahead as it does without the sweep. That is how
    /// the lyrics have always moved, and it is kept.
    pub fn moved(&self, shown: i64, t: i64, sweep: bool) -> bool {
        if !sweep {
            return true;
        }
        let Some(line) = self.sung_line(t) else { return true };
        self.sung_line(shown) != Some(line) || (self.sung_offset(line, t) - self.sung_offset(line, shown)).abs() >= SWEEP_STEP
    }

    /// How long until the page may look different after `t`: see [`Step::wait`].
    pub fn wait(&self, t: i64, sweep: bool) -> u32 {
        if !self.synced {
            0
        } else if sweep {
            SWEEP_FRAMES
        } else {
            self.next_switch_after(t).map_or(WAKE_MAX_MS, |next| (next - t).clamp(WAKE_MIN_MS, WAKE_MAX_MS)) as u32
        }
    }
}

/// A [`LyricTiming`] with what is on screen: the moment last drawn and the listener's nudge. Both are
/// atomics, so the clock can be shared with a platform's UI thread without a lock.
pub struct LyricClock {
    timing: LyricTiming,
    shown: AtomicI64,
    nudge: AtomicI64,
}

impl LyricClock {
    /// Starts showing the moment `position_ms`, un-nudged.
    pub fn new(timing: LyricTiming, position_ms: i64) -> Self {
        LyricClock { timing, shown: AtomicI64::new(position_ms), nudge: AtomicI64::new(0) }
    }

    pub fn timing(&self) -> &LyricTiming {
        &self.timing
    }

    /// The per-frame question: the player is at `position_ms`; `sweep` is whether the listener wants the
    /// fill (it only happens when the lyrics allow it). `force` draws the new moment whatever changed,
    /// for the first look after starting or resuming.
    pub fn advance(&self, position_ms: i64, sweep: bool, force: bool) -> Step {
        let sweep = sweep && self.timing.sweeps();
        let t = position_ms + self.nudge.load(Relaxed);
        let redraw = force || self.timing.moved(self.shown.load(Relaxed), t, sweep);
        if redraw {
            self.shown.store(t, Relaxed);
        }
        Step { frame: self.timing.frame(self.shown.load(Relaxed)), wait: self.timing.wait(t, sweep), redraw }
    }

    /// What is on screen now.
    pub fn shown(&self) -> Frame {
        self.timing.frame(self.shown.load(Relaxed))
    }

    /// A tap on `line`: shows it at once and returns where the player should seek to, which is the line's
    /// timestamp less the nudge, so the words land where they are drawn.
    pub fn tap(&self, line: usize) -> i64 {
        let Some(&start) = self.timing.starts.get(line) else { return self.shown.load(Relaxed).max(0) };
        self.shown.store(start, Relaxed);
        (start - self.nudge.load(Relaxed)).max(0)
    }

    /// `dir` > 0 moves the words sooner, < 0 later, 0 puts them back; returns the nudge in ms. Picked up
    /// by the next [`LyricClock::advance`].
    pub fn nudge(&self, dir: i32) -> i64 {
        let step = NUDGE_STEP_MS * dir.signum() as i64;
        if dir == 0 {
            self.nudge.store(0, Relaxed);
            0
        } else {
            self.nudge.fetch_add(step, Relaxed) + step
        }
    }
}

// ---- one i64 per answer -------------------------------------------------------------------------------------

const SUNG_BITS: u32 = 30;
/// Fraction bits of the sung offset: it crosses within a quarter of a millionth of a character of what was
/// worked out, a ten-thousandth of a pixel on the widest glyph, for lines of up to 4095 characters.
pub const SUNG_FRAC: u32 = 18;
const ACTIVE_BITS: u32 = 13;
const GLIDE_BITS: u32 = 10;
const WAIT_BITS: u32 = 9;
const ACTIVE_AT: u32 = SUNG_BITS;
const GLIDE_AT: u32 = ACTIVE_AT + ACTIVE_BITS;
const WAIT_AT: u32 = GLIDE_AT + GLIDE_BITS;
const REDRAW_AT: u32 = WAIT_AT + WAIT_BITS;
/// The bits of a packed [`Step`] that describe the [`Frame`]; the rest say when to ask again.
pub const FRAME_BITS: u32 = WAIT_AT;

fn field(v: i64, bits: u32) -> i64 {
    v.clamp(0, (1 << bits) - 1)
}

impl Frame {
    /// `sung` in the low 30 bits (fixed point, [`SUNG_FRAC`] fraction bits), then `active + 1` in 13 and
    /// `glide_ms` in 10.
    pub fn pack(&self) -> i64 {
        let sung = field((self.sung.max(0.0) * (1u32 << SUNG_FRAC) as f32) as i64, SUNG_BITS);
        sung | field(self.active as i64 + 1, ACTIVE_BITS) << ACTIVE_AT | field(self.glide_ms as i64, GLIDE_BITS) << GLIDE_AT
    }

    pub fn unpack(v: i64) -> Frame {
        Frame {
            active: ((v >> ACTIVE_AT) & ((1 << ACTIVE_BITS) - 1)) as i32 - 1,
            glide_ms: ((v >> GLIDE_AT) & ((1 << GLIDE_BITS) - 1)) as i32,
            sung: (v & ((1 << SUNG_BITS) - 1)) as f32 / (1u32 << SUNG_FRAC) as f32,
        }
    }
}

impl Step {
    /// The frame's bits ([`Frame::pack`]), then `wait` in 9 and `redraw` in 1: 63 bits, never negative.
    pub fn pack(&self) -> i64 {
        self.frame.pack() | field(self.wait as i64, WAIT_BITS) << WAIT_AT | (self.redraw as i64) << REDRAW_AT
    }

    pub fn unpack(v: i64) -> Step {
        Step { frame: Frame::unpack(v), wait: ((v >> WAIT_AT) & ((1 << WAIT_BITS) - 1)) as u32, redraw: (v >> REDRAW_AT) & 1 == 1 }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(starts: &[i64]) -> Vec<Line> {
        starts.iter().map(|&s| Line { start_ms: s, len: 10, words: Vec::new() }).collect()
    }

    fn w(start_ms: i64, end_ms: i64, start: u32, end: u32) -> Word {
        Word { start_ms, end_ms, start, end }
    }

    #[test]
    fn a_change_is_85_percent_of_the_gap_between_160_and_620_ms() {
        let t = LyricTiming::new(true, false, lines(&[0, 100, 400, 1000, 11_000, 12_000]));
        // 100 ms gap -> 85 -> 160; 300 -> 255; 600 -> 510; 10 s -> 620; 1 s -> 850 -> 620; last -> 620.
        assert_eq!(t.glide, [160, 255, 510, 620, 620, 620]);
        assert_eq!((t.glide_ms(-1), t.glide_ms(6), t.glide_ms(2)), (GLIDE_MS, GLIDE_MS, 510));
        // 0.85 of 301 ms is 255.85, truncated as the app did.
        assert_eq!(LyricTiming::new(true, false, lines(&[0, 301])).glide[0], 255);
    }

    #[test]
    fn a_line_takes_over_half_its_change_early_but_never_before_the_one_ahead() {
        let t = LyricTiming::new(true, false, lines(&[1000, 1100, 1110, 5000]));
        // Glides 160, 160, 620 (3890 gap -> 3306 -> 620), 620: leads 80, 80, 310, 310.
        assert_eq!(t.switch_at, [920, 1020, 1021, 4690]);
        assert_eq!((t.line_at(919), t.line_at(920), t.line_at(1020), t.line_at(1021), t.line_at(4689), t.line_at(4690)), (-1, 0, 1, 2, 2, 3));
        assert_eq!((t.next_switch_after(0), t.next_switch_after(1021), t.next_switch_after(4690)), (Some(920), Some(4690), None));
    }

    #[test]
    fn the_sweep_is_linear_in_a_word_and_rests_between_words() {
        let t = LyricTiming::new(true, true, vec![Line { start_ms: 1000, len: 11, words: vec![w(1000, 1400, 0, 5), w(1600, 2000, 6, 11)] }]);
        let at = |ms| t.sung_offset(0, ms);
        assert_eq!((at(900), at(1000), at(1200), at(1400), at(1500), at(1600), at(1700), at(2000), at(9000)), (0.0, 0.0, 2.5, 5.0, 5.0, 5.0, 7.25, 11.0, 11.0));
        // A word with no length is done the moment it starts; a line without words is all or nothing.
        let t = LyricTiming::new(true, true, vec![Line { start_ms: 0, len: 4, words: vec![w(500, 500, 0, 4)] }, Line { start_ms: 800, len: 7, words: vec![] }]);
        assert_eq!((t.sung_offset(0, 499), t.sung_offset(0, 500), t.sung_offset(1, 799), t.sung_offset(1, 800)), (0.0, 4.0, 0.0, 7.0));
    }

    #[test]
    fn the_line_sung_follows_timestamps_and_the_line_lit_leads_it() {
        let t = LyricTiming::new(true, true, lines(&[1000, 3000]));
        assert_eq!((t.sung_line(999), t.sung_line(1000), t.sung_line(2999), t.sung_line(3000)), (None, Some(0), Some(0), Some(1)));
        assert_eq!(t.line_at(2700), 1, "lit 310 ms before it is sung");
        // Out of order starts still find the last one reached, as the app's indexOfLast did.
        let t = LyricTiming::new(true, true, lines(&[1000, 500, 2000]));
        assert_eq!((t.sung_line(600), t.sung_line(1500)), (Some(1), Some(1)));
    }

    #[test]
    fn waking_is_at_the_next_change_within_8_to_500_ms_and_every_second_frame_when_sweeping() {
        let t = LyricTiming::new(true, false, lines(&[1000, 1200, 5000]));
        // Glides 170, 620, 620; switch_at: 1000-85=915, 1200-310=890 -> 916, 5000-310=4690.
        assert_eq!((t.wait(0, false), t.wait(700, false), t.wait(914, false), t.wait(921, false), t.wait(4700, false)), (500, 215, 8, 500, 500));
        assert_eq!(t.wait(0, true), SWEEP_FRAMES);
        assert_eq!(LyricTiming::new(false, false, lines(&[-1, -1])).wait(0, false), 0, "untimed lyrics never wake");
    }

    #[test]
    fn only_a_visible_change_of_the_sweep_redraws() {
        let t = LyricTiming::new(true, true, vec![Line { start_ms: 1000, len: 10, words: vec![w(1000, 2000, 0, 10)] }, Line { start_ms: 5000, len: 3, words: vec![] }]);
        assert!(t.moved(0, 500, false), "without the sweep every wake-up is a change");
        assert!(t.moved(0, 500, true), "before the first line");
        assert!(!t.moved(1000, 1003, true), "0.03 characters");
        assert!(t.moved(1000, 1005, true), "0.05 characters");
        assert!(!t.moved(2000, 4000, true), "between words nothing moves");
        // The lit line changes at 4690 but the sung one only at 5000: the page holds until then.
        assert!(!t.moved(2000, 4800, true) && t.line_at(4800) == 1);
        assert!(t.moved(2000, 5000, true));
    }

    #[test]
    fn a_clock_draws_only_what_changed_and_nudges_and_taps() {
        let timing = LyricTiming::new(true, true, vec![Line { start_ms: 1000, len: 10, words: vec![w(1000, 2000, 0, 10)] }, Line { start_ms: 5000, len: 3, words: vec![] }]);
        let c = LyricClock::new(timing, 0);
        assert_eq!(c.shown(), Frame { active: -1, glide_ms: GLIDE_MS, sung: 0.0 });
        let s = c.advance(1500, true, false);
        assert_eq!(s, Step { frame: Frame { active: 0, glide_ms: 620, sung: 5.0 }, wait: SWEEP_FRAMES, redraw: true });
        let s = c.advance(1502, true, false);
        assert!(!s.redraw && s.frame.sung == 5.0, "what is on screen stays");
        assert!(c.advance(1502, true, true).redraw);
        assert_eq!(c.nudge(1), 250);
        assert_eq!(c.advance(1500, true, false).frame.sung, 7.5, "sooner: the words are ahead of the player");
        assert_eq!((c.nudge(-1), c.nudge(-1), c.nudge(-1)), (0, -250, -500));
        assert_eq!(c.tap(1), 5500, "seek to where the line is drawn");
        assert_eq!(c.shown().active, 1);
        assert_eq!(c.nudge(0), 0);
        assert_eq!(c.tap(0), 1000);
        c.nudge(1);
        c.nudge(1);
        assert_eq!(c.tap(0), 500);
        // Words not timed: no sweep whatever the listener asked for, and a wake-up at the next line.
        let c = LyricClock::new(LyricTiming::new(true, false, lines(&[1000, 5000])), 0);
        assert_eq!(c.advance(0, true, false).wait, 500);
        assert_eq!(c.advance(4400, true, false).wait, 290);
    }

    #[test]
    fn untimed_lyrics_light_nothing_and_never_wake() {
        let c = LyricClock::new(LyricTiming::new(false, false, lines(&[-1, -1, -1])), 0);
        let s = c.advance(10_000, true, true);
        assert_eq!(s, Step { frame: Frame { active: -1, glide_ms: GLIDE_MS, sung: 0.0 }, wait: 0, redraw: true });
    }

    #[test]
    fn a_packed_step_comes_back() {
        let third = Step::unpack(Step { frame: Frame { active: 0, glide_ms: 620, sung: 1.0 / 3.0 }, wait: 0, redraw: false }.pack()).frame.sung;
        assert!((third - 1.0 / 3.0).abs() < 1.0 / (1 << SUNG_FRAC) as f32);
        for sung in [0.0f32, 7.5, 7.25, 63.999_99, 1234.567, 4095.99] {
            for (active, glide_ms, wait, redraw) in [(-1, 620, 0, false), (0, 160, 2, true), (8190, 594, 500, true)] {
                let s = Step { frame: Frame { active, glide_ms, sung }, wait, redraw };
                assert!(s.pack() >= 0);
                assert_eq!(Step::unpack(s.pack()), s);
                assert_eq!(Frame::unpack(s.pack() & ((1 << FRAME_BITS) - 1)), s.frame);
            }
        }
    }
}
