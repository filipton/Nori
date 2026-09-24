//! The browsing screens' rules: what each home shelf is, how the long lists page, and the listening
//! history's pages and date windows. The requests themselves stay with the app where they need the
//! network; everything the offline index can answer is answered here in one call.

use nori_model::{HistoryEntry, ListeningStats, Playlist, Song};

pub const DAY_MS: i64 = 86_400_000;

/// Albums per page of the album grid (a server request each).
const ALBUM_PAGE: u32 = 60;
/// Songs per page of the "all songs" list (the offline index).
pub const SONG_PAGE: u32 = 200;
/// Listens per page of the history.
pub const HISTORY_PAGE: u32 = 100;
/// Albums on an album shelf of the home page, and how many playlists its playlist shelf shows.
const SHELF: u32 = 20;
/// How many entries each top list of the listening stats holds.
pub const STATS_TOP: u32 = 10;

// ---- the home page ----------------------------------------------------------

/// What one row of the home page shows and where it comes from.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum HomeShelf {
    /// `getAlbumList2` of type `sort`, `size` albums. `follows_stars`: the one shelf that answers a star,
    /// the way the favourites screen does - asked again whenever something is starred, with this
    /// session's marks laid over it so an album that has just lost its heart leaves at once. No other
    /// shelf re-asks on a star, because a star changes nothing in any of them.
    Albums { sort: AlbumSort, size: u32, follows_stars: bool },
    /// Every playlist there is, newest first, the first `take` of them, as against the handful the user
    /// pinned. Somebody who keeps six playlists does not want to choose which of them is worth pinning.
    Playlists { take: u32 },
    /// Straight out of the offline index, so it costs no request at all: a page of [Core::browse_songs].
    Songs { sort: String, descending: bool, limit: u32 },
    /// The pinned playlists. They are fetched once for the whole page, because the row is a selection of
    /// something the page already has to hold (see [home_pinned]).
    Pinned,
    /// A row this core does not know; it shows nothing.
    Hidden,
}

fn shelf(row: &str) -> HomeShelf {
    let albums = |sort, follows_stars| HomeShelf::Albums { sort, size: SHELF, follows_stars };
    match row {
        "RECENT" => albums(AlbumSort::Recent, false),
        "NEWEST" => albums(AlbumSort::Newest, false),
        "FREQUENT" => albums(AlbumSort::Frequent, false),
        "RANDOM" => albums(AlbumSort::Random, false),
        "STARRED" => albums(AlbumSort::Starred, true),
        "PLAYLISTS" => HomeShelf::Playlists { take: SHELF },
        "TOP_SONGS" => HomeShelf::Songs { sort: "playCount".into(), descending: true, limit: SHELF },
        "PINNED" => HomeShelf::Pinned,
        _ => HomeShelf::Hidden,
    }
}

/// The shelves of the rows the user kept (by name: RECENT, NEWEST, FREQUENT, RANDOM, STARRED, PLAYLISTS,
/// TOP_SONGS, PINNED), one per row and in the same order. Only these are requested at all; a hidden
/// shelf costs no request.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn home_shelves(rows: Vec<String>) -> Vec<HomeShelf> {
    rows.iter().map(|r| shelf(r)).collect()
}

/// The pinned playlists, in the order the server lists them.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn home_pinned(playlists: Vec<Playlist>, pins: Vec<String>) -> Vec<Playlist> {
    playlists.into_iter().filter(|p| pins.contains(&p.id)).collect()
}

/// The home rows (by name) with the one at `from` moved to `to`; unchanged when either is not a row.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn home_rows_moved(mut rows: Vec<String>, from: u32, to: u32) -> Vec<String> {
    let (from, to) = (from as usize, to as usize);
    if from < rows.len() && to < rows.len() {
        let row = rows.remove(from);
        rows.insert(to, row);
    }
    rows
}

/// The home rows (by name) with `row` switched on or off. A row switched off leaves the order; one
/// switched back on comes back at the end of the page, where it can be seen, and can be carried up from
/// there.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn home_rows_toggled(mut rows: Vec<String>, row: String, on: bool) -> Vec<String> {
    rows.retain(|r| *r != row);
    if on {
        rows.push(row);
    }
    rows
}

