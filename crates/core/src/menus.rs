//! What a song's menu offers and in what order, what the sleep timer offers, and what a page's
//! download entry says: the screens draw these lists as they come, one icon per action, and do what the
//! action names. Made once each time a menu opens.

use crate::Song;

/// Something the song menu can do. The UI draws one icon per kind and does what it says.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum SongAction {
    /// The heart; `on` is what pressing it sets.
    Favourite { on: bool },
    PlayNext,
    AddToQueue,
    AddToPlaylist,
    RemoveDownload,
    StopDownload,
    Download,
    GoToAlbum { id: String },
    /// `name` is the artist's, for the page to show before it has loaded.
    GoToArtist { id: String, name: String },
    /// A provider's song: octo-fiesta fetches it into the library when it is starred.
    AddToLibrary,
    SleepTimer,
    StartRadio,
    InstantMix,
    ExcludeFromMixes,
    Share,
    Details,
}

/// One line of the song menu.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SongMenuItem {
    pub action: SongAction,
    pub label: String,
    /// Under "More": what one reaches for perhaps once a month.
    pub more: bool,
}

/// Where a song's download stands, as its menu needs to know.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum SongDownload {
    None,
    /// Waiting or on its way.
    Pending,
    Done,
}

/// The menu of `song`. `starred` is the heart as the screen shows it (this session's mark included);
/// `player` is true when the player's own ⋯ opened it, which adds the sleep timer.
///
/// What someone opens a menu for comes first: the heart (the one thing here about the song rather than
/// the queue), queueing it, keeping it, going where it came from. A provider's song (not in the library)
/// has no mix to seed or exclude from and no link to share: those need it on the server.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn song_menu(song: Song, starred: bool, download: SongDownload, player: bool) -> Vec<SongMenuItem> {
    let mut out = Vec::with_capacity(16);
    let mut add = |action: SongAction, label: String, more: bool| out.push(SongMenuItem { action, label, more });
    add(SongAction::Favourite { on: !starred }, (if starred { "Remove from favourites" } else { "Add to favourites" }).into(), false);
    add(SongAction::PlayNext, "Play next".into(), false);
    add(SongAction::AddToQueue, "Add to queue".into(), false);
    add(SongAction::AddToPlaylist, "Add to playlist…".into(), false);
    match download {
        SongDownload::Done => add(SongAction::RemoveDownload, "Remove download".into(), false),
        SongDownload::Pending => add(SongAction::StopDownload, "Stop download".into(), false),
        SongDownload::None => add(SongAction::Download, "Download".into(), false),
    }
    if let Some(id) = &song.album_id {
        add(SongAction::GoToAlbum { id: id.clone() }, "Go to album".into(), false);
    }
    // A song by several artists names each one it can go to; one by a single artist says "artist".
    if song.artists.len() > 1 {
        for a in song.artists.iter().filter(|a| !a.id.is_empty()) {
            add(SongAction::GoToArtist { id: a.id.clone(), name: a.name.clone() }, format!("Go to {}", a.name), false);
        }
    } else if let Some(id) = &song.artist_id {
        add(SongAction::GoToArtist { id: id.clone(), name: song.artist.clone() }, "Go to artist".into(), false);
    }
    if song.is_external {
        let provider = crate::fmt::provider_of(&song.id).unwrap_or_else(|| "provider".into());
        add(SongAction::AddToLibrary, format!("Add to library ({provider})"), false);
    }
    if player {
        add(SongAction::SleepTimer, "Sleep timer…".into(), false);
    }
    add(SongAction::StartRadio, "Start radio from this song".into(), true);
    if !song.is_external {
        add(SongAction::InstantMix, "Instant mix".into(), true);
        add(SongAction::ExcludeFromMixes, "Exclude from mixes".into(), true);
        add(SongAction::Share, "Share link".into(), true);
    }
    add(SongAction::Details, "Details".into(), true);
    out
}

/// One choice of the sleep timer.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SleepChoice {
    pub label: String,
    /// Minutes from now; 0 for the choices that are not a time.
    pub minutes: u32,
    pub end_of_track: bool,
    /// After this many songs; 0 for the rest.
    pub songs: u32,
}

