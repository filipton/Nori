//! The tree a car (Android Auto) or any other remote browser walks: which folders there are, what each
//! holds and how its rows read. The platform turns the rows into its own items and plays what is picked.

use crate::cache_policy::{Page, Read};
use crate::client::Client;
use crate::Song;

/// A folder in the tree: `id` is what is asked for next (see [`Client::browse_children`]).
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
const ART: u32 = 300;

fn folder(id: &str, title: &str) -> BrowseFolder {
    BrowseFolder { id: id.into(), title: title.into(), subtitle: None, art: None }
}

fn root() -> Vec<BrowseFolder> {
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

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Client {
    /// The root folder of the tree.
    pub fn browse_root(&self) -> BrowseFolder {
        folder(ROOT, "nori")
    }

    /// What the folder `parent` holds; nothing for one that is not known or cannot be read now.
    pub async fn browse_children(&self, parent: String) -> BrowsePage {
        let (kind, arg) = parent.split_once(':').unwrap_or((parent.as_str(), ""));
        let art = |id: &Option<String>| id.as_ref().map(|c| self.core.cover_url(c.clone(), ART));
        let songs = |p: Result<Page, _>| match p {
            Ok(Page::Songs { v }) => v,
            _ => Vec::new(),
        };
        let page = match kind {
            ROOT => BrowsePage { folders: root(), songs: Vec::new() },
            "albums" => match self.first(Read::AlbumList { kind: arg.to_lowercase(), size: 40, offset: 0, genre: None }).await {
                Ok(Page::Albums { v }) => BrowsePage {
                    folders: v
                        .iter()
                        .map(|a| BrowseFolder { id: format!("album:{}", a.id), title: a.name.clone(), subtitle: Some(a.artist.clone()), art: art(&a.cover_art) })
                        .collect(),
                    songs: Vec::new(),
                },
                _ => BrowsePage::default(),
            },
            "album" => BrowsePage { folders: Vec::new(), songs: songs(self.read_now(Read::AlbumSongs { id: arg.into() }).await) },
            "playlists" => match self.first(Read::PlaylistList).await {
                Ok(Page::Playlists { v }) => BrowsePage {
                    folders: v
                        .iter()
                        .map(|p| BrowseFolder {
                            id: format!("playlist:{}", p.id),
                            title: p.name.clone(),
                            subtitle: Some(format!("{} songs", p.song_count)),
                            art: art(&p.cover_art),
                        })
                        .collect(),
                    songs: Vec::new(),
                },
                _ => BrowsePage::default(),
            },
            "playlist" => BrowsePage { folders: Vec::new(), songs: songs(self.read_now(Read::PlaylistSongs { id: arg.into() }).await) },
            "starred" => match self.first(Read::StarredItems).await {
                Ok(Page::StarredPage { v }) => BrowsePage { folders: Vec::new(), songs: v.songs },
                _ => BrowsePage::default(),
            },
            "random" => BrowsePage { folders: Vec::new(), songs: songs(self.read_now(Read::RandomSongs { size: 50, genre: None }).await) },
            "downloads" => BrowsePage { folders: Vec::new(), songs: self.core.downloads(true).unwrap_or_default() },
            _ => BrowsePage::default(),
        };
        page
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{block, client};
    use crate::client::NetProfile;

    #[test]
    fn the_root_lists_the_folders_a_car_offers() {
        let (c, fake) = client(NetProfile { url: "h".into(), ..Default::default() });
        let p = block(c.browse_children(ROOT.into()));
        assert_eq!(p.folders.iter().map(|f| f.id.as_str()).collect::<Vec<_>>(), ["albums:recent", "albums:newest", "albums:frequent", "playlists", "starred", "random", "downloads"]);
        assert!(fake.asked.lock().is_empty(), "the root asks nothing of the server");
    }

    #[test]
    fn a_playlist_folder_says_how_many_songs() {
        let (c, fake) = client(NetProfile { url: "h".into(), ..Default::default() });
        fake.answer(r#"{"subsonic-response":{"status":"ok","playlists":{"playlist":[{"id":"p1","name":"Evening","songCount":12}]}}}"#);
        let p = block(c.browse_children("playlists".into()));
        assert_eq!(p.folders, vec![BrowseFolder { id: "playlist:p1".into(), title: "Evening".into(), subtitle: Some("12 songs".into()), art: None }]);
        assert!(block(c.browse_children("nonsense".into())).folders.is_empty());
    }
}
