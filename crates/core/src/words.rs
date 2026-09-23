//! The app's short confirmations, worded here so every player says the same.

/// A one-line confirmation after an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Said {
    PlayingNext,
    AddedToQueue,
    ExcludedFromMixes,
    /// Songs went into the playlist named with it.
    AddedToPlaylist,
    /// The playlist named with it was made.
    PlaylistCreated,
    NoServerQueue,
    /// A provider's item was starred on octo-fiesta, which fetches it into the library.
    ServerDownloading,
    /// Something failed without saying why.
    Failed,
}

/// The confirmation `what`; `name` is the playlist it is about, where it is about one.
#[uniffi::export]
pub fn words(what: Said, name: String) -> String {
    match what {
        Said::PlayingNext => "Playing next".into(),
        Said::AddedToQueue => "Added to queue".into(),
        Said::ExcludedFromMixes => "Excluded from mixes".into(),
        Said::AddedToPlaylist => format!("Added to {name}"),
        Said::PlaylistCreated => format!("Created {name}"),
        Said::NoServerQueue => "No queue saved on the server".into(),
        Said::ServerDownloading => "The server is downloading it into your library".into(),
        Said::Failed => "Failed".into(),
    }
}

/// What a heart says when pressed, or nothing with the favourite notice switched off in the settings.
/// The heart itself always changes.
#[uniffi::export]
pub fn words_favourite(on: bool) -> Option<String> {
    crate::settings_store::with_prefs(|p| p.favourite_notice).unwrap_or(true).then(|| favourite(on))
}

fn favourite(on: bool) -> String {
    (if on { "Added to favourites" } else { "Removed from favourites" }).into()
}

