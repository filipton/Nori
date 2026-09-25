//! The app's short confirmations, worded here so every player says the same.

use nori_model::lines::counted;
use nori_model::{AudioPrefs, OutputState, PlaybackError, Song};

/// A one-line confirmation after an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
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
#[cfg_attr(feature = "ffi", uniffi::export)]
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

/// The system media controls' extra buttons (the notification, the lock screen): a heart beside previous
/// and a shuffle toggle beside next. `heart` is none while no song of the library plays (nothing, or a
/// radio stream): there is nothing to favourite.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SessionButtons {
    pub heart: Option<String>,
    pub starred: bool,
    pub shuffle: String,
    pub shuffling: bool,
}

/// The session's buttons with a song of the library playing (`song`) or not, `starred` or not, `shuffle`
/// on or off. The core answers them for the song playing now (`words_session_buttons`).
pub fn session_buttons(song: bool, starred: bool, shuffle: bool) -> SessionButtons {
    SessionButtons {
        heart: song.then(|| (if starred { "Remove from favourites" } else { "Add to favourites" }).into()),
        starred: song && starred,
        shuffle: (if shuffle { "Shuffle off" } else { "Shuffle on" }).into(),
        shuffling: shuffle,
    }
}

/// What a radio stream shows as its title: what the station announces now (its ICY title), or, while it
/// announces nothing, the station's own name.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn radio_title(announced: Option<String>, station: Option<String>) -> Option<String> {
    announced.filter(|a| !a.trim().is_empty()).or(station)
}

/// What a radio stream is listed under where a song shows its artist.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_radio_artist() -> String {
    "Radio".into()
}

/// What a heart says when pressed; the core says it only with the favourite notice on (`words_favourite`).
pub fn favourite(on: bool) -> String {
    (if on { "Added to favourites" } else { "Removed from favourites" }).into()
}

/// How many songs a list holds: "1 song", "12 songs".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_songs(songs: u32) -> String {
    nori_model::lines::songs(songs)
}

/// The download queue's row in the library: "4 to go · 1 failed", or that nothing is downloading.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_download_queue(waiting: u32, failed: u32) -> String {
    match (waiting, failed) {
        (0, 0) => "Nothing downloading".into(),
        (w, 0) => format!("{w} to go"),
        (0, f) => format!("{f} failed"),
        (w, f) => format!("{w} to go · {f} failed"),
    }
}

/// The bit-perfect switch's second line: what the DAC is doing, why it cannot, or what the switch is for.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_bit_perfect(device: Option<String>, on: bool, sample_rate: u32, bits: u32, blocked_by: Option<String>, supported: bool) -> String {
    match (device, blocked_by) {
        (d, _) if on => format!("On: {}, {:?} kHz, {bits}-bit.", d.as_deref().unwrap_or("null"), sample_rate as f64 / 1000.0),
        (d, Some(why)) => format!("{}: {why}", d.as_deref().unwrap_or("USB DAC")),
        (Some(d), None) if supported => format!("{d} connected. Starts with the music."),
        (Some(d), None) => format!("{d} connected, but this device can't do bit perfect with it."),
        (None, None) => "Sends the file to a USB DAC unchanged. Skips the equalizer and volume levelling. Android 14 and later.".into(),
    }
}

/// What is actually going out, under the bit-perfect switch: "Offers … · playing … · output …".
#[cfg_attr(feature = "ffi", uniffi::export)]
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
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_offload(prefs: AudioPrefs, output: OutputState) -> String {
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
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_sleep(end_of_track: bool, left_ms: i64) -> String {
    if end_of_track {
        "Sleep · end of track".into()
    } else {
        format!("Sleep · {} min", ((left_ms + 59_999) / 60_000).max(1))
    }
}

/// After songs were put in the download queue.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_downloading(songs: u32) -> String {
    format!("Downloading {}", counted(songs, "song", "songs"))
}

/// After downloads were given back.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_downloads_removed(songs: u32) -> String {
    format!("Removed {}", counted(songs, "download", "downloads"))
}

/// How many releases an artist has: "1 release", "12 releases".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_releases(n: u32) -> String {
    counted(n, "release", "releases")
}

/// How many albums an artist has, under their name in the artists list: "1 album", "12 albums".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_albums(n: u32) -> String {
    nori_model::lines::albums(n)
}

/// A folder's caption: "2 folders · 14 songs".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_folder(folders: u32, songs: u32) -> String {
    format!("{} · {}", counted(folders, "folder", "folders"), counted(songs, "song", "songs"))
}

/// A folder's title: its name, or "Folder" for one the server did not name.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_folder_title(name: String) -> String {
    if name.is_empty() { "Folder".into() } else { name }
}

