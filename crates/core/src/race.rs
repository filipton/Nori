//! Lyrics from the lyrics services as the client's calls: asked through the client's transport, the
//! answers kept in the core's response cache. Which services to ask is the settings' (`lyrics_lookup`);
//! how each is asked, and whose answer is shown, are nori-lyrics' (services.rs, race.rs).

use std::sync::Arc;

use crate::client::{Client, NetResult};
use crate::settings::StoredPrefs;
use crate::{Core, Song};

pub use nori_lyrics::race::*;
use nori_settings::lyrics_sources::{lyrics_lookup, LyricsLookup};

impl LyricsCache for Core {
    fn get(&self, key: &str) -> Option<Vec<u8>> {
        self.cache_get(key.to_string()).ok().flatten()
    }

    fn fresh(&self, key: &str, max_age_ms: i64) -> bool {
        self.cache_fresh(key.to_string(), max_age_ms).unwrap_or(false)
    }

    fn put(&self, key: &str, body: Vec<u8>) {
        let _ = self.cache_put(key.to_string(), body);
    }
}

/// Hands each set of lyrics on to the platform with its timing kept, so the lyrics page starts its clock
/// on them by key (`look::keep`).
struct Keeping(Arc<dyn LyricsShown>);

impl LyricsShown for Keeping {
    fn show(&self, mut pick: LyricsPick) {
        crate::look::keep(&mut pick.lyrics);
        self.0.show(pick);
    }
}

/// Hands on only what differs from what it handed last ([`lyrics_replaces`]): the server's answer read
/// again, or the lookup's choice from before, are not new lyrics to show.
struct Screen {
    to: Arc<dyn LyricsShown>,
    last: parking_lot::Mutex<Option<LyricsPick>>,
}

impl Screen {
    fn show(&self, pick: LyricsPick) {
        let mut last = self.last.lock();
        if lyrics_replaces(last.as_ref(), &pick) {
            *last = Some(pick.clone());
            drop(last);
            self.to.show(pick);
        }
    }
}

impl LyricsShown for Screen {
    fn show(&self, pick: LyricsPick) {
        Screen::show(self, pick);
    }
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Client {
    /// Everything the lyrics page shows for song `id`, in order, each to `shown`: the server's own lyrics
    /// (what is stored, then the server's answer when it differs; an empty answer is not shown while a
    /// service may still have the song), then, when those are not timed, what the lyrics services find
    /// ([`Client::lyrics_lookup`]), and at the end an empty answer from the server when nobody had
    /// anything. Nothing is handed on twice. Returns when all is in; dropping the call cancels every
    /// request in it.
    pub async fn lyrics_for(&self, id: String, shown: Arc<dyn LyricsShown>) -> NetResult<()> {
        let screen = Arc::new(Screen { to: shown, last: parking_lot::Mutex::new(None) });
        let mut server: Option<crate::Lyrics> = None;
        // A failure to read the server's is no lyrics from it: the services are asked all the same.
        let _ = self
            .read_each(crate::cache_policy::Read::LyricsBySong { song_id: id.clone() }, |p| {
                if let crate::cache_policy::Page::LyricsPage { v } = p {
                    if !v.lines.is_empty() {
                        screen.show(LyricsPick { lyrics: v.clone(), origin: nori_settings::lyrics_sources::LyricsOrigin::Server });
                    }
                    server = Some(v);
                }
            })
            .await;
        let has_lines = server.as_ref().is_some_and(|l| !l.lines.is_empty());
        let synced = server.as_ref().is_some_and(|l| l.synced);
        self.lyrics_lookup(id, has_lines, synced, screen).await
    }

    /// What to show once the server's own lyrics are in (`server_has_lines`, `server_synced` describe
    /// them; the platform shows them itself): the lyrics services the settings switch on, asked together,
    /// each better answer handed to `shown` as it comes. The server's synced lyrics win and nothing is
    /// asked. An empty server answer is not shown while a service may still have the song (the panel read
    /// "No lyrics" for a moment and then filled in); it comes to `shown` at the end when nothing better
    /// was found. Returns when the lookup is over; dropping the call cancels every request in it.
    pub async fn lyrics_lookup(&self, id: String, server_has_lines: bool, server_synced: bool, shown: Arc<dyn LyricsShown>) -> NetResult<()> {
        // Nothing kept yet is the defaults, which ask nobody.
        let asked = crate::settings_store::with_prefs(lyrics_lookup).unwrap_or_else(|| lyrics_lookup(&StoredPrefs::default()));
        // The song playing, as the queue keeps it: the platform names it by id.
        let song = self.song_of(id).await?;
        self.lookup_with(&song, server_has_lines, server_synced, &asked, &Keeping(shown)).await;
        Ok(())
    }
}

impl Client {
    async fn lookup_with(&self, song: &Song, server_has_lines: bool, server_synced: bool, asked: &LyricsLookup, shown: &dyn LyricsShown) {
        let evict = |prefix: &str| {
            let _ = self.core.cache_evict(prefix.to_string());
        };
        if let Some(line) = lookup(&*self.transport, &*self.core, song, server_has_lines, server_synced, asked, shown, &evict).await {
            nori_perf::perf_log::note_core("lyrics", line.trim_start_matches("lyrics: "));
        }
    }
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    /// How much the lyrics looked up online take in the app's database, in bytes: every service's
    /// answers and the lyrics chosen for each song (Settings, Storage, "Lyrics").
    pub fn lyrics_cache_bytes(&self) -> i64 {
        let c = self.db.lock();
        c.query_row(
            "SELECT COALESCE(SUM(length(key) + length(body)), 0) FROM cache WHERE server=sid() AND key >= ?1 AND key < ?1 || x'ff'",
            [CACHE_PREFIX],
            |r| r.get(0),
        )
        .unwrap_or(0)
    }

