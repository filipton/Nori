//! The settings, owned here: read once when the app starts, kept in memory, and written back to the
//! app's database (`settings`, one row per key, the app's and not a server's) whenever they change. The
//! platform only shows and changes them. Codec, defaults and ranges are settings.rs's.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use rusqlite::{params, Connection, OptionalExtension};
use serde_json::{json, Value};

use crate::settings::{load, save, set_band, set_level, EqLevel, PrefValue, SoundBand, StoredPrefs};
use crate::{alog, background, db};

struct Kept {
    db: Arc<Mutex<Connection>>,
    prefs: StoredPrefs,
}

static KEPT: RwLock<Option<Kept>> = RwLock::new(None);
/// Bumped on every change; a write that finds a newer change waiting leaves the writing to that one.
static CHANGES: AtomicU64 = AtomicU64::new(0);

fn to_json(v: &PrefValue) -> String {
    match v {
        PrefValue::Flag { v } => json!({ "b": v }),
        PrefValue::Number { v } => json!({ "i": v }),
        PrefValue::Big { v } => json!({ "l": v }),
        PrefValue::Decimal { v } => json!({ "f": v }),
        PrefValue::Text { v } => json!({ "s": v }),
        PrefValue::Texts { v } => json!({ "ss": v }),
    }
    .to_string()
}

fn from_json(text: &str) -> Option<PrefValue> {
    let Value::Object(o) = serde_json::from_str(text).ok()? else { return None };
    let (t, v) = o.into_iter().next()?;
    Some(match (t.as_str(), v) {
        ("b", Value::Bool(v)) => PrefValue::Flag { v },
        ("i", Value::Number(n)) => PrefValue::Number { v: n.as_i64()? as i32 },
        ("l", Value::Number(n)) => PrefValue::Big { v: n.as_i64()? },
        ("f", Value::Number(n)) => PrefValue::Decimal { v: n.as_f64()? as f32 },
        ("s", Value::String(v)) => PrefValue::Text { v },
        ("ss", Value::Array(a)) => PrefValue::Texts { v: a.into_iter().filter_map(|x| x.as_str().map(str::to_string)).collect() },
        _ => return None,
    })
}

fn read(c: &Connection) -> rusqlite::Result<HashMap<String, PrefValue>> {
    let mut st = c.prepare("SELECT key, value FROM settings")?;
    let rows = st.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?)))?;
    Ok(rows.filter_map(|r| r.ok()).filter_map(|(k, v)| Some((k, from_json(&v)?))).collect())
}

/// Every value these settings store, in one transaction; keys no longer written go.
fn write(c: &mut Connection, prefs: &StoredPrefs) -> rusqlite::Result<()> {
    let put = save(prefs);
    let tx = c.transaction()?;
    tx.execute("DELETE FROM settings", [])?;
    {
        let mut st = tx.prepare("INSERT INTO settings(key, value) VALUES(?1, ?2)")?;
        for (k, v) in &put {
            st.execute(params![k, to_json(v)])?;
        }
    }
    tx.commit()
}

/// The settings kept in the app's database at `db_path`. The first time there are none, the defaults
/// become them.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn settings_open(db_path: String) -> crate::Result<StoredPrefs> {
    let mut c = db::open_app(&db_path)?;
    let raw = read(&c)?;
    let prefs = load(&raw);
    if raw.is_empty() {
        write(&mut c, &prefs)?;
    }
    *KEPT.write() = Some(Kept { db: Arc::new(Mutex::new(c)), prefs: prefs.clone() });
    changed(&prefs);
    Ok(prefs)
}

/// One of the app's own values (`app_kv`), read from the database the settings are kept in; none before
/// the settings are open.
pub(crate) fn app_value(key: &str) -> Option<String> {
    let db = KEPT.read().as_ref()?.db.clone();
    let c = db.lock();
    c.query_row("SELECT value FROM app_kv WHERE key=?1", [key], |r| r.get(0)).optional().ok().flatten()
}

/// The app's database the settings were opened from, for the few app-wide tables kept beside them
/// (perf_log.rs); none before the settings are open.
pub(crate) fn app_db() -> Option<Arc<Mutex<Connection>>> {
    KEPT.read().as_ref().map(|k| k.db.clone())
}

/// One of the app's own values kept, written on the core's background thread.
pub(crate) fn keep_app_value(key: &'static str, value: String) {
    let Some(db) = KEPT.read().as_ref().map(|k| k.db.clone()) else { return };
    background::run(move || {
        if let Err(e) = db.lock().execute("INSERT OR REPLACE INTO app_kv(key, value) VALUES(?1, ?2)", params![key, value]) {
            alog::info(&format!("{key}: could not write: {e}"));
        }
    });
}

/// [`settings_put`]'s answer: what the platform's player has to apply again. The sound chain and the
/// transition planner's settings follow by themselves.
/// Which parts of the output chain may run, speed and pitch (`dsp::audio_apply`).
pub const APPLY_AUDIO: u32 = 1;
/// The ReplayGain volume.
pub const APPLY_GAIN: u32 = 2;
/// A plan already made for the song playing is asked for again.
pub const REPLAN: u32 = 4;