/// A decade by its first year: "1990s".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_decade(start_year: u32) -> String {
    format!("{start_year}s")
}

/// The favourite songs row: how many of the starred songs are in the library (a provider's song that was
/// starred is being fetched by the server, not yet something to play). Carried by the favourites answer.
pub fn words_favourite_songs(songs: &[Song]) -> String {
    words_songs(songs.iter().filter(|s| !s.is_external).count() as u32)
}

/// A playlist's line in the library: "12 songs · 48:10".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_playlist_line(songs: u32, seconds: u32) -> String {
    nori_model::lines::playlist_line(songs, seconds)
}

/// What the player's title says with no song: the station playing, or that nothing is.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_player_idle(radio: Option<String>) -> String {
    radio.unwrap_or_else(|| "Nothing playing".into())
}

/// The line under the player's title while the offline bridge plays downloads in place of the queue.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_bridging() -> String {
    "Playing downloads until you’re online".into()
}

/// The now playing bar's second line: what went wrong with the song showing, else its artist, else
/// (a station) "Radio".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_bar_line(error: Option<String>, artist: Option<String>) -> String {
    error.or(artist).unwrap_or_else(|| "Radio".into())
}

/// Where lyrics came from, for the credit line under them: the server, or the lyrics service that
/// answered (nori-settings' `LyricsService`, one for one).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum LyricsOrigin {
    Server,
    Binilyrics,
    BetterLyrics,
    Paxsenix,
    LyricsPlus,
    Portato,
    PaxsenixMusixmatch,
    Simpmusic,
    Unison,
    Netease,
    Kugou,
    Lrclib,
    PaxsenixSpotify,
    YoutubeCaptions,
    Megalobiz,
    YoutubeMusic,
    Genius,
}

/// A lyrics source's name as the credit gives it: "your server", "LRCLIB". A service that relays
/// another catalogue is named for itself, which is who answered; Unison's data asks to be credited by name.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_lyrics_origin(origin: LyricsOrigin) -> String {
    match origin {
        LyricsOrigin::Server => "your server",
        LyricsOrigin::Binilyrics => "BiniLyrics",
        LyricsOrigin::BetterLyrics | LyricsOrigin::Portato => "BetterLyrics",
        LyricsOrigin::Paxsenix | LyricsOrigin::PaxsenixMusixmatch | LyricsOrigin::PaxsenixSpotify => "PaxSenix",
        LyricsOrigin::LyricsPlus => "LyricsPlus",
        LyricsOrigin::Simpmusic => "SimpMusic",
        LyricsOrigin::Unison => "Unison",
        LyricsOrigin::Netease => "NetEase",
        LyricsOrigin::Kugou => "KuGou",
        LyricsOrigin::Lrclib => "LRCLIB",
        LyricsOrigin::YoutubeCaptions => "YouTube",
        LyricsOrigin::Megalobiz => "Megalobiz",
        LyricsOrigin::YoutubeMusic => "YouTube Music",
        LyricsOrigin::Genius => "Genius",
    }
    .into()
}

/// The corner under the lyrics, closed: whose words these are when they are not the server's, and when
/// they came without timings, that they did - unsung words are all one brightness and a tap on one goes
/// nowhere, which looks broken unless the corner says why. The server's timed words say "Timing" (the
/// corner opens the nudge buttons); the server's untimed words have no corner at all (None).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_lyrics_credit(origin: LyricsOrigin, synced: bool) -> Option<String> {
    let source = (origin != LyricsOrigin::Server).then(|| words_lyrics_origin(origin));
    if source.is_none() && !synced {
        return None;
    }
    let first = source.or_else(|| synced.then(|| "Timing".to_string()));
    Some([first, (!synced).then(|| "not timed".to_string())].into_iter().flatten().collect::<Vec<_>>().join(" · "))
}

/// The heading over the queue in the player: what plays after this song.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_up_next() -> String {
    "Playing next".into()
}

/// The toast when the phone has no output picker to open.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_playing_through(output: String) -> String {
    format!("Playing through {output}")
}

/// Stopping every download, and whether it asks first.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StopAll {
    /// More than one song would leave the queue: ask before doing it.
    pub asks: bool,
    pub title: String,
    pub text: String,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_stop_all(unfinished: u32) -> StopAll {
    let (that, leave) = if unfinished == 1 { ("has", "leaves") } else { ("have", "leave") };
    StopAll {
        asks: unfinished > 1,
        title: "Stop all downloads?".into(),
        text: format!("{} that {that} not finished downloading {leave} the queue. Songs already downloaded stay.", counted(unfinished, "song", "songs")),
    }
}

