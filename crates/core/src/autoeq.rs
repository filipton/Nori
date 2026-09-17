//! The AutoEQ headphone database, kept on the device. Its `results/INDEX.md` is one 850 KB request
//! listing every measurement; it is parsed here into a table so searching 8000+ headphones is a local
//! FTS query rather than a network call per keystroke. Only the index and the chosen preset are ever
//! fetched, and only when the user asks for them.

use rusqlite::{params, Connection};

use crate::model::AutoEqEntry;

pub const INDEX_URL: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/INDEX.md";
const RAW: &str = "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results";

/// `- [Name](./source/form/Name) by source on target`
fn entry(line: &str) -> Option<AutoEqEntry> {
    let rest = line.strip_prefix("- [")?;
    let (name, rest) = rest.split_once("](")?;
    let (path, tail) = rest.split_once(')')?;
    let path = path.trim_start_matches("./");
    let mut parts = path.split('/');
    // The path is percent-encoded; the labels a person reads are not.
    let source = decode(parts.next()?);
    let form = decode(parts.next().unwrap_or_default());
    let target = tail.split_once(" on ").map(|(_, t)| t.trim().to_string()).unwrap_or_default();
    Some(AutoEqEntry { name: name.trim().to_string(), source, form, target, path: path.to_string() })
}

/// Where the parametric preset of an entry lives. The file repeats the folder name, percent-encoded.
pub fn preset_url(e: &AutoEqEntry) -> String {
    let leaf = e.path.rsplit('/').next().unwrap_or_default();
    format!("{RAW}/{}/{} ParametricEQ.txt", e.path, decode(leaf)).replace(' ', "%20")
}

/// The index stores paths already percent-encoded; the file name inside needs the decoded form.
fn decode(s: &str) -> String {
    if !s.contains('%') {
        return s.to_string();
    }
    let b = s.as_bytes();
    // Decoded bytes are collected so multi-byte UTF-8 (e.g. %C3%A9) survives.
    let mut bytes: Vec<u8> = Vec::with_capacity(s.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                bytes.push(v);
                i += 3;
                continue;
            }
        }
        bytes.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

pub fn store(c: &mut Connection, markdown: &str) -> rusqlite::Result<u32> {
    let tx = c.transaction()?;
    tx.execute("DELETE FROM autoeq", [])?;
    let mut n = 0;
    {
        let mut st = tx.prepare("INSERT INTO autoeq(name, source, form, target, path) VALUES(?1, ?2, ?3, ?4, ?5)")?;
        for e in markdown.lines().filter_map(entry) {
            st.execute(params![e.name, e.source, e.form, e.target, e.path])?;
            n += 1;
        }
    }
    tx.commit()?;
    Ok(n)
}

pub fn search(c: &Connection, query: &str, limit: u32) -> rusqlite::Result<Vec<AutoEqEntry>> {
    let like = format!("%{}%", query.trim().replace(' ', "%"));
    let mut st = c.prepare_cached(
        // Shortest name first, so "HD 600" beats "HD 600 (with pads)" when both match.
        "SELECT name, source, form, target, path FROM autoeq WHERE name LIKE ?1 ESCAPE '\\' ORDER BY length(name), name LIMIT ?2",
    )?;
    let rows = st.query_map(params![like, limit], |r| {
        Ok(AutoEqEntry { name: r.get(0)?, source: r.get(1)?, form: r.get(2)?, target: r.get(3)?, path: r.get(4)? })
    })?;
    rows.collect()
}

pub fn count(c: &Connection) -> rusqlite::Result<u32> {
    c.query_row("SELECT count(*) FROM autoeq", [], |r| r.get(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    const MD: &str = "# Index\nnot an entry\n- [64 Audio U12t](./crinacle/711%20in-ear/64%20Audio%20U12t) by crinacle on 711\n- [Sennheiser HD 600](./oratory1990/over-ear/Sennheiser%20HD%20600) by oratory1990 on Harman over-ear 2018\n- [Sennheiser HD 600 balanced](./Filk/over-ear/Sennheiser%20HD%20600%20balanced) by Filk\n";

    #[test]
    fn index_parses_and_searches() {
        let mut c = crate::db::open("").unwrap();
        assert_eq!(store(&mut c, MD).unwrap(), 3);
        let hits = search(&c, "hd 600", 10).unwrap();
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].name, "Sennheiser HD 600", "the shorter name ranks first");
        assert_eq!((hits[0].source.as_str(), hits[0].form.as_str(), hits[0].target.as_str()), ("oratory1990", "over-ear", "Harman over-ear 2018"));
        let encoded = entry("- [X](./crinacle/GRAS%2043AG-7%20over-ear/X) by crinacle on GRAS").unwrap();
        assert_eq!(encoded.form, "GRAS 43AG-7 over-ear");
        assert_eq!(search(&c, "u12t", 10).unwrap()[0].source, "crinacle");
        assert!(search(&c, "nothing here", 10).unwrap().is_empty());
        // Storing again replaces rather than duplicates.
        assert_eq!(store(&mut c, MD).unwrap(), 3);
        assert_eq!(count(&c).unwrap(), 3);
    }

    #[test]
    fn preset_url_repeats_the_decoded_leaf() {
        let e = entry("- [64 Audio U12t](./crinacle/711%20in-ear/64%20Audio%20U12t) by crinacle on 711").unwrap();
        assert_eq!(preset_url(&e), "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/crinacle/711%20in-ear/64%20Audio%20U12t/64%20Audio%20U12t%20ParametricEQ.txt");
    }
}
