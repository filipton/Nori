//! LRCLIB, an open, community-run database of synced lyrics with a documented API: its addresses, how
//! titles are cleaned before they are asked for (the other services clean them the same way), and which
//! lyrics in one of its records are worth taking. It is asked like every other service (services.rs).

use std::borrow::Cow;

use nori_model::{Lyrics, Song};
use serde_json::{Map, Value};

use crate::{formats, lyrics};

const BASE: &str = "https://lrclib.net/api";
/// Search hits further than this from the song's length are another recording.
pub const DURATION_SLACK_S: f64 = 4.0;

// ---- matching titles the way providers write them -------------------------------------------------------

/// `\s` as the platform's regular expressions had it: tab, line feed, form feed, carriage return and the
/// Unicode separators.
fn regex_space(c: char) -> bool {
    matches!(c, '\t' | '\n' | '\x0c' | '\r') || separator(c)
}

/// Unicode's space, line and paragraph separators.
fn separator(c: char) -> bool {
    matches!(c, ' ' | '\u{a0}' | '\u{1680}' | '\u{2000}'..='\u{200a}' | '\u{2028}' | '\u{2029}' | '\u{202f}' | '\u{205f}' | '\u{3000}')
}

/// What a line of text ends at for `.` and `$`.
fn line_end(c: char) -> bool {
    matches!(c, '\n' | '\x0b' | '\x0c' | '\r' | '\u{85}' | '\u{2028}' | '\u{2029}')
}

/// Case-insensitive, the way Unicode folds case: the long s and the Kelvin sign are an s and a k.
fn fold(c: char) -> char {
    match c {
        '\u{17f}' => 's',
        '\u{212a}' => 'k',
        c => c.to_ascii_lowercase(),
    }
}

/// The length in bytes of `word` at the start of `s`, ignoring case; `#` stands for a digit.
fn starts_with(s: &str, word: &str) -> Option<usize> {
    let mut it = s.char_indices();
    for w in word.chars() {
        let (_, c) = it.next()?;
        let ok = if w == '#' { c.is_ascii_digit() } else { fold(c) == w };
        if !ok {
            return None;
        }
    }
    Some(it.next().map_or(s.len(), |(i, _)| i))
}

fn starts_with_any(s: &str, words: &[&str]) -> Option<usize> {
    words.iter().find_map(|w| starts_with(s, w))
}

/// What may follow the opening bracket of a credit or version note: `feat.?|ft.?|with|remaster(ed)?|
/// \d{4} remaster|live|mono|stereo`. The rest up to the bracket is taken whatever it is, so the optional
/// endings need no words of their own.
const BRACKETED: [&str; 8] = ["feat", "ft", "with", "remaster", "#### remaster", "live", "mono", "stereo"];
/// What may follow " - ": `remaster(ed)?|\d{4} remaster|live|single version|radio edit`.
const DASHED: [&str; 5] = ["remaster", "#### remaster", "live", "single version", "radio edit"];