/// The empty-list, failure and help lines around the app, by where they are shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
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

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_note(note: Note) -> String {
    match note {
        Note::NoIndex => "No songs on this device yet. Settings, then Library and lists, then Update.",
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
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DownloadNoticeWords {
    pub complete: String,
    pub cancel: String,
    /// How long a batch's result stays when nothing failed; one that failed stays until tapped.
    pub result_timeout_ms: i64,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_download_notice() -> DownloadNoticeWords {
    DownloadNoticeWords { complete: "Downloads complete".into(), cancel: "Cancel".into(), result_timeout_ms: 8_000 }
}

/// The home-screen widget's title with nothing playing yet.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_widget_idle() -> String {
    "Nori".into()
}

/// After an M3U import: how many of its entries were found in the index and went into `playlist`.
pub fn m3u_imported(found: usize, entries: usize, playlist: &str) -> String {
    if found == 0 {
        format!("None of the {entries} entries are on this device yet. Update the offline search first.")
    } else {
        format!("Imported {found} of {entries} tracks into {playlist}")
    }
}

/// Every fixed word the screens say - titles, buttons, hints, and the names icons are read out by - so
/// every front end says the same. Read once, when the app starts; a word that depends on a number or a
/// name is a function of its own ([`words_selected`] and the rest).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct UiWords {
    // Everywhere: the tabs, the buttons and the icons' names.
    pub home: String,
    pub search: String,
    pub library: String,
    pub settings: String,
    pub back: String,
    pub more: String,
    pub next: String,
    pub previous: String,
    pub play: String,
    pub pause: String,
    pub shuffle: String,
    pub repeat: String,
    pub queue: String,
    pub lyrics: String,
    pub favourite: String,
    pub add_to_favourites: String,
    pub remove_from_favourites: String,
    pub remove: String,
    pub cancel: String,
    pub done: String,
    pub close: String,
    pub create: String,
    pub delete: String,
    pub edit: String,
    pub save: String,
    pub add: String,
    pub copy_it: String,
    pub clear: String,
    pub reset: String,
    pub open: String,
    pub import: String,
    pub name: String,
    pub sort: String,
    pub filter: String,
    pub get: String,
    pub retry: String,
    pub automatic: String,
    pub chosen: String,
    pub now_playing_bar: String,
    pub not_in_library_yet: String,
    pub nothing_matches: String,
    // Home.
    pub listen_now: String,
    pub shuffle_everything: String,
    pub resume_from_server: String,
    pub rearrange_rows: String,
    pub for_you: String,
    pub rows: String,
    pub hold_a_row_to_move_it: String,
    pub not_shown: String,
    // Search.
    pub search_hint: String,
    pub recent_searches: String,
    // The library and its pages.
    pub albums: String,
    pub artists: String,
    pub songs: String,
    pub playlists: String,
    pub smart: String,
    pub history: String,
    pub favourites: String,
    pub genres: String,
    pub decades: String,
    pub folders: String,
    pub radio: String,
    pub downloads: String,
    pub filter_artists: String,
    pub new_playlist: String,
    pub import_playlist_file: String,
    pub export_playlist_file: String,
    pub favourite_songs: String,
    pub starred_favourites: String,
    pub new_station: String,
    pub stream_url: String,
    pub download_queue: String,
    pub add_all_to_queue: String,
    pub download_all: String,
    pub download_everything: String,
    pub add_to_queue: String,
    pub open_in_browser: String,
    pub top_songs: String,
    pub similar_artists: String,
    // A song's menu and a selection.
    pub sleep_timer: String,
    pub add_to_playlist: String,
    pub details: String,
    pub playlist: String,
    pub clear_selection: String,
    // The player, its queue and its lyrics.
    pub added_by_you: String,
    pub reorder: String,
    pub later: String,
    pub sooner: String,
    /// Between the seek bar's times while an AutoMix or a crossfade is being heard; shown in capitals.
    pub mixing: String,
    // Mixes, smart playlists and listening.
    pub new_mix: String,
    pub new_smart_playlist: String,
    pub ready_made: String,
    pub smart_playlist: String,
    pub match_all: String,
    pub match_any: String,
    pub remove_rule: String,
    pub add_rule: String,
    pub sort_by: String,
    pub descending: String,
    pub limit: String,
    /// The limit field's placeholder while it is empty: no limit.
    pub no_limit: String,
    pub listening_stats: String,
    pub clear_history: String,
    pub listening: String,
    pub when_you_listen: String,
    pub top_artists: String,
    pub top_albums: String,
    pub top_genres: String,
    // Downloads.
    pub downloaded: String,
    pub download_failed: String,
    pub stop_all: String,
    pub downloading: String,
    pub waiting: String,
    pub failed: String,
    pub retry_all: String,
    pub finished: String,
    pub stop_download: String,
    // The equalizer.
    pub equalizer: String,
    pub eq_hint: String,
    pub add_band: String,
    pub paste_preset: String,
    pub presets: String,
    pub auto_preamp_hint: String,
    pub output: String,
    pub balance: String,
    pub mono: String,
    pub mono_detail: String,
    pub limiter: String,
    pub limiter_detail: String,
    pub profiles: String,
    pub save_these_settings: String,
    pub save_as_profile: String,
    pub crossfeed: String,
    pub import_preset: String,
    pub import_preset_hint: String,
    pub import_preset_example: String,
    pub no_filters_found: String,
    pub frequency: String,
    pub remove_band: String,
    // Headphone presets and devices.
    pub headphone_presets: String,
    pub autoeq_about: String,
    pub download_the_list: String,
    pub refresh_list: String,
    pub autoeq_no_curve: String,
    pub autoeq_credit: String,
    pub devices: String,
    pub autoeq_auto: String,
    pub autoeq_auto_detail: String,
    pub devices_note: String,
    pub flat: String,
    pub flat_detail: String,
    pub leave_as_is: String,
    pub leave_as_is_detail: String,
    pub saved_profile: String,
    pub autoeq_curves: String,
    pub autoeq_download_hint: String,
    pub forget_device: String,
    // Settings and signing in.
    pub search_settings: String,
    pub performance: String,
    pub performance_detail: String,
    pub server: String,
    pub server_hint: String,
    pub server_url: String,
    pub user: String,
    pub password: String,
    pub connecting: String,
    pub connect: String,
    pub hide_advanced: String,
    pub advanced: String,
    pub name_optional: String,
    pub second_address: String,
    pub second_address_hint: String,
    pub api_key: String,
    pub extra_headers: String,
    pub extra_headers_hint: String,
    pub legacy_auth: String,
    pub legacy_auth_hint: String,
    pub self_signed: String,
    pub self_signed_hint: String,
    pub wifi_only: String,
    pub wifi_only_hint: String,
    pub client_cert_password: String,
    pub import_client_cert: String,
    pub replace_client_cert: String,
    // About.
    pub under_the_hood: String,
    pub playback: String,
    pub library_and_search: String,
    pub automix: String,
    pub interface_title: String,
    pub open_source: String,
    pub free_software: String,
    pub licence_line: String,
    pub licences: String,
    pub licences_detail: String,
    pub rust_core: String,
    pub fonts_and_data: String,
    pub no_licence_text: String,
    pub licence_unreadable: String,
}

/// The screens' fixed words ([`UiWords`]), in one call.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_ui() -> UiWords {
    let w = |s: &str| s.to_string();
    UiWords {
        home: w("Home"),
        search: w("Search"),
        library: w("Library"),
        settings: w("Settings"),
        back: w("Back"),
        more: w("More"),
        next: w("Next"),
        previous: w("Previous"),
        play: w("Play"),
        pause: w("Pause"),
        shuffle: w("Shuffle"),
        repeat: w("Repeat"),
        queue: w("Queue"),
        lyrics: w("Lyrics"),
        favourite: w("Favourite"),
        add_to_favourites: w("Add to favourites"),
        remove_from_favourites: w("Remove from favourites"),
        remove: w("Remove"),
        cancel: w("Cancel"),
        done: w("Done"),
        close: w("Close"),
        create: w("Create"),
        delete: w("Delete"),
        edit: w("Edit"),
        save: w("Save"),
        add: w("Add"),
        copy_it: w("Copy"),
        clear: w("Clear"),
        reset: w("Reset"),
        open: w("Open"),
        import: w("Import"),
        name: w("Name"),
        sort: w("Sort"),
        filter: w("Filter"),
        get: w("Get"),
        retry: w("Retry"),
        automatic: w("Automatic"),
        chosen: w("Chosen"),
        now_playing_bar: w("Now playing bar"),
        not_in_library_yet: w("Not in library yet"),
        nothing_matches: w("Nothing matches"),
        listen_now: w("Listen now"),
        shuffle_everything: w("Shuffle everything"),
        resume_from_server: w("Resume from server"),
        rearrange_rows: w("Rearrange rows"),
        for_you: w("For you"),
        rows: w("Rows"),
        hold_a_row_to_move_it: w("Hold a row to move it"),
        not_shown: w("Not shown"),
        search_hint: w("Songs, albums, artists"),
        recent_searches: w("Recent searches"),
        albums: w("Albums"),
        artists: w("Artists"),
        songs: w("Songs"),
        playlists: w("Playlists"),
        smart: w("Smart"),
        history: w("History"),
        favourites: w("Favourites"),
        genres: w("Genres"),
        decades: w("Decades"),
        folders: w("Folders"),
        radio: w("Radio"),
        downloads: w("Downloads"),
        filter_artists: w("Filter artists"),
        new_playlist: w("New playlist"),
        import_playlist_file: w("Import M3U…"),
        export_playlist_file: w("Export M3U"),
        favourite_songs: w("Favourite songs"),
        starred_favourites: w("★ Favourites"),
        new_station: w("New station"),
        stream_url: w("Stream URL"),
        download_queue: w("Download queue"),
        add_all_to_queue: w("Add all to queue"),
        download_all: w("Download all"),
        download_everything: w("Download everything"),
        add_to_queue: w("Add to queue"),
        open_in_browser: w("Open in browser?"),
        top_songs: w("Top songs"),
        similar_artists: w("Similar artists"),
        sleep_timer: w("Sleep timer"),
        add_to_playlist: w("Add to playlist"),
        details: w("Details"),
        playlist: w("Playlist"),
        clear_selection: w("Clear selection"),
        added_by_you: w("Added by you"),
        reorder: w("Reorder"),
        later: w("Later"),
        sooner: w("Sooner"),
        mixing: w("Mixing"),
        new_mix: w("New mix"),
        new_smart_playlist: w("New smart playlist"),
        ready_made: w("Ready made"),
        smart_playlist: w("Smart playlist"),
        match_all: w("Match all"),
        match_any: w("Match any"),
        remove_rule: w("Remove rule"),
        add_rule: w("Add rule"),
        sort_by: w("Sort by"),
        descending: w("Descending"),
        limit: w("Limit"),
        no_limit: w("none"),
        listening_stats: w("Listening stats"),
        clear_history: w("Clear history"),
        listening: w("Listening"),
        when_you_listen: w("When you listen"),
        top_artists: w("Top artists"),
        top_albums: w("Top albums"),
        top_genres: w("Top genres"),
        downloaded: w("Downloaded"),
        download_failed: w("Download failed"),
        stop_all: w("Stop all"),
        downloading: w("Downloading"),
        waiting: w("Waiting"),
        failed: w("Failed"),
        retry_all: w("Retry all"),
        finished: w("Finished"),
        stop_download: w("Stop download"),
        equalizer: w("Equalizer"),
        eq_hint: w("Tap a band's label to change its frequency, width or type."),
        add_band: w("Add band"),
        paste_preset: w("Paste a preset"),
        presets: w("Presets"),
        auto_preamp_hint: w("Automatic pulls the level down by the largest boost so the curve cannot clip"),
        output: w("Output"),
        balance: w("Balance"),
        mono: w("Mono"),
        mono_detail: w("Both channels summed, for one-earbud listening"),
        limiter: w("Limiter"),
        limiter_detail: w("Catches what a boost or a positive ReplayGain would clip. Adds 5 ms of delay; below the ceiling the audio passes through untouched."),
        profiles: w("Profiles"),
        save_these_settings: w("Save these settings"),
        save_as_profile: w("Save current settings as a profile"),
        crossfeed: w("Crossfeed"),
        import_preset: w("Import preset"),
        import_preset_hint: w("Paste an AutoEQ ParametricEQ.txt or GraphicEQ.txt, or an Equalizer APO config."),
        import_preset_example: w("Preamp: -6.2 dB\nFilter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70"),
        no_filters_found: w("No filters found in that text"),
        frequency: w("Frequency"),
        remove_band: w("Remove band"),
        headphone_presets: w("Headphone presets"),
        autoeq_about: w("AutoEQ measures headphones and publishes a correction curve for each. The list is one 850 kB request to github.com, made by itself on Wi-Fi while Keep the AutoEQ list is on; after that, searching happens on this device."),
        download_the_list: w("Download the list"),
        refresh_list: w("Refresh list"),
        autoeq_no_curve: w("AutoEQ has no curve for that one, so it's left out of the list from now on."),
        autoeq_credit: w("Curves by the AutoEQ project (jaakkopasanen/AutoEq). Tapping one replaces the equalizer's bands."),
        devices: w("Devices"),
        autoeq_auto: w("Apply AutoEQ automatically"),
        autoeq_auto_detail: w("Headphones with a known AutoEQ curve get it as soon as they connect, instead of being asked."),
        devices_note: w("Devices keep their sound only while Settings, then Sound, then Remember sound per device is on."),
        flat: w("Flat"),
        flat_detail: w("The equalizer off on this device"),
        leave_as_is: w("Leave as is"),
        leave_as_is_detail: w("Nothing switches and nothing is offered"),
        saved_profile: w("Saved profile"),
        autoeq_curves: w("AutoEQ curves"),
        autoeq_download_hint: w("Download the AutoEQ list once (850 kB from github.com) to pick a curve by headphone name, for example the IEMs you plug into this DAC."),
        forget_device: w("Forget this device"),
        search_settings: w("Search settings"),
        performance: w("Performance"),
        performance_detail: w("Battery, CPU, wakeups and frames, as recorded"),
        server: w("Server"),
        server_hint: w("Navidrome, octo-fiesta or any Subsonic server"),
        server_url: w("Server URL"),
        user: w("User"),
        password: w("Password"),
        connecting: w("Connecting…"),
        connect: w("Connect"),
        hide_advanced: w("Hide advanced"),
        advanced: w("Advanced"),
        name_optional: w("Name (optional)"),
        second_address: w("Second address (e.g. public URL)"),
        second_address_hint: w("Used when the first one does not answer, for example away from home"),
        api_key: w("API key instead of password (OpenSubsonic)"),
        extra_headers: w("Extra HTTP headers"),
        extra_headers_hint: w("One per line, Name: value. For reverse proxies, basic auth, Cloudflare Access."),
        legacy_auth: w("Legacy authentication"),
        legacy_auth_hint: w("For old servers without token auth. Detected automatically when the server says so."),
        self_signed: w("Accept self-signed certificate"),
        self_signed_hint: w("Only for your own server. Certificates you installed in Android are accepted without this."),
        wifi_only: w("Wi-Fi only"),
        wifi_only_hint: w("Never contact this server over mobile data"),
        client_cert_password: w("Client certificate password"),
        import_client_cert: w("Import client certificate (.p12)"),
        replace_client_cert: w("Replace client certificate"),
        under_the_hood: w("Under the hood"),
        playback: w("Playback"),
        library_and_search: w("Library and search"),
        automix: w("AutoMix"),
        interface_title: w("Interface"),
        open_source: w("Open source"),
        free_software: w("Nori is free software"),
        licence_line: w("MIT licence  ·  Copyright (c) 2026 filipton"),
        licences: w("Licences"),
        licences_detail: w("The libraries, fonts and data this app is made of, and their terms"),
        rust_core: w("Rust core"),
        fonts_and_data: w("Fonts and data"),
        no_licence_text: w("No licence text to reproduce."),
        licence_unreadable: w("The licence text could not be read."),
    }
}

