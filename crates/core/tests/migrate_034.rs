//! migrate_034 against a phone as nori music 0.3.4 left it: its SharedPreferences files as Android writes
//! them (testdata/migrate_034/shared_prefs) and its databases in its own layout (schema_034.sql, verbatim
//! from the tag). A test binary of its own: the settings are one for the whole process.
//! Remove with migrate_034 once 0.3.4 users have updated.

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use nori_core::migrate_034::{migrate_034, parse_prefs, MARKER, PARKED};
use nori_core::settings::{AutoFillBasis, AutoFillKind, BandChannel, GainMode, HomeRow, PrefValue, SwipeAction, TapAction, ThemeMode};
use nori_core::{background, settings_store, Core, ServerConfig};
use rusqlite::Connection;

const SCHEMA: &str = include_str!("../testdata/migrate_034/schema_034.sql");
const PREFS: &str = include_str!("../testdata/migrate_034/shared_prefs/nori.xml");
const DEVICES: &str = include_str!("../testdata/migrate_034/shared_prefs/nori-devices.xml");
const OUTPUTS: &str = include_str!("../testdata/migrate_034/shared_prefs/nori-outputs.xml");

/// The settings are the process's: the tests take turns.
static TURN: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// An app's data directory (`files/`, `shared_prefs/`), gone when the test is.
struct Phone(PathBuf);

impl Phone {
    fn new(name: &str) -> Phone {
        let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let root = std::env::temp_dir().join(format!("nori-migrate034-{name}-{}-{nanos}", std::process::id()));
        fs::create_dir_all(root.join("files")).unwrap();
        fs::create_dir_all(root.join("shared_prefs")).unwrap();
        Phone(root)
    }

    fn files(&self) -> PathBuf {
        self.0.join("files")
    }

    fn prefs(&self, name: &str) -> PathBuf {
        self.0.join("shared_prefs").join(name)
    }

    fn db(&self) -> String {
        self.files().join("nori.db").display().to_string()
    }

    fn migrate(&self) {
        migrate_034(self.files().display().to_string());
    }

    /// 0.3.4 as a listener leaves it: two servers, "default" in use, settings, downloads and the rest.
    fn as_034_left_it(self) -> Phone {
        fs::write(self.prefs("nori.xml"), PREFS).unwrap();
        fs::write(self.prefs("nori-devices.xml"), DEVICES).unwrap();
        fs::write(self.prefs("nori-outputs.xml"), OUTPUTS).unwrap();
        old_db(&self.files().join("nori.db"), |c| {
            c.execute_batch(
                r#"
                INSERT INTO items(kind, id, json) VALUES(2, 's1', '{"id":"s1","title":"One"}');
                INSERT INTO kv VALUES('server', 'http://music.example:4533|alice');
                INSERT INTO kv VALUES('queue', '{"songs":[{"id":"s1","title":"One","album":"A","artist":"X","duration":200},{"id":"q2","title":"Two"}],"index":1,"position":42000}');
                INSERT INTO downloads VALUES('s3', '{"id":"s3","title":"Three","album":"A","artist":"X","duration":180,"size":4000000,"suffix":"flac"}', 3, 1);
                INSERT INTO downloads VALUES('s1', '{"id":"s1","title":"One","album":"A","artist":"X","duration":200,"size":5000000,"suffix":"flac"}', 1, 1);
                INSERT INTO downloads VALUES('s2', '{"id":"s2","title":"Two (waiting)","album":"A","artist":"X"}', 2, 0);
                INSERT INTO pending(endpoint, params) VALUES('star', '[["id","s1"]]');
                INSERT INTO profiles VALUES('Warm', '{"eqEnabled":true,"eqBands":"0:100.0:3.0:0.7:0","crossfeedDb":0.0,"balance":0.0,"mono":false,"limiter":false,"limiterThresholdDb":-1.0,"replayGain":0,"preampDb":0.0,"crossfadeSec":0,"hiRes":false,"bitPerfect":false}', 'USB: Old & loud DAC');
                INSERT INTO smart_playlists VALUES('sp-1', 'Nineties', '{"match":{"all":true,"rules":[{"field":"year","op":"is","value":1994}]}}', 5);
                INSERT INTO mix_excluded VALUES('s9');
                INSERT INTO searches VALUES('floyd', 1);
                INSERT INTO plays(song_id, started_ms, heard_ms, duration_ms, completed, skipped, hour, day) VALUES('s1', 1, 200000, 200000, 1, 0, 20, 3);
                "#,
            )
        });
        old_db(&self.files().join("nori-5f3a9c21.db"), |c| {
            c.execute_batch(
                r#"
                INSERT INTO downloads VALUES('x1', '{"id":"x1","title":"Elsewhere"}', 1, 1);
                INSERT INTO profiles VALUES('Warm', '{"eqEnabled":false}', '');
                INSERT INTO profiles VALUES('Bright', '{"eqEnabled":true}', 'Bluetooth: Car kit');
                "#,
            )
        });
        self
    }
}