/// The rows of `all` (every row there is, in its own order) that are not shown.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn home_rows_hidden(all: Vec<String>, shown: Vec<String>) -> Vec<String> {
    all.into_iter().filter(|r| !shown.contains(r)).collect()
}

/// The favourite playlists with `id` made one (`on`) or not. A playlist is a favourite on this phone: the
/// server has no way to star one.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn pins_toggled(mut pins: Vec<String>, id: String, on: bool) -> Vec<String> {
    pins.retain(|p| *p != id);
    if on {
        pins.push(id);
    }
    pins
}

/// The stored answers a manual refresh throws away first, so asking again really reaches the server
/// rather than being told the two-minute-old copy is still fresh.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn home_refresh_drops() -> Vec<String> {
    ["getAlbumList2", "getPlaylists", "getStarred2"].map(String::from).to_vec()
}

// ---- long lists ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Paging {
    pub albums: u32,
    pub songs: u32,
    pub history: u32,
}

/// A page of the album grid that came back shorter than asked for is the last one.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn albums_exhausted(got: u32) -> bool {
    got < ALBUM_PAGE
}

// ---- album orders -------------------------------------------------------------

/// The orders `getAlbumList2` knows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum AlbumSort {
    Newest,
    Recent,
    Frequent,
    Random,
    ByName,
    ByArtist,
    Starred,
    ByGenre,
    ByYear,
}

impl AlbumSort {
    const ALL: [AlbumSort; 9] = [
        AlbumSort::Newest, AlbumSort::Recent, AlbumSort::Frequent, AlbumSort::Random, AlbumSort::ByName, AlbumSort::ByArtist,
        AlbumSort::Starred, AlbumSort::ByGenre, AlbumSort::ByYear,
    ];

    /// The `type` the server is asked for.
    pub fn api(self) -> &'static str {
        match self {
            AlbumSort::Newest => "newest",
            AlbumSort::Recent => "recent",
            AlbumSort::Frequent => "frequent",
            AlbumSort::Random => "random",
            AlbumSort::ByName => "alphabeticalByName",
            AlbumSort::ByArtist => "alphabeticalByArtist",
            AlbumSort::Starred => "starred",
            AlbumSort::ByGenre => "byGenre",
            AlbumSort::ByYear => "byYear",
        }
    }
}

/// One entry of the album grid's sort menu.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct AlbumSortOption {
    pub sort: AlbumSort,
    pub label: String,
}

/// The album grid's sort menu, in its order.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn album_sorts() -> Vec<AlbumSortOption> {
    [
        (AlbumSort::ByName, "A–Z"),
        (AlbumSort::ByArtist, "Artist"),
        (AlbumSort::Newest, "Added"),
        (AlbumSort::Recent, "Played"),
        (AlbumSort::Frequent, "Most played"),
        (AlbumSort::Starred, "Favourites"),
        (AlbumSort::ByYear, "Year"),
        (AlbumSort::Random, "Random"),
    ]
    .map(|(sort, label)| AlbumSortOption { sort, label: label.into() })
    .to_vec()
}

/// The `type` the server is asked for.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn album_sort_api(sort: AlbumSort) -> String {
    sort.api().into()
}

/// One entry of the list settings (`listPrefs`): which order a long list was left in.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ListPref {
    pub key: String,
    pub value: String,
}

const ALBUMS_SORT_KEY: &str = "albums.sort";
const SONGS_SORT_KEY: &str = "songs.sort";

/// The album grid's order as it was left (A–Z the first time), kept under the server's name for it.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn album_sort_saved(prefs: std::collections::HashMap<String, String>) -> AlbumSort {
    prefs.get(ALBUMS_SORT_KEY).and_then(|n| AlbumSort::ALL.into_iter().find(|s| s.api() == n)).unwrap_or(AlbumSort::ByName)
}

/// The list setting that remembers the album grid's order.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn album_sort_kept(sort: AlbumSort) -> ListPref {
    ListPref { key: ALBUMS_SORT_KEY.into(), value: sort.api().into() }
}

/// The songs list's order as it was left (by title the first time), by [`song_sorts`] name.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn song_sort_saved(prefs: std::collections::HashMap<String, String>) -> String {
    prefs.get(SONGS_SORT_KEY).filter(|n| SONG_SORTS.iter().any(|s| s.0 == n.as_str())).cloned().unwrap_or_else(|| SONG_SORTS[0].0.into())
}

