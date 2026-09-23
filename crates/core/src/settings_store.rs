//! The settings, owned here: read once when the app starts, kept in memory, and written back to the
//! app's database (`settings`, one row per key, the app's and not a server's) whenever they change. The
//! platform hands over what it kept before (its old key-value store) the first time, and after that
//! only shows and changes them. Codec, defaults and ranges are settings.rs's.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use parking_lot::{Mutex, RwLock};
use rusqlite::{params, Connection};
use serde_json::{json, Value};

use crate::settings::{load, save, PrefValue, StoredPrefs};
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
    let w = save(prefs);
    let tx = c.transaction()?;
    tx.execute("DELETE FROM settings", [])?;
    {
        let mut st = tx.prepare("INSERT INTO settings(key, value) VALUES(?1, ?2)")?;
        for (k, v) in &w.put {
            st.execute(params![k, to_json(v)])?;
        }
    }
    tx.commit()
}

/// The settings kept in the app's database at `db_path`. The first time there are none, `legacy` (what
/// the platform kept until now) becomes them.
#[uniffi::export]
pub fn settings_open(db_path: String, legacy: HashMap<String, PrefValue>) -> crate::Result<StoredPrefs> {
    let mut c = db::open_app(&db_path)?;
    let raw = read(&c)?;
    let prefs = load(if raw.is_empty() { &legacy } else { &raw });
    if raw.is_empty() {
        write(&mut c, &prefs)?;
    }
    *KEPT.write() = Some(Kept { db: Arc::new(Mutex::new(c)), prefs: prefs.clone() });
    changed(&prefs);
    Ok(prefs)
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
#[uniffi::export]
pub fn settings_put(prefs: StoredPrefs) -> u32 {
    let (db, effect) = {
        let mut k = KEPT.write();
        let Some(k) = k.as_mut() else { return 0 };
        if k.prefs == prefs {
            return 0;
        }
        let effect = effects(&k.prefs, &prefs);
        k.prefs = prefs.clone();
        (k.db.clone(), effect)
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
    effect
}

/// What in the core follows the settings by itself, told at once.
fn changed(prefs: &StoredPrefs) {
    crate::automix::planner::settings_changed(prefs);
    crate::dsp::settings_changed(prefs);
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

    #[test]
    fn the_old_store_is_taken_once_then_the_database_is_the_settings() {
        let dir = std::env::temp_dir().join(format!("nori-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("nori.db").display().to_string();
        let mut legacy = HashMap::new();
        legacy.insert("fadeMs".to_string(), PrefValue::Number { v: 250 });
        assert_eq!(settings_open(path.clone(), legacy.clone()).unwrap().fade_ms, 250);
        let mut p = current().unwrap();
        p.fade_ms = 400;
        // Written straight away here rather than through the background thread.
        write(&mut KEPT.read().as_ref().unwrap().db.lock(), &p).unwrap();
        assert_eq!(settings_open(path, legacy).unwrap().fade_ms, 400, "the database wins over the old store");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
