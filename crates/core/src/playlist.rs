//! The queue as the core's and the client's calls: a song's credits read from the server, the queue saved
//! there. The queue itself is nori-queue's.

use crate::client::{Client, NetResult};
use crate::Core;

pub use nori_queue::playlist::*;

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    /// Saves the queue for next time (radio streams are left out: they do not come back).
    pub fn playlist_save(&self, position_ms: u64) -> crate::Result<()> {
        let (ids, index) = with(|p| (p.ids().to_vec(), p.current().unwrap_or(0) as u32));
        self.queue_save(ids, index, position_ms)
    }
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Client {
    /// Hands the server the queue as the core keeps it ([`playlist_to_push`]: radio left out, only while
    /// plays may be sent at all), `current` playing `position_ms` in. The ids stay in the core: this used
    /// to be two calls, the whole list crossing out and back.
    pub async fn playlist_push(&self, current: Option<String>, position_ms: i64) -> NetResult<()> {
        match push_write(current, position_ms) {
            Some(w) => self.write(w).await,
            None => Ok(()),
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use parking_lot::Mutex;

    use super::*;

    /// The one queue is the process's: tests that use it take turns (as nori-queue's own tests do).
    static TURN: Mutex<()> = Mutex::new(());

    pub(crate) fn hold(ids: &[&str], start: i32) -> parking_lot::MutexGuard<'static, ()> {
        let g = TURN.lock();
        playlist_set(ids.iter().map(|s| s.to_string()).collect(), start, false);
        playlist_repeat(0);
        g
    }
}
