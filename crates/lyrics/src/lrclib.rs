//! Lyrics from outside the server, for songs it has none (or only unsynced ones) for. LRCLIB is an open,
//! community-run database of synced lyrics with a documented API, which is why it is the one built in.
//! Each lookup is one or two small requests made when the lyrics are opened, never ahead of time, and
//! the answer is kept in the response cache so a song is looked up once.

use std::borrow::Cow;

use nori_model::{alog, Lyrics, Song};
use serde_json::{Map, Value};

use crate::lyrics;

const BASE: &str = "https://lrclib.net/api";
/// A miss is asked again after a week: someone may have added the song since.
pub const MISS_KEPT_MS: i64 = 7 * 24 * 3_600_000;
/// Search hits further than this from the song's length are another recording.
pub const DURATION_SLACK_S: f64 = 4.0;

/// Lyrics to show after the server's own, and where they came from.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct LyricsPick {
    pub lyrics: Lyrics,
    /// From LRCLIB; otherwise this is the server's empty answer, shown once nothing better came.
    pub lrclib: bool,
}

/// A provider's answer. `Failed` (no network, server error) must never be remembered as `Missing`.
pub enum Lookup {
    Found(Lyrics),
    Missing,
    Failed,
}

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
fn form_encode(out: &mut String, v: &str) {
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

/// The lyrics in one LRCLIB record: none for an instrumental, synced ones over plain ones.
pub fn pick(o: &Map<String, Value>) -> Option<Lyrics> {
    if flag(o, "instrumental") {
        return None;
    }
    let usable = |k| Some(text(o, k)).filter(|t| !blank(t) && t != "null");
    let chosen = usable("syncedLyrics").or_else(|| usable("plainLyrics"))?;
    Some(lyrics::from_lrc(&chosen)).filter(|l| !l.lines.is_empty())
}

pub fn json(body: &[u8]) -> Result<Value, String> {
    serde_json::from_str(&String::from_utf8_lossy(body)).map_err(|e| e.to_string())
}

pub fn failed(what: &str, why: &str) -> Lookup {
    alog::info(&format!("lrclib {what} failed: {why}"));
    Lookup::Failed
}

/// Back to LRC text for the cache; it is parsed again when read. Centiseconds round half up, as the
/// cache has always been written.
pub fn to_lrc(l: &Lyrics) -> String {
    let mut out = String::new();
    for (i, line) in l.lines.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        if line.start_ms >= 0 {
            let cs = (line.start_ms % 60_000 + 5) / 10;
            out.push_str(&format!("[{:02}:{:02}.{:02}]", line.start_ms / 60_000, cs / 100, cs % 100));
        }
        out.push_str(&line.text);
    }
    out
}

/// LRCLIB is asked only with third-party lookups on and its own switch on too.
pub fn lrclib_allowed(p: &nori_settings::settings::StoredPrefs) -> bool {
    p.third_party_lookups && p.lyrics_lrclib
}

#[cfg(test)]
mod tests {
    use super::*;
    
    use nori_model::LyricLine;
    

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
    fn lrc_is_written_with_centiseconds_rounded_half_up() {
        let line = |start_ms, text: &str| LyricLine { start_ms, text: text.into(), ..Default::default() };
        let l = Lyrics { synced: true, word_timed: false, lines: vec![line(5_200, "a"), line(65_125, "b"), line(3_599_995, "c"), line(-1, "plain")], key: 0 };
        assert_eq!(to_lrc(&l), "[00:05.20]a\n[01:05.13]b\n[59:60.00]c\nplain");
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
    fn lrclib_needs_both_switches() {
        let p = |third_party_lookups, lyrics_lrclib| nori_settings::settings::StoredPrefs { third_party_lookups, lyrics_lrclib, ..Default::default() };
        assert!(lrclib_allowed(&p(true, true)));
        assert!(!lrclib_allowed(&p(true, false)));
        assert!(!lrclib_allowed(&p(false, true)));
    }
}
