//! How the seek bar moves ([`SeekPace`]): drawn where the song is while it plays, and only when the song
//! has moved it a pixel or its times a second; gliding over a third of a second when the song's place jumps
//! (a new song, a seek, a mix handing over) while the times cross-fade; and set straight to the song when
//! a page comes back on screen. [`seek_step`] is the older easing, kept for the benchmarks.

/// The easing's time constant, in seconds.
const EASE_S: f32 = 0.14;
/// Closer than this, in pixels, and the bar jumps straight to the song: one draw, not an easing
/// that lands in two (most of the way, then the rest a frame later) for a move nobody can see.
const SETTLED_PX: f32 = 2.0;
/// Closer than this, in pixels, and the bar stays where it is: a song reported half a pixel on, or a
/// hair back (a position read from the audio chip jitters), changes nothing on the screen.
const STILL_PX: f32 = 0.5;
/// Waits between two draws of a bar that is keeping up: never shorter than a frame, and at least
/// once a second so a song whose length was not known yet still moves.
const MIN_WAIT_MS: i32 = 16;
const MAX_WAIT_MS: i32 = 1000;

/// One step of the bar at `bar` towards `target` (both 0..1), `dt_s` after the last one. `width_px` is
/// the bar's length on screen and `speed` how fast the song moves along it (bar lengths a second; 0
/// when paused). Returns where the bar is now and how long to wait before the next step: 0 for the
/// next frame, a number of milliseconds when it has caught up, -1 when it can stop (paused and
/// settled). A bar keeping up with a song is drawn once a pixel, when the song has moved it one.
pub fn seek_step(bar: f32, target: f32, dt_s: f32, width_px: f32, speed: f32) -> (f32, i32) {
    let width = width_px.max(1.0);
    let gap_px = (target - bar) * width;
    let next = if gap_px.abs() < STILL_PX {
        bar
    } else if gap_px.abs() < SETTLED_PX {
        target
    } else {
        let eased = bar + (target - bar) * (1.0 - (-dt_s / EASE_S).exp());
        // The easing's last step would land short by less than the jump rule allows: land it now.
        if ((target - eased) * width).abs() < SETTLED_PX { target } else { return (eased, 0) }
    };
    if speed <= 0.0 {
        return (next, -1);
    }
    // How long until the song is a pixel past where the bar is drawn.
    let ahead_px = ((target - next) * width).clamp(0.0, 1.0);
    let px_s = 1.0 / (width * speed);
    (next, (((1.0 - ahead_px) * px_s * 1000.0) as i32).clamp(MIN_WAIT_MS, MAX_WAIT_MS))
}

/// How long a jump of the bar glides (a new song, a seek, a mix handing over, a queue replaced): long
/// enough to be followed by the eye, short enough not to lag behind the music.
pub const GLIDE_S: f32 = 0.32;
/// A move of the song this many pixels past what it could have played since the last step is a jump,
/// and glides; anything smaller is the song playing, drawn where it is.
const JUMP_PX: f32 = 3.0;
/// A place this far from where the times said the song would be is a jump in the times too: they
/// cross-fade to the new ones rather than change in one frame. A second's tick is not one.
const JUMP_MS: i64 = 1_500;

