//! How the seek bar moves. It eases towards where the song is (a 140 ms exponential approach, settled
//! within a third of a second), and once it has caught up it is only drawn again when the song has
//! moved it a pixel: on a six-minute song a phone-wide bar moves a pixel every third of a second, and
//! drawing it sixty times a second in between changed nothing on the screen.

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
}