/// The sleep timer's choices, "Off" first while one is running (it is a choice of all zeros).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn sleep_choices(running: bool) -> Vec<SleepChoice> {
    let c = |label: String, minutes, end_of_track, songs| SleepChoice { label, minutes, end_of_track, songs };
    let mut out = Vec::with_capacity(10);
    if running {
        out.push(c("Off".into(), 0, false, 0));
    }
    out.extend([15, 30, 45, 60].map(|m| c(format!("{m} minutes"), m, false, 0)));
    out.push(c("End of track".into(), 0, true, 0));
    out.extend([2, 3, 5, 10].map(|n| c(format!("After {n} songs"), 0, false, n)));
    out
}

/// What a sideways swipe on a song row does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum RowSwipeAct {
    Queue,
    PlayNext,
    /// The heart; `on` is what letting go sets.
    Favourite { on: bool },
    Download,
}

/// What a swipe uncovers under a row, and what letting go does.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct RowSwipe {
    pub act: RowSwipeAct,
    pub label: String,
}

/// The swipe set in the settings as stored (0 nothing, 1 add to queue, 2 play next, 3 favourite,
/// 4 download), on a song whose heart is `starred`; None when that side does nothing.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn row_swipe(setting: u32, starred: bool) -> Option<RowSwipe> {
    let (act, label) = match setting {
        1 => (RowSwipeAct::Queue, "Add to queue"),
        2 => (RowSwipeAct::PlayNext, "Play next"),
        3 => (RowSwipeAct::Favourite { on: !starred }, if starred { "Remove" } else { "Favourite" }),
        4 => (RowSwipeAct::Download, "Download"),
        _ => return None,
    };
    Some(RowSwipe { act, label: label.into() })
}

/// The mark a song row shows for its download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum DownloadGlyph {
    None,
    /// Waiting or arriving: waiting and downloading share the ring, so one flows into the other.
    Ring,
    Done,
    Failed,
}

/// A row's download mark: this session's phase for the song when it has one (as `download_phase`: 0
/// waiting, 1 downloading, 2 failed, 3 done; -1 none), else whether it is downloaded or in the queue.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn download_glyph(phase: i32, downloaded: bool, pending: bool) -> DownloadGlyph {
    match phase {
        3 => DownloadGlyph::Done,
        2 => DownloadGlyph::Failed,
        0 | 1 => DownloadGlyph::Ring,
        _ if downloaded => DownloadGlyph::Done,
        _ if pending => DownloadGlyph::Ring,
        _ => DownloadGlyph::None,
    }
}

/// What a page's download entry does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum DownloadAct {
    /// Download every song of the page.
    All,
    /// Download the songs that are not here yet.
    Missing,
    /// Everything is here: give the space back.
    Remove,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DownloadEntry {
    pub label: String,
    pub act: DownloadAct,
}

/// A page's download entry, for `songs` songs of which `missing` are not downloaded: "Download" is the
/// wrong word once they are all here, and so is offering all of them when only a few are missing.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn download_entry(songs: u32, missing: u32) -> DownloadEntry {
    let e = |label: String, act| DownloadEntry { label, act };
    match (songs, missing) {
        (0, _) => e("Download".into(), DownloadAct::All),
        (_, 0) => e("Remove downloads".into(), DownloadAct::Remove),
        (s, m) if s == m => e("Download".into(), DownloadAct::All),
        (_, m) => e(format!("Download the other {m}"), DownloadAct::Missing),
    }
}