/// How many songs are selected, over a list: "3 selected".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_selected(n: u32) -> String {
    format!("{n} selected")
}

/// A song's second line where it is shown on its own (its menu's head): "Artist · Album".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_song_line(artist: String, album: String) -> String {
    if album.is_empty() { artist } else { format!("{artist} · {album}") }
}

/// A folder's row in a folder: a folder mark, then its name.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_folder_row(name: String) -> String {
    format!("📁  {name}")
}

/// What the player says when a song will not play, by what failed (`nori_player::queue::PlaybackError`),
/// where the seek bar's times and the now playing bar show it.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_playback_error(kind: PlaybackError) -> String {
    match kind {
        PlaybackError::Network => "The server could not be reached",
        PlaybackError::Output => "The audio output would not open",
        PlaybackError::Other => "This song would not play",
    }
    .into()
}

/// A request a third party (AutoEQ's lists) answered with an error status and nothing else.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_http_status(status: u16) -> String {
    format!("HTTP {status}")
}

/// What About says this build is made of, one line each, and all of it together for a bug report.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct AboutFacts {
    /// "Nori 0.3.4".
    pub title: String,
    /// The build, the commit, the processor and the Android release.
    pub build: String,
    pub playback: String,
    pub library: String,
    pub automix: String,
    pub ui: String,
    /// Everything above in lines, what a tap on the title copies.
    pub report: String,
}

