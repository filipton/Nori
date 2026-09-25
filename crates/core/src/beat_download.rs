//! Making the beat model for "Better beat detection" (`nori_automix::beat_model` says where it goes and how it
//! stands): the authors' checkpoint fetched through the platform's transport, the same way the AutoEQ list comes,
//! checked, and turned into the weights file the app's graph reads (nori-player `automix::weights`), once. It all
//! runs on the calling thread, which is nori-engine's measuring thread at the lowest priority. It is asked for only
//! when a song is about to be read by the model, so there is no timer and no network listener waiting for Wi-Fi: a
//! download that waited or failed is tried again at the next song. Every client does this the same way.

use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use nori_automix::beat_model::{self, BeatFailure, State, BYTES, CHECKPOINT_BYTES, CHECKPOINT_SHA256, CHECKPOINT_URL, SHA256};
use sha2::{Digest, Sha256};

/// How long the whole checkpoint may take.
const TIMEOUT_MS: u32 = 120_000;

/// The weights file, made first when it is not on the device yet and the network allows. None when it is not
/// wanted, not there and cannot come now (mobile data not allowed, no server connection, a failed download).
pub fn ensure() -> Option<PathBuf> {
    let wanted = || crate::settings_store::with_prefs(|p| p.auto_mix && p.auto_mix_better_beats).unwrap_or(false);
    if !wanted() {
        return None;
    }
    if let Some(f) = beat_model::ready() {
        return Some(f);
    }
    let file = beat_model::file()?;
    let mobile = crate::settings_store::with_prefs(|p| p.auto_mix_beats_mobile_data).unwrap_or(false);
    if nori_net::stream::metered() && !mobile {
        beat_model::set_state(State::WaitingForWifi);
        return None;
    }
    let client = crate::client::active_client()?;
    beat_model::set_state(State::Downloading);
    let t0 = std::time::Instant::now();
    let got = block_on(nori_net::transport::get(&*client.transport, CHECKPOINT_URL.to_string(), TIMEOUT_MS))
        .map_err(|e| (BeatFailure::Network, e.to_string()))
        .and_then(|ckpt| {
            let fetched = t0.elapsed();
            let weights = make(&ckpt)?;
            drop(ckpt);
            store(&file, &weights).map_err(|e| (BeatFailure::Storage, e.to_string()))?;
            Ok((fetched, t0.elapsed()))
        });
    match got {
        // Switched off while it came: nothing stays.
        Ok(_) if !wanted() => {
            let _ = std::fs::remove_file(&file);
            beat_model::set_state(State::Absent);
            None
        }
        Ok((fetched, all)) => {
            crate::alog::info(&format!("beat model: checkpoint fetched in {} ms, weights made and kept in {} ms", fetched.as_millis(), all.as_millis()));
            beat_model::set_state(State::Ready);
            Some(file)
        }
        Err((why, detail)) => {
            crate::alog::info(&format!("beat model not made: {detail}"));
            beat_model::set_state(State::Failed(why));
            None
        }
    }
}

/// The weights file from the checkpoint's bytes: the checkpoint checked against its pin, converted, and the result
/// checked against its own.
pub fn make(ckpt: &[u8]) -> Result<Vec<u8>, (BeatFailure, String)> {
    if ckpt.len() as u64 != CHECKPOINT_BYTES || hex(&Sha256::digest(ckpt)) != CHECKPOINT_SHA256 {
        return Err((BeatFailure::WrongFile, format!("{} bytes, not the pinned checkpoint", ckpt.len())));
    }
    let weights = nori_player::automix::weights::convert(ckpt).map_err(|e| (BeatFailure::WrongFile, format!("converting the checkpoint: {e}")))?;
    if weights.len() as u64 != BYTES || hex(&Sha256::digest(&weights)) != SHA256 {
        return Err((BeatFailure::WrongFile, format!("the checkpoint made {} bytes, not the pinned weights", weights.len())));
    }
    Ok(weights)
}

/// Written whole beside the database: an earlier model's files go first, and the new file is renamed into place.
fn store(file: &Path, weights: &[u8]) -> std::io::Result<()> {
    let dir = file.parent().expect("a file in the models directory");
    let _ = std::fs::remove_dir_all(dir);
    std::fs::create_dir_all(dir)?;
    let part = file.with_extension("part");
    std::fs::write(&part, weights)?;
    std::fs::rename(&part, file)
}

/// The weights file's bytes, checked against the pin again: a file changed on the disk says so instead of
/// misreading songs.
pub fn read(file: &Path) -> Result<Vec<u8>, String> {
    let bytes = std::fs::read(file).map_err(|e| e.to_string())?;
    if bytes.len() as u64 != BYTES || hex(&Sha256::digest(&bytes)) != SHA256 {
        return Err(format!("{} is not the pinned weights", file.display()));
    }
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Wakes the thread parked on a transport that has not answered yet.
struct Unpark(std::thread::Thread);

impl Wake for Unpark {
    fn wake(self: Arc<Self>) {
        self.0.unpark();
    }
}

/// Runs `f` to its end on this thread. A desktop transport finishes on its first poll; Android's is answered by
/// OkHttp's own threads, which wake this one.
fn block_on<F: Future>(f: F) -> F::Output {
    let waker = Waker::from(Arc::new(Unpark(std::thread::current())));
    let mut cx = Context::from_waker(&waker);
    let mut f = pin!(f);
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::park();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_digest_is_written_as_the_pin_is() {
        assert_eq!(hex(&Sha256::digest(b"abc")), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad");
    }

    #[test]
    fn only_the_pinned_checkpoint_is_converted() {
        assert_eq!(make(b"<html>not found</html>").unwrap_err().0, BeatFailure::WrongFile);
        assert_eq!(make(&vec![0u8; CHECKPOINT_BYTES as usize]).unwrap_err().0, BeatFailure::WrongFile);
    }
}
