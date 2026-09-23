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
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum HomeShelf {
    /// `getAlbumList2` of type `sort`, `size` albums. `follows_stars`: the one shelf that answers a star,
    /// the way the favourites screen does - asked again whenever something is starred, with this
    /// session's marks laid over it so an album that has just lost its heart leaves at once. No other
    /// shelf re-asks on a star, because a star changes nothing in any of them.
    Albums { sort: String, size: u32, follows_stars: bool },
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
    let albums = |sort: &str, follows_stars| HomeShelf::Albums { sort: sort.into(), size: SHELF, follows_stars };
    match row {
        "RECENT" => albums("recent", false),
        "NEWEST" => albums("newest", false),
        "FREQUENT" => albums("frequent", false),
        "RANDOM" => albums("random", false),
        "STARRED" => albums("starred", true),
        "PLAYLISTS" => HomeShelf::Playlists { take: SHELF },
        "TOP_SONGS" => HomeShelf::Songs { sort: "playCount".into(), descending: true, limit: SHELF },
        "PINNED" => HomeShelf::Pinned,
        _ => HomeShelf::Hidden,
    }
}

/// The shelves of the rows the user kept (by name: RECENT, NEWEST, FREQUENT, RANDOM, STARRED, PLAYLISTS,
/// TOP_SONGS, PINNED), one per row and in the same order. Only these are requested at all; a hidden
/// shelf costs no request.
#[uniffi::export]
pub fn home_shelves(rows: Vec<String>) -> Vec<HomeShelf> {
    rows.iter().map(|r| shelf(r)).collect()
}

/// The pinned playlists, in the order the server lists them.
#[uniffi::export]
pub fn home_pinned(playlists: Vec<Playlist>, pins: Vec<String>) -> Vec<Playlist> {
    playlists.into_iter().filter(|p| pins.contains(&p.id)).collect()
}

/// The home rows (by name) with the one at `from` moved to `to`; unchanged when either is not a row.
#[uniffi::export]
pub fn home_rows_moved(mut rows: Vec<String>, from: u32, to: u32) -> Vec<String> {
    let (from, to) = (from as usize, to as usize);
    if from < rows.len() && to < rows.len() {
        let row = rows.remove(from);
        rows.insert(to, row);
    }
    rows
}

/// The stored answers a manual refresh throws away first, so asking again really reaches the server
/// rather than being told the two-minute-old copy is still fresh.
#[uniffi::export]
pub fn home_refresh_drops() -> Vec<String> {
    ["getAlbumList2", "getPlaylists", "getStarred2"].map(String::from).to_vec()
}

// ---- long lists ---------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct Paging {
    pub albums: u32,
    pub songs: u32,
    pub history: u32,
}

/// Page sizes of the long lists. A page shorter than its size is the last one.
#[uniffi::export]
pub fn browse_paging() -> Paging {
    Paging { albums: ALBUM_PAGE, songs: SONG_PAGE, history: HISTORY_PAGE }
}

/// One way to order the "all songs" list: `name` is what the app stores and passes back, `key` the song
/// field it sorts on.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
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

#[uniffi::export]
pub fn song_sorts() -> Vec<SongSortOption> {
    SONG_SORTS.iter().map(|(name, key, label, descending)| SongSortOption { name: (*name).into(), key: (*key).into(), label: (*label).into(), descending: *descending }).collect()
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct SongsPage {
    pub songs: Vec<Song>,
    /// True when this was the last page.
    pub exhausted: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct HistoryPage {
    pub entries: Vec<HistoryEntry>,
    pub exhausted: bool,
}

#[uniffi::export]
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
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::song;

    #[test]
    fn shelves_follow_the_rows() {
        let s = home_shelves(["PINNED", "STARRED", "RECENT", "TOP_SONGS", "PLAYLISTS", "NOPE"].map(String::from).to_vec());
        assert_eq!(s[0], HomeShelf::Pinned);
        assert_eq!(s[1], HomeShelf::Albums { sort: "starred".into(), size: 20, follows_stars: true });
        assert_eq!(s[2], HomeShelf::Albums { sort: "recent".into(), size: 20, follows_stars: false });
        assert_eq!(s[3], HomeShelf::Songs { sort: "playCount".into(), descending: true, limit: 20 });
        assert_eq!(s[4], HomeShelf::Playlists { take: 20 });
        assert_eq!(s[5], HomeShelf::Hidden);
        let p = |id: &str| Playlist { id: id.into(), ..Default::default() };
        let pinned = home_pinned(vec![p("a"), p("b"), p("c")], vec!["c".into(), "a".into(), "z".into()]);
        assert_eq!(pinned.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["a", "c"]);
        assert_eq!(home_refresh_drops(), ["getAlbumList2", "getPlaylists", "getStarred2"]);
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
