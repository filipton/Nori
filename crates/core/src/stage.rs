//! How the app draws its pages and times them, as numbers and small rules every front end shares: how
//! long the waits and fades are, which glyph the transport shows, where the seek bar's time comes from and
//! every gradient's stops (`nori_look::sleeve`). Where things sit on a phone's screen is the phone's. The platform reads [`stage`] once,
//! at start, and asks the rules at the moment something happens (a song changes) - never per frame. How a
//! touch gesture feels (flick speeds, how far a drag turns a record, where the sheet settles) is the
//! platform's own: a desktop or terminal client has other input.

use nori_look::sleeve;

/// One stop of a gradient: where along it (0..1) and how opaque.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct GradientStop {
    pub at: f32,
    pub alpha: f32,
}

fn stops(s: &[sleeve::Stop]) -> Vec<GradientStop> {
    s.iter().map(|s| GradientStop { at: s.at, alpha: s.alpha }).collect()
}

/// Everything the platform lays out and times by, read once.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Stage {
    /// How much of the sleeve's height goes soft at its bottom; the same share its colour is averaged
    /// from (`nori_look::cover`).
    pub melt: f32,
    pub rub_out: Vec<GradientStop>,
    pub soft: Vec<GradientStop>,
    pub soft_from: f32,
    pub soft_to: f32,
    pub status_shade: f32,
    pub status_shade_to: f32,
    pub hero_stops: Vec<f32>,
    pub floor_stops: Vec<f32>,
    pub lyrics_mask: Vec<GradientStop>,

    /// The spinner in Play only comes in after this long buffering: most skips start inside it, and a
    /// spinner flicking into the pause button for a frame made skipping feel rough.
    pub spinner_after_ms: i64,
    /// How long the artwork, the lyrics and the queue take to dissolve into one another.
    pub panel_ms: i32,
    /// How long a skip keeps the old picture on the sleeve before letting it go to the plate.
    pub sleeve_hold_ms: i64,
    /// A song with no colours of its own gets the plain page once it has had this long to find some.
    pub colour_wait_ms: i64,
    /// How long the page's colours take to cross-fade to a song's (as long as a record takes to slide).
    pub colour_fade_ms: i32,
    /// How long the lyrics stay where a finger left them.
    pub lyrics_reading_ms: i64,
    /// A sung word's rise takes at least this long, its settling back this long once it is done, and a
    /// note held `lyrics_held_ms` or more glows, fading over `lyrics_glow_fade_ms` after it ends. The
    /// clock draws every frame while any of it moves (`nori_look::lyrics::MOTION_TAIL_MS`).
    pub lyrics_rise_min_ms: i64,
    pub lyrics_settle_ms: i64,
    pub lyrics_held_ms: i64,
    pub lyrics_glow_fade_ms: i64,
    /// How lit the unsung words of the line being filled are (`nori_look::lyrics::UNSUNG`).
    pub lyrics_unsung: f32,
    /// Data that arrives within this long of a page opening was never waited for: it snaps in.
    pub quick_load_ms: i64,
    /// How often the limiter's meter is read while the equalizer is on screen: quick enough to follow a
    /// peak, slow enough that the screen is not redrawn for nothing between them.
    pub meter_ms: i64,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn stage() -> Stage {
    Stage {
        melt: nori_look::cover::MELT,
        rub_out: stops(&sleeve::RUB_OUT),
        soft: stops(&sleeve::SOFT),
        soft_from: sleeve::SOFT_FROM,
        soft_to: sleeve::SOFT_TO,
        status_shade: sleeve::STATUS_SHADE,
        status_shade_to: sleeve::STATUS_SHADE_TO,
        hero_stops: sleeve::HERO_STOPS.to_vec(),
        floor_stops: sleeve::FLOOR_STOPS.to_vec(),
        lyrics_mask: stops(&sleeve::LYRICS_MASK),
        spinner_after_ms: 300,
        panel_ms: 360,
        sleeve_hold_ms: 600,
        colour_wait_ms: 1_200,
        colour_fade_ms: 420,
        lyrics_reading_ms: nori_look::lyrics::READING_MS,
        lyrics_rise_min_ms: nori_look::lyrics::RISE_MIN_MS,
        lyrics_settle_ms: nori_look::lyrics::SETTLE_MS,
        lyrics_held_ms: nori_look::lyrics::HELD_MS,
        lyrics_glow_fade_ms: nori_look::lyrics::GLOW_FADE_MS,
        lyrics_unsung: nori_look::lyrics::UNSUNG,
        quick_load_ms: 300,
        meter_ms: 120,
    }
}

// ---- the transport -----------------------------------------------------------------------------------------

/// What the play button shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum TransportGlyph {
    Play,
    Pause,
    /// Buffering long enough to notice.
    Spinner,
}

