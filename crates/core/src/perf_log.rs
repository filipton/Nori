//! What the perf build measured, stretch by stretch: one row per stretch of the app's life (screen off
//! and playing, the player open, charging, ...), kept in the app's database for a couple of weeks so
//! a battery test of several days can be read back and shared. The platform measures and describes a
//! stretch; this only keeps it, as the JSON the platform wrote, in the order the stretches ended.
//!
//! Only the perf build calls these. The table is made by the first row, so the database of every
//! other build never has it.

use rusqlite::{params, Connection};

use crate::settings_store;

/// How long a stretch is kept after it ended.
pub const KEEP_MS: i64 = 14 * 24 * 3600 * 1000;

fn table(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS perf_stretches(ended_ms INTEGER NOT NULL, row TEXT NOT NULL)")
}

fn add(c: &Connection, ended_ms: i64, row: &str) -> rusqlite::Result<()> {
    table(c)?;
    c.execute("INSERT INTO perf_stretches(ended_ms, row) VALUES(?1, ?2)", params![ended_ms, row])?;
    c.execute("DELETE FROM perf_stretches WHERE ended_ms < ?1", [ended_ms - KEEP_MS])?;
    Ok(())
}

fn rows(c: &Connection, since_ms: i64) -> rusqlite::Result<Vec<String>> {
    table(c)?;
    let mut st = c.prepare("SELECT row FROM perf_stretches WHERE ended_ms >= ?1 ORDER BY ended_ms, rowid")?;
    let out = st.query_map([since_ms], |r| r.get(0))?.collect();
    out
}

/// A stretch that ended at `ended_ms` (wall clock), described by the platform in `row`. Stretches older
/// than [`KEEP_MS`] go at the same time. Written on the calling thread; nothing before the settings are open.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_log_add(ended_ms: i64, row: String) {
    let Some(db) = settings_store::app_db() else { return };
    let written = add(&db.lock(), ended_ms, &row);
    if let Err(e) = written {
        crate::alog::info(&format!("perf log: could not write: {e}"));
    }
}

/// The stretches that ended at or after `since_ms`, oldest first.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_log_rows(since_ms: i64) -> Vec<String> {
    let Some(db) = settings_store::app_db() else { return Vec::new() };
    let read = rows(&db.lock(), since_ms);
    read.unwrap_or_default()
}

/// Every stretch forgotten: "Start fresh".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_log_clear() {
    let Some(db) = settings_store::app_db() else { return };
    let c = db.lock();
    if table(&c).is_ok() {
        let _ = c.execute("DELETE FROM perf_stretches", []);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_rows_in_order_and_forgets_old_ones() {
        let c = Connection::open_in_memory().unwrap();
        add(&c, 1_000, "a").unwrap();
        add(&c, 2_000, "b").unwrap();
        assert_eq!(rows(&c, 0).unwrap(), vec!["a", "b"]);
        assert_eq!(rows(&c, 1_500).unwrap(), vec!["b"]);
        // A row two weeks and a bit later takes the first two with it.
        add(&c, 2_001 + KEEP_MS, "c").unwrap();
        assert_eq!(rows(&c, 0).unwrap(), vec!["c"]);
    }

    #[test]
    fn reads_nothing_from_a_database_without_the_table() {
        let c = Connection::open_in_memory().unwrap();
        assert!(rows(&c, 0).unwrap().is_empty());
    }
}
