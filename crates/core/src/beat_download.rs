//! Fetching the beat model for "Better beat detection" (`nori_automix::beat_model` says where it goes and how it
//! stands): through the platform's transport, the same way the AutoEQ list comes, on the calling thread, which is
//! nori-engine's measuring thread at the lowest priority. It is asked for only when a song is about to be read by
//! the model, so there is no timer and no network listener waiting for Wi-Fi: a download that waited or failed is
//! tried again at the next song. A client that ships the model (`beat_model::set_bundled`) never downloads it.

use std::future::Future;
use std::pin::pin;
use std::sync::Arc;
use std::task::{Context, Poll, Wake, Waker};

use nori_automix::beat_model::{self, BeatFailure, Source, State, BYTES, SHA256, URL};
use sha2::{Digest, Sha256};

/// How long the whole file may take.
const TIMEOUT_MS: u32 = 120_000;

/// The model, fetched first when it is not on the device yet and the network allows. None when it is not
/// wanted, not there and cannot come now (mobile data not allowed, no server connection, a failed download).
pub fn ensure() -> Option<Source> {
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
    let got = block_on(nori_net::transport::get(&*client.transport, URL.to_string(), TIMEOUT_MS)).map_err(|e| (BeatFailure::Network, e.to_string())).and_then(|body| {
        if body.len() as u64 != BYTES || hex(&Sha256::digest(&body)) != SHA256 {
            return Err((BeatFailure::WrongFile, format!("{} bytes, not the pinned file", body.len())));
        }
        let disk = |e: std::io::Error| (BeatFailure::Storage, e.to_string());
        let dir = file.parent().expect("a file in the models directory");
        // An earlier model's file, from before an update pinned a new one, goes; then the new one arrives whole.
        let _ = std::fs::remove_dir_all(dir);
        std::fs::create_dir_all(dir).map_err(disk)?;
        let part = file.with_extension("part");
        std::fs::write(&part, &body).and_then(|_| std::fs::rename(&part, &file)).map_err(disk)
    });
    match got {
        // Switched off while it came: nothing stays.
        Ok(()) if !wanted() => {
            let _ = std::fs::remove_file(&file);
            beat_model::set_state(State::Absent);
            None
        }
        Ok(()) => {
            crate::alog::info(&format!("beat model downloaded: {BYTES} bytes"));
            beat_model::set_state(State::Ready);
            Source::whole(file).ok()
        }
        Err((why, detail)) => {
            crate::alog::info(&format!("beat model download failed: {detail}"));
            beat_model::set_state(State::Failed(why));
            None
        }
    }
}

/// The model's bytes from `source`, checked against the pinned SHA-256: a file shipped with the app is checked
/// the same way a downloaded one was, so a build that bundled another file says so instead of misreading songs.
pub fn read(source: &Source) -> Result<Vec<u8>, String> {
    let bytes = source.read().map_err(|e| e.to_string())?;
    if bytes.len() as u64 != BYTES || hex(&Sha256::digest(&bytes)) != SHA256 {
        return Err(format!("{} is not the pinned model", source.path.display()));
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
}
