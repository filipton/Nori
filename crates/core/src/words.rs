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

/// The system media controls' extra buttons (the notification, the lock screen): a heart beside previous
/// and a shuffle toggle beside next. `heart` is none while no song of the library plays (nothing, or a
/// radio stream): there is nothing to favourite.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct SessionButtons {
    pub heart: Option<String>,
    pub starred: bool,
    pub shuffle: String,
    pub shuffling: bool,
}

/// The session's buttons for the song playing now in the core's queue, `starred` or not, `shuffle` on or off.
#[uniffi::export]
pub fn words_session_buttons(starred: bool, shuffle: bool) -> SessionButtons {
    let song = crate::playlist::with(|p| p.current_id().is_some_and(|id| !id.starts_with(crate::queue::RADIO_PREFIX)));
    session_buttons(song, starred, shuffle)
}

fn session_buttons(song: bool, starred: bool, shuffle: bool) -> SessionButtons {
    SessionButtons {
        heart: song.then(|| (if starred { "Remove from favourites" } else { "Add to favourites" }).into()),
        starred: song && starred,
        shuffle: (if shuffle { "Shuffle off" } else { "Shuffle on" }).into(),
        shuffling: shuffle,
    }
}

/// What a radio stream shows as its title: what the station announces now (its ICY title), or, while it
/// announces nothing, the station's own name.
#[uniffi::export]
pub fn radio_title(announced: Option<String>, station: Option<String>) -> Option<String> {
    announced.filter(|a| !a.trim().is_empty()).or(station)
}

/// What a radio stream is listed under where a song shows its artist.
#[uniffi::export]
pub fn words_radio_artist() -> String {
    "Radio".into()
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

/// How many releases an artist has: "1 release", "12 releases".
#[uniffi::export]
pub fn words_releases(n: u32) -> String {
    counted(n, "release", "releases")
}

/// How many albums an artist has, under their name in the artists list: "1 album", "12 albums".
#[uniffi::export]
pub fn words_albums(n: u32) -> String {
    counted(n, "album", "albums")
}

/// A folder's caption: "2 folders · 14 songs".
#[uniffi::export]
pub fn words_folder(folders: u32, songs: u32) -> String {
    format!("{} · {}", counted(folders, "folder", "folders"), counted(songs, "song", "songs"))
}

/// A folder's title: its name, or "Folder" for one the server did not name.
#[uniffi::export]
pub fn words_folder_title(name: String) -> String {
    if name.is_empty() { "Folder".into() } else { name }
}

/// A decade by its first year: "1990s".
#[uniffi::export]
pub fn words_decade(start_year: u32) -> String {
    format!("{start_year}s")
}

/// The favourite songs row: how many of the starred songs are in the library (a provider's song that was
/// starred is being fetched by the server, not yet something to play).
#[uniffi::export]
pub fn words_favourite_songs(songs: Vec<crate::Song>) -> String {
    words_songs(songs.iter().filter(|s| !s.is_external).count() as u32)
}

/// A playlist's line in the library: "12 songs · 48:10".
#[uniffi::export]
pub fn words_playlist_line(songs: u32, seconds: u32) -> String {
    format!("{} · {}", words_songs(songs), crate::fmt::duration(seconds as i64))
}

/// What the player's title says with no song: the station playing, or that nothing is.
#[uniffi::export]
pub fn words_player_idle(radio: Option<String>) -> String {
    radio.unwrap_or_else(|| "Nothing playing".into())
}

/// The line under the player's title while the offline bridge plays downloads in place of the queue.
#[uniffi::export]
pub fn words_bridging() -> String {
    "Playing downloads until you’re online".into()
}

/// The now playing bar's second line: what went wrong with the song showing, else its artist, else
/// (a station) "Radio".
#[uniffi::export]
pub fn words_bar_line(error: Option<String>, artist: Option<String>) -> String {
    error.or(artist).unwrap_or_else(|| "Radio".into())
}

/// Where lyrics came from, for the credit line under them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LyricsOrigin {
    Server,
    Lrclib,
}

/// A lyrics source's name: "your server", "LRCLIB".
#[uniffi::export]
pub fn words_lyrics_origin(origin: LyricsOrigin) -> String {
    match origin {
        LyricsOrigin::Server => "your server",
        LyricsOrigin::Lrclib => "LRCLIB",
    }
    .into()
}

