//! Lyrics from the services a lookup asks (the settings' `lyrics_lookup`), asked together and ranked
//! best first, for a song the server has no timed lyrics for.
//!
//! What each service answered before is read first: a hit is kept for good, a miss is asked again after
//! a week (someone may have added the song since), and neither is asked for again on the next play. The
//! rest are asked together, a few at a time in the order they rank ([`AT_ONCE`] out at once, each with
//! its own time, services.rs's `deadline_ms`), and [`Race`] decides whose answer is shown: a finer one
//! goes on screen as soon as it arrives, and the asking stops, dropping whatever is still out (which
//! cancels it), as soon as nobody still to answer could beat the one in hand. A failed or too slow
//! service is never remembered as a miss, but it is not asked about the same song again for a while,
//! and one that keeps failing rests for a while altogether, so a service that is down is not hammered.
//! Provider (`ext-`) songs are never looked up. Nothing here runs a thread: the lookup is one future,
//! polled by whoever asked.

use std::time::{Duration, Instant};

use futures_util::stream::{FuturesUnordered, StreamExt};
use nori_model::{Lyrics, Song};
use nori_net::transport::Transport;
use nori_settings::lyrics_sources::{LyricsLookup, LyricsService};
use nori_words::words::LyricsOrigin;
use parking_lot::Mutex;

use crate::fit::{agree, plausible};
use crate::formats::{from_cache, timing, to_cache};
use crate::services::{self, Ask, Lookup, Shared};

/// How many services are asked at once. Enough that a song nobody has does not take a wave of timeouts
/// per service; few enough that the top-ranked ones, asked first, can settle it before the rest are
/// asked at all.
pub const AT_ONCE: usize = 6;
/// A miss is asked again after a week: someone may have added the song since.
pub const MISS_KEPT_MS: i64 = 7 * 24 * 3_600_000;
/// A service that failed for a song is not asked about it again for this long.
const FAILED_REST: Duration = Duration::from_secs(30 * 60);
/// A service that failed this many songs in a row rests...
const FAILURES_IN_A_ROW: u32 = 3;
/// ...for this long, for every song.
const SERVICE_REST: Duration = Duration::from_secs(10 * 60);

/// Lyrics to show, and where they came from.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct LyricsPick {
    pub lyrics: Lyrics,
    pub origin: LyricsOrigin,
}

/// Where the platform is told each set of lyrics to show, as they come.
#[cfg_attr(feature = "ffi", uniffi::export(with_foreign))]
pub trait LyricsShown: Send + Sync {
    fn show(&self, pick: LyricsPick);
}

/// Where answers are remembered: the core's response cache in the app's database.
pub trait LyricsCache: Send + Sync {
    fn get(&self, key: &str) -> Option<Vec<u8>>;
    /// Whether `key` was kept less than `max_age_ms` ago.
    fn fresh(&self, key: &str, max_age_ms: i64) -> bool;
    fn put(&self, key: &str, body: Vec<u8>);
}

/// Which service's answer is shown, as answers come in from services asked together. Services are
/// ranked by their position (0 is the best); `best` is the finest timing each could answer with (3 word
/// by word, 2 line by line, 1 not timed).
///
/// With `prefer_words`, finer timing wins whoever has it, and rank decides between equals: a lyric timed
/// by line from the top service is held back while a lower one may still time the words. Without it, any
/// timed answer beats untimed words, and between timed answers rank alone decides. The server's own
/// untimed words (`server_timing` 1) rank above every service's untimed words.
///
/// Once there is a winner, an answer takes its place only when it is the same song's words
/// ([`agree`]): a service that fell back to another song must not replace lyrics already found. And once
/// lyrics are on screen they are replaced only by finer timing, never by an equal answer from a
/// higher-ranked service: the words do not swap under the listener for nothing they could see.
pub struct Race {
    best: Vec<u8>,
    prefer_words: bool,
    done: Vec<bool>,
    waiting: Vec<usize>,
    shown: Option<usize>,
    shown_timing: u8,
    /// The winner so far, or none for the server's own words or nothing.
    leader: Option<usize>,
    leader_timing: u8,
    lyrics: Option<Lyrics>,
}

impl Race {
    pub fn new(best: Vec<u8>, prefer_words: bool, server_timing: u8) -> Self {
        let n = best.len();
        Race { best, prefer_words, done: vec![false; n], waiting: (0..n).collect(), shown: None, shown_timing: server_timing, leader: None, leader_timing: server_timing, lyrics: None }
    }