/// The play button: a spinner once buffering has lasted `spinner_after_ms` (`waited`); before that the
/// wait is "playing" - the player is going to play, that is what buffering means - and showing Play
/// meanwhile said "paused" for a fraction of a second after every skip.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn transport_glyph(playing: bool, buffering: bool, waited: bool) -> TransportGlyph {
    if waited {
        TransportGlyph::Spinner
    } else if playing || buffering {
        TransportGlyph::Pause
    } else {
        TransportGlyph::Play
    }
}

/// Whether movement is kept to a minimum: the app's own switch, or the system's animations turned off
/// (Developer options, or the accessibility setting some people rely on) unless the listener asked the
/// app to animate regardless.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn motion_reduced(reduce: bool, ignore_system: bool, system_off: bool) -> bool {
    reduce || (system_off && !ignore_system)
}

/// Whether the player's page goes black (see `nori_look::sleeve::player_black`).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn player_black(amoled: bool, player_colours: bool) -> bool {
    sleeve::player_black(amoled, player_colours)
}

/// The sleeve band's colour matrix for the look's band tint (see `nori_look::sleeve::band_matrix`).
/// Asked once per tint and kept.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn band_matrix(strength: f32, kr: f32, kg: f32, kb: f32) -> Vec<f32> {
    sleeve::band_matrix(strength, kr, kg, kb).to_vec()
}

/// Whether the screen stays on for the lyrics.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn lyrics_keep_screen_on(asked: bool, shown: bool, playing: bool) -> bool {
    nori_look::lyrics::keeps_screen_on(asked, shown, playing)
}

// ---- the seek bar ------------------------------------------------------------------------------------------

/// The times either side of the seek bar, in whole seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SeekTimes {
    pub at_s: i64,
    pub left_s: i64,
}

/// The seek bar shows one of three places: the finger's, while it is down (`drag`, a share of the bar);
/// the place a released scrub asked for (`held_ms`, -1 for none) until the player is really there;
/// otherwise the music's. The time left is counted from the same place.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn seek_times(dragging: bool, drag: f32, held_ms: i64, position_ms: i64, duration_ms: i64) -> SeekTimes {
    let d = duration_ms.max(1) as f32;
    let shown = if dragging {
        (drag * d) as i64
    } else if held_ms >= 0 {
        held_ms
    } else {
        position_ms
    };
    SeekTimes { at_s: shown / 1000, left_s: (duration_ms - shown).max(0) / 1000 }
}

// ---- the queue panel ---------------------------------------------------------------------------------------

/// The queue as the panel lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct QueueRows {
    /// Positions in the queue in the order they will play (under shuffle not the list's own order).
    pub order: Vec<u32>,
    /// A drag moves a song within the list, so reordering is offered only when the two orders are one.
    pub reorderable: bool,
}

/// The panel's rows for a queue of `len` songs as the page holds it, in the core's play order; when that
/// does not cover the page's queue (the change has not reached it yet) the list's own order stands in.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn queue_rows(len: u32, shuffle: bool) -> QueueRows {
    rows(crate::playlist::with(|p| (p.len() == len as usize).then(|| p.play_order().map(|i| i as u32).collect())), len, shuffle)
}

fn rows(order: Option<Vec<u32>>, len: u32, shuffle: bool) -> QueueRows {
    let order = order.filter(|o| o.len() == len as usize).unwrap_or_else(|| (0..len).collect());
    QueueRows { order, reorderable: !shuffle }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn play_shows_pause_while_it_waits_and_spins_only_late() {
        assert_eq!(transport_glyph(false, true, false), TransportGlyph::Pause);
        assert_eq!(transport_glyph(false, true, true), TransportGlyph::Spinner);
        assert_eq!(transport_glyph(true, false, false), TransportGlyph::Pause);
        assert_eq!(transport_glyph(false, false, false), TransportGlyph::Play);
        assert!(motion_reduced(true, true, false));
        assert!(motion_reduced(false, false, true));
        assert!(!motion_reduced(false, true, true));
        assert!(!motion_reduced(false, false, false));
    }

    #[test]
    fn the_seek_bar_reads_finger_then_hold_then_music() {
        assert_eq!(seek_times(true, 0.5, 9_000, 1_000, 200_000), SeekTimes { at_s: 100, left_s: 100 });
        assert_eq!(seek_times(false, 0.5, 9_000, 1_000, 200_000), SeekTimes { at_s: 9, left_s: 191 });
        assert_eq!(seek_times(false, 0.5, -1, 61_500, 200_000), SeekTimes { at_s: 61, left_s: 138 });
        assert_eq!(seek_times(false, 0.0, -1, 5_000, 0), SeekTimes { at_s: 5, left_s: 0 });
    }

    #[test]
    fn the_queue_lists_in_play_order_and_reorders_only_unshuffled() {
        assert_eq!(rows(Some(vec![2, 0, 1]), 3, true), QueueRows { order: vec![2, 0, 1], reorderable: false });
        assert_eq!(rows(None, 3, false), QueueRows { order: vec![0, 1, 2], reorderable: true });
    }
}
