//! Lyrics from LRCLIB as the client's calls: the lookups go out through the client's transport and are
//! kept in the response cache. Matching and reading the answers are nori-lyrics'.

use serde_json::{Map, Value};

use crate::client::{Client, NetResult};
use crate::{lyrics, transport, Lyrics, Song};

pub use nori_lyrics::lrclib::*;

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

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Client {
    /// What to show once the server's own lyrics are in (`server_has_lines`, `server_synced` describe
    /// them; the platform shows them itself). The server's synced lyrics win; otherwise, when the
    /// settings allow LRCLIB and the song is in the library, LRCLIB's - synced ones over the server's
    /// plain ones. None: keep showing what the server gave. An empty server answer is not shown while
    /// LRCLIB may still have the song (the panel read "No lyrics" for a moment and then filled in); it
    /// comes back from here when nothing better was found.
    pub async fn lyrics_after_server(&self, id: String, server_has_lines: bool, server_synced: bool) -> NetResult<Option<LyricsPick>> {
        let allow = crate::settings_store::with_prefs(lrclib_allowed).unwrap_or(false);
        // The song playing, as the queue keeps it: the platform names it by id.
        let song = self.song_of(id).await?;
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
pub(crate) mod tests {
    use super::*;
    use crate::client::tests::{block, client, Fake};
    use crate::client::NetProfile;
    use crate::transport::FailureKind;
    use std::sync::Arc;

    fn song() -> Song {
        Song { id: "1".into(), title: "Dogs (Remastered)".into(), artist: "Pink Floyd".into(), album: "Animals".into(), duration: 1024, ..Default::default() }
    }

    fn setup() -> (Arc<Client>, Arc<Fake>) {
        client(NetProfile { url: "h".into(), ..Default::default() })
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
    fn server_synced_lyrics_win_and_an_empty_answer_goes_out_last() {
        let (c, fake) = setup();
        assert!(block(c.after_server(song(), true, true, true)).unwrap().is_none());
        let none = block(c.after_server(song(), false, false, false)).unwrap().unwrap();
        assert!(!none.lrclib && none.lyrics.lines.is_empty());
        let external = Song { is_external: true, ..song() };
        assert!(!block(c.after_server(external, true, false, false)).unwrap().unwrap().lrclib);
        assert!(fake.asked().is_empty());
    }
}
