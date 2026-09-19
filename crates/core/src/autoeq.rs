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

/// Letters and digits only, lowercased: "WH-1000XM5", "wh1000xm5" and "WH 1000 XM5" are one name.
fn compact(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Words that say what kind of thing a device is, not which one. A name made only of these ("USB Audio",
/// "USB-C to 3.5mm Headphone Jack Adapter", "Headset") cannot be looked up.
const GENERIC: &[&str] = &[
    "usb", "usbc", "c", "type", "typec", "audio", "device", "dac", "digital", "analog", "analogue", "headset", "headsets",
    "headphone", "headphones", "earphone", "earphones", "earbuds", "speaker", "speakers", "adapter", "adaptor", "jack",
    "to", "35mm", "3", "5mm", "the", "stereo", "wireless", "bluetooth", "le", "bt", "hifi", "hi", "fi", "out", "output",
];

/// The part of a device's own name that can be looked up: "LE_WH-1000XM5" -> "WH-1000XM5", "Filip's
/// AirPods Pro" -> "AirPods Pro", "Galaxy Buds2 Pro (1A2B)" -> "Galaxy Buds2 Pro". None when nothing is
/// left that names a model.
pub fn device_query(device: &str) -> Option<String> {
    let mut name = device.trim().replace('_', " ");
    for prefix in ["LE ", "LE-", "BT ", "BT-"] {
        if name.len() > prefix.len() && name.is_char_boundary(prefix.len()) && name[..prefix.len()].eq_ignore_ascii_case(prefix) {
            name = name[prefix.len()..].to_string();
        }
    }
    // "Filip's AirPods Pro": the owner is not part of the model.
    for mark in ["'s ", "\u{2019}s "] {
        if let Some(i) = name.find(mark) {
            name = name[i + mark.len()..].to_string();
        }
    }
    // A pairing suffix: "(1A2B)", "[LE]".
    while let Some(open) = name.rfind(['(', '[']) {
        if open == 0 {
            break;
        }
        name.truncate(open);
    }
    let name = name.split_whitespace().collect::<Vec<_>>().join(" ");
    let meaningful = name
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty() && !GENERIC.contains(&w.to_lowercase().as_str()))
        .map(str::len)
        .sum::<usize>();
    (meaningful >= 3 && compact(&name).len() >= 3).then_some(name)
}

/// Measurements from these rigs are the ones AutoEQ itself recommends first.
fn source_rank(source: &str) -> u8 {
    match source {
        "oratory1990" => 0,
        "crinacle" => 1,
        "Rtings" => 2,
        _ => 3,
    }
}

/// The curves that are this device, best first. Unlike [search] this ignores spacing and punctuation
/// ("WH1000XM5" finds "Sony WH-1000XM5") and prefers the exact model over a longer one that contains it.
/// A short name must be a whole model name (with or without its brand): "Buds" alone matches nothing
/// rather than every Galaxy, Pixel and Nothing earbud.
pub fn matching(c: &Connection, device: &str, limit: u32) -> rusqlite::Result<Vec<AutoEqEntry>> {
    let Some(query) = device_query(device) else { return Ok(Vec::new()) };
    let q = compact(&query);
    let mut st = c.prepare_cached("SELECT name, source, form, target, path FROM autoeq")?;
    let rows = st.query_map([], |r| Ok(AutoEqEntry { name: r.get(0)?, source: r.get(1)?, form: r.get(2)?, target: r.get(3)?, path: r.get(4)? }))?;
    let mut hits: Vec<(u8, usize, u8, AutoEqEntry)> = Vec::new();
    for e in rows {
        let e = e?;
        let name = compact(&e.name);
        let model = e.name.split_once(' ').map(|(_, m)| compact(m)).unwrap_or_default();
        let fit = if name == q || model == q {
            0
        } else if q.len() >= 5 && name.ends_with(&q) {
            1
        } else if q.len() >= 5 && name.contains(&q) {
            2
        } else {
            continue;
        };
        hits.push((fit, name.len(), source_rank(&e.source), e));
    }
    hits.sort_by(|a, b| (a.0, a.1, a.2, &a.3.name).cmp(&(b.0, b.1, b.2, &b.3.name)));
    Ok(hits.into_iter().take(limit as usize).map(|h| h.3).collect())
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
    fn device_names_are_cleaned_before_lookup() {
        assert_eq!(device_query("LE_WH-1000XM5").as_deref(), Some("WH-1000XM5"));
        assert_eq!(device_query("Filip's AirPods Pro").as_deref(), Some("AirPods Pro"));
        assert_eq!(device_query("Filip\u{2019}s AirPods Pro").as_deref(), Some("AirPods Pro"));
        assert_eq!(device_query("Galaxy Buds2 Pro (1A2B)").as_deref(), Some("Galaxy Buds2 Pro"));
        for generic in ["USB Audio", "USB-C to 3.5mm Headphone Jack Adapter", "DAC", "device", "Headset", "BT", ""] {
            assert_eq!(device_query(generic), None, "{generic}");
        }
    }

    #[test]
    fn devices_find_their_curve() {
        let mut c = crate::db::open("").unwrap();
        let md = "- [Sony WH-1000XM5](./Rtings/over-ear/Sony%20WH-1000XM5) by Rtings\n\
- [Sony WH-1000XM5](./oratory1990/over-ear/Sony%20WH-1000XM5) by oratory1990\n\
- [Sony WH-1000XM5 (ANC off)](./crinacle/over-ear/Sony%20WH-1000XM5%20(ANC%20off)) by crinacle\n\
- [Apple AirPods Pro](./crinacle/in-ear/Apple%20AirPods%20Pro) by crinacle\n\
- [Apple AirPods Pro 2](./crinacle/in-ear/Apple%20AirPods%20Pro%202) by crinacle\n\
- [Samsung Galaxy Buds2 Pro](./Rtings/in-ear/Samsung%20Galaxy%20Buds2%20Pro) by Rtings\n\
- [Google Pixel Buds](./Rtings/in-ear/Google%20Pixel%20Buds) by Rtings\n";
        store(&mut c, md).unwrap();
        let hit = |d: &str| matching(&c, d, 5).unwrap().first().map(|e| (e.name.clone(), e.source.clone()));
        assert_eq!(hit("LE_WH1000XM5"), Some(("Sony WH-1000XM5".into(), "oratory1990".into())), "spacing ignored, best rig first");
        assert_eq!(hit("Filip's AirPods Pro"), Some(("Apple AirPods Pro".into(), "crinacle".into())), "the exact model beats the Pro 2");
        assert_eq!(hit("Galaxy Buds2 Pro"), Some(("Samsung Galaxy Buds2 Pro".into(), "Rtings".into())));
        assert_eq!(hit("Buds"), None, "a short name has to be a whole model");
        assert_eq!(hit("USB Audio"), None);
    }

    #[test]
    fn preset_url_repeats_the_decoded_leaf() {
        let e = entry("- [64 Audio U12t](./crinacle/711%20in-ear/64%20Audio%20U12t) by crinacle on 711").unwrap();
        assert_eq!(preset_url(&e), "https://raw.githubusercontent.com/jaakkopasanen/AutoEq/master/results/crinacle/711%20in-ear/64%20Audio%20U12t/64%20Audio%20U12t%20ParametricEQ.txt");
    }
}
