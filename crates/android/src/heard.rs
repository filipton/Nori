//! Which song the ear is on and where (`nori_core::heard`), asked every frame the seek bar is drawn:
//! primitives in and one packed `long` out, nothing allocated on either side.

use jni::sys::{jboolean, jint, jlong};
use nori_core::heard::HeardClock;
use parking_lot::Mutex;

use crate::{native, Class};

pub(crate) static HEARD: Class = Class {
    name: c"dev/nori/music/playback/HeardJni",
    methods: &[native!(c"create", c"()J", create), native!(c"destroy", c"(J)V", destroy), native!(c"at", c"(JJZIIJ)J", at)],
};

pub(crate) static PLAYHEAD: Class = Class {
    name: c"dev/nori/music/playback/PlayheadJni",
    methods: &[
        native!(c"position", c"(JJZIIJI)J", position),
        native!(c"runOn", c"(JJZ)J", run_on),
        native!(c"jumped", c"(J)V", jumped),
        native!(c"durationMs", c"(JJJ)J", duration_ms),
    ],
};

fn clock<'a>(h: jlong) -> Option<&'a Mutex<HeardClock>> {
    // SAFETY: a non-zero `h` is a pointer `create` made, and Kotlin never passes one on after `destroy`.
    (h != 0).then(|| unsafe { &*(h as *const Mutex<HeardClock>) })
}

extern "system" fn create() -> jlong {
    Box::into_raw(Box::new(Mutex::new(HeardClock::new()))) as jlong
}

extern "system" fn destroy(h: jlong) {
    if h != 0 {
        // SAFETY: `h` came from `create` and Kotlin destroys it once.
        drop(unsafe { Box::from_raw(h as *mut Mutex<HeardClock>) });
    }
}

/// `on` is the player's current index and `next` the one it goes to next (-1 for none). Returns
/// `HeardAt::pack`; index -1 meaning the player's own word stands (and ms is then `position_ms`).
extern "system" fn at(h: jlong, now_ms: jlong, playing: jboolean, on: jint, next: jint, position_ms: jlong) -> jlong {
    let Some(c) = clock(h) else { return position_ms.max(0) };
    c.lock().at(now_ms, playing != 0, usize::try_from(on).ok(), usize::try_from(next).ok(), position_ms).pack()
}

/// [`at`] for the seek bar itself, whose page shows queue index `shown` (-1: nothing).
#[allow(clippy::too_many_arguments)]
extern "system" fn position(h: jlong, now_ms: jlong, playing: jboolean, on: jint, next: jint, position_ms: jlong, shown: jint) -> jlong {
    let Some(c) = clock(h) else { return position_ms.max(0) };
    c.lock().position(now_ms, playing != 0, usize::try_from(on).ok(), usize::try_from(next).ok(), position_ms, usize::try_from(shown).ok()).pack()
}

/// The listener asked for a place: the bar shows the next reading as it is, even a moment back.
extern "system" fn jumped(h: jlong) {
    if let Some(c) = clock(h) {
        c.lock().jumped();
    }
}

extern "system" fn run_on(h: jlong, now_ms: jlong, playing: jboolean) -> jlong {
    clock(h).map_or(0, |c| c.lock().run_on(now_ms, playing != 0))
}

/// The length the player page shows for its song (`nori_player::heard::shown_duration_ms`); `heard_s`
/// is -1 when the ear is where the player is.
extern "system" fn duration_ms(heard_s: jlong, player_ms: jlong, tagged_ms: jlong) -> jlong {
    nori_player::heard::shown_duration_ms((heard_s >= 0).then_some(heard_s), player_ms, tagged_ms)
}