/// What a change from `a` to `b` asks of the player.
fn effects(a: &StoredPrefs, b: &StoredPrefs) -> u32 {
    let audio = (a.eq_enabled, a.crossfeed_db, a.balance, a.mono, a.limiter, a.skip_silence, a.offload, a.crossfade_sec, a.auto_mix, a.speed, a.pitch, a.bit_perfect)
        != (b.eq_enabled, b.crossfeed_db, b.balance, b.mono, b.limiter, b.skip_silence, b.offload, b.crossfade_sec, b.auto_mix, b.speed, b.pitch, b.bit_perfect);
    let gain = (a.replay_gain, a.preamp_db, a.untagged_gain_db) != (b.replay_gain, b.preamp_db, b.untagged_gain_db);
    let plan = (
        a.auto_mix,
        a.crossfade_sec,
        a.auto_mix_max_s,
        a.auto_mix_beat_match,
        a.auto_mix_max_tempo_pct,
        a.auto_mix_bass_swap,
        a.auto_mix_filters,
        a.auto_mix_echo_out,
        a.auto_mix_keep_pitch,
        a.crossfade_keep_albums,
        a.replay_gain,
    ) != (
        b.auto_mix,
        b.crossfade_sec,
        b.auto_mix_max_s,
        b.auto_mix_beat_match,
        b.auto_mix_max_tempo_pct,
        b.auto_mix_bass_swap,
        b.auto_mix_filters,
        b.auto_mix_echo_out,
        b.auto_mix_keep_pitch,
        b.crossfade_keep_albums,
        b.replay_gain,
    );
    (if audio { APPLY_AUDIO } else { 0 }) | (if gain { APPLY_GAIN } else { 0 }) | (if plan { REPLAN } else { 0 })
}

/// The settings changed; kept now and written on the core's background thread. Returns what the
/// platform's player has to apply again ([`APPLY_AUDIO`], [`APPLY_GAIN`], [`REPLAN`]); 0 for a change
/// only screens care about.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn settings_put(prefs: StoredPrefs) -> u32 {
    edit(|_| prefs).unwrap_or(0)
}

/// The kept settings replaced by what `make` makes of them, and written as [`settings_put`] writes
/// them. Returns what the player has to apply again, or none when nothing changed.
fn edit(make: impl FnOnce(&StoredPrefs) -> StoredPrefs) -> Option<u32> {
    let (db, effect, prefs) = {
        let mut k = KEPT.write();
        let k = k.as_mut()?;
        let prefs = make(&k.prefs);
        if k.prefs == prefs {
            return None;
        }
        let effect = effects(&k.prefs, &prefs);
        k.prefs = prefs.clone();
        (k.db.clone(), effect, prefs)
    };
    changed(&prefs);
    let change = CHANGES.fetch_add(1, Ordering::SeqCst) + 1;
    background::run(move || {
        // A newer change is queued behind this one and writes everything anyway.
        if CHANGES.load(Ordering::SeqCst) != change {
            return;
        }
        if let Some(p) = current() {
            if let Err(e) = write(&mut db.lock(), &p) {
                alog::info(&format!("settings: could not write: {e}"));
            }
        }
    });
    Some(effect)
}

/// One band of the equalizer moved (`settings::set_band`), edited where the settings are kept: what the
/// player has to apply again and the band as it was kept, held in its ranges; none when nothing changed.
pub fn edit_band(index: u32, asked: SoundBand) -> Option<(u32, SoundBand)> {
    let mut kept = asked;
    let effect = edit(|p| {
        let s = set_band(p.sound(), index, asked);
        if let Some(k) = s.eq_bands.get(index as usize) {
            kept = *k;
        }
        p.clone().with_sound(s)
    })?;
    Some((effect, kept))
}

/// Pre-amp, balance, limiter ceiling or crossfeed moved (`settings::set_level`), edited where the
/// settings are kept: what the player has to apply again and the value as it was kept, held in range
/// and snapped; none when nothing changed.
pub fn edit_level(level: EqLevel, value: f32) -> Option<(u32, f32)> {
    let mut kept = value;
    let effect = edit(|p| {
        let s = set_level(p.sound(), level, value);
        kept = match level {
            EqLevel::Preamp => s.eq_preamp_db.unwrap_or(value),
            EqLevel::Balance => s.balance,
            EqLevel::Limiter => s.limiter_threshold_db,
            EqLevel::Crossfeed => s.crossfeed_db,
        };
        p.clone().with_sound(s)
    })?;
    Some((effect, kept))
}

/// What in the core follows the settings by itself, told at once.
fn changed(prefs: &StoredPrefs) {
    crate::automix::planner::settings_changed(prefs);
    crate::dsp::settings_changed(prefs);
}

