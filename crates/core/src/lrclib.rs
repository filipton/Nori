//! Lyrics from outside the server, for songs it has none (or only unsynced ones) for. LRCLIB is an open,
//! community-run database of synced lyrics with a documented API, which is why it is the one built in.
//! Each lookup is one or two small requests made when the lyrics are opened, never ahead of time, and
//! the answer is kept in the response cache so a song is looked up once.

use std::borrow::Cow;

use serde_json::{Map, Value};

use crate::client::{Client, NetResult};
use crate::transport;
use crate::{alog, lyrics, Lyrics, Song};

const BASE: &str = "https://lrclib.net/api";
/// A miss is asked again after a week: someone may have added the song since.
const MISS_KEPT_MS: i64 = 7 * 24 * 3_600_000;
/// Search hits further than this from the song's length are another recording.
const DURATION_SLACK_S: f64 = 4.0;

/// Lyrics to show after the server's own, and where they came from.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct LyricsPick {
    pub lyrics: Lyrics,
    /// From LRCLIB; otherwise this is the server's empty answer, shown once nothing better came.
    pub lrclib: bool,
}

/// A provider's answer. `Failed` (no network, server error) must never be remembered as `Missing`.
enum Lookup {
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
fn clean(title: &str) -> String {
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

fn get_url(song: &Song, title: &str) -> String {
    let mut u = format!("{BASE}/get?artist_name=");
    form_encode(&mut u, &song.artist);
    u.push_str("&track_name=");
    form_encode(&mut u, title);
    u.push_str("&album_name=");
    form_encode(&mut u, &song.album);
    u.push_str(&format!("&duration={}", song.duration));
    u
}

fn search_url(song: &Song, title: &str) -> String {
    let mut u = format!("{BASE}/search?track_name=");
    form_encode(&mut u, title);
    u.push_str("&artist_name=");
    form_encode(&mut u, &song.artist);
    u
}

// ---- reading LRCLIB's answers ---------------------------------------------------------------------------

/// A field as text: missing is empty, JSON null is the word "null" (hence the guard in [`pick`]).
fn text<'a>(o: &'a Map<String, Value>, k: &str) -> Cow<'a, str> {
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

fn number(o: &Map<String, Value>, k: &str) -> f64 {
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
fn pick(o: &Map<String, Value>) -> Option<Lyrics> {
    if flag(o, "instrumental") {
        return None;
    }
    let usable = |k| Some(text(o, k)).filter(|t| !blank(t) && t != "null");
    let chosen = usable("syncedLyrics").or_else(|| usable("plainLyrics"))?;
    Some(lyrics::from_lrc(&chosen)).filter(|l| !l.lines.is_empty())
}

fn json(body: &[u8]) -> Result<Value, String> {
    serde_json::from_str(&String::from_utf8_lossy(body)).map_err(|e| e.to_string())
}

impl Client {
    async fn lrclib(&self, song: &Song) -> Lookup {
        let title = clean(&song.title);
        // The exact lookup first: LRCLIB matches the duration within a couple of seconds itself.
        let exact = match transport::get(&*self.transport, get_url(song, &title), 0).await.map_err(|e| e.to_string()).and_then(|b| json(&b)) {
            Ok(Value::Object(o)) => o,
            Ok(_) => return failed("get", "not an object"),
            Err(e) => return failed("get", &e),
        };
        if !exact.contains_key("statusCode") {
            if let Some(l) = pick(&exact) {
                return Lookup::Found(l);
            }
        }
        // Otherwise a search, ranked by how close the duration is: the first hit is often a remix or a live take.
        let hits = match transport::get(&*self.transport, search_url(song, &title), 0).await.map_err(|e| e.to_string()).and_then(|b| json(&b)) {
            Ok(Value::Array(a)) => a,
            Ok(_) => return failed("search", "not an array"),
            Err(e) => return failed("search", &e),
        };
        let Some(hits) = hits.iter().map(Value::as_object).collect::<Option<Vec<_>>>() else { return failed("search", "not a list of records") };
        let duration = song.duration as f64;
        let off = |o: &Map<String, Value>| (number(o, "duration") - duration).abs();
        let mut candidates: Vec<_> = hits.into_iter().filter(|o| off(o) <= DURATION_SLACK_S || song.duration == 0).collect();
        candidates.sort_by(|a, b| text(a, "syncedLyrics").is_empty().cmp(&text(b, "syncedLyrics").is_empty()).then(off(a).total_cmp(&off(b))));
        candidates.into_iter().find_map(pick).map_or(Lookup::Missing, Lookup::Found)
    }
}

fn failed(what: &str, why: &str) -> Lookup {
    alog::info(&format!("lrclib {what} failed: {why}"));
    Lookup::Failed
}

/// Back to LRC text for the cache; it is parsed again when read. Centiseconds round half up, as the
/// cache has always been written.
fn to_lrc(l: &Lyrics) -> String {
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
fn lrclib_allowed(p: &crate::settings::StoredPrefs) -> bool {
    p.third_party_lookups && p.lyrics_lrclib
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Client {
    /// What to show once the server's own lyrics are in (`server_has_lines`, `server_synced` describe
    /// them; the platform shows them itself). The server's synced lyrics win; otherwise, when the
    /// settings allow LRCLIB and the song is in the library, LRCLIB's - synced ones over the server's
    /// plain ones. None: keep showing what the server gave. An empty server answer is not shown while
    /// LRCLIB may still have the song (the panel read "No lyrics" for a moment and then filled in); it
    /// comes back from here when nothing better was found.
    pub async fn lyrics_after_server(&self, song: Song, server_has_lines: bool, server_synced: bool) -> NetResult<Option<LyricsPick>> {
        let allow = crate::settings_store::with_prefs(lrclib_allowed).unwrap_or(false);
        let mut pick = self.after_server(song, allow, server_has_lines, server_synced).await?;
        if let Some(p) = &mut pick {
            crate::look::keep(&mut p.lyrics);
        }
        Ok(pick)
    }
}

impl Client {
    async fn after_server(&self, song: Song, allow_lrclib: bool, server_has_lines: bool, server_synced: bool) -> NetResult<Option<LyricsPick>> {
        let nothing = || Some(LyricsPick { lyrics: Lyrics::default(), lrclib: false });
        if !allow_lrclib || song.is_external || (server_has_lines && server_synced) {
            return Ok(if server_has_lines { None } else { nothing() });
        }
        // A hit is kept for good; a miss is asked again after a week; a failed request (offline, server
        // down) is not remembered at all.
        let key = format!("lrclib2|{}|{}|{}", song.artist, song.title, song.duration);
        let cached = self.core.cache_get(key.clone())?.map(|b| String::from_utf8_lossy(&b).into_owned());
        let miss_fresh = cached.as_deref() == Some("") && self.core.cache_fresh(key.clone(), MISS_KEPT_MS)?;
        let found = match cached {
            Some(text) if !text.is_empty() => Some(lyrics::from_lrc(&text)),
            _ if miss_fresh => None,
            _ => match self.lrclib(&song).await {
                Lookup::Found(l) => {
                    self.core.cache_put(key, to_lrc(&l).into_bytes())?;
                    Some(l)
                }
                Lookup::Missing => {
                    self.core.cache_put(key, Vec::new())?;
                    None
                }
                Lookup::Failed => None,
            },
        };
        Ok(match found {
            Some(l) if !server_has_lines || l.synced => Some(LyricsPick { lyrics: l, lrclib: true }),
            _ if !server_has_lines => nothing(),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{block, client, Fake};
    use crate::client::NetProfile;
    use crate::transport::FailureKind;
    use crate::LyricLine;
    use std::sync::Arc;

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

    fn song() -> Song {
        Song { id: "1".into(), title: "Dogs (Remastered)".into(), artist: "Pink Floyd".into(), album: "Animals".into(), duration: 1024, ..Default::default() }
    }

    fn setup() -> (Arc<Client>, Arc<Fake>) {
        client(NetProfile { url: "h".into(), ..Default::default() })
    }

    #[test]
    fn lrclib_needs_both_switches() {
        let p = |third_party_lookups, lyrics_lrclib| crate::settings::StoredPrefs { third_party_lookups, lyrics_lrclib, ..Default::default() };
        assert!(lrclib_allowed(&p(true, true)));
        assert!(!lrclib_allowed(&p(true, false)));
        assert!(!lrclib_allowed(&p(false, true)));
    }

    #[test]
    fn server_synced_lyrics_win_and_an_empty_answer_goes_out_last() {
        let (c, fake) = setup();
        assert!(block(c.after_server(song(), true, true, true)).unwrap().is_none());
        let none = block(c.after_server(song(), false, false, false)).unwrap().unwrap();
        assert!(!none.lrclib && none.lyrics.lines.is_empty());
        let external = Song { is_external: true, ..song() };
        assert!(!block(c.after_server(external, true, false, false)).unwrap().unwrap().lrclib);
        assert!(fake.asked().is_empty());
    }

    #[test]
    fn a_hit_is_kept_and_a_search_is_ranked_by_duration() {
        let (c, fake) = setup();
        fake.answer(r#"{"statusCode":404,"message":"not found"}"#);
        fake.answer(r#"[{"duration":1000,"syncedLyrics":"[00:01.00]far"},{"duration":1026,"plainLyrics":"close plain"},{"duration":1022,"syncedLyrics":"[00:02.00]close synced"}]"#);
        let got = block(c.after_server(song(), true, true, false)).unwrap().unwrap();
        assert!(got.lrclib && got.lyrics.synced);
        assert_eq!(got.lyrics.lines[0].text, "close synced");
        let asked = fake.asked();
        assert!(asked[0].contains("track_name=Dogs&"));
        assert!(asked[1].starts_with("https://lrclib.net/api/search?track_name=Dogs&artist_name=Pink+Floyd"));

        // Kept: no request the second time.
        let again = block(c.after_server(song(), true, false, false)).unwrap().unwrap();
        assert_eq!(again.lyrics.lines[0].text, "close synced");
        assert_eq!(fake.asked().len(), 2);
    }

    #[test]
    fn plain_lrclib_lyrics_do_not_replace_the_servers_plain_ones() {
        let (c, fake) = setup();
        fake.answer(r#"{"plainLyrics":"just words"}"#);
        assert!(block(c.after_server(song(), true, true, false)).unwrap().is_none());
        assert_eq!(fake.asked().len(), 1);
    }

    #[test]
    fn a_miss_is_remembered_for_a_week_and_a_failure_not_at_all() {
        let (c, fake) = setup();
        fake.fail(FailureKind::UnknownHost);
        assert!(!block(c.after_server(song(), true, false, false)).unwrap().unwrap().lrclib);
        assert!(c.core.cache_get("lrclib2|Pink Floyd|Dogs (Remastered)|1024".into()).unwrap().is_none());

        fake.answer(r#"{"statusCode":404}"#);
        fake.answer("[]");
        block(c.after_server(song(), true, false, false)).unwrap();
        assert_eq!(c.core.cache_get("lrclib2|Pink Floyd|Dogs (Remastered)|1024".into()).unwrap(), Some(vec![]));
        block(c.after_server(song(), true, false, false)).unwrap();
        assert_eq!(fake.asked().len(), 3, "the miss is fresh");

        c.core.db.lock().execute("UPDATE cache SET ts = 0", []).unwrap();
        fake.answer(r#"{"syncedLyrics":"[00:01.00]now"}"#);
        assert!(block(c.after_server(song(), true, false, false)).unwrap().unwrap().lrclib);
    }
}