impl Drop for Phone {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// A database in 0.3.4's layout, filled by `fill`.
fn old_db(path: &Path, fill: impl FnOnce(&Connection) -> rusqlite::Result<()>) {
    let c = Connection::open(path).unwrap();
    c.execute_batch(SCHEMA).unwrap();
    fill(&c).unwrap();
}

/// What the core's background thread was handed has been written.
fn written() {
    let (tx, rx) = mpsc::channel();
    background::run(move || tx.send(()).unwrap());
    rx.recv_timeout(Duration::from_secs(10)).unwrap();
}

fn ids(songs: Vec<nori_core::Song>) -> Vec<String> {
    let mut ids: Vec<String> = songs.into_iter().map(|s| s.id).collect();
    ids.sort();
    ids
}

fn count(db: &str, sql: &str) -> i64 {
    Connection::open(db).unwrap().query_row(sql, [], |r| r.get(0)).unwrap()
}

#[test]
fn everything_0_3_4_kept_comes_over() {
    let _turn = TURN.lock();
    let phone = Phone::new("full").as_034_left_it();
    phone.migrate();

    // The settings, servers and login first: the app opens them next.
    let p = settings_store::settings_open(phone.db()).unwrap();
    assert_eq!(p.active_server_id, "default");
    assert_eq!(p.servers.len(), 2);
    let home = &p.servers[0];
    assert_eq!((home.id.as_str(), home.name.as_str(), home.url.as_str(), home.alt_url.as_str()), ("default", "Home", "http://music.example:4533", "https://music.example.org"));
    assert_eq!((home.user.as_str(), home.password.as_str()), ("alice", "fixture <pass> & word"), "entities read back");
    assert_eq!(home.headers.get("X-Proxy-Auth").map(String::as_str), Some("fixture-token"));
    assert!(home.allow_self_signed);
    assert_eq!((home.music_folder_id.as_str(), home.alt_max_bit_rate), ("3", 192));
    let other = &p.servers[1];
    assert_eq!((other.api_key.as_str(), other.legacy_auth, other.wifi_only), ("fixture-api-key", true, true));
    assert_eq!((other.client_cert.as_str(), other.client_cert_password.as_str()), ("bob.p12", "fixture-cert-pass"));
    assert_eq!((p.mobile.bit_rate, p.mobile.format.as_str()), (128, "opus"));
    assert_eq!((p.download.bit_rate, p.download.format.as_str()), (320, "mp3"));
    assert_eq!((p.cache_mb, p.parallel_downloads, p.covers_ahead), (2048, 3, 5));
    assert_eq!((p.replay_gain, p.preamp_db, p.fade_ms), (GainMode::Auto, -2.5, 300));
    assert!(p.eq_enabled);
    assert_eq!(p.eq_bands.len(), 3);
    assert_eq!((p.eq_bands[1].kind as i32, p.eq_bands[1].freq, p.eq_bands[1].gain_db, p.eq_bands[1].channel), (1, 100.0, -2.5, BandChannel::Left));
    assert_eq!((p.eq_preamp_db, p.crossfeed_db, p.balance, p.limiter, p.limiter_threshold_db), (Some(-3.0), 4.5, -0.2, true, -1.5));
    assert_eq!((p.auto_mix, p.auto_mix_max_s, p.auto_mix_beat_match, p.auto_mix_max_tempo_pct, p.auto_mix_keep_pitch), (true, 10, false, 4.0, false));
    assert_eq!((p.crossfade_sec, p.crossfade_keep_albums, p.skip_silence, p.scrobble, p.scrobble_percent), (6, false, true, false, 70));
    assert_eq!((p.auto_fill_kind, p.auto_fill_basis, p.bridge_offline, p.previous_always_skips), (AutoFillKind::Albums, AutoFillBasis::Genre, true, true));
    assert_eq!((p.theme, p.amoled, p.dynamic_color, p.accent, p.player_colours), (ThemeMode::Dark, true, false, 4282557941, false));
    assert!(!p.third_party_lookups, "the listener's own choice, kept");
    assert!(p.ignore_system_motion, "0.3.4's old motion switch is not carried over; the new default stands");
    assert_eq!((p.tap_action, p.swipe_right, p.swipe_left), (TapAction::PlayOne, SwipeAction::PlayNext, SwipeAction::Download), "the left swipe from swipeLeft3");
    assert_eq!(p.home_rows, [HomeRow::Recent, HomeRow::Pinned, HomeRow::Newest]);
    assert_eq!(p.pinned_playlists, ["pl-7", "pl-2"]);
    assert_eq!(p.list_prefs.get("albums").map(String::as_str), Some("grid|year"));
    assert_eq!((p.lyrics_size, p.lyrics_sweep, p.favourite_notice), (2, false, false));

    assert_eq!(settings_store::app_value(MARKER).as_deref(), Some("1"));
    let quiet: Vec<String> = serde_json::from_str(&settings_store::app_value(nori_devices::profiles::QUIET).unwrap()).unwrap();
    assert_eq!(quiet, ["Bluetooth: Car kit", "USB: Old & loud DAC"]);
    assert!(nori_core::outputs::outputs_known().contains(&"USB: Old & loud DAC".to_string()), "every device seen");

    // The server in use: its downloads, queue, smart playlists, exclusions and waiting writes.
    let core = Core::new(phone.db(), "default".into()).unwrap();
    // The app connects as it always does: the queue is not taken for another server's and dropped.
    core.configure(ServerConfig { url: "http://music.example:4533".into(), user: "alice".into(), password: "fixture <pass> & word".into(), api_key: None, legacy_auth: false }).unwrap();
    assert_eq!(ids(core.downloads(true).unwrap()), ["s1", "s3"]);
    assert_eq!(ids(core.downloads(false).unwrap()), ["s2"], "still waiting: media3's own queue resumes it");
    assert_eq!(core.downloads(true).unwrap().iter().find(|s| s.id == "s1").unwrap().size, 5_000_000);
    let q = core.load_queue().unwrap();
    assert_eq!((ids(q.songs), q.index, q.position_ms), (vec!["q2".to_string(), "s1".into()], 1, 42000));
    let smart = core.smart_list().unwrap();
    assert_eq!((smart.len(), smart[0].id.as_str(), smart[0].name.as_str()), (1, "sp-1", "Nineties"));
    let pending = core.pending_list().unwrap();
    assert_eq!((pending.len(), pending[0].endpoint.as_str(), pending[0].params[0].value.as_str()), (1, "star", "s1"));
    assert_eq!(count(&phone.db(), "SELECT count(*) FROM mix_excluded WHERE server='default' AND song_id='s9'"), 1);
    assert_eq!(count(&phone.db(), "SELECT count(*) FROM items"), 0, "the library index is fetched again");
    assert_eq!(count(&phone.db(), "SELECT count(*) FROM plays"), 0, "the play history stays behind");

    // The equalizer profiles, app-wide: the server in use wins a name both had.
    let profiles = core.profiles().unwrap();
    let names: Vec<&str> = profiles.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Bright", "Warm"]);
    let warm = &profiles[1];
    assert!(warm.json.contains("0:100.0:3.0"), "the server in use's Warm");
    assert_eq!(warm.outputs, ["USB: Old & loud DAC"]);
    assert_eq!(core.profile_for_output("Bluetooth: Car kit".into()).unwrap().map(|p| p.name), Some("Bright".into()));

    // The other server's own rows.
    let other = Core::new(phone.db(), "5f3a9c21".into()).unwrap();
    assert_eq!(ids(other.downloads(true).unwrap()), ["x1"]);
    assert!(other.load_queue().unwrap().songs.is_empty());

    // 0.3.4's files are gone once everything came over.
    for f in ["nori.xml", "nori-devices.xml", "nori-outputs.xml"] {
        assert!(!phone.prefs(f).exists(), "{f}");
    }
    assert!(!phone.files().join(PARKED).exists());
    assert!(!phone.files().join("nori-5f3a9c21.db").exists());
}

#[test]
fn a_second_run_does_nothing() {
    let _turn = TURN.lock();
    let phone = Phone::new("twice").as_034_left_it();
    phone.migrate();
    let mut p = settings_store::settings_open(phone.db()).unwrap();
    p.theme = ThemeMode::Light;
    settings_store::settings_put(p);
    written();

    // Nothing of 0.3.4 is left: nothing is looked at.
    phone.migrate();
    // Even with 0.3.4's settings back (a copy restored, a cut-short tidy), the marker stops it.
    fs::write(phone.prefs("nori.xml"), PREFS).unwrap();
    phone.migrate();
    assert!(!phone.prefs("nori.xml").exists(), "tidied away");
    assert_eq!(settings_store::settings_open(phone.db()).unwrap().theme, ThemeMode::Light, "the settings since are kept");
    let core = Core::new(phone.db(), "default".into()).unwrap();
    assert_eq!(core.pending_list().unwrap().len(), 1, "a waiting write is not sent twice");
    assert_eq!(core.downloads(true).unwrap().len(), 2);
    assert_eq!(core.smart_list().unwrap().len(), 1);
}

#[test]
fn a_run_cut_short_goes_on_from_the_parked_files() {
    let _turn = TURN.lock();
    let phone = Phone::new("cut").as_034_left_it();
    // Stopped after the databases were moved aside, before anything was written.
    let parked = phone.files().join(PARKED);
    fs::create_dir_all(&parked).unwrap();
    fs::rename(phone.files().join("nori.db"), parked.join("default.db")).unwrap();
    fs::rename(phone.files().join("nori-5f3a9c21.db"), parked.join("5f3a9c21.db")).unwrap();
    phone.migrate();
    let core = Core::new(phone.db(), "default".into()).unwrap();
    assert_eq!(ids(core.downloads(true).unwrap()), ["s1", "s3"]);
    assert_eq!(ids(Core::new(phone.db(), "5f3a9c21".into()).unwrap().downloads(true).unwrap()), ["x1"]);
    assert_eq!(settings_store::settings_open(phone.db()).unwrap().servers.len(), 2);
    assert!(!parked.exists());
}

#[test]
fn a_new_install_is_not_touched() {
    let _turn = TURN.lock();
    let phone = Phone::new("fresh");
    phone.migrate();
    assert!(!phone.files().join("nori.db").exists(), "nothing opened, nothing made");
    // This app's own database is not 0.3.4's.
    settings_store::settings_open(phone.db()).unwrap();
    Core::new(phone.db(), "default".into()).unwrap();
    phone.migrate();
    assert!(settings_store::app_value(MARKER).is_none());
}

#[test]
fn corrupt_settings_leave_the_rest_to_come_over() {
    let _turn = TURN.lock();
    let phone = Phone::new("bad-prefs").as_034_left_it();
    fs::write(phone.prefs("nori.xml"), b"ABX\0\x01binary, not text").unwrap();
    fs::write(phone.prefs("nori-devices.xml"), "<map><set name=\"quiet\"><string>half").unwrap();
    phone.migrate();
    let p = settings_store::settings_open(phone.db()).unwrap();
    assert_eq!(p, nori_core::settings::StoredPrefs::default(), "a fresh start");
    // Without the server list only the "default" database is known; its rows come over.
    let core = Core::new(phone.db(), "default".into()).unwrap();
    assert_eq!(ids(core.downloads(true).unwrap()), ["s1", "s3"]);
    // Kept for a look, out of the way, and not tried again.
    let parked = phone.files().join(PARKED);
    assert!(parked.join("nori.xml").exists() && parked.join("default.db").exists());
    assert!(!phone.prefs("nori.xml").exists());
    assert_eq!(settings_store::app_value(MARKER).as_deref(), Some("1"));
    phone.migrate();
    assert!(parked.join("default.db").exists(), "a second start leaves them be");
}

#[test]
fn a_broken_database_costs_only_its_own_rows() {
    let _turn = TURN.lock();
    let phone = Phone::new("bad-db").as_034_left_it();
    fs::write(phone.files().join("nori-5f3a9c21.db"), b"not a database at all, but long enough to have a header....................................").unwrap();
    // An older 0.3.x without smart playlists.
    Connection::open(phone.files().join("nori.db")).unwrap().execute_batch("DROP TABLE smart_playlists").unwrap();
    phone.migrate();
    let p = settings_store::settings_open(phone.db()).unwrap();
    assert_eq!(p.servers.len(), 2, "the settings came over");
    let core = Core::new(phone.db(), "default".into()).unwrap();
    assert_eq!(ids(core.downloads(true).unwrap()), ["s1", "s3"]);
    assert_eq!(core.pending_list().unwrap().len(), 1);
    assert!(core.smart_list().unwrap().is_empty());
    assert!(Core::new(phone.db(), "5f3a9c21".into()).unwrap().downloads(true).unwrap().is_empty());
    assert!(phone.files().join(PARKED).join("5f3a9c21.db").exists(), "kept for a look");
}

#[test]
fn a_missing_server_database_is_nothing_to_carry() {
    let _turn = TURN.lock();
    let phone = Phone::new("no-db").as_034_left_it();
    fs::remove_file(phone.files().join("nori-5f3a9c21.db")).unwrap();
    phone.migrate();
    assert_eq!(settings_store::settings_open(phone.db()).unwrap().servers.len(), 2);
    assert!(!phone.files().join(PARKED).exists(), "nothing went wrong: tidied");
}

#[test]
fn preferences_read_as_android_writes_them() {
    let xml = "<?xml version='1.0' encoding='utf-8' standalone='yes' ?>\n<map>\n    <string name=\"a\">x&#10;y &lt;&amp;&gt; &quot;q&quot; &apos;</string>\n    <string name=\"empty\"></string>\n    <string name=\"closed\" />\n    <null name=\"gone\" />\n    <float name=\"f\" value=\"1.0E-4\" />\n    <long name=\"l\" value=\"-4282557941\" />\n    <set name=\"s\" />\n</map>\n";
    let m = parse_prefs(xml).unwrap();
    assert_eq!(m.get("a"), Some(&PrefValue::Text { v: "x\ny <&> \"q\" '".into() }));
    assert_eq!(m.get("empty"), Some(&PrefValue::Text { v: String::new() }));
    assert_eq!(m.get("closed"), Some(&PrefValue::Text { v: String::new() }));
    assert_eq!(m.get("gone"), None);
    assert_eq!(m.get("f"), Some(&PrefValue::Decimal { v: 1.0e-4 }));
    assert_eq!(m.get("l"), Some(&PrefValue::Big { v: -4282557941 }));
    assert_eq!(m.get("s"), Some(&PrefValue::Texts { v: vec![] }));
    assert!(parse_prefs("<map />").unwrap().is_empty());
    assert!(parse_prefs("<map><int name=\"x\" value=\"seven\" /></map>").is_err());
    assert!(parse_prefs("<map><string name=\"x\">open").is_err());
    assert!(parse_prefs("").is_err());
    let full = parse_prefs(PREFS).unwrap();
    assert!(matches!(full.get("servers"), Some(PrefValue::Text { v }) if v.starts_with("[{\"id\":\"default\"")));
    assert_eq!(full.len(), 79, "every entry of the file");
}

#[test]
fn why_it_runs_first_0_3_4_s_database_has_this_app_s_name_and_not_its_layout() {
    let phone = Phone::new("why");
    old_db(&phone.files().join("nori.db"), |_| Ok(()));
    let unusable = Core::new(phone.db(), "default".into()).and_then(|c| c.downloads(true));
    assert!(unusable.is_err(), "left where it is, every server's rows fail to read");
}
