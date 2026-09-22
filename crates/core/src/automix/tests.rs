//! The analysis store: rows in, rows out, migrations, and the streaming handle Kotlin drives.

use nori_player::automix::synth::Synth;

use super::*;
use crate::Core;

#[test]
fn analysis_is_stored_and_reported_missing() {
    let core = Core::new(String::new()).unwrap();
    assert_eq!(core.analysis_get("a".into()).unwrap(), None);
    assert_eq!(core.analysis_missing(vec!["a".into(), "b".into()]).unwrap(), vec!["a".to_string(), "b".to_string()]);
    let s = Synth::new(120.0);
    let bytes: Vec<u8> = s.render().iter().flat_map(|v| v.to_le_bytes()).collect();
    let t = core.analysis_run("a".into(), bytes, s.rate as i32, 1, PCM_FLOAT).unwrap();
    assert_eq!(core.analysis_get("a".into()).unwrap(), Some(t.clone()));
    assert_eq!(core.analysis_missing(vec!["a".into(), "b".into()]).unwrap(), vec!["b".to_string()]);
    core.analysis_store(TrackAnalysis { song_id: "b".into(), analysis_version: ANALYSIS_VERSION - 1, ..t.clone() }).unwrap();
    assert_eq!(core.analysis_missing(vec!["a".into(), "b".into()]).unwrap(), vec!["b".to_string()], "old versions are redone");
    core.analysis_store(TrackAnalysis { bpm: 99.5, ..t.clone() }).unwrap();
    assert_eq!(core.analysis_get("a".into()).unwrap().unwrap().bpm, 99.5, "store replaces");
}

#[test]
fn a_v1_database_migrates_to_v2() {
    // A database from before the overlap-window columns: migrate keeps the row readable with
    // zeroed windows, and the old version still reports the track for re-analysis.
    let dir = std::env::temp_dir().join(format!("nori-mig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mig.db").to_string_lossy().into_owned();
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE track_analysis(song_id TEXT PRIMARY KEY, analysis_version INTEGER NOT NULL, duration_ms INTEGER NOT NULL, bpm REAL NOT NULL, bpm_confidence REAL NOT NULL, beat_offset_ms REAL NOT NULL, stability REAL NOT NULL, downbeat_phase INTEGER NOT NULL, downbeat_confidence REAL NOT NULL, lufs REAL NOT NULL, key INTEGER NOT NULL, key_confidence REAL NOT NULL, silence_start_ms INTEGER NOT NULL, silence_end_ms INTEGER NOT NULL, mixramp_start_ms INTEGER NOT NULL, mixramp_end_ms INTEGER NOT NULL, intro_end_ms INTEGER NOT NULL, outro_start_ms INTEGER NOT NULL, analysed_ms INTEGER NOT NULL) WITHOUT ROWID").unwrap();
        c.execute("INSERT INTO track_analysis VALUES('v1', 1, 240000, 128.0, 0.9, 120.0, 0.9, 0, 0.8, -9.0, 8, 0.8, 100, 238500, 400, 236000, 15000, 220000, 0)", []).unwrap();
    }
    let core = Core::new(path.clone()).unwrap();
    let a = core.analysis_get("v1".into()).unwrap().unwrap();
    assert_eq!((a.bpm, a.outro_vocal, a.intro_centroid), (128.0, 0.0, 0.0));
    assert_eq!(core.analysis_missing(vec!["v1".into()]).unwrap(), vec!["v1".to_string()]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_streaming_handle_finishes_into_the_store() {
    use ::jni::sys::jlong;
    let core = Core::new(String::new()).unwrap();
    // The handle a JNI create would return, built the same way.
    let s = Synth::new(128.0);
    let x = s.render();
    let h = {
        let mut a = analysis::Analyzer::new(s.rate, 60_000);
        a.feed(&x[..s.rate as usize * 10]);
        store_handle(a)
    };
    assert_eq!(core.analysis_finish_stream("x".into(), h).unwrap(), None, "10 s is not a track");
    let t = {
        with_handle(h, |a| a.feed(&x));
        core.analysis_finish_stream("x".into(), h).unwrap().unwrap()
    };
    assert!((t.bpm - 128.0).abs() < 0.05);
    assert_eq!(core.analysis_get("x".into()).unwrap(), Some(t));
    assert_eq!(core.analysis_finish_stream("x".into(), 0).unwrap(), None);
    store::test_destroy(h as jlong);
}

fn store_handle(a: analysis::Analyzer) -> i64 {
    store::test_handle(a)
}

fn with_handle(h: i64, f: impl FnOnce(&mut analysis::Analyzer)) {
    store::test_with(h, f)
}


#[test]
fn plan_through_the_ffi() {
    let a = analyse("a", &Synth { secs: 120.0, ..Synth::new(128.0) }.render(), 44100).track;
    let b = analyse("b", &Synth { secs: 120.0, ..Synth::new(125.0) }.render(), 44100).track;
    let p = plan_transition(Some(a.clone()), Some(b.clone()), a.duration_ms, b.duration_ms, crate::AutoMixSettings::default());
    assert_eq!(p.kind, crate::TransitionKind::BeatMatched, "{}", p.reason);
    assert_eq!(automix_mixer_params(p.clone()).len(), mixer::param::COUNT);
}
