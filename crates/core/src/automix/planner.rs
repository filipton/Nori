//! The transition planner as the audio path uses it: the window of songs coming up and the user's
//! settings are handed in when they change, and the engine asks for its plan, whether a song wants
//! measuring, and hands over a finished measurement - all here, without a call into Kotlin. The engine
//! asks again every couple of seconds while nothing is planned (a null is often momentary), so that
//! question costs a lookup and no allocation unless the answer changes.

use std::sync::mpsc::{channel, Sender};
use std::sync::OnceLock;

use nori_player::automix::analysis::Analyzer;
use nori_player::engine::Plan;
use nori_player::transitions::{engine_plan, pick, whole_song, Skip, TransitionPrefs, WindowSong};
use parking_lot::Mutex;

use super::store::{get, missing, put};
use crate::alog;

struct Planner {
    prefs: Option<TransitionPrefs>,
    transitions_off: bool,
    window: Vec<WindowSong>,
    shuffling: bool,
    /// Bumped whenever anything a plan depends on changes: the window, the settings, a stored analysis.
    generation: u64,
    /// The last "no transition" answer - the song, the generation it was worked out in, and why
    /// (`None`: the planner chose gapless) - so the engine's retries cost a comparison until something
    /// changes, and the reason is logged once.
    none: Option<(String, u64, Option<Skip>)>,
}

static PLANNER: Mutex<Planner> =
    Mutex::new(Planner { prefs: None, transitions_off: false, window: Vec::new(), shuffling: false, generation: 0, none: None });

/// The user's transition settings, and whether the output forbids touching samples at all
/// (`AudioPolicy::transitions_off`). Handed in whenever the settings change.
#[uniffi::export]
pub fn transition_setup(prefs: crate::TransitionPrefs, transitions_off: bool) {
    let mut p = PLANNER.lock();
    p.prefs = Some(prefs);
    p.transitions_off = transitions_off;
    p.generation += 1;
}

/// The songs the player will play: the one before the current one first, then the current one and those
/// after it, in play order. Handed in whenever that window changes.
#[uniffi::export]
pub fn transition_window(window: Vec<crate::WindowSong>, shuffling: bool) {
    let mut p = PLANNER.lock();
    p.window = window;
    p.shuffling = shuffling;
    p.generation += 1;
}

/// The engine's question: how to mix out of `outgoing_id`, if at all.
pub fn plan_for(outgoing_id: &str) -> Option<Plan> {
    let mut p = PLANNER.lock();
    let prefs = p.prefs?;
    let generation = p.generation;
    if p.none.as_ref().is_some_and(|(id, g, _)| *g == generation && id == outgoing_id) {
        return None;
    }
    let chosen = match pick(&prefs, p.transitions_off, &p.window, outgoing_id, p.shuffling) {
        Err(skip) => {
            let repeated = p.none.as_ref().is_some_and(|(id, _, s)| *s == Some(skip) && id == outgoing_id);
            if !repeated {
                alog::info(&format!("planFor: {}", skip.describe(outgoing_id)));
            }
            p.none = Some((outgoing_id.to_string(), generation, Some(skip)));
            return None;
        }
        Ok(pk) => pk,
    };
    let (o, n) = (p.window[chosen.out].clone(), p.window[chosen.next].clone());
    drop(p);
    // Analyses are read from the database; a plan is asked for once per song (and again only when the
    // window or settings change), so this is not on the per-buffer path.
    let (a, b) = if prefs.auto_mix {
        crate::active().map_or((None, None), |core| {
            let c = core.db.lock();
            (get(&c, &o.id).ok().flatten(), get(&c, &n.id).ok().flatten())
        })
    } else {
        (None, None)
    };
    let t = nori_player::automix::plan::plan(a.as_ref(), b.as_ref(), o.duration_ms, n.duration_ms, &chosen.settings);
    let plan = engine_plan(&t, &n.id);
    PLANNER.lock().none = plan.is_none().then(|| (outgoing_id.to_string(), generation, None));
    match &plan {
        None => alog::info(&format!("planFor: gapless ({})", t.reason)),
        Some(_) => alog::info(&format!(
            "transition {} -> {}: {} {} ms at {}, tempo x{:.3} ({})",
            o.title,
            n.title,
            screaming(&format!("{:?}", t.kind)),
            t.duration_ms,
            t.out_start_ms,
            t.tempo_ratio,
            t.reason
        )),
    }
    plan
}