/// The corner under the lyrics, closed: whose words these are when they are not the server's, and when
/// they came without timings, that they did - unsung words are all one brightness and a tap on one goes
/// nowhere, which looks broken unless the corner says why. The server's timed words say "Timing" (the
/// corner opens the nudge buttons); the server's untimed words have no corner at all (None).
#[uniffi::export]
pub fn words_lyrics_credit(origin: LyricsOrigin, synced: bool) -> Option<String> {
    let source = (origin != LyricsOrigin::Server).then(|| words_lyrics_origin(origin));
    if source.is_none() && !synced {
        return None;
    }
    let first = source.or_else(|| synced.then(|| "Timing".to_string()));
    Some([first, (!synced).then(|| "not timed".to_string())].into_iter().flatten().collect::<Vec<_>>().join(" · "))
}

/// The heading over the queue in the player: what plays after this song.
#[uniffi::export]
pub fn words_up_next() -> String {
    "Playing next".into()
}

/// The toast when the phone has no output picker to open.
#[uniffi::export]
pub fn words_playing_through(output: String) -> String {
    format!("Playing through {output}")
}

/// Stopping every download, and whether it asks first.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StopAll {
    /// More than one song would leave the queue: ask before doing it.
    pub asks: bool,
    pub title: String,
    pub text: String,
}

#[uniffi::export]
pub fn words_stop_all(unfinished: u32) -> StopAll {
    let (that, leave) = if unfinished == 1 { ("has", "leaves") } else { ("have", "leave") };
    StopAll {
        asks: unfinished > 1,
        title: "Stop all downloads?".into(),
        text: format!("{} that {that} not finished downloading {leave} the queue. Songs already downloaded stay.", counted(unfinished, "song", "songs")),
    }
}

/// The empty-list, failure and help lines around the app, by where they are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum Note {
    /// No index yet: the songs and decades lists.
    NoIndex,
    NoPlaylists,
    NoStations,
    NothingDownloaded,
    NoDownloads,
    NoDownloadsHelp,
    DownloadFailed,
    NoFavouriteSongs,
    NothingToMix,
    SmartHelp,
    NoHistory,
    NothingFound,
    NoLyrics,
    /// Over the reason, where a page could not be read.
    CouldNotLoad,
    CouldNotEvaluate,
}

#[uniffi::export]
pub fn words_note(note: Note) -> String {
    match note {
        Note::NoIndex => "No songs on this phone yet. Settings, then Library and lists, then Update.",
        Note::NoPlaylists => "No playlists yet",
        Note::NoStations => "No stations yet",
        Note::NothingDownloaded => "Nothing downloaded yet",
        Note::NoDownloads => "No downloads",
        Note::NoDownloadsHelp => "Songs you download show here while they arrive, and for a while after.",
        Note::DownloadFailed => "Couldn't download",
        Note::NoFavouriteSongs => "No favourite songs yet. Tap the heart on a song and it will be here.",
        Note::NothingToMix => "Nothing to mix yet. Play some music, or sync the library in Settings.",
        Note::SmartHelp => "Matched against the synced library: sync it in Settings so every song can be found.",
        Note::NoHistory => "Nothing played yet, or listening history is off in Settings, under Library and lists.",
        Note::NothingFound => "Nothing found",
        Note::NoLyrics => "No lyrics",
        Note::CouldNotLoad => "Could not load",
        Note::CouldNotEvaluate => "Could not evaluate",
    }
    .into()
}

/// The download notification's words that do not change: the batch is over, and its button.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DownloadNoticeWords {
    pub complete: String,
    pub cancel: String,
    /// How long a batch's result stays when nothing failed; one that failed stays until tapped.
    pub result_timeout_ms: i64,
}

#[uniffi::export]
pub fn words_download_notice() -> DownloadNoticeWords {
    DownloadNoticeWords { complete: "Downloads complete".into(), cancel: "Cancel".into(), result_timeout_ms: 8_000 }
}