/// About's facts for the Android app `version`, from `versions` as the build writes them
/// ("media3=1.8.0;okhttp=5.1.0;...", an empty value for one it could not read), a `debug` build or not,
/// its commit `sha` (blank when built outside a checkout), the first `abi` the phone runs and its
/// Android `release` and `sdk`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_about_android(version: String, versions: String, debug: bool, sha: String, abi: Option<String>, release: String, sdk: i32) -> AboutFacts {
    let known: Vec<(&str, &str)> =
        versions.split(';').filter_map(|p| p.split_once('=')).filter(|(_, v)| !v.contains('=') && !v.trim().is_empty()).collect();
    let v = |name: &str| known.iter().find(|(k, _)| *k == name).map(|(_, v)| format!(" {v}")).unwrap_or_default();
    let sha = if sha.trim().is_empty() { "no commit".to_string() } else { sha };
    let abi = abi.unwrap_or_else(|| "unknown ABI".into());
    let build = format!("{} {sha}  ·  {abi}  ·  Android {release} (API {sdk})", if debug { "debug" } else { "release" });
    let playback = format!("Media3 ExoPlayer{}  ·  OkHttp{}  ·  equalizer, crossfeed and limiter in the Rust core", v("media3"), v("okhttp"));
    let library = format!("SQLite with full-text search through rusqlite{}  ·  Rust core reached through uniffi{}", v("rusqlite"), v("uniffi"));
    let automix = format!("Beat and key analysis on this device with RustFFT{}  ·  tempo changes by Signalsmith Stretch{}", v("rustfft"), v("signalsmith-stretch"));
    let ui = format!("Jetpack Compose and Material 3 (BOM{})  ·  covers fetched, kept and decoded by the Rust core", v("composeBom"));
    let title = format!("Nori {version}");
    let report = format!("{title} ({build})\nPlayback: {playback}\nLibrary: {library}\nAutoMix: {automix}\nInterface: {ui}");
    AboutFacts { title, build, playback, library, automix, ui, report }
}

