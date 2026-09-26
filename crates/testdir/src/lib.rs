//! A directory of a test's own, under the system's temp directory (`TMPDIR`), removed when the guard goes:
//! at the end of the test, and when it panics too, as the unwinding drops it. For the crates' tests only
//! (a dev-dependency). Left to themselves, the tests' directories filled a small `/tmp` until unrelated
//! tests failed to write.
//!
//! The name is the test's prefix, the process, a count within the process and the time, so two tests of
//! one binary, or two binaries run at once, never share one.

use std::ops::Deref;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

/// A directory that exists while this lives, and is removed with everything in it when it is dropped.
#[derive(Debug)]
pub struct TempDir(PathBuf);

impl TempDir {
    /// A new, empty directory named `nori-<prefix>-...`.
    pub fn new(prefix: &str) -> TempDir {
        static COUNT: AtomicU64 = AtomicU64::new(0);
        let n = COUNT.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!("nori-{prefix}-{}-{n}-{nanos}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap_or_else(|e| panic!("a test directory at {}: {e}", dir.display()));
        TempDir(dir)
    }

    pub fn path(&self) -> &Path {
        &self.0
    }
}

impl Deref for TempDir {
    type Target = Path;

    fn deref(&self) -> &Path {
        &self.0
    }
}

impl AsRef<Path> for TempDir {
    fn as_ref(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_is_its_own_and_goes_with_its_guard_even_through_a_panic() {
        let (a, b) = (TempDir::new("testdir"), TempDir::new("testdir"));
        assert_ne!(a.path(), b.path());
        std::fs::write(a.join("f"), b"x").unwrap();
        let kept = a.to_path_buf();
        drop(a);
        assert!(!kept.exists(), "removed with what was in it");
        let inside = std::sync::Arc::new(std::sync::Mutex::new(None));
        let seen = inside.clone();
        let failed = std::thread::spawn(move || {
            let d = TempDir::new("testdir");
            *seen.lock().unwrap() = Some(d.to_path_buf());
            panic!("a failing test");
        })
        .join();
        assert!(failed.is_err());
        let path = inside.lock().unwrap().clone().unwrap();
        assert!(!path.exists(), "removed as the panic unwound");
    }
}
