//! The browsing screens' reads of the core's index and history. The shelves, pages and windows are
//! nori-library's.

use crate::{db, history, Core, ListeningStats, Result};

pub use nori_library::browse::*;

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    /// The page of the "all songs" list at `offset`: sorted by the [song_sorts] entry called `sort` (an
    /// unknown name keeps index order), only starred songs when `starred_only`, only the years
    /// `year_from..=year_to` when `year_to` is not 0. Nothing here touches the network.
    pub fn songs_page(&self, sort: String, starred_only: bool, year_from: u32, year_to: u32, offset: u32) -> Result<SongsPage> {
        let (key, descending) = SONG_SORTS.iter().find(|s| s.0 == sort).map_or(("", false), |s| (s.1, s.2));
        let songs = self.browse_songs(key.into(), descending, starred_only, year_from, year_to, offset, SONG_PAGE)?;
        Ok(SongsPage { exhausted: (songs.len() as u32) < SONG_PAGE, songs })
    }

    /// The listening history's page at `offset`, newest first, skips left out.
    pub fn history_page(&self, offset: u32) -> Result<HistoryPage> {
        let entries = self.history_recent(HISTORY_PAGE, offset, false)?;
        Ok(HistoryPage { exhausted: (entries.len() as u32) < HISTORY_PAGE, entries })
    }

    /// [`Core::stats_days`] with what the listening page reads out of them.
    pub fn stats_page(&self, days: u32) -> Result<StatsPage> {
        Ok(StatsPage::new(self.stats_days(days)?))
    }

    /// Decades that have songs in the index, newest first, with how many: what "browse by decade" lists.
    pub fn browse_decades(&self) -> Result<Vec<Decade>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT (json_extract(json, '$.year') / 10) * 10 AS d, count(*) FROM items WHERE server=sid() AND kind=?1 AND json_extract(json, '$.year') > 0 GROUP BY d ORDER BY d DESC")?;
        let rows = st.query_map([db::SONG], |r| {
            let start: u32 = r.get(0)?;
            Ok(Decade { start, song_count: r.get(1)? })
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }
}

/// Asked only in Rust, so not exported to Kotlin.
impl Core {
    /// The listening stats of the last `days` days up to now; 0 means everything.
    pub fn stats_days(&self, days: u32) -> Result<ListeningStats> {
        let now = db::now_ms();
        let from = if days == 0 { 0 } else { now - days as i64 * DAY_MS };
        Ok(history::summary(&self.db.lock(), from, now, STATS_TOP)?)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::history::tests::song;
    use crate::Song;

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