/// The perf build's Performance page: its buttons, headings and notes.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfWords {
    pub share_report: String,
    pub start_fresh: String,
    pub by_state: String,
    pub nothing_recorded: String,
    pub frames: String,
    pub no_frames: String,
    pub stretches: String,
    pub benchmarks: String,
    pub run_calls: String,
    pub run_covers: String,
    pub calls: String,
    pub covers: String,
    /// A benchmark under way.
    pub running: String,
    /// The app's own log and crashes, folded away until asked for.
    pub log: String,
    pub show_log: String,
    pub hide_log: String,
    pub log_note: String,
    /// A stretch's events, unfolded.
    pub hide_events: String,
}

/// The Performance page's fixed words ([`PerfWords`]), in one call.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_perf() -> PerfWords {
    let w = |s: &str| s.to_string();
    PerfWords {
        share_report: w("Share report"),
        start_fresh: w("Start fresh"),
        by_state: w("By state"),
        nothing_recorded: w("Nothing recorded yet. A stretch is kept when the app moves into another state: the screen goes off, music stops, the player opens."),
        frames: w("Frames"),
        no_frames: w("No frames counted yet: they are counted while the app is on screen."),
        stretches: w("Stretches"),
        benchmarks: w("Benchmarks"),
        run_calls: w("Run call benchmark"),
        run_covers: w("Run cover benchmark"),
        calls: w("Calls"),
        covers: w("Covers"),
        running: w("running..."),
        log: w("Log"),
        show_log: w("Show the log"),
        hide_log: w("Hide the log"),
        log_note: w("The app's own recent log and any crash, read when shown. The shared report ends with them."),
        hide_events: w("Hide events"),
    }
}

