//! How the app moves and lays its pages out, as numbers and small rules every front end shares: when a
//! drag changes the record and when it springs back, how long the waits and fades are, which glyph the
//! transport shows, where the seek bar's time comes from, the sleeve's geometry and every gradient's
//! stops (`nori_look::sleeve`). The platform reads [`stage`] once, at start, and asks the rules at the
//! moment something happens (a finger lifts, a song changes) - never per frame.

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
    /// Width over height of the player's sleeve.
    pub sleeve: f32,
    /// How much of the sleeve runs on under the title block.
    pub sleeve_under_text: f32,
    /// How much of the width a record held by a finger takes.
    pub lifted_width: f32,
    pub rub_out: Vec<GradientStop>,
    pub soft: Vec<GradientStop>,
    pub soft_from: f32,
    pub soft_to: f32,
    pub status_shade: f32,
    pub status_shade_to: f32,
    pub hero_stops: Vec<f32>,
    pub floor_stops: Vec<f32>,
    pub lyrics_mask: Vec<GradientStop>,

    /// A slow drag changes the record past this share of the width; the same share arms a row's swipe.
    pub turn: f32,
    /// Towards a record that is not there a drag gives this much of the finger's travel, and no more
    /// than this share of the width.
    pub give: f32,
    pub give_limit: f32,
    /// How far the back gesture takes the player down before it is let go.
    pub back_travel: f32,

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
    /// Data that arrives within this long of a page opening was never waited for: it snaps in.
    pub quick_load_ms: i64,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn stage() -> Stage {
    Stage {
        melt: nori_look::cover::MELT,
        sleeve: sleeve::SLEEVE,
        sleeve_under_text: sleeve::SLEEVE_UNDER_TEXT,
        lifted_width: sleeve::LIFTED_WIDTH,
        rub_out: stops(&sleeve::RUB_OUT),
        soft: stops(&sleeve::SOFT),
        soft_from: sleeve::SOFT_FROM,
        soft_to: sleeve::SOFT_TO,
        status_shade: sleeve::STATUS_SHADE,
        status_shade_to: sleeve::STATUS_SHADE_TO,
        hero_stops: sleeve::HERO_STOPS.to_vec(),
        floor_stops: sleeve::FLOOR_STOPS.to_vec(),
        lyrics_mask: stops(&sleeve::LYRICS_MASK),
        turn: TURN,
        give: GIVE,
        give_limit: GIVE_LIMIT,
        back_travel: BACK_TRAVEL,
        spinner_after_ms: 300,
        panel_ms: 360,
        sleeve_hold_ms: 600,
        colour_wait_ms: 1_200,
        colour_fade_ms: 420,
        lyrics_reading_ms: nori_look::lyrics::READING_MS,
        quick_load_ms: 300,
    }
}

// ---- gestures ----------------------------------------------------------------------------------------------

const TURN: f32 = 0.3;
/// A release faster than this, in pixels a second, changes the record whatever the distance: on the
/// player's sleeve...
const FLICK_PX_S: f32 = 1_000.0;
/// ...and on the now playing bar, a small strip under the thumb, where a flick is shorter and slower.
const BAR_FLICK_PX_S: f32 = 900.0;
const GIVE: f32 = 0.2;
const GIVE_LIMIT: f32 = 0.06;
/// The player sheet: a release faster than this goes the way it was flicked...
const SHEET_FLICK_PX_S: f32 = 900.0;
/// ...and a slow drag that has come this share of the way finishes the move it started.
const SHEET_COMMIT: f32 = 0.15;
const BACK_TRAVEL: f32 = 0.2;

/// Where a sideways drag on a row of records goes when the finger lifts at `offset` pixels (negative:
/// towards the next) with `velocity` pixels a second, on a row `width` wide: -1 to the next record, 1 to
/// the one before, 0 back where it was. Past [`TURN`] of the width, or flicked; never towards a record
/// that is not there. `bar` is the now playing bar, which takes a slower flick than the sleeve.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn swipe_turn(offset: f32, velocity: f32, width: f32, has_before: bool, has_after: bool, bar: bool) -> i32 {
    let flick = if bar { BAR_FLICK_PX_S } else { FLICK_PX_S };
    if offset < 0.0 && has_after && (velocity < -flick || offset < -width * TURN) {
        -1
    } else if offset > 0.0 && has_before && (velocity > flick || offset > width * TURN) {
        1
    } else {
        0
    }
}