/// The list setting that remembers the songs list's order.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn song_sort_kept(name: String) -> ListPref {
    ListPref { key: SONGS_SORT_KEY.into(), value: name }
}

// ---- the library ----------------------------------------------------------------

/// The library's sections, in the order their pills run.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn library_sections() -> Vec<String> {
    ["Albums", "Favourites", "Artists", "Songs", "Playlists", "Smart", "History", "Genres", "Decades", "Folders", "Radio", "Downloads"].map(String::from).to_vec()
}

/// A decade's years, first and last, from its first year.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct YearSpan {
    pub from: u32,
    pub to: u32,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn decade_years(start_year: u32) -> YearSpan {
    YearSpan { from: start_year, to: start_year + 9 }
}

/// Whether a new radio station can be added: it has a name and its stream is a web address.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn radio_can_add(name: String, url: String) -> bool {
    !name.trim().is_empty() && url.starts_with("http")
}

/// How many songs the library's own reads ask for at a time.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct LibrarySizes {
    /// A search on the server: songs, albums and artists. One big page: octo-fiesta repeats its
    /// provider results on every offset.
    pub search_songs: i32,
    pub search_albums: i32,
    pub search_artists: i32,
    /// The offline index's answer to a keystroke.
    pub local_search: u32,
    /// Songs a smart playlist is evaluated to.
    pub smart_songs: u32,
    /// The server's random songs, and a genre's songs.
    pub random_songs: i32,
    pub genre_songs: i32,
    /// Songs per request while the index is synced.
    pub sync_page: u32,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn library_sizes() -> LibrarySizes {
    LibrarySizes { search_songs: 40, search_albums: 20, search_artists: 10, local_search: 30, smart_songs: 500, random_songs: 100, genre_songs: 200, sync_page: 500 }
}

/// Page sizes of the long lists. A page shorter than its size is the last one.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn browse_paging() -> Paging {
    Paging { albums: ALBUM_PAGE, songs: SONG_PAGE, history: HISTORY_PAGE }
}

/// One way to order the "all songs" list: `name` is what the app stores and passes back, `key` the song
/// field it sorts on.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SongSortOption {
    pub name: String,
    pub key: String,
    pub label: String,
    pub descending: bool,
}

/// Newest, longest and most played read from the top, the rest from A.
pub const SONG_SORTS: [(&str, &str, &str, bool); 7] = [
    ("TITLE", "title", "Title", false),
    ("ARTIST", "artist", "Artist", false),
    ("ALBUM", "album", "Album", false),
    ("YEAR", "year", "Year", true),
    ("ADDED", "created", "Added", true),
    ("PLAYS", "playCount", "Most played", true),
    ("LONGEST", "duration", "Longest", true),
];

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn song_sorts() -> Vec<SongSortOption> {
    SONG_SORTS.iter().map(|(name, key, label, descending)| SongSortOption { name: (*name).into(), key: (*key).into(), label: (*label).into(), descending: *descending }).collect()
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SongsPage {
    pub songs: Vec<Song>,
    /// True when this was the last page.
    pub exhausted: bool,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntry>,
    pub exhausted: bool,
}

/// The listening page: the stats, and what it says about them.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StatsPage {
    pub stats: ListeningStats,
    pub words: nori_words::fmt::StatsWords,
    pub tiles: Vec<nori_words::fmt::StatTileWords>,
}

/// A decade that has songs in the index: its first year, its name ("1990s") and how many songs.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Decade {
    pub start: u32,
    pub name: String,
    pub song_count: u32,
}

#[cfg(test)]
mod tests {
    use super::*;
    

