//! The browsing screens' rules: what each home shelf is, how the long lists page, and the listening
//! history's pages and date windows. The requests themselves stay with the app where they need the
//! network; everything the offline index can answer is answered here in one call.

use crate::{db, history, Core, HistoryEntry, ListeningStats, Playlist, Result, Song};

const DAY_MS: i64 = 86_400_000;

/// Albums per page of the album grid (a server request each).
const ALBUM_PAGE: u32 = 60;
/// Songs per page of the "all songs" list (the offline index).
const SONG_PAGE: u32 = 200;
/// Listens per page of the history.
const HISTORY_PAGE: u32 = 100;
/// Albums on an album shelf of the home page, and how many playlists its playlist shelf shows.
const SHELF: u32 = 20;
/// How many entries each top list of the listening stats holds.
const STATS_TOP: u32 = 10;

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

    /// The name it is kept under in the list settings; these were the Kotlin enum's names, and settings
    /// saved by earlier versions still say them.
    fn kept_as(self) -> &'static str {
        match self {
            AlbumSort::Newest => "NEWEST",
            AlbumSort::Recent => "RECENT",
            AlbumSort::Frequent => "FREQUENT",
            AlbumSort::Random => "RANDOM",
            AlbumSort::ByName => "BY_NAME",
            AlbumSort::ByArtist => "BY_ARTIST",
            AlbumSort::Starred => "STARRED",
            AlbumSort::ByGenre => "BY_GENRE",
            AlbumSort::ByYear => "BY_YEAR",
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

/// The album grid's order as it was left (A–Z the first time).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn album_sort_saved(prefs: std::collections::HashMap<String, String>) -> AlbumSort {
    prefs.get(ALBUMS_SORT_KEY).and_then(|n| AlbumSort::ALL.into_iter().find(|s| s.kept_as() == n)).unwrap_or(AlbumSort::ByName)
}