/// The songs of a page not downloaded yet, as positions in `songs` in order: what a [`download_entry`]
/// of [`DownloadAct::Missing`] fetches. `done` answers for one song id.
///
/// Twin of the `missing` list in `downloadEntry` (app/.../ui/DetailScreens.kt), which Android keeps.
pub fn download_missing<'a>(songs: impl IntoIterator<Item = &'a str>, done: impl Fn(&str) -> bool) -> Vec<usize> {
    songs.into_iter().enumerate().filter(|(_, id)| !done(id)).map(|(i, _)| i).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::ArtistRef;

    fn labels(m: &[SongMenuItem]) -> Vec<(&str, bool)> {
        m.iter().map(|i| (i.label.as_str(), i.more)).collect()
    }

    #[test]
    fn a_library_song_offers_everything() {
        let s = Song { id: "1".into(), album_id: Some("al".into()), artist_id: Some("ar".into()), artist: "Björk".into(), ..Default::default() };
        let m = song_menu(s, false, SongDownload::None, false);
        assert_eq!(
            labels(&m),
            [
                ("Add to favourites", false), ("Play next", false), ("Add to queue", false), ("Add to playlist…", false), ("Download", false),
                ("Go to album", false), ("Go to artist", false),
                ("Start radio from this song", true), ("Instant mix", true), ("Exclude from mixes", true), ("Share link", true), ("Details", true),
            ]
        );
        assert_eq!(m[0].action, SongAction::Favourite { on: true });
        assert_eq!(m[6].action, SongAction::GoToArtist { id: "ar".into(), name: "Björk".into() });
    }

    #[test]
    fn a_providers_song_is_offered_to_the_library_and_nothing_that_needs_it_there() {
        let s = Song {
            id: "ext-deezer-song-9".into(),
            is_external: true,
            artists: vec![ArtistRef { id: "a".into(), name: "A".into() }, ArtistRef { id: String::new(), name: "Nobody".into() }, ArtistRef { id: "b".into(), name: "B".into() }],
            ..Default::default()
        };
        let m = song_menu(s, true, SongDownload::Pending, true);
        assert_eq!(
            labels(&m),
            [
                ("Remove from favourites", false), ("Play next", false), ("Add to queue", false), ("Add to playlist…", false), ("Stop download", false),
                ("Go to A", false), ("Go to B", false), ("Add to library (Deezer)", false), ("Sleep timer…", false),
                ("Start radio from this song", true), ("Details", true),
            ]
        );
        let done = song_menu(Song::default(), false, SongDownload::Done, false);
        assert_eq!(done[4].action, SongAction::RemoveDownload);
    }

    #[test]
    fn sleep_offers_off_only_while_running() {
        let idle = sleep_choices(false);
        assert_eq!(idle.iter().map(|c| c.label.as_str()).collect::<Vec<_>>(), ["15 minutes", "30 minutes", "45 minutes", "60 minutes", "End of track", "After 2 songs", "After 3 songs", "After 5 songs", "After 10 songs"]);
        assert_eq!((idle[4].end_of_track, idle[5].songs, idle[1].minutes), (true, 2, 30));
        let running = sleep_choices(true);
        assert_eq!(running[0], SleepChoice { label: "Off".into(), minutes: 0, end_of_track: false, songs: 0 });
    }

    #[test]
    fn a_row_swipe_says_what_letting_go_does() {
        assert_eq!(row_swipe(0, false), None);
        assert_eq!(row_swipe(1, false), Some(RowSwipe { act: RowSwipeAct::Queue, label: "Add to queue".into() }));
        assert_eq!(row_swipe(2, true).unwrap().label, "Play next");
        assert_eq!(row_swipe(3, true), Some(RowSwipe { act: RowSwipeAct::Favourite { on: false }, label: "Remove".into() }));
        assert_eq!(row_swipe(3, false), Some(RowSwipe { act: RowSwipeAct::Favourite { on: true }, label: "Favourite".into() }));
        assert_eq!(row_swipe(4, false).unwrap().act, RowSwipeAct::Download);
    }

    #[test]
    fn a_rows_download_mark() {
        assert_eq!(download_glyph(3, false, false), DownloadGlyph::Done);
        assert_eq!(download_glyph(2, true, false), DownloadGlyph::Failed, "this session's phase wins");
        assert_eq!((download_glyph(0, false, false), download_glyph(1, false, false)), (DownloadGlyph::Ring, DownloadGlyph::Ring));
        assert_eq!(download_glyph(-1, true, true), DownloadGlyph::Done);
        assert_eq!(download_glyph(-1, false, true), DownloadGlyph::Ring);
        assert_eq!(download_glyph(-1, false, false), DownloadGlyph::None);
    }

    #[test]
    fn the_download_entry_says_what_is_left() {
        assert_eq!(download_entry(0, 0), DownloadEntry { label: "Download".into(), act: DownloadAct::All });
        assert_eq!(download_entry(10, 0), DownloadEntry { label: "Remove downloads".into(), act: DownloadAct::Remove });
        assert_eq!(download_entry(10, 10), DownloadEntry { label: "Download".into(), act: DownloadAct::All });
        assert_eq!(download_entry(10, 3), DownloadEntry { label: "Download the other 3".into(), act: DownloadAct::Missing });
    }
}