/// Where the player sheet settles when the finger lifts: 1 open, 0 put away. `from` is the end it was
/// nearer when the drag began, `progress` where it is now and `velocity` the finger's (down positive).
/// A flick goes the way it was flicked; a drag that has come a little way finishes the move it started,
/// and a smaller one goes back. Deciding by the halfway point instead meant a pull down from the player
/// had to cover half the screen before it would close.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn sheet_target(from: f32, progress: f32, velocity: f32) -> f32 {
    let moved = progress - from;
    if velocity < -SHEET_FLICK_PX_S {
        1.0
    } else if velocity > SHEET_FLICK_PX_S {
        0.0
    } else if moved > SHEET_COMMIT {
        1.0
    } else if moved < -SHEET_COMMIT {
        0.0
    } else {
        from
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

/// What the middle of the seek bar's times says: an error, or the sleep timer (`sleep_left_ms` None when
/// none is set by time), or nothing - it holds its place so the two times either side never move. It
/// said "Mixing" through every crossfade once, which is a word about the plumbing rather than the music.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn seek_middle(error: Option<String>, sleep_end_of_track: bool, sleep_left_ms: Option<i64>) -> String {
    if let Some(e) = error {
        return e;
    }
    if sleep_end_of_track || sleep_left_ms.is_some() {
        return crate::words::words_sleep(sleep_end_of_track, sleep_left_ms.unwrap_or(0));
    }
    String::new()
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

/// `order` is the player's play order; when it does not cover the queue (the player has not said yet)
/// the list's own order stands in.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn queue_rows(order: Vec<u32>, len: u32, shuffle: bool) -> QueueRows {
    let order = if order.len() == len as usize { order } else { (0..len).collect() };
    QueueRows { order, reorderable: !shuffle }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_swipe_turns_past_a_third_or_flicked() {
        assert_eq!(swipe_turn(-310.0, 0.0, 1000.0, true, true, false), -1);
        assert_eq!(swipe_turn(-290.0, 0.0, 1000.0, true, true, false), 0);
        assert_eq!(swipe_turn(-20.0, -1_001.0, 1000.0, true, true, false), -1);
        assert_eq!(swipe_turn(-20.0, -999.0, 1000.0, true, true, false), 0);
        assert_eq!(swipe_turn(-500.0, -5_000.0, 1000.0, true, false, false), 0, "no record that way");
        assert_eq!(swipe_turn(400.0, 0.0, 1000.0, true, false, false), 1);
        assert_eq!(swipe_turn(40.0, 1_200.0, 1000.0, false, true, false), 0);
        // The now playing bar takes the slower flick it always did.
        assert_eq!(swipe_turn(-20.0, -950.0, 1000.0, true, true, true), -1);
        assert_eq!(swipe_turn(-20.0, -899.0, 1000.0, true, true, true), 0);
    }

    #[test]
    fn the_sheet_finishes_what_a_drag_started() {
        assert_eq!(sheet_target(0.0, 0.1, -901.0), 1.0);
        assert_eq!(sheet_target(1.0, 0.9, 901.0), 0.0);
        assert_eq!(sheet_target(0.0, 0.16, 0.0), 1.0);
        assert_eq!(sheet_target(1.0, 0.84, 0.0), 0.0);
        assert_eq!(sheet_target(1.0, 0.9, 0.0), 1.0);
        assert_eq!(sheet_target(0.0, 0.1, 500.0), 0.0);
    }

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
        assert_eq!(seek_middle(Some("Offline".into()), true, Some(1)), "Offline");
        assert_eq!(seek_middle(None, true, None), "Sleep · end of track");
        assert_eq!(seek_middle(None, false, Some(60_000)), "Sleep · 1 min");
        assert_eq!(seek_middle(None, false, None), "");
    }

    #[test]
    fn the_queue_lists_in_play_order_and_reorders_only_unshuffled() {
        assert_eq!(queue_rows(vec![2, 0, 1], 3, true), QueueRows { order: vec![2, 0, 1], reorderable: false });
        assert_eq!(queue_rows(vec![], 3, false), QueueRows { order: vec![0, 1, 2], reorderable: true });
    }

    #[test]
    fn one_stage() {
        let s = stage();
        assert_eq!((s.melt, s.sleeve, s.turn, s.spinner_after_ms, s.colour_wait_ms), (0.19, 0.74, 0.3, 300, 1_200));
        assert_eq!(s.rub_out.len(), 7);
        assert_eq!(band_matrix(0.0, 1.0, 1.0, 1.0)[0], 1.0);
        assert!(player_black(true, false));
    }
}