/// `\s*[(\[](feat\.?|ft\.?|with|remaster(ed)?|\d{4} remaster|live|mono|stereo)[^)\]]*[)\]]`, all of them.
fn drop_bracketed(title: &str) -> String {
    let mut out = String::with_capacity(title.len());
    let mut i = 0;
    'scan: while i < title.len() {
        let rest = &title[i..];
        let ws = rest.find(|c| !regex_space(c)).unwrap_or(rest.len());
        let open = &rest[ws..];
        if open.starts_with(['(', '[']) {
            let inner = &open[1..];
            if starts_with_any(inner, &BRACKETED).is_some() {
                if let Some(close) = inner.find([')', ']']) {
                    i += ws + 1 + close + 1;
                    continue 'scan;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        i += c.len_utf8();
    }
    out
}

/// `\s+-\s+(remaster(ed)?|\d{4} remaster|live|single version|radio edit).*$`: the first one and all after it.
fn drop_dashed(title: &str) -> &str {
    for (i, c) in title.char_indices() {
        if !regex_space(c) || title[..i].ends_with(regex_space) {
            continue;
        }
        let rest = &title[i..];
        let ws = rest.find(|c| !regex_space(c)).unwrap_or(rest.len());
        let Some(after) = rest[ws..].strip_prefix('-') else { continue };
        let ws2 = after.find(|c| !regex_space(c)).unwrap_or(after.len());
        if ws2 == 0 {
            continue;
        }
        let Some(n) = starts_with_any(&after[ws2..], &DASHED) else { continue };
        // `.*$`: the rest of the line, which has to be the end of the text (or a last line break).
        let tail = &after[ws2 + n..];
        match tail.find(line_end) {
            None => return &title[..i],
            Some(j) if matches!(&tail[j..], "\n" | "\r" | "\r\n" | "\x0b" | "\x0c" | "\u{85}" | "\u{2028}" | "\u{2029}") => return &title[..i],
            Some(_) => {}
        }
    }
    title
}

/// Kotlin's `trim()`: whitespace and the Unicode separators.
fn trim(s: &str) -> &str {
    s.trim_matches(|c| matches!(c, '\t'..='\r' | '\x1c'..='\x1f') || separator(c))
}

/// Removes "(feat. X)", "- Remastered 2011" and similar, which providers rarely carry.
pub fn clean(title: &str) -> String {
    trim(drop_dashed(&drop_bracketed(title))).to_string()
}

/// `application/x-www-form-urlencoded`, as LRCLIB's query string wants it.
pub fn form_encode(out: &mut String, v: &str) {
    for b in v.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'.' | b'-' | b'*' | b'_' => out.push(b as char),
            b' ' => out.push('+'),
            _ => {
                const H: &[u8; 16] = b"0123456789ABCDEF";
                out.push('%');
                out.push(H[(b >> 4) as usize] as char);
                out.push(H[(b & 15) as usize] as char);
            }
        }
    }
}

/// LRCLIB's exact lookup of `song` by its cleaned `title`.
pub fn get_url(song: &Song, title: &str) -> String {
    let mut u = format!("{BASE}/get?artist_name=");
    form_encode(&mut u, &song.artist);
    u.push_str("&track_name=");
    form_encode(&mut u, title);
    u.push_str("&album_name=");
    form_encode(&mut u, &song.album);
    u.push_str(&format!("&duration={}", song.duration));
    u
}

/// LRCLIB's search for `song` by its cleaned `title`, for when the exact lookup finds nothing.
pub fn search_url(song: &Song, title: &str) -> String {
    let mut u = format!("{BASE}/search?track_name=");
    form_encode(&mut u, title);
    u.push_str("&artist_name=");
    form_encode(&mut u, &song.artist);
    u
}

// ---- reading LRCLIB's answers ---------------------------------------------------------------------------

/// A field as text: missing is empty, JSON null is the word "null" (hence the guard in [`pick`]).
pub fn text<'a>(o: &'a Map<String, Value>, k: &str) -> Cow<'a, str> {
    match o.get(k) {
        None => Cow::Borrowed(""),
        Some(Value::String(s)) => Cow::Borrowed(s),
        Some(Value::Null) => Cow::Borrowed("null"),
        Some(v) => Cow::Owned(v.to_string()),
    }
}