/// The settings as they are kept now, for a Rust client that edits them and puts them back; none
/// before the app opened them.
pub fn settings_current() -> Option<StoredPrefs> {
    current()
}

/// The settings as they are now, for the core's own decisions; none before the app opened them.
pub(crate) fn current() -> Option<StoredPrefs> {
    with_prefs(StoredPrefs::clone)
}

/// One answer from the settings as they are now, without copying them; none before the app opened them.
pub(crate) fn with_prefs<R>(f: impl FnOnce(&StoredPrefs) -> R) -> Option<R> {
    KEPT.read().as_ref().map(|k| f(&k.prefs))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_kind_of_value_survives_the_database() {
        for v in [
            PrefValue::Flag { v: true },
            PrefValue::Number { v: -3 },
            PrefValue::Big { v: 1 << 40 },
            PrefValue::Decimal { v: 0.1 },
            PrefValue::Text { v: "x\"y".into() },
            PrefValue::Texts { v: vec!["1".into(), "2".into()] },
        ] {
            assert_eq!(from_json(&to_json(&v)), Some(v));
        }
    }

    #[test]
    fn only_what_the_player_uses_asks_it_to_apply_again() {
        let a = StoredPrefs::default();
        let theme = StoredPrefs { amoled: !a.amoled, ..a.clone() };
        assert_eq!(effects(&a, &theme), 0, "a screen's setting");
        let bands = StoredPrefs { eq_bands: vec![crate::settings::SoundBand { kind: 0, freq: 100.0, gain_db: 3.0, q: 1.0, channel: 0 }], ..a.clone() };
        assert_eq!(effects(&a, &bands), 0, "the sound chain follows its bands by itself");
        assert_eq!(effects(&a, &StoredPrefs { eq_enabled: true, ..a.clone() }), APPLY_AUDIO);
        assert_eq!(effects(&a, &StoredPrefs { replay_gain: 1, ..a.clone() }), APPLY_GAIN | REPLAN);
        assert_eq!(effects(&a, &StoredPrefs { crossfade_sec: 6, ..a.clone() }), APPLY_AUDIO | REPLAN);
        assert_eq!(effects(&a, &StoredPrefs { auto_mix_bass_swap: !a.auto_mix_bass_swap, ..a.clone() }), REPLAN);
    }

    /// The tests that open the store take turns: it is one for the whole process.
    static OPEN: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

    #[test]
    fn a_slider_edits_the_kept_settings_in_place() {
        let _turn = OPEN.lock();
        let dir = std::env::temp_dir().join(format!("nori-settings-edit-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        settings_open(dir.join("nori.db").display().to_string()).unwrap();
        let band = current().unwrap().eq_bands[2];
        let (effect, kept) = edit_band(2, SoundBand { gain_db: 99.0, ..band }).unwrap();
        assert_eq!(effect, 0, "the sound chain follows its bands by itself");
        assert_eq!(kept.gain_db, crate::settings::EQ_RANGES.gain.max, "held in range");
        assert_eq!(current().unwrap().eq_bands[2], kept);
        assert_eq!(edit_band(2, kept), None, "the same band again changes nothing");
        assert_eq!(edit_band(99, band), None, "a band that is not there");
        assert_eq!(edit_level(EqLevel::Balance, 0.02), None, "near the middle is the middle, as it was");
        assert_eq!(edit_level(EqLevel::Balance, -0.5), Some((APPLY_AUDIO, -0.5)));
        assert_eq!(current().unwrap().balance, -0.5);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_known_outputs_are_kept_with_the_settings() {
        let _turn = OPEN.lock();
        let dir = std::env::temp_dir().join(format!("nori-outputs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nori.db").display().to_string();
        settings_open(path.clone()).unwrap();
        let speaker = crate::outputs::outputs_speaker();
        assert_eq!(crate::outputs::outputs_known(), [speaker.clone()], "the speaker the first time");
        let seen = crate::outputs::outputs_refresh(vec![8], vec!["Buds".into()], vec![speaker.clone()], None);
        assert_eq!(seen.known.unwrap(), ["Bluetooth: Buds", speaker.as_str()]);
        // Written on the background thread; a job behind it has seen it done.
        let (tx, rx) = std::sync::mpsc::channel();
        background::run(move || tx.send(()).unwrap());
        rx.recv().unwrap();
        settings_open(path).unwrap();
        assert_eq!(crate::outputs::outputs_known(), ["Bluetooth: Buds", speaker.as_str()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_defaults_first_then_the_database_is_the_settings() {
        let _turn = OPEN.lock();
        let dir = std::env::temp_dir().join(format!("nori-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nori.db").display().to_string();
        assert_eq!(settings_open(path.clone()).unwrap(), StoredPrefs::default());
        let mut p = current().unwrap();
        p.fade_ms = 400;
        // Written straight away here rather than through the background thread.
        write(&mut KEPT.read().as_ref().unwrap().db.lock(), &p).unwrap();
        assert_eq!(settings_open(path).unwrap().fade_ms, 400, "what was saved comes back");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
