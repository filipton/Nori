//! Every word the app says is nori-words'; the two here also read the app's state, the settings and the
//! queue playing, which are the core's.

pub use nori_words::words::*;

/// What a heart says when pressed, or nothing with the favourite notice switched off in the settings.
/// The heart itself always changes.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_favourite(on: bool) -> Option<String> {
    crate::settings_store::with_prefs(|p| p.favourite_notice).unwrap_or(true).then(|| favourite(on))
}

/// The session's buttons for the song playing now in the core's queue, `starred` or not, `shuffle` on or off.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_session_buttons(starred: bool, shuffle: bool) -> SessionButtons {
    let song = crate::playlist::with(|p| p.current_id().is_some_and(|id| !id.starts_with(crate::queue::RADIO_PREFIX)));
    session_buttons(song, starred, shuffle)
}