    fn tier(&self, timing: u8) -> u8 {
        if self.prefer_words {
            timing
        } else {
            timing.min(2)
        }
    }

    /// Whether an answer timed `timing` from `rank` would win over the leader.
    fn beats(&self, timing: u8, rank: usize) -> bool {
        let (mine, theirs) = (self.tier(timing), self.tier(self.leader_timing));
        timing > 0 && (mine > theirs || (mine == theirs && self.leader.is_some_and(|l| rank < l)))
    }

    /// Whether `rank`, not yet heard from, could still win: also whether it is worth asking at all.
    pub fn worth(&self, rank: usize) -> bool {
        !self.done[rank] && self.beats(self.best[rank], rank)
    }

    /// Nobody still to be heard from can win: the leader is final.
    pub fn settled(&self) -> bool {
        (0..self.best.len()).all(|r| !self.worth(r))
    }

    /// Every service that could time something has been heard from.
    fn timed_done(&self) -> bool {
        (0..self.best.len()).all(|r| self.done[r] || self.best[r] < 2)
    }

    /// `rank` answered, with lyrics or with nothing (a miss, a failure, too slow).
    pub fn answer(&mut self, rank: usize, found: Option<Lyrics>) {
        if self.done[rank] {
            return;
        }
        self.done[rank] = true;
        self.waiting.retain(|w| *w != rank);
        let t = found.as_ref().map_or(0, timing);
        let same_song = |l: &Lyrics| self.lyrics.as_ref().is_none_or(|had| agree(had, l));
        if let Some(l) = found.filter(|l| self.beats(t, rank) && same_song(l)) {
            self.leader = Some(rank);
            self.leader_timing = t;
            self.lyrics = Some(l);
        }
    }

    /// The ranks to start asking now, best first, so that no more than `pool` are out at once. A service
    /// that can no longer win is dropped unasked. One with untimed words only is asked last: when every
    /// service that could time something has answered, and none did.
    pub fn next(&mut self, running: usize, pool: usize) -> Vec<usize> {
        let hopeless: Vec<usize> = self.waiting.iter().copied().filter(|r| !self.worth(*r)).collect();
        for r in hopeless {
            self.answer(r, None);
        }
        let timed_done = self.timed_done();
        let start: Vec<usize> = self.waiting.iter().copied().filter(|r| self.best[*r] >= 2 || timed_done).take(pool.saturating_sub(running)).collect();
        self.waiting.retain(|w| !start.contains(w));
        start
    }

    /// What to put on screen now, if anything: the leader, when it is new and either finer than what is
    /// shown (worth showing at once, while better may still come) or, at the `last`, the first lyrics
    /// found at all. An answer timed alike never replaces lyrics already on screen.
    pub fn to_show(&mut self, last: bool) -> Option<(usize, Lyrics)> {
        let found = self.lyrics.as_ref()?;
        let leader = self.leader?;
        let finer = self.leader_timing > self.shown_timing;
        if self.shown == Some(leader) || !(finer || (last && self.shown.is_none())) {
            return None;
        }
        self.shown = Some(leader);
        self.shown_timing = self.leader_timing;
        Some((leader, found.clone()))
    }

    /// The winner's timing; 0 when nobody has anything.
    pub fn leader_timing(&self) -> u8 {
        self.leader_timing
    }
}

/// What a service's answers about a song are remembered under. BetterLyrics answers more with a key
/// than without, so a song it had nothing for without one is asked again once a key is given.
fn cache_key(service: LyricsService, song: &Song, lookup: &LyricsLookup) -> String {
    let keyed = matches!(service, LyricsService::BetterLyrics | LyricsService::Portato) && !lookup.better_lyrics_key.is_empty();
    format!("lyrics|{}{}|{}|{}|{}", service.name(), if keyed { "+key" } else { "" }, song.artist, song.title, song.duration)
}

/// What a service answered before.
enum Remembered {
    Hit(Lyrics),
    Miss,
    Unknown,
}

fn remembered(cache: &dyn LyricsCache, key: &str, song: &Song) -> Remembered {
    match cache.get(key) {
        // An entry that no longer reads falls through to asking again; one kept before answers were
        // checked, which cannot be this song's, is the miss it should have been.
        Some(b) if !b.is_empty() => match Some(from_cache(&String::from_utf8_lossy(&b))).filter(|l| !l.lines.is_empty()) {
            Some(l) if plausible(&l, song) => Remembered::Hit(l),
            Some(_) => Remembered::Miss,
            None => Remembered::Unknown,
        },
        Some(_) if cache.fresh(key, MISS_KEPT_MS) => Remembered::Miss,
        _ => Remembered::Unknown,
    }
}

