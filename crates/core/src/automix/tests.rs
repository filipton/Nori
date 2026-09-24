//! The analysis store: rows in, rows out, and the streaming handle Kotlin drives.

use nori_player::automix::synth::Synth;

use super::*;
use crate::Core;

#[test]
fn analysis_is_stored_and_reported_missing() {
    let core = Core::new(String::new(), "t".into()).unwrap();
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
fn the_streaming_handle_finishes_into_the_store() {
    let core = Core::new(String::new(), "t".into()).unwrap();
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
    store::free_stream_handle(h);
}

fn store_handle(a: analysis::Analyzer) -> i64 {
    store::stream_handle(a, 1)
}

fn with_handle(h: i64, f: impl FnOnce(&mut analysis::Analyzer)) {
    f(&mut store::stream(h).unwrap().analyzer())
}


#[test]
fn plan_through_the_ffi() {
    let a = analyse("a", &Synth { secs: 120.0, ..Synth::new(128.0) }.render(), 44100).track;
    let b = analyse("b", &Synth { secs: 120.0, ..Synth::new(125.0) }.render(), 44100).track;
    let p = plan_transition(Some(a.clone()), Some(b.clone()), a.duration_ms, b.duration_ms, crate::AutoMixSettings::default());
    assert_eq!(p.kind, crate::TransitionKind::BeatMatched, "{}", p.reason);
    assert_eq!(automix_mixer_params(p.clone()).len(), mixer::param::COUNT);
}
