//! The file behind "Better beat detection" and how its download stands. The model is Beat This!'s small0
//! checkpoint as ONNX with its attention fused and fp16 weights (tools/beat-this/export.py makes it; 5 MB).
//!
//! Two ways it reaches a client. Shipped with the app ([`set_bundled`]: the Android app stores it uncompressed
//! in its APK and says where, so it is read in place, never copied), it is there from the start and nothing is
//! downloaded. Otherwise it is fetched once from [`URL`] and checked against its SHA-256 (the core's
//! `beat_download` does that, on nori-engine's measuring thread), kept beside the app's database, and deleted when
//! the switch goes off. What it found stays stored either way.
//!
//! Nothing here runs while the switch is off: no thread, no listener, and no file is read or fetched.

use std::path::{Path, PathBuf};

use parking_lot::Mutex;

/// Pinned: the file's name, its SHA-256 and size, and where a client that does not ship it downloads it from (the
/// one place to change when it is hosted somewhere else). A new model gets a new name, and files of an earlier one
/// are deleted when it arrives. The Android build checks the file it bundles against these (core/build.gradle.kts).
pub const FILE_NAME: &str = "beat-this-small0-v1.onnx";
pub const URL: &str = "https://github.com/filipton/Nori/releases/download/beat-this-small0-v1/beat-this-small0-v1.onnx";
pub const SHA256: &str = "847b51aaef519a60a47c815fa58440782de73bff7000210396673b0353e2cc8c";
pub const BYTES: u64 = 5_069_707;
/// About what it costs to download, in megabytes, for a settings screen to say.
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
    Failed(BeatFailure),
}

/// Why the model's download did not work, for the client to say (the details go to the log).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[repr(u8)]
pub enum BeatFailure {
    /// The request failed or the server answered with an error.
    Network,
    /// Something arrived, but not the pinned file.
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

/// Where the model's bytes are: `len` bytes from `offset` in the file at `path`. A downloaded model is a whole
/// file; one shipped with the app is a stretch of its package.
#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub path: PathBuf,
    pub offset: u64,
    pub len: u64,
}

impl Source {
    /// The whole file at `path`.
    pub fn whole(path: PathBuf) -> std::io::Result<Source> {
        let len = std::fs::metadata(&path)?.len();
        Ok(Source { path, offset: 0, len })
    }

    /// The model's bytes, read in one go.
    pub fn read(&self) -> std::io::Result<Vec<u8>> {
        use std::io::{Read, Seek, SeekFrom};
        let mut f = std::fs::File::open(&self.path)?;
        f.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::with_capacity(self.len as usize);
        f.take(self.len).read_to_end(&mut bytes)?;
        if bytes.len() as u64 != self.len {
            return Err(std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "the model's file is shorter than said"));
        }
        Ok(bytes)
    }
}

/// The model shipped with the app, when it is.
static BUNDLED: Mutex<Option<Source>> = Mutex::new(None);

/// The app ships the model: `len` bytes from `offset` in the file at `path` (an asset stored uncompressed in the
/// APK). Called once as the app starts, before anything measures; nothing is read here.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn beat_model_bundled(path: String, offset: u64, len: u64) {
    set_bundled(Some(Source { path: path.into(), offset, len }));
}

/// The name the model's file has, in the app's assets as anywhere else.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn beat_model_file_name() -> String {
    FILE_NAME.into()
}

pub fn set_bundled(source: Option<Source>) {
    *BUNDLED.lock() = source;
}

/// The model shipped with the app, if this one ships it.
pub fn bundled() -> Option<Source> {
    BUNDLED.lock().clone()
}

/// The model is kept in `models` beside the app's database at `db_path` (none for a database in memory).
pub fn set_home(db_path: &str) {
    let dir = Path::new(db_path).parent().filter(|_| !db_path.is_empty()).map(|p| p.join("models"));
    let mut k = KEPT.lock();
    // Only a checked download is ever renamed into place, so a file there is ready, whatever an earlier try said.
    if k.state != State::Downloading && dir.as_ref().is_some_and(|d| d.join(FILE_NAME).is_file()) {
        k.state = State::Ready;
    }
    k.dir = dir;
}

/// Where the model's file is (or goes), whether it is there or not.
pub fn file() -> Option<PathBuf> {
    KEPT.lock().dir.as_ref().map(|d| d.join(FILE_NAME))
}

/// The model, when it is on the device: shipped with the app, or downloaded and checked.
pub fn ready() -> Option<Source> {
    if let Some(b) = bundled() {
        return Some(b);
    }
    let k = KEPT.lock();
    let f = k.dir.as_ref()?.join(FILE_NAME);
    if k.state != State::Ready {
        return None;
    }
    Source::whole(f).ok()
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
        assert_eq!(ready(), Some(Source { path: dir.join("models").join(FILE_NAME), offset: 0, len: 5 }));
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

    #[test]
    fn a_model_shipped_inside_another_file_is_read_in_place() {
        let dir = nori_testdir::TempDir::new("bundled");
        let apk = dir.join("base.apk");
        std::fs::write(&apk, b"zip headers|the model|more zip").unwrap();
        let s = Source { path: apk.clone(), offset: 12, len: 9 };
        assert_eq!(s.read().unwrap(), b"the model");
        assert!(Source { len: 99, ..s.clone() }.read().is_err());
        assert_eq!(Source::whole(apk).unwrap().len, 30);
    }
}