    #[test]
    fn shelves_follow_the_rows() {
        let s = home_shelves(["PINNED", "STARRED", "RECENT", "TOP_SONGS", "PLAYLISTS", "NOPE"].map(String::from).to_vec());
        assert_eq!(s[0], HomeShelf::Pinned);
        assert_eq!(s[1], HomeShelf::Albums { sort: AlbumSort::Starred, size: 20, follows_stars: true });
        assert_eq!(s[2], HomeShelf::Albums { sort: AlbumSort::Recent, size: 20, follows_stars: false });
        assert_eq!(s[3], HomeShelf::Songs { sort: "playCount".into(), descending: true, limit: 20 });
        assert_eq!(s[4], HomeShelf::Playlists { take: 20 });
        assert_eq!(s[5], HomeShelf::Hidden);
        let p = |id: &str| Playlist { id: id.into(), ..Default::default() };
        let pinned = home_pinned(vec![p("a"), p("b"), p("c")], vec!["c".into(), "a".into(), "z".into()]);
        assert_eq!(pinned.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(home_refresh_drops(), ["getAlbumList2", "getPlaylists", "getStarred2"]);
    }

    #[test]
    fn rows_and_pins_switch_on_at_the_end() {
        let rows = ["A", "B", "C"].map(String::from).to_vec();
        assert_eq!(home_rows_toggled(rows.clone(), "B".into(), false), ["A", "C"]);
        assert_eq!(home_rows_toggled(["A", "C"].map(String::from).to_vec(), "B".into(), true), ["A", "C", "B"]);
        assert_eq!(home_rows_toggled(rows.clone(), "A".into(), true), ["B", "C", "A"], "never twice");
        assert_eq!(home_rows_hidden(["A", "B", "C", "D"].map(String::from).to_vec(), vec!["C".into(), "A".into()]), ["B", "D"]);
        assert_eq!(pins_toggled(vec!["p".into()], "q".into(), true), ["p", "q"]);
        assert_eq!(pins_toggled(vec!["p".into(), "q".into()], "p".into(), false), ["q"]);
    }

    #[test]
    fn album_orders_are_kept_by_their_server_names() {
        use std::collections::HashMap;
        let labels: Vec<(AlbumSort, String)> = album_sorts().into_iter().map(|o| (o.sort, o.label)).collect();
        assert_eq!(labels[0], (AlbumSort::ByName, "A–Z".to_string()));
        assert_eq!(labels.len(), 8);
        assert_eq!(album_sort_api(AlbumSort::ByArtist), "alphabeticalByArtist");
        assert_eq!(album_sort_saved(HashMap::new()), AlbumSort::ByName);
        let kept = album_sort_kept(AlbumSort::Frequent);
        assert_eq!((kept.key.as_str(), kept.value.as_str()), ("albums.sort", "frequent"));
        assert_eq!(album_sort_saved(HashMap::from([(kept.key, kept.value)])), AlbumSort::Frequent);
        assert_eq!(album_sort_saved(HashMap::from([("albums.sort".to_string(), "byYear".to_string())])), AlbumSort::ByYear);
        assert_eq!(song_sort_saved(HashMap::new()), "TITLE");
        assert_eq!(song_sort_saved(HashMap::from([("songs.sort".to_string(), "PLAYS".to_string())])), "PLAYS");
        assert_eq!(song_sort_saved(HashMap::from([("songs.sort".to_string(), "GONE".to_string())])), "TITLE");
        assert_eq!(song_sort_kept("YEAR".into()), ListPref { key: "songs.sort".into(), value: "YEAR".into() });
        assert!(albums_exhausted(59) && !albums_exhausted(60));
    }

    #[test]
    fn library_rules() {
        assert_eq!(library_sections()[0], "Albums");
        assert_eq!(library_sections().len(), 12);
        assert_eq!(decade_years(1990), YearSpan { from: 1990, to: 1999 });
        assert!(radio_can_add("FIP".into(), "https://x".into()));
        assert!(!radio_can_add(" ".into(), "https://x".into()));
        assert!(!radio_can_add("FIP".into(), "ftp://x".into()));
        let z = library_sizes();
        assert_eq!((z.search_songs, z.search_albums, z.search_artists, z.random_songs, z.genre_songs), (40, 20, 10, 100, 200));
    }

    #[test]
    fn moving_a_home_row() {
        let rows = ["A", "B", "C"].map(String::from).to_vec();
        assert_eq!(home_rows_moved(rows.clone(), 0, 2), ["B", "C", "A"]);
        assert_eq!(home_rows_moved(rows.clone(), 2, 0), ["C", "A", "B"]);
        assert_eq!(home_rows_moved(rows.clone(), 1, 3), rows);
    }
}