    /// Forgets every lyrics lookup: the next time a song's lyrics are opened, the services are asked
    /// again. The server's own lyrics are not touched.
    pub fn lyrics_cache_clear(&self) {
        let _ = self.cache_evict(CACHE_PREFIX.to_string());
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::client::tests::{block, client, Fake};
    use crate::client::NetProfile;
    use crate::transport::FailureKind;
    use nori_settings::lyrics_sources::LyricsService;
    use nori_settings::lyrics_sources::LyricsOrigin;
    use parking_lot::Mutex;

    fn song() -> Song {
        Song { id: "1".into(), title: "Dogs (Remastered)".into(), artist: "Pink Floyd".into(), album: "Animals".into(), duration: 1024, ..Default::default() }
    }

    fn setup() -> (Arc<Client>, Arc<Fake>) {
        client(NetProfile { url: "h".into(), ..Default::default() })
    }

    fn lrclib() -> LyricsLookup {
        LyricsLookup { services: vec![LyricsService::Lrclib], prefer_words: true, paxsenix_key: String::new(), better_lyrics_key: String::new() }
    }

    #[derive(Default)]
    struct Screen(Mutex<Vec<LyricsPick>>);

    impl LyricsShown for Screen {
        fn show(&self, pick: LyricsPick) {
            self.0.lock().push(pick);
        }
    }

    fn run(c: &Client, s: &Song, has_lines: bool, synced: bool, asked: &LyricsLookup) -> Vec<LyricsPick> {
        let screen = Screen::default();
        block(c.lookup_with(s, has_lines, synced, asked, &screen));
        screen.0.into_inner()
    }

    #[derive(Default)]
    struct Page(Mutex<Vec<LyricsPick>>);

    impl LyricsShown for Page {
        fn show(&self, pick: LyricsPick) {
            self.0.lock().push(pick);
        }
    }

    const SYNCED: &str = r#"{"subsonic-response":{"status":"ok","lyricsList":{"structuredLyrics":[{"synced":true,"line":[{"start":1500,"value":"timed"}]}]}}}"#;
    const NONE: &str = r#"{"subsonic-response":{"status":"ok","lyricsList":{}}}"#;

    #[test]
    fn the_servers_lyrics_come_first_and_nothing_is_shown_twice() {
        let (c, fake) = setup();
        // Titles of their own: a service that fails for a song rests for it, whichever test asked.
        crate::queue::queue_register(vec![Song { id: "lf1".into(), title: "In Order One".into(), ..song() }, Song { id: "lf2".into(), title: "In Order Two".into(), ..song() }]);
        fake.answer(SYNCED);
        let page = Arc::new(Page::default());
        block(c.lyrics_for("lf1".into(), page.clone())).unwrap();
        let got = page.0.lock().clone();
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].origin, got[0].lyrics.lines[0].text.as_str()), (LyricsOrigin::Server, "timed"));
        assert_eq!(fake.asked().len(), 1, "timed lyrics from the server: no service asked");
        // Stale, and the server answers the same: not handed on again.
        c.core.db.lock().execute("UPDATE cache SET ts = 0", []).unwrap();
        fake.answer(SYNCED);
        let again = Arc::new(Page::default());
        block(c.lyrics_for("lf1".into(), again.clone())).unwrap();
        assert_eq!(again.0.lock().len(), 1, "the stored answer only");
        // No lyrics at the server: said once, at the end, not while services may still have them.
        fake.answer(NONE);
        let none = Arc::new(Page::default());
        block(c.lyrics_for("lf2".into(), none.clone())).unwrap();
        let got = none.0.lock().clone();
        assert_eq!(got.len(), 1);
        assert!(got[0].lyrics.lines.is_empty() && got[0].origin == LyricsOrigin::Server);
    }

    #[test]
    fn a_hit_is_kept_in_the_response_cache_and_a_search_is_ranked_by_length() {
        let (c, fake) = setup();
        fake.answer(r#"{"statusCode":404,"message":"not found"}"#);
        fake.answer(r#"[{"duration":1000,"syncedLyrics":"[00:01.00]far"},{"duration":1026,"plainLyrics":"close plain"},{"duration":1022,"syncedLyrics":"[00:02.00]close synced\n[01:00.00]you gotta be crazy\n[02:00.00]you gotta have a real need\n[03:00.00]you gotta sleep on your toes"}]"#);
        let got = run(&c, &song(), true, false, &lrclib());
        assert_eq!(got.len(), 1);
        assert_eq!((got[0].origin, got[0].lyrics.lines[0].text.as_str()), (LyricsOrigin::Lrclib, "close synced"));
        let asked = fake.asked();
        assert!(asked[0].contains("track_name=Dogs&"));
        assert!(asked[1].starts_with("https://lrclib.net/api/search?track_name=Dogs&artist_name=Pink+Floyd"));
        assert!(c.core.cache_get("lyrics|LRCLIB|Pink Floyd|Dogs (Remastered)|1024".into()).unwrap().is_some());
        // Kept: no request the second time.
        let again = run(&c, &song(), false, false, &lrclib());
        assert_eq!(again[0].lyrics.lines[0].text, "close synced");
        assert_eq!(fake.asked().len(), 2);
    }

    #[test]
    fn plain_words_do_not_replace_the_servers_plain_ones_and_a_miss_is_kept() {
        let (c, fake) = setup();
        let s = Song { title: "Plain".into(), ..song() };
        fake.answer(r#"{"plainLyrics":"just words"}"#);
        fake.answer("[]");
        assert!(run(&c, &s, true, false, &lrclib()).is_empty(), "the server's own words stay");
        let (c, fake) = setup();
        fake.answer(r#"{"statusCode":404}"#);
        fake.answer("[]");
        let none = run(&c, &s, false, false, &lrclib());
        assert!(none[0].lyrics.lines.is_empty() && none[0].origin == LyricsOrigin::Server);
        assert_eq!(c.core.cache_get("lyrics|LRCLIB|Pink Floyd|Plain|1024".into()).unwrap(), Some(vec![]), "the miss is kept");
        run(&c, &s, false, false, &lrclib());
        assert_eq!(fake.asked().len(), 2, "and fresh for a week");
    }

    #[test]
    fn the_lyrics_cache_is_measured_and_cleared_alone() {
        let (c, fake) = setup();
        fake.answer(r#"{"statusCode":404}"#);
        fake.answer(r#"[{"duration":1022,"syncedLyrics":"[00:02.00]line one\n[01:00.00]line two here\n[02:00.00]a third line\n[03:00.00]and the fourth"}]"#);
        assert_eq!(c.core.lyrics_cache_bytes(), 0);
        c.core.cache_put("getAlbum|1".into(), b"{}".to_vec()).unwrap();
        run(&c, &song(), false, false, &lrclib());
        let bytes = c.core.lyrics_cache_bytes();
        assert!(bytes > 100, "the answer and the choice: {bytes}");
        c.core.lyrics_cache_clear();
        assert_eq!(c.core.lyrics_cache_bytes(), 0);
        assert!(c.core.cache_get("getAlbum|1".into()).unwrap().is_some(), "the rest of the cache stays");
    }

    #[test]
    fn server_synced_lyrics_win_and_a_failure_is_not_kept() {
        let (c, fake) = setup();
        assert!(run(&c, &song(), true, true, &lrclib()).is_empty());
        assert!(fake.asked().is_empty());
        let s = Song { title: "Offline".into(), ..song() };
        fake.fail(FailureKind::UnknownHost);
        assert_eq!(run(&c, &s, false, false, &lrclib())[0].origin, LyricsOrigin::Server);
        assert!(c.core.cache_get("lyrics|LRCLIB|Pink Floyd|Offline|1024".into()).unwrap().is_none());
    }
}