/// The home-screen widget's title with nothing playing yet.
#[uniffi::export]
pub fn words_widget_idle() -> String {
    "Nori".into()
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
    fn the_session_buttons_say_what_a_press_does() {
        let b = session_buttons(true, true, false);
        assert_eq!((b.heart.as_deref(), b.starred, b.shuffle.as_str()), (Some("Remove from favourites"), true, "Shuffle on"));
        let b = session_buttons(true, false, true);
        assert_eq!((b.heart.as_deref(), b.shuffle.as_str()), (Some("Add to favourites"), "Shuffle off"));
        let b = session_buttons(false, true, false);
        assert_eq!((b.heart, b.starred), (None, false), "a radio stream or nothing: no heart");
    }

    #[test]
    fn a_stream_is_titled_by_what_it_announces() {
        let s = |v: &str| Some(v.to_string());
        assert_eq!(radio_title(s("Artist - Song"), s("FM 4")), s("Artist - Song"));
        assert_eq!(radio_title(s("  "), s("FM 4")), s("FM 4"), "announcing nothing");
        assert_eq!(radio_title(None, s("FM 4")), s("FM 4"));
        assert_eq!(radio_title(None, None), None);
        assert_eq!(words_radio_artist(), "Radio");
    }

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
    fn counts_and_lines_the_screens_used_to_write() {
        assert_eq!((words_releases(1), words_releases(3)), ("1 release".into(), "3 releases".into()));
        assert_eq!((words_albums(1), words_albums(0)), ("1 album".into(), "0 albums".into()));
        assert_eq!(words_folder(2, 14), "2 folders · 14 songs");
        assert_eq!(words_folder(1, 1), "1 folder · 1 song");
        assert_eq!((words_folder_title(String::new()), words_folder_title("Rock".into())), ("Folder".into(), "Rock".into()));
        assert_eq!(words_decade(1990), "1990s");
        let s = |ext| crate::Song { is_external: ext, ..Default::default() };
        assert_eq!(words_favourite_songs(vec![s(false), s(true), s(false)]), "2 songs");
        assert_eq!(words_playlist_line(12, 2890), "12 songs · 48:10");
        assert_eq!(words_playlist_line(1, 200), "1 song · 3:20");
        assert_eq!(words_player_idle(None), "Nothing playing");
        assert_eq!(words_player_idle(Some("FIP".into())), "FIP");
        assert_eq!(words_bridging(), "Playing downloads until you’re online");
        assert_eq!(words_bar_line(Some("Offline".into()), Some("A".into())), "Offline");
        assert_eq!(words_bar_line(None, Some("A".into())), "A");
        assert_eq!(words_bar_line(None, None), "Radio");
        assert_eq!(words_playing_through("Phone speaker".into()), "Playing through Phone speaker");
        assert_eq!(words_note(Note::DownloadFailed), "Couldn't download");
        assert_eq!(words_note(Note::CouldNotLoad), "Could not load");
        assert_eq!(words_download_notice().complete, "Downloads complete");
        assert_eq!(words_widget_idle(), "Nori");
    }

    #[test]
    fn the_lyrics_corner_says_whose_words_and_whether_timed() {
        assert_eq!(words_lyrics_credit(LyricsOrigin::Server, true).as_deref(), Some("Timing"));
        assert_eq!(words_lyrics_credit(LyricsOrigin::Server, false), None);
        assert_eq!(words_lyrics_credit(LyricsOrigin::Lrclib, true).as_deref(), Some("LRCLIB"));
        assert_eq!(words_lyrics_credit(LyricsOrigin::Lrclib, false).as_deref(), Some("LRCLIB · not timed"));
        assert_eq!(words_lyrics_origin(LyricsOrigin::Server), "your server");
    }

    #[test]
    fn stopping_everything_asks_only_for_more_than_one() {
        assert!(!words_stop_all(1).asks);
        let many = words_stop_all(3);
        assert!(many.asks);
        assert_eq!(many.title, "Stop all downloads?");
        assert_eq!(many.text, "3 songs that have not finished downloading leave the queue. Songs already downloaded stay.");
    }

    #[test]
    fn confirmations() {
        assert_eq!(words(Said::AddedToPlaylist, "Road".into()), "Added to Road");
        assert_eq!(words(Said::PlaylistCreated, "Road".into()), "Created Road");
        assert_eq!(words(Said::PlayingNext, String::new()), "Playing next");
        assert_eq!((favourite(true), favourite(false)), ("Added to favourites".into(), "Removed from favourites".into()));
    }
}