fn flag(o: &Map<String, Value>, k: &str) -> bool {
    match o.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A number field of an answer; 0 when it is missing or not a number.
pub fn number(o: &Map<String, Value>, k: &str) -> f64 {
    match o.get(k) {
        Some(Value::Number(n)) => n.as_f64().unwrap_or(0.0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0.0),
        _ => 0.0,
    }
}

fn blank(s: &str) -> bool {
    s.chars().all(|c| matches!(c, '\t'..='\r' | '\x1c'..='\x1f') || separator(c))
}

/// The lyrics in one LRCLIB record: none for an instrumental, the finest timed of what it has. LRCLIB
/// keeps word timing only in the record's lyricsfile (open YAML, contributed with LRCGET's editor); its
/// LRC fields are made from it a line at a time. So the lyricsfile wins when it times words, and the LRC
/// otherwise, synced over plain.
pub fn pick(o: &Map<String, Value>) -> Option<Lyrics> {
    if flag(o, "instrumental") {
        return None;
    }
    let usable = |k| Some(text(o, k)).filter(|t| !blank(t) && t != "null");
    let file = usable("lyricsfile").map(|t| formats::from_lyricsfile(&t)).filter(|l| !l.lines.is_empty());
    if file.as_ref().is_some_and(|f| f.word_timed) {
        return file;
    }
    let lrc = usable("syncedLyrics").or_else(|| usable("plainLyrics")).map(|t| lyrics::from_lrc(&t)).filter(|l| !l.lines.is_empty());
    match (lrc, file) {
        (Some(l), Some(f)) if formats::timing(&f) > formats::timing(&l) => Some(f),
        (Some(l), _) => Some(l),
        (None, f) => f,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_lose_credits_and_version_notes() {
        assert_eq!(clean("Song (feat. Someone)"), "Song");
        assert_eq!(clean("Song [Ft Someone] (Live at Wembley)"), "Song");
        assert_eq!(clean("Song (2011 Remaster)"), "Song");
        assert_eq!(clean("Song - Remastered 2011"), "Song");
        assert_eq!(clean("Song - 2009 Remaster"), "Song");
        assert_eq!(clean("Song - Single Version"), "Song");
        assert_eq!(clean("Song - Radio Edit (x)"), "Song");
        assert_eq!(clean("Song (Without You]"), "Song", "any closing bracket, and `with` starts `without`");
        assert_eq!(clean("Song (Acoustic)"), "Song (Acoustic)");
        assert_eq!(clean("Song (feat. unclosed"), "Song (feat. unclosed");
        assert_eq!(clean("Song -Live"), "Song -Live", "a dash needs space on both sides");
        assert_eq!(clean("Song - Liverpool - Live"), "Song", "the first match takes the rest of the line");
        assert_eq!(clean("Song - Live\nmore"), "Song - Live\nmore", "not at the end of the text");
        assert_eq!(clean("Song\u{a0}(ſtereo)  "), "Song");
        assert_eq!(clean("  Zażółć (Mono) "), "Zażółć");
    }

    #[test]
    fn query_strings_are_form_encoded() {
        let song = Song { artist: "AC/DC & Co".into(), album: "Ünï".into(), duration: 200, ..Default::default() };
        assert_eq!(get_url(&song, "It's *a*-b_c.d"), "https://lrclib.net/api/get?artist_name=AC%2FDC+%26+Co&track_name=It%27s+*a*-b_c.d&album_name=%C3%9Cn%C3%AF&duration=200");
        assert_eq!(search_url(&song, "t"), "https://lrclib.net/api/search?track_name=t&artist_name=AC%2FDC+%26+Co");
    }

    #[test]
    fn records_pick_synced_then_plain_and_skip_instrumentals() {
        let o = |j: &str| serde_json::from_str::<Value>(j).unwrap().as_object().unwrap().clone();
        assert!(pick(&o(r#"{"instrumental":true,"syncedLyrics":"[00:01.00]x"}"#)).is_none());
        assert!(pick(&o(r#"{"instrumental":"TRUE","plainLyrics":"x"}"#)).is_none());
        assert!(pick(&o(r#"{"syncedLyrics":"[00:01.00]x","plainLyrics":"y"}"#)).unwrap().synced);
        assert!(!pick(&o(r#"{"syncedLyrics":null,"plainLyrics":"y"}"#)).unwrap().synced);
        assert!(pick(&o(r#"{"syncedLyrics":"  ","plainLyrics":null}"#)).is_none());
    }

    #[test]
    fn a_word_timed_lyricsfile_beats_the_lrc_beside_it() {
        let o = |j: &str| serde_json::from_str::<Value>(j).unwrap().as_object().unwrap().clone();
        let file = "version: '1.0'\nmetadata: {title: t, artist: a}\nlines:\n  - {text: hi there, start_ms: 1000, words: [{text: 'hi ', start_ms: 1000, end_ms: 1400}, {text: there, start_ms: 1400, end_ms: 2000}]}\n";
        let both = serde_json::json!({"syncedLyrics": "[00:01.00]hi there", "lyricsfile": file}).to_string();
        assert!(pick(&o(&both)).unwrap().word_timed);
        // A lyricsfile timed by line only is no better than the LRC made from it.
        let lined = serde_json::json!({"syncedLyrics": "[00:01.00]from lrc", "lyricsfile": "version: '1.0'\nmetadata: {title: t, artist: a}\nlines:\n  - {text: from file, start_ms: 1000}\n"}).to_string();
        assert_eq!(pick(&o(&lined)).unwrap().lines[0].text, "from lrc");
        // Plain words only, and a lyricsfile that times them: the file.
        let plain = serde_json::json!({"plainLyrics": "words", "lyricsfile": "version: '1.0'\nmetadata: {title: t, artist: a}\nlines:\n  - {text: timed, start_ms: 1000}\n"}).to_string();
        assert!(pick(&o(&plain)).unwrap().synced);
    }
}
