//! The few answers about what the screens should show that read the app's state (the settings, the queue
//! playing). They are facts, not words: each client says them its own way.

/// Whether a heart press is confirmed on screen ("Added to favourites"): the favourite notice in the
/// settings. The heart itself always changes.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn favourite_notice() -> bool {
    crate::settings_store::with_prefs(|p| p.favourite_notice).unwrap_or(true)
}

/// The system media controls' extra buttons (the notification, the lock screen): a heart beside previous
/// and a shuffle toggle beside next. The client labels them ("Add to favourites", "Shuffle on").
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SessionButtons {
    /// A song of the library plays, so there is something to favourite; none for nothing or a radio stream.
    pub heart: bool,
    /// The heart is filled: a press takes the song out of the favourites.
    pub starred: bool,
    /// Shuffle is on: a press turns it off.
    pub shuffling: bool,
}

/// The session's buttons with a song of the library playing (`song`) or not, `starred` or not, `shuffle`
/// on or off.
pub fn session_buttons(song: bool, starred: bool, shuffle: bool) -> SessionButtons {
    SessionButtons { heart: song, starred: song && starred, shuffling: shuffle }
}

/// The session's buttons for the song playing now in the core's queue, `starred` or not, `shuffle` on or off.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn session_buttons_now(starred: bool, shuffle: bool) -> SessionButtons {
    let song = crate::playlist::with(|p| p.current_id().is_some_and(|id| !id.starts_with(crate::queue::RADIO_PREFIX)));
    session_buttons(song, starred, shuffle)
}

/// What a radio stream shows as its title: what the station announces now (its ICY title), or, while it
/// announces nothing, the station's own name.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn radio_title(announced: Option<String>, station: Option<String>) -> Option<String> {
    announced.filter(|a| !a.trim().is_empty()).or(station)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_session_buttons_say_what_a_press_does() {
        assert_eq!(session_buttons(true, true, false), SessionButtons { heart: true, starred: true, shuffling: false });
        assert_eq!(session_buttons(true, false, true), SessionButtons { heart: true, starred: false, shuffling: true });
        let b = session_buttons(false, true, false);
        assert_eq!((b.heart, b.starred), (false, false), "a radio stream or nothing: no heart");
    }

    #[test]
    fn a_stream_is_titled_by_what_it_announces() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(radio_title(s("Artist - Song"), s("FM 4")), s("Artist - Song"));
        assert_eq!(radio_title(s("  "), s("FM 4")), s("FM 4"), "announcing nothing");
        assert_eq!(radio_title(None, s("FM 4")), s("FM 4"));
        assert_eq!(radio_title(None, None), None);
    }
}