/// The list setting that remembers the album grid's order.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn album_sort_kept(sort: AlbumSort) -> ListPref {
    ListPref { key: ALBUMS_SORT_KEY.into(), value: sort.kept_as().into() }
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
const SONG_SORTS: [(&str, &str, &str, bool); 7] = [
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

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    /// The page of the "all songs" list at `offset`: sorted by the [song_sorts] entry called `sort` (an
    /// unknown name keeps index order), only starred songs when `starred_only`, only the years
    /// `year_from..=year_to` when `year_to` is not 0. Nothing here touches the network.
    pub fn songs_page(&self, sort: String, starred_only: bool, year_from: u32, year_to: u32, offset: u32) -> Result<SongsPage> {
        let (key, descending) = SONG_SORTS.iter().find(|s| s.0 == sort).map_or(("", false), |s| (s.1, s.3));
        let songs = self.browse_songs(key.into(), descending, starred_only, year_from, year_to, offset, SONG_PAGE)?;
        Ok(SongsPage { exhausted: (songs.len() as u32) < SONG_PAGE, songs })
    }

    /// The listening history's page at `offset`, newest first, skips left out.
    pub fn history_page(&self, offset: u32) -> Result<HistoryPage> {
        let entries = self.history_recent(HISTORY_PAGE, offset, false)?;
        Ok(HistoryPage { exhausted: (entries.len() as u32) < HISTORY_PAGE, entries })
    }

    /// The listening stats of the last `days` days up to now; 0 means everything.
    pub fn stats_days(&self, days: u32) -> Result<ListeningStats> {
        let now = db::now_ms();
        let from = if days == 0 { 0 } else { now - days as i64 * DAY_MS };
        Ok(history::summary(&self.db.lock(), from, now, STATS_TOP)?)
    }

    /// [`Core::stats_days`] with its words and tiles, as the listening page shows them.
    pub fn stats_page(&self, days: u32) -> Result<StatsPage> {
        let stats = self.stats_days(days)?;
        Ok(StatsPage { words: crate::fmt::stats_words(&stats), tiles: crate::fmt::stats_tiles(&stats), stats })
    }

    /// Decades that have songs in the index, newest first, with how many: what "browse by decade" lists.
    pub fn browse_decades(&self) -> Result<Vec<Decade>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT (json_extract(json, '$.year') / 10) * 10 AS d, count(*) FROM items WHERE server=sid() AND kind=?1 AND json_extract(json, '$.year') > 0 GROUP BY d ORDER BY d DESC")?;
        let rows = st.query_map([db::SONG], |r| {
            let start: u32 = r.get(0)?;
            Ok(Decade { start, name: crate::words::words_decade(start), song_count: r.get(1)? })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

/// The listening page: the stats, and what it says about them.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StatsPage {
    pub stats: ListeningStats,
    pub words: crate::fmt::StatsWords,
    pub tiles: Vec<crate::fmt::StatTileWords>,
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
    use crate::history::tests::song;

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
    fn album_orders_are_kept_by_their_old_names() {
        use std::collections::HashMap;
        let labels: Vec<(AlbumSort, String)> = album_sorts().into_iter().map(|o| (o.sort, o.label)).collect();
        assert_eq!(labels[0], (AlbumSort::ByName, "A–Z".to_string()));
        assert_eq!(labels.len(), 8);
        assert_eq!(album_sort_api(AlbumSort::ByArtist), "alphabeticalByArtist");
        assert_eq!(album_sort_saved(HashMap::new()), AlbumSort::ByName);
        let kept = album_sort_kept(AlbumSort::Frequent);
        assert_eq!((kept.key.as_str(), kept.value.as_str()), ("albums.sort", "FREQUENT"));
        assert_eq!(album_sort_saved(HashMap::from([(kept.key, kept.value)])), AlbumSort::Frequent);
        assert_eq!(album_sort_saved(HashMap::from([("albums.sort".to_string(), "BY_YEAR".to_string())])), AlbumSort::ByYear);
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

    #[test]
    fn songs_page_sorts_and_knows_the_end() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let all: Vec<Song> = (0..250).map(|i| song(&format!("s{i:03}"), &format!("T{:03}", 249 - i), "A", "B", "", 1990 + (i % 20) as u32)).collect();
        db::index(&mut core.db.lock(), &[], &[], &all).unwrap();
        let first = core.songs_page("TITLE".into(), false, 0, 0, 0).unwrap();
        assert_eq!((first.songs.len(), first.exhausted), (200, false));
        assert_eq!(first.songs[0].title, "T000");
        let last = core.songs_page("TITLE".into(), false, 0, 0, 200).unwrap();
        assert_eq!((last.songs.len(), last.exhausted), (50, true));
        let years = core.songs_page("YEAR".into(), false, 2000, 2009, 0).unwrap();
        assert_eq!(years.songs[0].year, 2009);
        assert!(years.exhausted && years.songs.iter().all(|s| (2000..=2009).contains(&s.year)));
        assert_eq!(song_sorts().iter().map(|s| s.name.as_str()).collect::<Vec<_>>(), ["TITLE", "ARTIST", "ALBUM", "YEAR", "ADDED", "PLAYS", "LONGEST"]);
    }

    #[test]
    fn history_pages_and_stats_windows() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let s = song("1", "t", "a", "b", "", 0);
        db::index(&mut core.db.lock(), &[], &[], std::slice::from_ref(&s)).unwrap();
        let now = db::now_ms();
        crate::history::record(&mut core.db.lock(), &s, now - 40 * DAY_MS, 200_000, 0, now).unwrap();
        crate::history::record(&mut core.db.lock(), &s, now - DAY_MS, 200_000, 0, now).unwrap();
        let page = core.history_page(0).unwrap();
        assert_eq!((page.entries.len(), page.exhausted), (2, true));
        assert_eq!(core.stats_days(7).unwrap().plays, 1);
        assert_eq!(core.stats_days(0).unwrap().plays, 2);
    }
}
