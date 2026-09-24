//! The tree a car (Android Auto) or any other remote browser walks: which folders there are, what each
//! holds and how its rows read. The platform turns the rows into its own items and plays what is picked.

use nori_model::Song;

/// A folder in the tree: `id` is what is asked for next (the client's `browse_children`).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct BrowseFolder {
    pub id: String,
    pub title: String,
    pub subtitle: Option<String>,
    /// Signed artwork url, when the folder has a cover.
    pub art: Option<String>,
}

/// What a folder holds: more folders, or songs to play.
#[derive(Debug, Clone, Default)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct BrowsePage {
    pub folders: Vec<BrowseFolder>,
    pub songs: Vec<Song>,
}

/// The tree's root, as the platform names it.
pub const ROOT: &str = "root";
/// How large a cover a car draws a folder with.
pub const ART: u32 = 300;

/// A folder with no subtitle and no cover.
pub fn folder(id: &str, title: &str) -> BrowseFolder {
    BrowseFolder { id: id.into(), title: title.into(), subtitle: None, art: None }
}

/// The folders at the tree's root, in the order a car lists them.
pub fn root() -> Vec<BrowseFolder> {
    vec![
        folder("albums:recent", "Recently played"),
        folder("albums:newest", "Recently added"),
        folder("albums:frequent", "Most played"),
        folder("playlists", "Playlists"),
        folder("starred", "Favourites"),
        folder("random", "Random"),
        folder("downloads", "Downloads"),
    ]
}

/// The rows of page `page` of `page_size` a browser asks for, out of `len`: what the platform hands back
/// for one request; empty past the end.
///
/// Twin of the paging in `PlaybackService.Callback.onGetChildren` and `onGetSearchResult`
/// (core/.../playback/PlaybackService.kt, `drop(page * pageSize).take(pageSize)`), which Android keeps on
/// its media3 lists.
pub fn page(len: usize, page: u32, page_size: u32) -> std::ops::Range<usize> {
    let from = (page as usize).saturating_mul(page_size as usize).min(len);
    from..from.saturating_add(page_size as usize).min(len)
}