/// Services that failed lately: for which song (its cache key) and when, and how many songs each has
/// failed in a row. In memory only: a failure is never an answer to keep.
struct Failures {
    songs: Vec<(String, Instant)>,
    services: Vec<(LyricsService, u32, Option<Instant>)>,
}

static FAILURES: Mutex<Failures> = Mutex::new(Failures { songs: Vec::new(), services: Vec::new() });
/// How many songs' failures are remembered.
const FAILURES_KEPT: usize = 64;

impl Failures {
    /// Whether `service` should not be asked about the song under `key` now.
    fn resting(&self, service: LyricsService, key: &str, now: Instant) -> bool {
        let song = self.songs.iter().any(|(k, at)| k == key && now.duration_since(*at) < FAILED_REST);
        let all = self.services.iter().any(|(s, _, until)| *s == service && until.is_some_and(|u| now < u));
        song || all
    }

    fn failed(&mut self, service: LyricsService, key: String, now: Instant) {
        if self.songs.len() >= FAILURES_KEPT {
            self.songs.remove(0);
        }
        self.songs.push((key, now));
        match self.services.iter_mut().find(|(s, _, _)| *s == service) {
            Some(e) => {
                e.1 += 1;
                if e.1 >= FAILURES_IN_A_ROW {
                    e.1 = 0;
                    e.2 = Some(now + SERVICE_REST);
                }
            }
            None => self.services.push((service, 1, None)),
        }
    }

    fn answered(&mut self, service: LyricsService) {
        self.services.retain(|(s, _, _)| *s != service);
    }
}

/// Entries from before every word timing was kept (LRC, under the old key) go, once per run.
static OLD_DROPPED: std::sync::Once = std::sync::Once::new();

/// Looks `song` up with the services `lookup` names, after the server's own lyrics, which the platform
/// already shows (`server_has_lines`, `server_synced` describe them). Each set of lyrics to show goes to
/// `shown` as soon as it wins: a finer one replaces what is shown, and when the server had nothing and
/// nobody found anything, an empty set from the server goes out at the end so the page can say so.
/// `evict` drops cache entries by the start of their key (the old LRC entries).
pub async fn lookup(
    transport: &dyn Transport,
    cache: &dyn LyricsCache,
    song: &Song,
    server_has_lines: bool,
    server_synced: bool,
    lookup: &LyricsLookup,
    shown: &dyn LyricsShown,
    evict: &(dyn Fn(&str) + Sync),
) {
    let none = || LyricsPick { lyrics: Lyrics::default(), origin: LyricsOrigin::Server };
    let provider = song.is_external || song.id.starts_with("ext-");
    let mut services: Vec<LyricsService> = Vec::new();
    for s in &lookup.services {
        if !services.contains(s) {
            services.push(*s);
        }
    }
    if services.is_empty() || provider || (server_has_lines && server_synced) {
        if !server_has_lines {
            shown.show(none());
        }
        return;
    }
    OLD_DROPPED.call_once(|| evict("lrclib2|"));
    let keys: Vec<String> = services.iter().map(|s| cache_key(*s, song, lookup)).collect();
    // The server's untimed words count as found: a service's untimed words do not replace them.
    let mut race = Race::new(services.iter().map(|s| s.best()).collect(), lookup.prefer_words, u8::from(server_has_lines));
    let now = Instant::now();
    for rank in 0..services.len() {
        if !race.worth(rank) {
            continue;
        }
        match remembered(cache, &keys[rank], song) {
            Remembered::Hit(l) => race.answer(rank, Some(l)),
            Remembered::Miss => race.answer(rank, None),
            Remembered::Unknown if FAILURES.lock().resting(services[rank], &keys[rank], now) => race.answer(rank, None),
            Remembered::Unknown => {}
        }
    }
    let shared = Shared::default();
    let mut out = FuturesUnordered::new();
    loop {
        for rank in race.next(out.len(), AT_ONCE) {
            let (service, key, shared) = (services[rank], keys[rank].as_str(), &shared);
            out.push(async move { (rank, ask(transport, cache, lookup, shared, service, key, song).await) });
        }
        let last = race.settled() || out.is_empty();
        if let Some((rank, lyrics)) = race.to_show(last) {
            shown.show(LyricsPick { lyrics, origin: services[rank].origin() });
        }
        if last {
            break;
        }
        let Some((rank, lyrics)) = out.next().await else { break };
        race.answer(rank, lyrics);
    }
    // Whatever is still out can no longer win; dropping it cancels its requests.
    drop(out);
    if race.leader_timing() == 0 {
        shown.show(none());
    }
}