/// The button that unfolds a stretch's events: "3 events".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_perf_events(count: u32) -> String {
    if count == 1 { "1 event".into() } else { format!("{count} events") }
}

/// A benchmark that stopped with `error`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_perf_failed(error: String) -> String {
    format!("failed: {error}")
}

/// What deleting the profile `name` is read out as.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_delete_named(name: String) -> String {
    format!("Delete {name}")
}

/// A settings search that found nothing.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_nothing_matches(query: String) -> String {
    format!("Nothing matches \"{query}\"")
}

/// A headphone curve was put on the equalizer.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_autoeq_applied(name: String) -> String {
    format!("Applied {name}. The equalizer screen now holds that curve.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_fixed_word_is_said_and_said_once() {
        let w = words_ui();
        assert_eq!((w.back.as_str(), w.play.as_str(), w.shuffle.as_str()), ("Back", "Play", "Shuffle"));
        assert_eq!(w.import_preset_example.lines().count(), 2);
        assert_eq!(words_selected(3), "3 selected");
        assert_eq!(words_nothing_matches("eq".into()), "Nothing matches \"eq\"");
    }

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
        assert_eq!(words_bit_perfect(dac, false, 0, 0, None, false), "Qudelix connected, but this device can't do bit perfect with it.");
        assert!(words_bit_perfect(None, false, 0, 0, None, false).starts_with("Sends the file"));
        assert_eq!(words_dac_detail(vec!["44.1 kHz / 24 bit".into(), "48 kHz / 24 bit".into()], None, Some("48 kHz".into())).as_deref(), Some("Offers 44.1 kHz / 24 bit, 48 kHz / 24 bit  ·  output 48 kHz"));
        assert_eq!(words_dac_detail(vec![], None, None), None);
        let prefs = AudioPrefs { dsp: false, skip_silence: false, offload: true, crossfade_s: 0, auto_mix: false, speed: 1.0, pitch: 1.0 };
        let out = OutputState::default();
        assert!(words_offload(prefs, OutputState { usb: true, ..out }).starts_with("Not available"));
        assert!(words_offload(prefs, out).starts_with("The audio chip"));
        // Everything the player stands offload down for says so, AutoMix and pitch included, which
        // the note used to miss.
        for p in [
            AudioPrefs { speed: 1.25, ..prefs },
            AudioPrefs { pitch: 0.9, ..prefs },
            AudioPrefs { auto_mix: true, ..prefs },
            AudioPrefs { dsp: true, ..prefs },
        ] {
            assert!(words_offload(p, out).starts_with("Paused now"), "{p:?}");
        }
        assert!(words_offload(AudioPrefs { offload: false, dsp: true, ..prefs }, out).starts_with("The audio chip"));
        assert_eq!(words_downloading(0), "Downloading 0 songs");
        assert_eq!(words_downloading(12), "Downloading 12 songs");
        assert_eq!(words_downloads_removed(1), "Removed 1 download");
        assert_eq!(words_downloads_removed(3), "Removed 3 downloads");
        assert_eq!(m3u_imported(0, 4, "Road"), "None of the 4 entries are on this device yet. Update the offline search first.");
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
        let s = |ext| Song { is_external: ext, ..Default::default() };
        assert_eq!(words_favourite_songs(&[s(false), s(true), s(false)]), "2 songs");
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
    fn lines_the_screens_used_to_put_together() {
        assert_eq!(words_song_line("Radiohead".into(), "Amnesiac".into()), "Radiohead · Amnesiac");
        assert_eq!(words_song_line("Radiohead".into(), String::new()), "Radiohead", "no album, no dot");
        assert_eq!(words_folder_row("Live".into()), "📁  Live");
        assert_eq!(words_http_status(404), "HTTP 404");
        assert_eq!(words_ui().no_limit, "none");
        assert_eq!(words_playback_error(PlaybackError::Network), "The server could not be reached");
        assert_eq!(words_playback_error(PlaybackError::Output), "The audio output would not open");
        assert_eq!(words_perf().run_calls, "Run call benchmark");
        assert_eq!(words_perf_failed("boom".into()), "failed: boom");
    }

    #[test]
    fn about_says_what_the_build_is_made_of() {
        let f = words_about_android("0.3.4".into(), "media3=1.8.0;okhttp=;uniffi=0.29;broken".into(), false, "abc1234".into(), Some("arm64-v8a".into()), "16".into(), 36);
        assert_eq!(f.title, "Nori 0.3.4");
        assert_eq!(f.build, "release abc1234  ·  arm64-v8a  ·  Android 16 (API 36)");
        assert_eq!(f.playback, "Media3 ExoPlayer 1.8.0  ·  OkHttp  ·  equalizer, crossfeed and limiter in the Rust core", "a version not read is left out");
        assert_eq!(f.library, "SQLite with full-text search through rusqlite  ·  Rust core reached through uniffi 0.29");
        assert_eq!(f.report.lines().next().unwrap(), "Nori 0.3.4 (release abc1234  ·  arm64-v8a  ·  Android 16 (API 36))");
        assert_eq!(f.report.lines().nth(4).unwrap(), format!("Interface: {}", f.ui));
        let bare = words_about_android("1".into(), String::new(), true, " ".into(), None, "8".into(), 26);
        assert_eq!(bare.build, "debug no commit  ·  unknown ABI  ·  Android 8 (API 26)");
    }

    #[test]
    fn the_lyrics_corner_says_whose_words_and_whether_timed() {
        assert_eq!(words_lyrics_credit(LyricsOrigin::Server, true).as_deref(), Some("Timing"));
        assert_eq!(words_lyrics_credit(LyricsOrigin::Server, false), None);
        assert_eq!(words_lyrics_credit(LyricsOrigin::Lrclib, true).as_deref(), Some("LRCLIB"));
        assert_eq!(words_lyrics_credit(LyricsOrigin::Lrclib, false).as_deref(), Some("LRCLIB · not timed"));
        assert_eq!(words_lyrics_origin(LyricsOrigin::Server), "your server");
        assert_eq!(words_lyrics_credit(LyricsOrigin::Portato, true).as_deref(), Some("BetterLyrics"));
        assert_eq!(words_lyrics_credit(LyricsOrigin::Genius, false).as_deref(), Some("Genius · not timed"));
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
