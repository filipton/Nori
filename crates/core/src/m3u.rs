//! M3U in and out. Export writes extended M3U8 with paths in the layout most libraries use
//! (`artist/album/NN - title.suffix`), so the file is useful to another player pointed at a copy
//! of the music. Import reads whatever is out there (BOM, CRLF, no `#EXTINF` at all, Windows
//! paths) and resolves entries against the local index, since a Subsonic server has no paths to match.

use rusqlite::{params, Connection};

use crate::{db, model::*, Core, Result};

/// One path component: nothing a file system refuses, never empty.
fn component(s: &str) -> String {
    let clean: String = s.chars().map(|c| if c.is_control() || r#"/\:*?"<>|"#.contains(c) { '_' } else { c }).collect();
    let clean = clean.trim().trim_end_matches('.').trim();
    if clean.is_empty() { "Unknown".into() } else { clean.into() }
}

/// EXTINF is one line.
fn line(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[uniffi::export]
pub fn m3u_export(name: String, songs: Vec<Song>) -> String {
    let mut out = String::from("#EXTM3U\n");
    if !line(&name).is_empty() {
        out.push_str(&format!("#PLAYLIST:{}\n", line(&name)));
    }
    for s in &songs {
        let label = if s.artist.is_empty() { line(&s.title) } else { format!("{} - {}", line(&s.artist), line(&s.title)) };
        let number = if s.track > 0 { format!("{:02} - ", s.track) } else { String::new() };
        let suffix = if s.suffix.is_empty() { String::new() } else { format!(".{}", component(&s.suffix)) };
        out.push_str(&format!("#EXTINF:{},{label}\n", if s.duration > 0 { s.duration as i64 } else { -1 }));
        out.push_str(&format!("{}/{}/{number}{}{suffix}\n", component(&s.artist), component(&s.album), component(&s.title)));
    }
    out
}

/// "Artist - Title" when there is a separator, else a title only.
fn split_label(label: &str) -> (String, String) {
    match label.split_once(" - ") {
        Some((a, t)) if !a.trim().is_empty() && !t.trim().is_empty() => (a.trim().into(), t.trim().into()),
        _ => (String::new(), label.trim().into()),
    }
}

/// What a path says when there was no EXTINF: `artist/album/03 - title.flac`, or just a file name.
fn from_path(path: &str) -> (String, String) {
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty()).collect();
    let file = parts.last().copied().unwrap_or("");
    let stem = match file.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() && ext.len() <= 5 => stem,
        _ => file,
    };
    // "03 - title", "03. title", "1-03 title". A bare "99 Luftballons.mp3" keeps its number: only a file inside
    // a folder, a leading zero or a disc-track pair make "NN title" a track number.
    let rest = stem.trim_start_matches(|c: char| c.is_ascii_digit() || c == '-');
    let number = &stem[..stem.len() - rest.len()];
    let numbered = !number.is_empty() && (rest.starts_with(" - ") || rest.starts_with(['.', '_']) || (rest.starts_with(' ') && (parts.len() > 1 || number.starts_with('0') || number.contains('-'))));
    let title = if numbered { rest.trim_start_matches([' ', '.', '_', '-']) } else { stem };
    let title = if title.is_empty() { stem } else { title };
    let (artist, title) = split_label(title);
    if artist.is_empty() && parts.len() >= 3 && !path.contains("://") {
        return (parts[parts.len() - 3].to_string(), title);
    }
    (artist, title)
}

#[uniffi::export]
pub fn m3u_parse(text: String) -> Vec<M3uEntry> {
    let mut out = Vec::new();
    let mut info: Option<(i32, String)> = None;
    for raw in text.trim_start_matches('\u{feff}').lines() {
        let l = raw.trim();
        if l.is_empty() {
            continue;
        }
        if let Some(rest) = l.strip_prefix("#EXTINF:") {
            // "#EXTINF:123,Artist - Title", the duration may be a float, -1, or carry attributes ("123 tvg-id=..")
            let (head, label) = rest.split_once(',').unwrap_or((rest, ""));
            let secs = head.split_whitespace().next().and_then(|d| d.parse::<f64>().ok()).filter(|d| *d >= 0.0 && d.is_finite());
            info = Some((secs.map(|d| d.round().min(i32::MAX as f64) as i32).unwrap_or(-1), label.trim().to_string()));
        } else if !l.starts_with('#') {
            let (duration_s, label) = info.take().unwrap_or((-1, String::new()));
            let (artist, title) = if label.is_empty() { from_path(l) } else { split_label(&label) };
            out.push(M3uEntry { duration_s, artist, title, path: l.to_string() });
        }
    }
    out
}

fn same(a: &str, b: &str) -> bool {
    a.trim().to_lowercase() == b.trim().to_lowercase()
}