/// Asks one service and remembers its answer (see [`remembered`]); a failure is remembered only in
/// memory, for a while, so the song is not asked about again straight away.
async fn ask(transport: &dyn Transport, cache: &dyn LyricsCache, lookup: &LyricsLookup, shared: &Shared, service: LyricsService, key: &str, song: &Song) -> Option<Lyrics> {
    let a = Ask::new(transport, lookup, shared, service);
    match services::ask(service, &a, song).await {
        Lookup::Found(l) if plausible(&l, song) => {
            FAILURES.lock().answered(service);
            cache.put(key, to_cache(&l).into_bytes());
            Some(l)
        }
        // Words that cannot be this song's (a fragment, lines past its end) are another song's: a miss.
        Lookup::Found(_) => {
            nori_model::alog::info(&format!("{} lyrics do not fit the song: taken as a miss", service.name()));
            FAILURES.lock().answered(service);
            cache.put(key, Vec::new());
            None
        }
        Lookup::Missing => {
            FAILURES.lock().answered(service);
            cache.put(key, Vec::new());
            None
        }
        Lookup::Failed => {
            FAILURES.lock().failed(service, key.to_string(), Instant::now());
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::tests::{block, song, Web};
    use nori_model::LyricLine;
    use serde_json::json;
    use std::collections::HashMap;

    fn lyrics(timing: u8) -> Lyrics {
        let line = LyricLine { start_ms: if timing >= 2 { 1000 } else { -1 }, text: "x".into(), ..Default::default() };
        Lyrics { synced: timing >= 2, word_timed: timing == 3, lines: vec![line], ..Default::default() }
    }

    #[test]
    fn finer_timing_wins_and_rank_decides_between_equals() {
        let mut r = Race::new(vec![3, 3, 2], true, 0);
        assert_eq!(r.next(0, 6), [0, 1, 2]);
        r.answer(1, Some(lyrics(2)));
        assert_eq!(r.to_show(false).map(|x| x.0), Some(1), "timed by line: worth showing at once");
        r.answer(2, Some(lyrics(2)));
        assert!(!r.settled(), "rank 0 may still time the words");
        r.answer(0, Some(lyrics(3)));
        assert!(r.settled());
        assert_eq!(r.to_show(true).map(|x| x.0), Some(0));
    }

    #[test]
    fn without_preferring_words_rank_decides_between_timed_answers() {
        let mut r = Race::new(vec![3, 3], false, 0);
        r.next(0, 6);
        r.answer(0, Some(lyrics(2)));
        assert!(r.settled(), "nobody below the top can beat a timed answer from it");
        assert_eq!(r.to_show(true).map(|x| x.0), Some(0));
    }

    #[test]
    fn untimed_services_wait_for_the_timed_ones_and_the_servers_words_rank_above_them() {
        let mut r = Race::new(vec![3, 1], true, 0);
        assert_eq!(r.next(0, 6), [0], "Genius waits");
        r.answer(0, None);
        assert_eq!(r.next(0, 6), [1]);
        let mut s = Race::new(vec![3, 1], true, 1);
        assert_eq!(s.next(0, 6), [0], "the server's own untimed words: an untimed service cannot beat them");
        s.answer(0, None);
        assert!(s.next(0, 6).is_empty() && s.settled());
    }

    #[test]
    fn no_more_than_the_pool_is_out_and_a_swap_between_equals_waits() {
        let mut r = Race::new(vec![3; 8], true, 0);
        assert_eq!(r.next(0, 6).len(), 6);
        assert!(r.next(6, 6).is_empty());
        r.answer(4, Some(lyrics(3)));
        assert_eq!(r.to_show(false).map(|x| x.0), Some(4));
        r.answer(2, Some(lyrics(3)));
        assert_eq!(r.to_show(false), None, "timed alike: no swap until the end");
        assert!(r.next(4, 6).is_empty(), "ranks past the leader can no longer win");
    }

    /// The response cache, in memory.
    #[derive(Default)]
    struct Kept(Mutex<HashMap<String, Vec<u8>>>);

    impl LyricsCache for Kept {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.0.lock().get(key).cloned()
        }
        fn fresh(&self, key: &str, _max_age_ms: i64) -> bool {
            self.0.lock().contains_key(key)
        }
        fn put(&self, key: &str, body: Vec<u8>) {
            self.0.lock().insert(key.to_string(), body);
        }
    }

    #[derive(Default)]
    struct Screen(Mutex<Vec<LyricsPick>>);

    impl LyricsShown for Screen {
        fn show(&self, pick: LyricsPick) {
            self.0.lock().push(pick);
        }
    }

    fn asked(services: &[LyricsService]) -> LyricsLookup {
        LyricsLookup { services: services.to_vec(), prefer_words: true, paxsenix_key: String::new(), better_lyrics_key: String::new() }
    }

    fn run(web: &Web, cache: &Kept, s: &Song, server: (bool, bool), l: &LyricsLookup) -> Vec<LyricsPick> {
        let screen = Screen::default();
        block(lookup(web, cache, s, server.0, server.1, l, &screen, &|_| {}));
        screen.0.into_inner()
    }

    const VERSE: [&str; 6] = [
        "Paper boats drift down the harbour",
        "Lanterns burning low tonight",
        "Every wave that takes you farther",
        "Brings the morning into sight",
        "Hold on, hold on to the water",
        "Hold on, hold on to the light",
    ];

    /// `lines` as LRC across the song, a line every ten seconds; `words` times each word inline.
    fn lrc(lines: &[&str], words: bool) -> String {
        let at = |ms: usize| format!("{:02}:{:02}.{:02}", ms / 60_000, ms / 1000 % 60, ms % 1000 / 10);
        let mut out = String::new();
        for (i, line) in lines.iter().cycle().take(18).enumerate() {
            let ms = 10_000 + i * 10_000;
            out.push_str(&format!("[{}]", at(ms)));
            if words {
                for (k, w) in line.split(' ').enumerate() {
                    out.push_str(&format!("<{}>{w} ", at(ms + k * 400)));
                }
            } else {
                out.push_str(line);
            }
            out.push('\n');
        }
        out
    }

    fn lrclib_synced(web: &Web, body: &str) {
        web.answer("https://lrclib.net/api/get", 200, &json!({"syncedLyrics": body}).to_string());
    }

    fn unison(web: &Web, body: &str) {
        web.answer("https://unison.boidu.dev/", 200, &json!({"success": true, "data": {"lyrics": body, "format": "lrc", "duration": 239}}).to_string());
    }

    #[test]
    fn a_song_is_asked_once_and_remembered_with_every_word() {
        let (web, cache) = (Web::default(), Kept::default());
        unison(&web, &lrc(&VERSE, true));
        web.answer("https://lrclib.net/", 404, r#"{"statusCode":404}"#);
        let s = Song { title: "Remembered".into(), ..song() };
        let l = asked(&[LyricsService::Unison, LyricsService::Lrclib]);
        let first = run(&web, &cache, &s, (false, false), &l);
        assert_eq!(first.len(), 1);
        assert!(first[0].lyrics.word_timed && first[0].origin == LyricsOrigin::Unison);
        let asked_first = web.asked().len();
        let again = run(&web, &cache, &s, (false, false), &l);
        assert_eq!(again, first, "the same answer");
        assert_eq!(web.asked().len(), asked_first, "and nothing asked again: it came out of the cache");
    }

    #[test]
    fn nothing_is_asked_for_synced_server_lyrics_a_provider_song_or_with_no_service() {
        let (web, cache) = (Web::default(), Kept::default());
        let l = asked(&[LyricsService::Unison]);
        assert!(run(&web, &cache, &song(), (true, true), &l).is_empty());
        let ext = Song { id: "ext-deezer-1".into(), ..song() };
        assert_eq!(run(&web, &cache, &ext, (false, false), &l), [LyricsPick { lyrics: Lyrics::default(), origin: LyricsOrigin::Server }]);
        assert_eq!(run(&web, &cache, &song(), (false, false), &asked(&[])).len(), 1, "nothing asked: the empty answer, once");
        assert!(web.asked().is_empty());
    }

    #[test]
    fn a_failure_is_not_kept_but_not_asked_again_at_once() {
        let (web, cache) = (Web::default(), Kept::default());
        let s = Song { title: "Failing".into(), ..song() };
        let l = asked(&[LyricsService::Unison]);
        web.answer("https://unison.boidu.dev/", 503, "busy");
        assert_eq!(run(&web, &cache, &s, (false, false), &l)[0].origin, LyricsOrigin::Server);
        assert!(cache.0.lock().is_empty(), "a failure is never an answer");
        run(&web, &cache, &s, (false, false), &l);
        assert_eq!(web.asked().len(), 1, "the same song is not asked again straight away");
    }

    #[test]
    fn line_timed_words_show_first_and_word_timed_ones_replace_them() {
        let (web, cache) = (Web::default(), Kept::default());
        unison(&web, &lrc(&VERSE, true));
        lrclib_synced(&web, &lrc(&VERSE, false));
        let s = Song { title: "Both".into(), ..song() };
        // LRCLIB first in rank, Unison below it: LRCLIB's lines are shown, then Unison's words win.
        let picks = run(&web, &cache, &s, (true, false), &asked(&[LyricsService::Lrclib, LyricsService::Unison]));
        let origins: Vec<LyricsOrigin> = picks.iter().map(|p| p.origin).collect();
        assert_eq!(*origins.last().unwrap(), LyricsOrigin::Unison);
        assert!(picks.last().unwrap().lyrics.word_timed);
    }

    /// Three lines of another song, over and over: what took the place of the right lyrics.
    const OTHER: [&str; 3] = ["Pour another glass for me", "The whisky's on the table", "Drink until the morning comes"];

    #[test]
    fn another_songs_words_never_replace_the_lyrics_shown() {
        let (web, cache) = (Web::default(), Kept::default());
        // LRCLIB's lines are found first; Unison, below it, times words - but of another song.
        lrclib_synced(&web, &lrc(&VERSE, false));
        unison(&web, &lrc(&OTHER, true));
        let s = Song { title: "Stable".into(), ..song() };
        let picks = run(&web, &cache, &s, (false, false), &asked(&[LyricsService::Lrclib, LyricsService::Unison]));
        assert_eq!(picks.len(), 1, "shown once: {picks:?}");
        assert_eq!((picks[0].origin, picks[0].lyrics.lines[0].text.as_str()), (LyricsOrigin::Lrclib, VERSE[0]));
    }

    #[test]
    fn a_fragment_is_a_miss_and_one_kept_before_the_check_is_not_shown() {
        let (web, cache) = (Web::default(), Kept::default());
        let s = Song { title: "Fragment".into(), ..song() };
        let only = asked(&[LyricsService::Unison]);
        unison(&web, "[00:05.00]<00:05.00>Pour <00:05.50>another\n[00:09.00]<00:09.00>The <00:09.50>whisky\n[00:13.00]Drink\n[00:17.00]<00:17.00>Pour <00:17.50>another\n");
        let picks = run(&web, &cache, &s, (false, false), &only);
        assert_eq!(picks, [LyricsPick { lyrics: Lyrics::default(), origin: LyricsOrigin::Server }], "nothing found");
        assert_eq!(cache.0.lock().values().next(), Some(&Vec::new()), "kept as a miss");
        // The same fragment, kept for good by a build before the check: not shown either.
        let (web, cache) = (Web::default(), Kept::default());
        let junk = crate::lyrics::from_lrc("[00:05.00]Pour another\n[00:09.00]The whisky\n[00:13.00]Pour another\n");
        cache.put(&cache_key(LyricsService::Unison, &s, &only), to_cache(&junk).into_bytes());
        assert_eq!(run(&web, &cache, &s, (false, false), &only)[0].origin, LyricsOrigin::Server);
        assert!(web.asked().is_empty(), "a miss for the week, not asked again");
    }

    #[test]
    fn lyrics_on_screen_are_not_swapped_for_an_answer_timed_alike() {
        let (web, cache) = (Web::default(), Kept::default());
        let s = Song { title: "Alike".into(), ..song() };
        // Unison's lines (rank 1) come out of the cache and are shown at once; LRCLIB's (rank 0), timed
        // the same, arrive later: what is on screen stays.
        let l = asked(&[LyricsService::Lrclib, LyricsService::Unison]);
        let mut lines = crate::lyrics::from_lrc(&lrc(&VERSE, false));
        lines.lines[0].text = "Paper boats drift down the harbor".into();
        cache.put(&cache_key(LyricsService::Unison, &s, &l), to_cache(&lines).into_bytes());
        lrclib_synced(&web, &lrc(&VERSE, false));
        let picks = run(&web, &cache, &s, (false, false), &l);
        assert_eq!(picks.iter().map(|p| p.origin).collect::<Vec<_>>(), [LyricsOrigin::Unison]);
    }
}
