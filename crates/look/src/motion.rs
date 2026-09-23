//! How the seek bar moves. It eases towards where the song is (a 140 ms exponential approach, settled
//! within a third of a second), and once it has caught up it is only drawn again when the song has
//! moved it a pixel: on a six-minute song a phone-wide bar moves a pixel every third of a second, and
//! drawing it sixty times a second in between changed nothing on the screen.

/// The easing's time constant, in seconds.
const EASE_S: f32 = 0.14;
/// Closer than this, in pixels, and the bar is where the song is: one step lands in one frame.
const SETTLED_PX: f32 = 1.0;
/// Waits between two draws of a bar that is keeping up: never shorter than a frame, and at least
/// once a second so a song whose length was not known yet still moves.
const MIN_WAIT_MS: i32 = 16;
const MAX_WAIT_MS: i32 = 1000;

/// One step of the bar at `bar` towards `target` (both 0..1), `dt_s` after the last one. `width_px` is
/// the bar's length on screen and `speed` how fast the song moves along it (bar lengths a second; 0
/// when paused). Returns where the bar is now and how long to wait before the next step: 0 for the
/// next frame, a number of milliseconds when it has caught up, -1 when it can stop (paused and
/// settled).
pub fn seek_step(bar: f32, target: f32, dt_s: f32, width_px: f32, speed: f32) -> (f32, i32) {
    let width = width_px.max(1.0);
    let gap = target - bar;
    let next = if gap.abs() * width < SETTLED_PX { target } else { bar + gap * (1.0 - (-dt_s / EASE_S).exp()) };
    if next != target {
        return (next, 0);
    }
    if speed <= 0.0 {
        return (next, -1);
    }
    // How long until the song has moved the bar a pixel.
    let px_s = 1.0 / (width * speed);
    (next, ((px_s * 1000.0) as i32).clamp(MIN_WAIT_MS, MAX_WAIT_MS))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_eases_in_frames_then_waits_for_a_pixel() {
        // A seek: far from the target, the next frame.
        let (b, w) = seek_step(0.0, 0.5, 0.016, 1000.0, 1.0 / 366.0);
        assert!(b > 0.0 && b < 0.5 && w == 0);
        // A pixel behind: there in one frame, and on a six-minute song over 1000 px the next draw in
        // ~366 ms, not 16.
        let (b, w) = seek_step(0.5, 0.5009, 0.016, 1000.0, 1.0 / 366.0);
        assert_eq!((b, w), (0.5009, 366));
        // Paused and settled: stop.
        assert_eq!(seek_step(0.3, 0.3, 0.016, 1000.0, 0.0), (0.3, -1));
        // A very short song on a wide bar still waits at least a frame; an unknown length at most a second.
        assert_eq!(seek_step(0.3, 0.3, 0.016, 3000.0, 1.0).1, MIN_WAIT_MS);
        assert_eq!(seek_step(0.3, 0.3, 0.016, 10.0, 0.0001).1, MAX_WAIT_MS);
    }
}
