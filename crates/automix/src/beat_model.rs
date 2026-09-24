//! The file behind "Better beat detection" and how its download stands. The model is Beat This!'s small0
//! checkpoint as ONNX with its attention fused and fp16 weights (tools/beat-this/export.py makes it; 5 MB),
//! fetched once from a pinned address and checked against its SHA-256 (the core's `beat_model` does that, on
//! nori-engine's measuring thread), kept beside the app's database, and deleted when the switch goes off. What
//! it found stays stored either way.
//!
//! Nothing here runs while the switch is off: no thread, no listener, and the file is not there.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

/// Pinned: the release asset and its SHA-256 (5,069,715 bytes). A new model gets a new name, and files of an
/// earlier one are deleted when it arrives.
pub const FILE_NAME: &str = "beat-this-small0-v1.onnx";
pub const URL: &str = "https://github.com/filipton/Nori/releases/download/beat-this-small0-v1/beat-this-small0-v1.onnx";
pub const SHA256: &str = "4c1008bb81b1ec0f4b707bbf870fa3a69d76f016b9a08779ce9c5f564caa1845";
pub const BYTES: u64 = 5_069_715;
/// What the settings say it costs to download.
pub const SIZE_MB: u32 = 5;

/// Where the model's download stands.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Not on the device, and not asked for yet: it comes the next time AutoMix measures a song.
    Absent,
    /// Asked for, and the phone is on mobile data, which the user has not allowed for it.
    WaitingForWifi,
    Downloading,
    Ready,
    Failed(String),
}

struct Kept {
    /// The directory the model is kept in; none before a core was opened on a file.
    dir: Option<PathBuf>,
    state: State,
    /// The switch as it was last seen, so only turning it off deletes the file.
    on: bool,
}

static KEPT: Mutex<Kept> = Mutex::new(Kept { dir: None, state: State::Absent, on: false });

/// The model is kept in `models` beside the app's database at `db_path` (none for a database in memory).
pub fn set_home(db_path: &str) {
    let dir = Path::new(db_path).parent().filter(|_| !db_path.is_empty()).map(|p| p.join("models"));
    let mut k = KEPT.lock();
    if k.state == State::Absent && dir.as_ref().is_some_and(|d| d.join(FILE_NAME).is_file()) {
        k.state = State::Ready;
    }
    k.dir = dir;
}

/// Where the model's file is (or goes), whether it is there or not.
pub fn file() -> Option<PathBuf> {
    KEPT.lock().dir.as_ref().map(|d| d.join(FILE_NAME))
}

/// The model's file, when it is on the device and checked.
pub fn ready() -> Option<PathBuf> {
    let k = KEPT.lock();
    let f = k.dir.as_ref()?.join(FILE_NAME);
    (k.state == State::Ready && f.is_file()).then_some(f)
}

pub fn state() -> State {
    KEPT.lock().state.clone()
}

pub fn set_state(s: State) {
    KEPT.lock().state = s;
}

/// The settings changed. Turned off, the model goes: its file, and a download's leftovers.
pub fn switched(on: bool) {
    let mut k = KEPT.lock();
    let was = std::mem::replace(&mut k.on, on);
    if was && !on {
        k.state = State::Absent;
        if let Some(dir) = k.dir.clone() {
            drop(k);
            // Off the caller's thread: a settings change comes from the screen.
            nori_db::background::run(move || {
                let _ = std::fs::remove_dir_all(dir);
            });
        }
    }
}

/// What "Better beat detection" says under its title.
pub fn detail(on: bool) -> String {
    if !on {
        return format!("Listens to the start and end of each song so mixes land on the beat. A one-time download of about {SIZE_MB} MB.");
    }
    match state() {
        State::Absent => format!("Downloaded (about {SIZE_MB} MB) the next time AutoMix measures a song."),
        State::WaitingForWifi => format!("Waiting for Wi-Fi to download it (about {SIZE_MB} MB)."),
        State::Downloading => format!("Downloading (about {SIZE_MB} MB)…"),
        State::Ready => "On. Each song is listened to once, just before it plays.".into(),
        State::Failed(reason) => format!("The download didn't work ({reason}). It tries again at the next song."),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_lives_beside_the_database_and_goes_when_switched_off() {
        let dir = std::env::temp_dir().join(format!("nori-model-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(dir.join("models").join(FILE_NAME), b"model").unwrap();
        set_home(&dir.join("nori.db").to_string_lossy());
        assert_eq!(ready(), Some(dir.join("models").join(FILE_NAME)));
        assert!(detail(true).starts_with("On."));
        assert!(detail(false).contains("5 MB"));
        // Turning it on keeps the file; off deletes it, on the background thread.
        switched(true);
        switched(true);
        assert!(dir.join("models").join(FILE_NAME).is_file());
        switched(false);
        for _ in 0..200 {
            if !dir.join("models").exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(!dir.join("models").exists());
        assert_eq!((state(), ready()), (State::Absent, None));
        std::fs::remove_dir_all(&dir).ok();
    }
}
