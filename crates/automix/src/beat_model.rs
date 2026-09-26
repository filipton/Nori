//! The weights behind "Better beat detection" and how their download stands. The network is Beat This!'s small0
//! (Foscarin, Schlüter and Widmer, ISMIR 2024; MIT), and the app carries only its graph (nori-player
//! `automix::weights::GRAPH`, made by tools/beat-this/export.py). The weights come from the authors themselves:
//! with the switch on, the core fetches their PyTorch checkpoint once from [`CHECKPOINT_URL`], checks its SHA-256,
//! turns it into the weights file the graph reads (through a restricted unpickler: nothing in the checkpoint is
//! run), checks that file against its own pin and keeps it beside the app's database (the core's `beat_download`
//! does that, on nori-engine's measuring thread). No one else ships or hosts a copy of the weights. The file is
//! deleted when the switch goes off; what it found stays stored.
//!
//! Nothing here runs while the switch is off: no thread, no listener, and no file is read or fetched.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

/// The authors' checkpoint: where they publish it (beat_this's README and inference code fetch it from there), its
/// size and SHA-256.
pub const CHECKPOINT_URL: &str = "https://cloud.cp.jku.at/public.php/dav/files/7ik4RrBKTS273gp/small0.ckpt";
pub const CHECKPOINT_SHA256: &str = "6074be2c4d490c5f6101fcc374a1ec72ae93456e23bb6019783b849f5dc7d47b";
pub const CHECKPOINT_BYTES: u64 = 8_451_101;
/// Pinned: the weights file made from it, the one kept on the device: its name, SHA-256 and size. Every platform
/// makes the same bytes (export.py makes them too, and prints these). Another graph or checkpoint gets a new name,
/// and files of an earlier one are deleted when it arrives.
pub const FILE_NAME: &str = "beat-this-small0.weights";
pub const SHA256: &str = "e9349da04b9da4ad41c5e416c71a9471af3a416249e7addef0101b3d569df5a7";
pub const BYTES: u64 = 4_229_216;
/// About what it costs to download, in megabytes, for a settings screen to say.
pub const SIZE_MB: u32 = 8;

/// Where the model's download stands.
#[derive(Debug, Clone, PartialEq)]
pub enum State {
    /// Not on the device, and not asked for yet: it comes the next time AutoMix measures a song.
    Absent,
    /// Asked for, and the phone is on mobile data, which the user has not allowed for it.
    WaitingForWifi,
    /// Being downloaded, or made from the download.
    Downloading,
    Ready,
    Failed(BeatFailure),
}

/// Why the model could not be made, for the client to say (the details go to the log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[repr(u8)]
pub enum BeatFailure {
    /// The request failed or the server answered with an error.
    Network,
    /// Something arrived, but not the pinned checkpoint, or it did not make the pinned weights.
    WrongFile,
    /// It could not be written to the device.
    Storage,
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
    // Only a checked file is ever renamed into place, so a file there is ready, whatever an earlier try said.
    if k.state != State::Downloading && dir.as_ref().is_some_and(|d| d.join(FILE_NAME).is_file()) {
        k.state = State::Ready;
    }
    k.dir = dir;
}

/// Where the weights file is (or goes), whether it is there or not.
pub fn file() -> Option<PathBuf> {
    KEPT.lock().dir.as_ref().map(|d| d.join(FILE_NAME))
}

/// The weights file, when it is on the device: made from the checkpoint and checked.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_model_lives_beside_the_database_and_goes_when_switched_off() {
        let dir = nori_testdir::TempDir::new("model");
        std::fs::create_dir_all(dir.join("models")).unwrap();
        std::fs::write(dir.join("models").join(FILE_NAME), b"model").unwrap();
        set_home(&dir.join("nori.db").to_string_lossy());
        assert_eq!(ready(), Some(dir.join("models").join(FILE_NAME)));
        assert_eq!(state(), State::Ready);
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
    }
}