fn counted(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// How many songs a list holds: "1 song", "12 songs".
#[uniffi::export]
pub fn words_songs(songs: u32) -> String {
    counted(songs, "song", "songs")
}

/// The download queue's row in the library: "4 to go · 1 failed", or that nothing is downloading.
#[uniffi::export]
pub fn words_download_queue(waiting: u32, failed: u32) -> String {
    match (waiting, failed) {
        (0, 0) => "Nothing downloading".into(),
        (w, 0) => format!("{w} to go"),
        (0, f) => format!("{f} failed"),
        (w, f) => format!("{w} to go · {f} failed"),
    }
}

/// The bit-perfect switch's second line: what the DAC is doing, why it cannot, or what the switch is for.
#[uniffi::export]
pub fn words_bit_perfect(device: Option<String>, on: bool, sample_rate: u32, bits: u32, blocked_by: Option<String>, supported: bool) -> String {
    match (device, blocked_by) {
        (d, _) if on => format!("On: {}, {:?} kHz, {bits}-bit.", d.as_deref().unwrap_or("null"), sample_rate as f64 / 1000.0),
        (d, Some(why)) => format!("{}: {why}", d.as_deref().unwrap_or("USB DAC")),
        (Some(d), None) if supported => format!("{d} connected. Starts with the music."),
        (Some(d), None) => format!("{d} connected, but this phone can't do bit perfect with it."),
        (None, None) => "Sends the file to a USB DAC unchanged. Skips the equalizer and volume levelling. Android 14 and later.".into(),
    }
}

/// What is actually going out, under the bit-perfect switch: "Offers … · playing … · output …".
#[uniffi::export]
pub fn words_dac_detail(modes: Vec<String>, playing: Option<String>, track: Option<String>) -> Option<String> {
    let parts: Vec<String> = [
        (!modes.is_empty()).then(|| format!("Offers {}", modes.join(", "))),
        playing.map(|p| format!("playing {p}")),
        track.map(|t| format!("output {t}")),
    ]
    .into_iter()
    .flatten()
    .collect();
    (!parts.is_empty()).then(|| parts.join("  ·  "))
}

/// The battery saver's second line. Whether it is paused is the player's own rule
/// (`nori_player::policy::audio_policy`, the call the playback service makes with the same settings),
/// so the note never says the chip decodes while the player has stood it down.
#[uniffi::export]
pub fn words_offload(prefs: crate::AudioPrefs, output: crate::OutputState) -> String {
    let policy = nori_player::policy::audio_policy(&prefs, &output);
    if output.usb {
        "Not available while a USB DAC is connected.".into()
    } else if prefs.offload && !policy.offload {
        "Paused now: the equalizer or another effect is on.".into()
    } else {
        "The audio chip decodes instead of the processor. Pauses while effects are on.".into()
    }
}

/// The sleep timer under the seek bar: "Sleep · end of track", or the minutes left rounded up, never
/// fewer than one ("Sleep · 12 min").
#[uniffi::export]
pub fn words_sleep(end_of_track: bool, left_ms: i64) -> String {
    if end_of_track {
        "Sleep · end of track".into()
    } else {
        format!("Sleep · {} min", ((left_ms + 59_999) / 60_000).max(1))
    }
}

/// After songs were put in the download queue.
#[uniffi::export]
pub fn words_downloading(songs: u32) -> String {
    format!("Downloading {}", counted(songs, "song", "songs"))
}

/// After downloads were given back.
#[uniffi::export]
pub fn words_downloads_removed(songs: u32) -> String {
    format!("Removed {}", counted(songs, "download", "downloads"))
}

/// After an M3U import: how many of its entries were found in the index and went into `playlist`.
pub(crate) fn m3u_imported(found: usize, entries: usize, playlist: &str) -> String {
    if found == 0 {
        format!("None of the {entries} entries are on this phone yet. Update the offline search first.")
    } else {
        format!("Imported {found} of {entries} tracks into {playlist}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn numbers_agree_with_their_nouns() {
        assert_eq!((words_songs(1), words_songs(0)), ("1 song".into(), "0 songs".into()));
        assert_eq!(words_download_queue(0, 0), "Nothing downloading");
        assert_eq!(words_download_queue(4, 0), "4 to go");
        assert_eq!(words_download_queue(0, 2), "2 failed");
        assert_eq!(words_download_queue(4, 1), "4 to go · 1 failed");
        assert_eq!(words_downloading(1), "Downloading 1 song");
        assert_eq!(words_sleep(true, 0), "Sleep · end of track");
        assert_eq!((words_sleep(false, 60_000), words_sleep(false, 60_001)), ("Sleep · 1 min".into(), "Sleep · 2 min".into()));
        assert_eq!(words_sleep(false, -5_000), "Sleep · 1 min", "a timer about to fire still says a minute");
    }

    #[test]
    fn the_output_rows_say_what_is_going_out() {
        let dac = Some("Qudelix".to_string());
        assert_eq!(words_bit_perfect(dac.clone(), true, 44_100, 24, None, true), "On: Qudelix, 44.1 kHz, 24-bit.");
        assert_eq!(words_bit_perfect(None, false, 0, 0, Some("no 24-bit".into()), false), "USB DAC: no 24-bit");
        assert_eq!(words_bit_perfect(dac.clone(), false, 0, 0, None, true), "Qudelix connected. Starts with the music.");
        assert_eq!(words_bit_perfect(dac, false, 0, 0, None, false), "Qudelix connected, but this phone can't do bit perfect with it.");
        assert!(words_bit_perfect(None, false, 0, 0, None, false).starts_with("Sends the file"));
        assert_eq!(words_dac_detail(vec!["44.1 kHz / 24 bit".into(), "48 kHz / 24 bit".into()], None, Some("48 kHz".into())).as_deref(), Some("Offers 44.1 kHz / 24 bit, 48 kHz / 24 bit  ·  output 48 kHz"));
        assert_eq!(words_dac_detail(vec![], None, None), None);
        let prefs = crate::AudioPrefs { dsp: false, skip_silence: false, offload: true, crossfade_s: 0, auto_mix: false, speed: 1.0, pitch: 1.0 };
        let out = crate::OutputState::default();
        assert!(words_offload(prefs, crate::OutputState { usb: true, ..out }).starts_with("Not available"));
        assert!(words_offload(prefs, out).starts_with("The audio chip"));
        // Everything the player stands offload down for says so, AutoMix and pitch included, which
        // the note used to miss.
        for p in [
            crate::AudioPrefs { speed: 1.25, ..prefs },
            crate::AudioPrefs { pitch: 0.9, ..prefs },
            crate::AudioPrefs { auto_mix: true, ..prefs },
            crate::AudioPrefs { dsp: true, ..prefs },
        ] {
            assert!(words_offload(p, out).starts_with("Paused now"), "{p:?}");
        }
        assert!(words_offload(crate::AudioPrefs { offload: false, dsp: true, ..prefs }, out).starts_with("The audio chip"));
        assert_eq!(words_downloading(0), "Downloading 0 songs");
        assert_eq!(words_downloading(12), "Downloading 12 songs");
        assert_eq!(words_downloads_removed(1), "Removed 1 download");
        assert_eq!(words_downloads_removed(3), "Removed 3 downloads");
        assert_eq!(m3u_imported(0, 4, "Road"), "None of the 4 entries are on this phone yet. Update the offline search first.");
        assert_eq!(m3u_imported(3, 4, "Road"), "Imported 3 of 4 tracks into Road");
    }

    #[test]
    fn confirmations() {
        assert_eq!(words(Said::AddedToPlaylist, "Road".into()), "Added to Road");
        assert_eq!(words(Said::PlaylistCreated, "Road".into()), "Created Road");
        assert_eq!(words(Said::PlayingNext, String::new()), "Playing next");
        assert_eq!((favourite(true), favourite(false)), ("Added to favourites".into(), "Removed from favourites".into()));
    }
}