/// Every word must be present, as a whole word: import should not guess.
fn words(text: &str) -> Option<String> {
    let w: Vec<String> = text.split(|c: char| !c.is_alphanumeric()).filter(|t| !t.is_empty()).map(|t| format!("\"{t}\"")).collect();
    (!w.is_empty()).then(|| w.join(" "))
}

fn candidates(c: &Connection, query: Option<String>) -> rusqlite::Result<Vec<Song>> {
    let Some(q) = query else { return Ok(Vec::new()) };
    let mut st = c.prepare_cached("SELECT i.json FROM fts JOIN items i ON i.rowid = fts.rowid WHERE fts MATCH ?1 AND i.server = sid() AND i.kind = ?2 ORDER BY rank LIMIT 25")?;
    let rows = st.query_map(params![q, db::SONG], |r| r.get::<_, String>(0))?;
    Ok(rows.collect::<rusqlite::Result<Vec<_>>>()?.iter().filter_map(|j| serde_json::from_str(j).ok()).collect())
}

/// Among equally good names the one whose length fits is the right master; without a duration, the first.
fn closest(mut l: Vec<Song>, duration_s: i32) -> Option<Song> {
    if duration_s >= 0 {
        l.sort_by_key(|s| (s.duration as i64 - duration_s as i64).abs());
    }
    l.into_iter().next()
}

fn resolve(c: &Connection, e: &M3uEntry) -> rusqlite::Result<Option<Song>> {
    if e.title.trim().is_empty() {
        return Ok(None);
    }
    // The index has no (artist, title) key, but FTS narrows to a handful of rows that an exact comparison can then pick from.
    let found = candidates(c, words(&format!("{} {}", e.title, e.artist)))?;
    let (exact, loose): (Vec<Song>, Vec<Song>) = found.into_iter().partition(|s| same(&s.title, &e.title) && (e.artist.is_empty() || same(&s.artist, &e.artist)));
    if !exact.is_empty() {
        return Ok(closest(exact, e.duration_s));
    }
    // All words of title and artist matched somewhere ("Title (Remastered)", "Artist feat. X"): best rank wins.
    if !e.artist.is_empty() && !loose.is_empty() {
        return Ok(loose.into_iter().next());
    }
    // The artist is spelled differently or missing: only an exact title is trusted.
    let by_title: Vec<Song> = candidates(c, words(&e.title))?.into_iter().filter(|s| same(&s.title, &e.title)).collect();
    Ok(closest(by_title, e.duration_s))
}