/// Soft at both ends, the same curve both ways: a glide turned round in the middle does not jump.
fn ease(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The seek bar's place and the times under it, stepped towards where the song is. While the song simply
/// plays, the bar is drawn where the song is (no easing lag) and only when that has moved it a pixel, and
/// the times when their second changes. Anything else - a new song, a seek, a mix handing over, a queue
/// replaced - is a jump: the bar glides to the new place over [`GLIDE_S`], chasing it as it moves on, and
/// the times cross-fade from the old ones to the new. A page that was not on screen when the song moved is
/// [`SeekPace::sync`]ed as it comes back, so its first frame is the song's real place, not a glide from a
/// stale one.
#[derive(Debug, Clone, PartialEq)]
pub struct SeekPace {
    bar: f32,
    /// Where the glide started, NaN when none.
    from: f32,
    glide_s: f32,
    /// Where the song was at the last step, 0..1.
    target: f32,
    /// The place and length the times show.
    at_ms: i64,
    duration_ms: i64,
    /// The times fading out, and how far the fade is (`from_ms` < 0: none).
    from_ms: i64,
    from_duration_ms: i64,
    fade_s: f32,
    synced: bool,
}

impl Default for SeekPace {
    fn default() -> Self {
        SeekPace::new()
    }
}

fn fraction(position_ms: i64, duration_ms: i64) -> f32 {
    (position_ms.max(0) as f64 / duration_ms.max(1) as f64).clamp(0.0, 1.0) as f32
}

fn times(position_ms: i64, duration_ms: i64) -> (i64, i64) {
    let at = position_ms.clamp(0, duration_ms.max(0));
    (at / 1000, (duration_ms - at).max(0) / 1000)
}

impl SeekPace {
    pub const fn new() -> Self {
        SeekPace { bar: 0.0, from: f32::NAN, glide_s: 0.0, target: 0.0, at_ms: 0, duration_ms: 0, from_ms: -1, from_duration_ms: 0, fade_s: 0.0, synced: false }
    }

    /// Everything where the song is, at once: for a page coming on screen (or first drawn) after the song
    /// moved while it was away. Nothing glides from where it was left.
    pub fn sync(&mut self, position_ms: i64, duration_ms: i64) {
        let bar = fraction(position_ms, duration_ms);
        *self = SeekPace { bar, target: bar, at_ms: position_ms.max(0), duration_ms, synced: true, ..SeekPace::new() };
    }

    /// Holds the bar at `bar` (0..1) with nothing moving: where a released scrub left it.
    pub fn hold(&mut self, bar: f32, position_ms: i64, duration_ms: i64) {
        self.sync(position_ms, duration_ms);
        self.bar = bar.clamp(0.0, 1.0);
        self.target = self.bar;
    }

    /// One step, `dt_s` after the last, with the song at `position_ms` of `duration_ms` playing at `rate`
    /// times (0 paused) on a bar `width_px` long. Returns how long to wait before the next step: 0 for the
    /// next frame, a number of milliseconds while the song simply plays, -1 when nothing will move until
    /// something changes (paused, and every glide and fade done).
    pub fn step(&mut self, position_ms: i64, duration_ms: i64, dt_s: f32, width_px: f32, rate: f32) -> i32 {
        if !self.synced {
            self.sync(position_ms, duration_ms);
        }
        let width = width_px.max(1.0);
        let dt = dt_s.max(0.0);
        let rate = rate.max(0.0);
        let target = fraction(position_ms, duration_ms);
        // How far the song could have gone since the last step.
        let played_ms = (dt * 1000.0 * rate) as i64;
        let expected_px = if duration_ms > 0 { played_ms as f32 / duration_ms as f32 * width } else { 0.0 };

        // The times: a new length, or a place far from where the song would be, fades over.
        let predicted = self.at_ms + played_ms;
        if duration_ms != self.duration_ms || (position_ms - predicted).abs() >= JUMP_MS {
            let old = times(self.at_ms, self.duration_ms);
            if old != times(position_ms, duration_ms) {
                self.from_ms = self.at_ms;
                self.from_duration_ms = self.duration_ms;
                self.fade_s = 0.0;
            }
        } else if self.from_ms >= 0 {
            self.fade_s += dt;
        }
        if self.from_ms >= 0 && self.fade_s >= GLIDE_S {
            self.from_ms = -1;
        }
        self.at_ms = position_ms.max(0);
        self.duration_ms = duration_ms;

        // The bar: a jump starts a glide from wherever the bar is drawn (a glide already under way is
        // taken over from its place, so nothing jumps back).
        let gap_px = (target - self.bar) * width;
        let gliding = !self.from.is_nan();
        let jumped_again = gliding && ((target - self.target) * width).abs() > JUMP_PX + expected_px;
        if gliding && !jumped_again {
            self.glide_s += dt;
        }
        self.target = target;
        if jumped_again || !gliding && gap_px.abs() > JUMP_PX + expected_px {
            self.from = self.bar;
            self.glide_s = 0.0;
        }
        if !self.from.is_nan() {
            let t = self.glide_s / GLIDE_S;
            if t >= 1.0 {
                self.bar = target;
                self.from = f32::NAN;
            } else {
                // Chases the song as it moves on: the start holds, the end is where the song is now.
                self.bar = self.from + (target - self.from) * ease(t);
                return 0;
            }
        } else if gap_px.abs() >= STILL_PX {
            self.bar = target;
        }
        if self.from_ms >= 0 {
            return 0;
        }
        if rate <= 0.0 {
            return -1;
        }
        // Keeping up: the next draw when the song has moved the bar a pixel on, or the times' second
        // changes, whichever is first.
        let ahead_px = ((target - self.bar) * width).clamp(0.0, 1.0);
        let px_ms = if duration_ms > 0 { duration_ms as f32 / (width * rate) } else { MAX_WAIT_MS as f32 };
        let pixel = (1.0 - ahead_px) * px_ms;
        let second = (1000 - position_ms.rem_euclid(1000)) as f32 / rate;
        (pixel.min(second) as i32).clamp(MIN_WAIT_MS, MAX_WAIT_MS)
    }

    /// Where the bar is drawn, 0..1.
    pub fn bar(&self) -> f32 {
        self.bar
    }

    /// Whether a glide or a fade is under way.
    pub fn moving(&self) -> bool {
        !self.from.is_nan() || self.from_ms >= 0
    }

    /// The times shown, elapsed and left, whole seconds.
    pub fn times(&self) -> (i64, i64) {
        times(self.at_ms, self.duration_ms)
    }

    /// The times fading out, if any, and how far the new ones have come in (0..1, eased; 1 with none).
    pub fn fading(&self) -> Option<((i64, i64), f32)> {
        (self.from_ms >= 0).then(|| (times(self.from_ms, self.from_duration_ms), ease(self.fade_s / GLIDE_S)))
    }

    /// How strongly the times now are drawn, 0..1.
    pub fn fade(&self) -> f32 {
        self.fading().map_or(1.0, |(_, f)| f)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_eases_in_frames_then_waits_for_a_pixel() {
        // A seek: far from the target, the next frame.
        let (b, w) = seek_step(0.0, 0.5, 0.016, 1000.0, 1.0 / 366.0);
        assert!(b > 0.0 && b < 0.5 && w == 0);
        // A pixel behind: there in one draw, and on a six-minute song over 1000 px the next draw in
        // ~366 ms, not 16.
        let (b, w) = seek_step(0.5, 0.501, 0.016, 1000.0, 1.0 / 366.0);
        assert_eq!((b, w), (0.501, 366));
        // Paused and settled: stop.
        assert_eq!(seek_step(0.3, 0.3, 0.016, 1000.0, 0.0), (0.3, -1));
        // A very short song on a wide bar still waits at least a frame; an unknown length at most a second.
        assert_eq!(seek_step(0.3, 0.3, 0.016, 3000.0, 1.0).1, MIN_WAIT_MS);
        assert_eq!(seek_step(0.3, 0.3, 0.016, 10.0, 0.0001).1, MAX_WAIT_MS);
    }

    #[test]
    fn a_hair_either_way_is_not_drawn() {
        // A quarter pixel on, or back (the chip's position jitters): the bar stays, and waits for the
        // rest of the pixel.
        let (b, w) = seek_step(0.5, 0.50025, 0.4, 1000.0, 1.0 / 366.0);
        assert_eq!(b, 0.5);
        assert!((270..=280).contains(&w), "{w}");
        assert_eq!(seek_step(0.5, 0.49975, 0.4, 1000.0, 1.0 / 366.0).0, 0.5);
    }

    #[test]
    fn keeping_up_is_one_draw_a_pixel() {
        // Six minutes over 900 px, stepped the way the frame loop steps it: every wake that moves the
        // bar moves it a pixel or so, and none is a sub-pixel follow-up.
        let (width, secs) = (900.0f32, 366.0f32);
        let speed = 1.0 / secs;
        let (mut bar, mut t, mut draws) = (0.2f32, 0.0f32, 0);
        let mut dt = 0.016;
        while t < 10.0 {
            t += dt;
            let target = 0.2 + t * speed;
            let (next, wait) = seek_step(bar, target, dt, width, speed);
            if next != bar {
                draws += 1;
                assert!((next - bar) * width >= STILL_PX, "a sub-pixel draw at {t}");
            }
            bar = next;
            dt = if wait > 0 { wait as f32 / 1000.0 } else { 0.016 };
        }
        // 10 s of a 366 s song over 900 px is ~24.6 px: about one draw a pixel, not two.
        assert!((20..=28).contains(&draws), "{draws} draws");
    }

    /// Steps `pace` a frame at a time for `secs`, the song at `pos(t)`; returns the bar after each step.
    fn frames(pace: &mut SeekPace, secs: f32, duration_ms: i64, pos: impl Fn(f32) -> i64) -> Vec<f32> {
        let mut out = Vec::new();
        let mut t = 0.0;
        while t < secs {
            t += 0.016;
            pace.step(pos(t), duration_ms, 0.016, 1000.0, 1.0);
            out.push(pace.bar());
        }
        out
    }

    #[test]
    fn a_song_playing_is_drawn_where_it_is() {
        let mut p = SeekPace::new();
        p.sync(100_000, 200_000);
        // Stepped as the frame loop steps it, with its waits: never behind the song by more than a pixel.
        let (mut t, mut dt) = (0.0f32, 0.016f32);
        while t < 20.0 {
            t += dt;
            let pos = 100_000 + (t * 1000.0) as i64;
            let wait = p.step(pos, 200_000, dt, 1000.0, 1.0);
            assert!(((fraction(pos, 200_000) - p.bar()) * 1000.0).abs() <= 1.01, "lagging at {t}");
            assert!(!p.moving(), "a song playing is not a jump, at {t}");
            assert!(wait > 0, "waits for the next pixel or second");
            dt = wait as f32 / 1000.0;
        }
        // The times are the song's, each second as it comes.
        assert_eq!(p.times(), (120, 79));
    }

    #[test]
    fn the_next_wait_ends_at_the_next_second() {
        let mut p = SeekPace::new();
        p.sync(10_000, 3_600_000);
        // An hour over 1000 px is 3.6 s a pixel: the times still change on the second.
        assert_eq!(p.step(10_250, 3_600_000, 0.25, 1000.0, 1.0), 750);
    }

    #[test]
    fn a_new_song_glides_there_and_the_times_cross_fade() {
        let mut p = SeekPace::new();
        p.sync(150_000, 200_000);
        // The next song, from its start, playing on.
        let bars = frames(&mut p, 0.6, 180_000, |t| (t * 1000.0) as i64);
        let mut last = 0.75f32;
        for (i, b) in bars.iter().enumerate() {
            assert!((last - b) * 1000.0 <= 80.0, "a teleport at frame {i}: {last} -> {b}");
            last = *b;
        }
        assert!(bars[0] > 0.7, "the first frame is where the bar was, just set off: {}", bars[0]);
        let landed = bars.iter().position(|b| *b < 0.01).unwrap();
        assert!((15..=22).contains(&landed), "there in about a third of a second: frame {landed}");
        assert!(!p.moving());
        assert_eq!(p.times(), (0, 179));
    }

    #[test]
    fn the_times_fade_from_the_old_ones() {
        let mut p = SeekPace::new();
        p.sync(63_000, 395_000);
        assert_eq!(p.step(0, 259_000, 0.016, 1000.0, 1.0), 0);
        let ((old_at, old_left), f) = p.fading().unwrap();
        assert_eq!((old_at, old_left), (63, 332));
        assert!(f < 0.05, "the new times start faint: {f}");
        assert_eq!(p.times(), (0, 259));
        frames(&mut p, 0.4, 259_000, |t| (t * 1000.0) as i64);
        assert!(p.fading().is_none());
        assert_eq!(p.fade(), 1.0);
    }

    #[test]
    fn a_page_come_back_starts_where_the_song_is() {
        let mut p = SeekPace::new();
        p.sync(150_000, 200_000);
        // Put away; the song changed and played on. Back on screen: synced, nothing glides.
        p.sync(4_000, 180_000);
        assert_eq!(p.bar(), fraction(4_000, 180_000));
        assert!(!p.moving());
        assert!(p.step(4_016, 180_000, 0.016, 1000.0, 1.0) > 0);
        assert!(!p.moving());
    }

    #[test]
    fn a_jump_in_the_middle_of_a_glide_carries_on_from_where_the_bar_is() {
        let mut p = SeekPace::new();
        p.sync(150_000, 200_000);
        frames(&mut p, 0.1, 200_000, |_| 0);
        let mid = p.bar();
        assert!(mid > 0.1 && mid < 0.7, "{mid}");
        // Skipped again: to the middle of another song.
        let bars = frames(&mut p, 0.5, 200_000, |_| 180_000);
        assert!((bars[0] - mid).abs() * 1000.0 <= 80.0, "{mid} -> {}", bars[0]);
        assert_eq!(p.bar(), 0.9);
    }

    #[test]
    fn paused_it_settles_and_stops() {
        let mut p = SeekPace::new();
        p.sync(50_000, 200_000);
        assert_eq!(p.step(50_000, 200_000, 0.016, 1000.0, 0.0), -1);
        // A seek while paused glides, then stops.
        let mut waits = Vec::new();
        for _ in 0..40 {
            waits.push(p.step(100_000, 200_000, 0.016, 1000.0, 0.0));
        }
        assert_eq!(waits[0], 0);
        assert_eq!(*waits.last().unwrap(), -1);
        assert_eq!(p.bar(), 0.5);
    }

    #[test]
    fn a_seek_of_a_moment_is_not_a_glide() {
        let mut p = SeekPace::new();
        p.sync(100_000, 200_000);
        // Half a second on over 1000 px is 2.5 px: drawn there.
        p.step(100_516, 200_000, 0.016, 1000.0, 1.0);
        assert!(!p.moving());
    }
}