/// `BeatMix` as the app's logs have always named it: `BEAT_MIX`.
fn screaming(camel: &str) -> String {
    let mut s = String::with_capacity(camel.len() + 4);
    for (i, c) in camel.chars().enumerate() {
        if c.is_uppercase() && i > 0 {
            s.push('_');
        }
        s.push(c.to_ascii_uppercase());
    }
    s
}

/// Ids of radio streams and of files from outside the library, which are never analysed.
pub(crate) const RADIO_PREFIX: &str = "radio:";
pub(crate) const EXTERNAL_PREFIX: &str = "ext-";

/// Whether `song_id` should be measured as it plays, and its length in ms (0 unknown) so the measurement
/// is sized up front. Asked once per song, when its first buffer arrives.
pub fn wants_analysis(song_id: &str) -> Option<u64> {
    let (auto_mix, duration) = {
        let p = PLANNER.lock();
        (p.prefs.is_some_and(|x| x.auto_mix), p.window.iter().find(|s| s.id == song_id).map_or(0, |s| s.duration_ms.max(0) as u64))
    };
    if !auto_mix || song_id.starts_with(RADIO_PREFIX) || song_id.starts_with(EXTERNAL_PREFIX) {
        return None;
    }
    let core = crate::active()?;
    let wanted = missing(&core.db.lock(), &[song_id.to_string()]).is_ok_and(|m| !m.is_empty());
    wanted.then_some(duration)
}

struct Finished {
    song_id: String,
    analyzer: Analyzer,
    frames: u64,
    rate: u32,
}

/// Measurements are finished (the heavy part: tempo, beat grids, key, loudness) and stored on a thread of
/// their own, never on the audio thread that hands them over.
fn worker() -> &'static Mutex<Sender<Finished>> {
    static WORKER: OnceLock<Mutex<Sender<Finished>>> = OnceLock::new();
    WORKER.get_or_init(|| {
        let (tx, rx) = channel::<Finished>();
        std::thread::Builder::new()
            .name("nori-analysis".into())
            .spawn(move || {
                for f in rx {
                    finish(f);
                }
            })
            .expect("the analysis thread starts");
        Mutex::new(tx)
    })
}

/// The engine heard `song_id` from its first sample to its last.
pub fn analysed(song_id: &str, analyzer: Analyzer, frames: u64, rate: u32) {
    let _ = worker().lock().send(Finished { song_id: song_id.to_string(), analyzer, frames, rate });
}

fn finish(mut f: Finished) {
    let expected_ms = PLANNER.lock().window.iter().find(|s| s.id == f.song_id).map_or(0, |s| s.duration_ms);
    let heard_ms = (f.frames * 1000 / f.rate.max(1) as u64) as i64;
    // Only a song heard whole is an analysis of it; nor is anything under 30 s worth keeping.
    if !whole_song(heard_ms, expected_ms) || f.analyzer.samples() < (f.analyzer.rate() * 30.0) as u64 {
        alog::info(&format!("analysed {}: not stored: heard {heard_ms} ms of {expected_ms} ms", f.song_id));
        return;
    }
    let features = f.analyzer.take_features();
    let a = super::finish(&f.song_id, &features).track;
    let stored = crate::active().is_some_and(|core| put(&core.db.lock(), &a).is_ok());
    // A pair that was gapless for want of this analysis may mix now.
    PLANNER.lock().generation += 1;
    alog::info(&format!(
        "analysed {}: {:.2} bpm (conf {:.2}, stab {:.2}), key {}, heard {} ms of {} ms, {} frames at {} Hz{}",
        f.song_id,
        a.bpm,
        a.bpm_confidence,
        a.stability,
        nori_player::automix::structure::camelot_name(a.key),
        a.duration_ms,
        expected_ms,
        f.frames,
        f.rate,
        if stored { "" } else { " (not stored: no database)" }
    ));
}