#[uniffi::export]
impl Core {
    /// One result per entry, in order; None where the index has nothing that fits. One call for the whole
    /// playlist: exact artist and title first, then the full-text index.
    pub fn m3u_match(&self, entries: Vec<M3uEntry>) -> Result<Vec<Option<Song>>> {
        let c = self.db.lock();
        Ok(entries.iter().map(|e| resolve(&c, e)).collect::<rusqlite::Result<_>>()?)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::song;

    fn entry(duration_s: i32, artist: &str, title: &str) -> M3uEntry {
        M3uEntry { duration_s, artist: artist.into(), title: title.into(), path: String::new() }
    }

    #[test]
    fn export_is_extended_m3u_with_safe_paths() {
        let mut dogs = song("1", "Dogs", "Pink Floyd", "Animals", "", 1977);
        dogs.track = 2;
        dogs.duration = 1024;
        let mut odd = song("2", "What?\n/Why: \"Now\"", "AC/DC", "...", "", 0);
        odd.suffix = String::new();
        odd.duration = 0;
        let bare = Song { id: "3".into(), title: "Jóga".into(), suffix: "mp3".into(), track: 11, ..Default::default() };
        let text = m3u_export("Road\ntrip".into(), vec![dogs, odd, bare]);
        assert_eq!(
            text,
            "#EXTM3U\n#PLAYLIST:Road trip\n\
             #EXTINF:1024,Pink Floyd - Dogs\nPink Floyd/Animals/02 - Dogs.flac\n\
             #EXTINF:-1,AC/DC - What? /Why: \"Now\"\nAC_DC/Unknown/What___Why_ _Now_\n\
             #EXTINF:-1,Jóga\nUnknown/Unknown/11 - Jóga.mp3\n"
        );
        assert_eq!(m3u_export(String::new(), vec![]), "#EXTM3U\n");
    }

    #[test]
    fn export_round_trips_through_parse() {
        let songs = vec![song("1", "Dogs", "Pink Floyd", "Animals", "", 1977), song("2", "Jóga", "Björk", "Homogenic", "", 1997)];
        let parsed = m3u_parse(m3u_export("x".into(), songs.clone()));
        assert_eq!(parsed.len(), 2);
        for (p, s) in parsed.iter().zip(&songs) {
            assert_eq!((p.artist.as_str(), p.title.as_str(), p.duration_s), (s.artist.as_str(), s.title.as_str(), 200));
        }
        assert_eq!(parsed[1].path, "Björk/Homogenic/Jóga.flac");
    }

    #[test]
    fn parse_is_tolerant() {
        let text = "\u{feff}#EXTM3U\r\n# a comment\r\n\r\n#EXTINF:215,Björk - Jóga\r\nC:\\Music\\Björk\\Homogenic\\03 - Jóga.flac\r\n\
                    #EXTINF:-1 tvg-id=\"x\",Just A Title\r\nhttp://host/stream.mp3\r\n\
                    #EXTINF:61.6,A - B - C\r\n#EXTVLCOPT:network-caching=1000\r\nfile.ogg\r\n\
                    #EXTINF:garbage\r\nx.mp3\r\n";
        let l = m3u_parse(text.into());
        assert_eq!(l.len(), 4);
        assert_eq!(l[0], M3uEntry { duration_s: 215, artist: "Björk".into(), title: "Jóga".into(), path: "C:\\Music\\Björk\\Homogenic\\03 - Jóga.flac".into() });
        assert_eq!((l[1].duration_s, l[1].artist.as_str(), l[1].title.as_str()), (-1, "", "Just A Title"));
        assert_eq!((l[2].duration_s, l[2].artist.as_str(), l[2].title.as_str()), (62, "A", "B - C"));
        assert_eq!((l[3].duration_s, l[3].artist.as_str(), l[3].title.as_str()), (-1, "", "x"));
        assert!(m3u_parse(String::new()).is_empty());
        assert!(m3u_parse("#EXTM3U\n#EXTINF:1,dangling".into()).is_empty());
    }

    #[test]
    fn plain_m3u_reads_names_from_paths() {
        let l = m3u_parse("Pink Floyd/Animals/02 - Dogs.flac\n/mnt/music/Miles Davis/Kind of Blue/1-01 So What.mp3\nBjörk - Jóga.mp3\n07. Hunter.ogg\n1999.mp3\n99 Luftballons.mp3\n05 Pigs.mp3\nhttp://radio.example/live\n".into());
        let got: Vec<(&str, &str)> = l.iter().map(|e| (e.artist.as_str(), e.title.as_str())).collect();
        assert_eq!(got, [("Pink Floyd", "Dogs"), ("Miles Davis", "So What"), ("Björk", "Jóga"), ("", "Hunter"), ("", "1999"), ("", "99 Luftballons"), ("", "Pigs"), ("", "live")]);
        assert!(l.iter().all(|e| e.duration_s == -1));
    }

    #[test]
    fn match_prefers_exact_then_falls_back_to_full_text() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let mut live = song("live", "Dogs", "Pink Floyd", "Live", "", 1977);
        live.duration = 900;
        let mut studio = song("studio", "Dogs", "Pink Floyd", "Animals", "", 1977);
        studio.duration = 1024;
        let songs = vec![
            live,
            studio,
            song("cover", "Dogs", "Some Tribute Band", "Covers", "", 2010),
            song("joga", "Jóga", "Björk", "Homogenic", "", 1997),
            song("remaster", "So What (2009 Remaster)", "Miles Davis", "Kind of Blue", "", 1959),
        ];
        db::index(&mut core.db.lock(), &[], &[], &songs).unwrap();
        let ids = |entries: Vec<M3uEntry>| -> Vec<Option<String>> { core.m3u_match(entries).unwrap().into_iter().map(|s| s.map(|s| s.id)).collect() };
        let some = |id: &str| Some(id.to_string());

        assert_eq!(
            ids(vec![
                entry(1020, "Pink Floyd", "Dogs"),     // exact, duration picks the studio cut
                entry(-1, "pink floyd", "DOGS"),       // exact without a duration: the first
                entry(-1, "BJÖRK", "jóga"),            // unicode case folding
                entry(-1, "Bjork", "Joga"),            // diacritics via the full-text index
                entry(-1, "Miles Davis", "So What"),   // not exact, every word matches
                entry(2000, "", "Dogs"),               // title only
                entry(-1, "Pink Floid", "Dogs"),       // misspelt artist: exact title
                entry(-1, "Miles Davis", "So"),        // a shortened title whose words all match
                entry(-1, "Nobody", "Nothing"),
                entry(-1, "", ""),
                entry(-1, "", "\"' OR * NEAR("),
            ]),
            [some("studio"), some("live"), some("joga"), some("joga"), some("remaster"), some("studio"), some("live"), some("remaster"), None, None, None]
        );
        assert!(core.m3u_match(vec![]).unwrap().is_empty());
        assert_eq!(Core::new(String::new(), "t".into()).unwrap().m3u_match(vec![entry(1, "a", "b")]).unwrap(), vec![None]);
    }
}
