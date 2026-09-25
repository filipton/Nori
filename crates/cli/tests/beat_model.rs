//! "Better beat detection" as the terminal client gets it: the same core path as every client, over nori-http. With
//! `NORI_BEAT_THIS_NET=1` it fetches the authors' checkpoint from their server, checks it, converts it and keeps the
//! weights, and prints what that cost; without it there is nothing to run (it needs the network).
//! `NORI_BEAT_THIS_NET=1 cargo test --release -p nori-cli --features neural-beats --test beat_model -- --nocapture`
#![cfg(feature = "neural-beats")]

use nori_core::beat_model::{self, State};
use nori_core::client::Client;
use nori_core::Core;

#[test]
fn the_weights_come_from_the_authors() {
    if std::env::var("NORI_BEAT_THIS_NET").is_err() {
        eprintln!("NORI_BEAT_THIS_NET not set: skipped (it downloads 8 MB)");
        return;
    }
    let dir = nori_testdir::TempDir::new("beat-model");
    let core = Core::new(dir.join("nori.db").to_string_lossy().into_owned(), "test".into()).unwrap();
    let mut prefs = nori_core::settings_store::settings_open(dir.join("app.db").to_string_lossy().into_owned()).unwrap();
    (prefs.auto_mix, prefs.auto_mix_better_beats, prefs.auto_mix_beats_mobile_data) = (true, true, true);
    nori_core::settings_store::settings_put(prefs);
    let _client = Client::new(core, nori_http::Http::new());
    let rss = || {
        let s = std::fs::read_to_string("/proc/self/status").unwrap_or_default();
        s.lines().find_map(|l| l.strip_prefix("VmHWM:").map(|v| v.trim().to_string())).unwrap_or_default()
    };
    let before = rss();
    let t0 = std::time::Instant::now();
    let file = nori_core::beat_download::ensure();
    println!("fetched, checked, converted and kept in {:.2} s; peak RSS {before} before, {} after", t0.elapsed().as_secs_f64(), rss());
    assert_eq!(beat_model::state(), State::Ready);
    let file = file.expect("the weights file");
    assert_eq!(file, dir.join("models").join(beat_model::FILE_NAME));
    let t1 = std::time::Instant::now();
    let weights = nori_core::beat_download::read(&file).unwrap();
    nori_player::automix::neural::BeatThis::from_weights(&weights).unwrap();
    println!("read, checked and loaded in {:.2} s; peak RSS {}", t1.elapsed().as_secs_f64(), rss());
}
