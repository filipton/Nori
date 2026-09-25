//! Lyrics from the services a lookup asks (the settings' `lyrics_lookup`), for a song the server has no
//! timed lyrics for, each answer scored (trust.rs) and the best shown.
//!
//! The lyrics chosen for a song last time are read first, with their score and source: they are shown
//! at once and nothing is asked, unless they scored low, in which case the services are asked again
//! after a few days ([`LOW_RETRY_MS`]) for something better. Otherwise what each service answered before
//! is read (a hit kept for good, a miss asked again after a week), and the rest are asked in waves:
//! first the cheap and good ones (`LyricsService::first_wave`), together; the others that are on only
//! when the first wave missed or its best scored low ([`WIDEN_BELOW`]); services with untimed words
//! only when nobody timed anything. [`Race`] scores every answer against the song and against each
//! other, and decides what is shown: the best, at once when it is sure, otherwise when the first wave
//! is in; and what is shown is replaced only by an answer strictly better by score that agrees with it.
//! A failed or too slow service is never remembered as a miss, but it is not asked about the same song
//! again for a while, and one that keeps failing rests for a while altogether. Provider (`ext-`) songs
//! are never looked up. Nothing here runs a thread: the lookup is one future, polled by whoever asked.

use std::time::{Duration, Instant};

use futures_util::stream::{FuturesUnordered, StreamExt};
use nori_model::{Lyrics, Song};
use nori_net::transport::Transport;
use nori_settings::lyrics_sources::{LyricsLookup, LyricsService};
use nori_words::words::LyricsOrigin;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

use crate::credits::strip_edges;
use crate::fit::{agree, plausible};
use crate::formats::{from_cache, timing};
use crate::services::{self, Ask, Lookup, Shared};
use crate::trust::{score, Named, Trust};

/// How many services are asked at once.
pub const AT_ONCE: usize = 6;
/// A miss is asked again after a week: someone may have added the song since.
pub const MISS_KEPT_MS: i64 = 7 * 24 * 3_600_000;
/// An answer this sure is shown as soon as it arrives, when there is evidence beyond its service's word
/// (it names the song, or another answer agrees); otherwise it waits for the first wave.
pub const SURE: f64 = 0.75;
/// The first wave's best must score at least this for the other services not to be asked: a lone
/// word-timed answer naming the song passes, a lone line-timed one or one naming nothing does not, and
/// line-timed lyrics another service agrees with do.
pub const WIDEN_BELOW: f64 = 0.82;
/// Never shown below this: more likely another song's words than this one's.
pub const FLOOR: f64 = 0.4;
/// What is shown is replaced by an answer that agrees with it and scores at least this much more...
pub const MARGIN: f64 = 0.03;
/// ...or by one that does not agree and scores this much more (what is shown was another song's).
pub const OVERRULE: f64 = 0.2;
/// Lyrics chosen with a score below this are asked about again after [`LOW_RETRY_MS`]; at or above it
/// they are kept for good.
pub const LOW: f64 = 0.7;
pub const LOW_RETRY_MS: i64 = 3 * 24 * 3_600_000;
/// The user's ranking breaks near ties: the first service gets this much, the last nothing.
const RANK_BONUS: f64 = 0.03;
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

/// Every lookup's cache entries start with this: what "Clear lyrics cache" empties.
pub const CACHE_PREFIX: &str = "lyrics|";

/// One service in a race: the finest timing it can answer with (3 word by word, 2 line by line, 1 not
/// timed), how far it is trusted, and whether it is in the first wave.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry {
    pub best: u8,
    pub prior: f64,
    pub first_wave: bool,
}

impl Entry {
    pub fn of(s: LyricsService) -> Self {
        Entry { best: s.best(), prior: s.prior(), first_wave: s.first_wave() }
    }
}

/// Which answer is shown, as answers come in from services asked together. Services are ranked by their
/// position (0 is the best), which only breaks near ties; the score (trust.rs) decides. The server's own
/// untimed words (`server_timing` 1) are never replaced by a service's untimed words.
pub struct Race {
    song: Song,
    entries: Vec<Entry>,
    prefer_words: bool,
    server_timing: u8,
    done: Vec<bool>,
    waiting: Vec<usize>,
    answers: Vec<Option<(Lyrics, Named)>>,
    /// What is on screen: its rank.
    shown: Option<usize>,
}

impl Race {
    pub fn new(song: &Song, entries: Vec<Entry>, prefer_words: bool, server_timing: u8) -> Self {
        let n = entries.len();
        Race { song: song.clone(), entries, prefer_words, server_timing, done: vec![false; n], waiting: (0..n).collect(), answers: vec![None; n], shown: None }
    }

    /// Every answer's score, none for a rank without one or whose timing is no better than the server's.
    pub fn scores(&self) -> Vec<Option<Trust>> {
        let n = self.entries.len().max(1) as f64;
        (0..self.entries.len())
            .map(|r| {
                let (l, named) = self.answers[r].as_ref()?;
                if timing(l) <= self.server_timing {
                    return None;
                }
                let others: Vec<(&Lyrics, &Named)> = self.answers.iter().enumerate().filter(|(o, _)| *o != r).filter_map(|(_, a)| a.as_ref().map(|a| (&a.0, &a.1))).collect();
                let mut t = score(&self.song, l, named, self.entries[r].prior, &others, self.prefer_words);
                t.score = (t.score + RANK_BONUS * (1.0 - r as f64 / n)).min(1.0);
                Some(t)
            })
            .collect()
    }

    /// The best answer that clears the [`FLOOR`], by score and then by rank.
    fn leader_in(scores: &[Option<Trust>]) -> Option<(usize, f64)> {
        scores.iter().enumerate().filter_map(|(r, t)| t.map(|t| (r, t.score))).filter(|(_, s)| *s >= FLOOR).fold(None, |best, (r, s)| match best {
            Some((_, b)) if b >= s => best,
            _ => Some((r, s)),
        })
    }

    /// The best answer so far and its score.
    pub fn leader(&self) -> Option<(usize, f64)> {
        Self::leader_in(&self.scores())
    }

    fn first_wave_done(&self) -> bool {
        (0..self.entries.len()).all(|r| self.done[r] || !self.entries[r].first_wave || self.entries[r].best <= self.server_timing)
    }

    fn timed_done(&self) -> bool {
        (0..self.entries.len()).all(|r| self.done[r] || self.entries[r].best < 2)
    }

    /// `rank` answered, with lyrics and what it named, or with nothing (a miss, a failure, too slow).
    pub fn answer(&mut self, rank: usize, found: Option<(Lyrics, Named)>) {
        if self.done[rank] {
            return;
        }
        self.done[rank] = true;
        self.waiting.retain(|w| *w != rank);
        self.answers[rank] = found.filter(|(l, _)| !l.lines.is_empty());
    }

    /// Lyrics already on screen as `rank`'s answer: the ones chosen last time.
    pub fn shown_already(&mut self, rank: usize, lyrics: Lyrics, named: Named) {
        self.answer(rank, Some((lyrics, named)));
        self.shown = Some(rank);
    }

    /// Whether `rank`, not asked yet, may be asked now.
    fn may_ask(&self, rank: usize, leader: Option<(usize, f64)>) -> bool {
        let e = self.entries[rank];
        if e.best <= self.server_timing {
            return false;
        }
        if e.best < 2 {
            return self.timed_done() && leader.is_none();
        }
        e.first_wave || (self.first_wave_done() && leader.is_none_or(|(_, s)| s < WIDEN_BELOW))
    }

    /// Whether `rank`, not asked yet, can ever be asked: a service whose timing is no better than the
    /// server's never is, nor one past a wave the answers in hand made unnecessary.
    fn hopeless(&self, rank: usize, leader: Option<(usize, f64)>) -> bool {
        let e = self.entries[rank];
        e.best <= self.server_timing
            || (e.best >= 2 && !e.first_wave && self.first_wave_done() && leader.is_some_and(|(_, s)| s >= WIDEN_BELOW))
            || (e.best < 2 && self.timed_done() && leader.is_some())
    }

    /// The ranks to start asking now, best first, so that no more than `pool` are out at once. Those
    /// that can never be asked are dropped unasked.
    pub fn next(&mut self, running: usize, pool: usize) -> Vec<usize> {
        let leader = self.leader();
        let hopeless: Vec<usize> = self.waiting.iter().copied().filter(|r| self.hopeless(*r, leader)).collect();
        for r in hopeless {
            self.answer(r, None);
        }
        let start: Vec<usize> = self.waiting.iter().copied().filter(|r| self.may_ask(*r, leader)).take(pool.saturating_sub(running)).collect();
        self.waiting.retain(|w| !start.contains(w));
        start
    }

    /// What to put on screen now, if anything. Nothing shown yet: the leader, once it is sure, or the
    /// first wave is in, or at the `last`. Something shown: the leader only when it is strictly better by
    /// score and the same song's words, or far better (what is shown was another song's).
    pub fn to_show(&mut self, last: bool) -> Option<(usize, Lyrics)> {
        let scores = self.scores();
        let (leader, best) = Self::leader_in(&scores)?;
        let take = match self.shown {
            Some(r) if r == leader => false,
            Some(r) => {
                let had = scores[r].map_or(0.0, |t| t.score);
                let same = self.answers[r].as_ref().zip(self.answers[leader].as_ref()).is_some_and(|(a, b)| agree(&a.0, &b.0));
                (best > had + MARGIN && same) || best > had + OVERRULE
            }
            None => (best >= SURE && self.evidenced(leader)) || (self.first_wave_done() && best >= WIDEN_BELOW) || last,
        };
        if !take {
            return None;
        }
        self.shown = Some(leader);
        self.answers[leader].as_ref().map(|a| (leader, a.0.clone()))
    }

    /// Whether `rank`'s answer is backed by more than its service's word: the title it named is this
    /// song's, or another answer has the same words.
    fn evidenced(&self, rank: usize) -> bool {
        let Some((l, named)) = self.answers[rank].as_ref() else { return false };
        named.title.as_deref().is_some_and(|t| crate::trust::name_alike(t, &self.song.title) >= 0.85)
            || self.answers.iter().enumerate().any(|(o, a)| o != rank && a.as_ref().is_some_and(|a| agree(l, &a.0)))
    }

    /// What is on screen, with its score and what its service named.
    pub fn chosen(&self) -> Option<(usize, Trust, &Lyrics, &Named)> {
        let r = self.shown?;
        let t = self.scores()[r]?;
        let (l, n) = self.answers[r].as_ref()?;
        Some((r, t, l, n))
    }

    /// The best of the rest, by score.
    pub fn runner_up(&self) -> Option<(usize, f64)> {
        let shown = self.shown;
        self.scores().iter().enumerate().filter(|(r, _)| Some(*r) != shown).filter_map(|(r, t)| t.map(|t| (r, t.score))).fold(None, |best, (r, s)| match best {
            Some((_, b)) if b >= s => best,
            _ => Some((r, s)),
        })
    }
}

/// How an answer's timing is said in the log.
pub fn timing_words(l: &Lyrics) -> &'static str {
    match timing(l) {
        3 => "word-timed",
        2 => "line-timed",
        1 => "not timed",
        _ => "empty",
    }
}

/// What a service's answers about a song are remembered under. BetterLyrics answers more with a key
/// than without, so a song it had nothing for without one is asked again once a key is given.
fn cache_key(service: LyricsService, song: &Song, lookup: &LyricsLookup) -> String {
    let keyed = matches!(service, LyricsService::BetterLyrics | LyricsService::Portato) && !lookup.better_lyrics_key.is_empty();
    format!("lyrics|{}{}|{}|{}|{}", service.name(), if keyed { "+key" } else { "" }, song.artist, song.title, song.duration)
}

/// What the lyrics chosen for a song are remembered under.
fn best_key(song: &Song) -> String {
    format!("{CACHE_PREFIX}BEST|{}|{}|{}", song.artist, song.title, song.duration)
}

/// A service's answer as it is kept: the words and what the service named, after this mark. Entries
/// kept before (the words alone, formats.rs's cache form) still read, naming nothing.
const ANSWER_MARK: &str = "nori-answer1:";

#[derive(Serialize, Deserialize)]
struct Kept {
    lyrics: Lyrics,
    #[serde(default)]
    named: Named,
    /// The chosen lyrics only: whose they are and how they scored.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    score: Option<f64>,
}

fn kept(lyrics: &Lyrics, named: &Named, chosen: Option<(LyricsService, f64)>) -> Vec<u8> {
    let k = Kept { lyrics: lyrics.clone(), named: named.clone(), source: chosen.map(|c| c.0.name().to_string()), score: chosen.map(|c| c.1) };
    format!("{ANSWER_MARK}{}", serde_json::to_string(&k).unwrap_or_default()).into_bytes()
}

fn read_kept(body: &[u8]) -> Option<Kept> {
    let text = String::from_utf8_lossy(body);
    match text.strip_prefix(ANSWER_MARK) {
        Some(json) => serde_json::from_str(json).ok(),
        None => Some(Kept { lyrics: from_cache(&text), named: Named::default(), source: None, score: None }),
    }
}

/// What a service answered before.
enum Remembered {
    Hit(Lyrics, Named),
    Miss,
    Unknown,
}

fn remembered(cache: &dyn LyricsCache, key: &str, song: &Song) -> Remembered {
    match cache.get(key) {
        // An entry that no longer reads falls through to asking again; one kept before answers were
        // checked, which cannot be this song's, is the miss it should have been. Credits are stripped
        // again, for entries kept before they were.
        Some(b) if !b.is_empty() => match read_kept(&b).map(|mut k| {
            strip_edges(&mut k.lyrics, &song.title, &song.artist);
            k
        }) {
            Some(k) if k.lyrics.lines.is_empty() => Remembered::Unknown,
            Some(k) if plausible(&k.lyrics, song) => Remembered::Hit(k.lyrics, k.named),
            Some(_) => Remembered::Miss,
            None => Remembered::Unknown,
        },
        Some(_) if cache.fresh(key, MISS_KEPT_MS) => Remembered::Miss,
        _ => Remembered::Unknown,
    }
}

/// The lyrics chosen for `song` last time, if their service is still asked: its rank, the words, what it
/// named, and the score.
fn chosen_before(cache: &dyn LyricsCache, song: &Song, services: &[LyricsService]) -> Option<(usize, Lyrics, Named, f64)> {
    let k = read_kept(&cache.get(&best_key(song))?)?;
    let rank = services.iter().position(|s| Some(s.name()) == k.source.as_deref())?;
    plausible(&k.lyrics, song).then(|| (rank, k.lyrics, k.named, k.score.unwrap_or(0.0)))
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
/// `shown` as it is chosen, and when the server had nothing and nobody found anything, an empty set from
/// the server goes out at the end so the page can say so. `evict` drops cache entries by the start of
/// their key (the old LRC entries). Returns what was chosen, in the log's words
/// ("chose BiniLyrics (0.91, word-timed), runner-up LRCLIB (0.84)"), when anything was.
#[allow(clippy::too_many_arguments)]
pub async fn lookup(
    transport: &dyn Transport,
    cache: &dyn LyricsCache,
    song: &Song,
    server_has_lines: bool,
    server_synced: bool,
    lookup: &LyricsLookup,
    shown: &dyn LyricsShown,
    evict: &(dyn Fn(&str) + Sync),
) -> Option<String> {
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
        return None;
    }
    OLD_DROPPED.call_once(|| evict("lrclib2|"));
    let server_timing = u8::from(server_has_lines);
    let mut race = Race::new(song, services.iter().map(|s| Entry::of(*s)).collect(), lookup.prefer_words, server_timing);
    // The lyrics chosen last time: shown at once, and final unless they scored low and a few days passed.
    let mut before: Option<(usize, f64)> = None;
    if let Some((rank, lyrics, named, was)) = chosen_before(cache, song, &services).filter(|c| timing(&c.1) > server_timing) {
        shown.show(LyricsPick { lyrics: lyrics.clone(), origin: services[rank].origin() });
        let line = format!("lyrics: kept {} ({was:.2}, {})", services[rank].title(), timing_words(&lyrics));
        if was >= LOW || cache.fresh(&best_key(song), LOW_RETRY_MS) {
            nori_model::alog::info(&line);
            return Some(line);
        }
        race.shown_already(rank, lyrics, named);
        before = Some((rank, was));
    }
    let keys: Vec<String> = services.iter().map(|s| cache_key(*s, song, lookup)).collect();
    let now = Instant::now();
    for rank in 0..services.len() {
        if race.done[rank] || race.entries[rank].best <= server_timing {
            continue;
        }
        match remembered(cache, &keys[rank], song) {
            Remembered::Hit(l, n) => race.answer(rank, Some((l, n))),
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
        let last = out.is_empty();
        if let Some((rank, lyrics)) = race.to_show(last) {
            shown.show(LyricsPick { lyrics, origin: services[rank].origin() });
        }
        if last {
            break;
        }
        let Some((rank, answer)) = out.next().await else { break };
        race.answer(rank, answer);
    }
    drop(out);
    let Some((rank, trust, lyrics, named)) = race.chosen() else {
        if !server_has_lines {
            shown.show(none());
        }
        return None;
    };
    // What is on screen is what is kept, with its score, and served from here next time.
    cache.put(&best_key(song), kept(lyrics, named, Some((services[rank], trust.score))));
    let mut line = format!("lyrics: chose {} ({:.2}, {})", services[rank].title(), trust.score, timing_words(lyrics));
    if let Some((r, s)) = race.runner_up() {
        line.push_str(&format!(", runner-up {} ({s:.2})", services[r].title()));
    }
    if let Some((r, was)) = before {
        line.push_str(&format!(", was {} ({was:.2})", services[r].title()));
    }
    nori_model::alog::info(&line);
    Some(line)
}

/// Asks one service and remembers its answer (see [`remembered`]), its credits stripped first; a
/// failure is remembered only in memory, for a while, so the song is not asked about again straight away.
async fn ask(transport: &dyn Transport, cache: &dyn LyricsCache, lookup: &LyricsLookup, shared: &Shared, service: LyricsService, key: &str, song: &Song) -> Option<(Lyrics, Named)> {
    let a = Ask::new(transport, lookup, shared, service);
    match services::ask(service, &a, song).await {
        Lookup::Found(mut l, named) => {
            FAILURES.lock().answered(service);
            strip_edges(&mut l, &song.title, &song.artist);
            if l.lines.is_empty() || !plausible(&l, song) {
                // Words that cannot be this song's (a fragment, lines past its end, only credits) are
                // another song's: a miss.
                nori_model::alog::info(&format!("{} lyrics do not fit the song: taken as a miss", service.name()));
                cache.put(key, Vec::new());
                return None;
            }
            cache.put(key, kept(&l, &named, None));
            Some((l, named))
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
    use crate::fit::tests::timed;
    use crate::formats::to_cache;
    use crate::services::tests::{block, song, Web};
    use serde_json::json;
    use std::collections::{HashMap, HashSet};

    // ---- the race, answer by answer ----------------------------------------------------------------------

    /// Six invented lines, and six others: two songs' words.
    const OURS: [&str; 6] = ["first line of the song", "line two goes here", "a third one follows", "and then the fourth", "the chorus comes around", "and the chorus ends"];
    const THEIRS: [&str; 6] = ["something else entirely", "nothing alike at all", "words of another tune", "a verse we never heard", "somebody else's chorus", "and a different ending"];

    fn tune() -> Song {
        Song { title: "Glass Harbour".into(), artist: "The Lanterns".into(), album: "Low Tide".into(), duration: 180, ..Default::default() }
    }

    /// `lines` over the song, a line every nine seconds from ten, word-timed or line-timed.
    fn words(lines: &[&str], word_timed: bool) -> Lyrics {
        let at: Vec<(i64, &str)> = (0..18).map(|i| (10_000 + i * 9_000, lines[i as usize % lines.len()])).collect();
        timed(&at, word_timed)
    }

    fn naming(title: &str) -> Named {
        Named::new(title, "The Lanterns", "", 180.0)
    }

    fn entry(prior: f64, first_wave: bool, best: u8) -> Entry {
        Entry { best, prior, first_wave }
    }

    #[test]
    fn a_wrong_song_from_a_loose_source_is_not_chosen() {
        // A service that matches loosely and names nothing answers first, word by word, with another song;
        // LRCLIB (line by line, naming the song) and Unison (word by word) have this one's words.
        let mut r = Race::new(&tune(), vec![entry(0.9, true, 3), entry(0.85, true, 3), entry(0.85, true, 3)], true, 0);
        assert_eq!(r.next(0, 6), [0, 1, 2]);
        r.answer(0, Some((words(&THEIRS, true), Named::default())));
        assert_eq!(r.to_show(false), None, "a lone answer naming nothing waits for the first wave");
        r.answer(1, Some((words(&OURS, false), naming("Glass Harbour"))));
        r.answer(2, Some((words(&OURS, true), Named::default())));
        let scores = r.scores();
        assert!(scores[0].unwrap().score + OVERRULE < scores[2].unwrap().score, "outvoted: {:?}", scores[0]);
        let (rank, _) = r.to_show(false).expect("shown");
        assert_eq!(rank, 2, "the word-timed answer the others agree with");
        assert!(r.runner_up().is_some_and(|(r, _)| r == 1));
    }

    #[test]
    fn a_line_timed_answer_naming_the_song_beats_word_timed_words_nobody_backs() {
        let mut r = Race::new(&tune(), vec![entry(0.85, true, 3), entry(0.85, true, 3)], true, 0);
        r.next(0, 6);
        r.answer(0, Some((words(&OURS, false), naming("Glass Harbour"))));
        r.answer(1, Some((words(&THEIRS, true), Named::default())));
        assert_eq!(r.to_show(true).map(|x| x.0), Some(0), "the answer naming the song, though timed by line");
        // A third answer with the named one's words, timed word by word, then wins over both.
        let mut three = Race::new(&tune(), vec![entry(0.85, true, 3), entry(0.85, true, 3), entry(0.9, true, 3)], true, 0);
        three.next(0, 6);
        three.answer(0, Some((words(&OURS, false), naming("Glass Harbour"))));
        three.answer(1, Some((words(&THEIRS, true), Named::default())));
        three.answer(2, Some((words(&OURS, true), naming("Glass Harbour"))));
        assert_eq!(three.to_show(true).map(|x| x.0), Some(2));
    }

    #[test]
    fn a_junk_answer_loses_to_clean_lyrics() {
        // Word-timed, but one line over and over with a credit and a placeholder left in the middle.
        let junk: Vec<(i64, &str)> = (0..18).map(|i| (10_000 + i * 9_000, match i {
            6 => "Lyrics by: Some Person",
            12 => "[Instrumental]",
            _ => "la la la",
        })).collect();
        let mut r = Race::new(&tune(), vec![entry(0.9, true, 3), entry(0.85, true, 3)], true, 0);
        r.next(0, 6);
        r.answer(0, Some((timed(&junk, true), naming("Glass Harbour"))));
        r.answer(1, Some((words(&OURS, false), naming("Glass Harbour"))));
        let s = r.scores();
        assert!(s[0].unwrap().penalty > 0.2, "{:?}", s[0]);
        assert_eq!(r.to_show(true).map(|x| x.0), Some(1));
    }

    #[test]
    fn the_second_wave_is_asked_only_when_the_first_misses_or_scores_low() {
        let entries = vec![entry(0.95, true, 3), entry(0.85, true, 2), entry(0.75, false, 3), entry(0.7, false, 1)];
        let mut good = Race::new(&tune(), entries.clone(), true, 0);
        assert_eq!(good.next(0, 6), [0, 1], "the first wave only");
        good.answer(0, Some((words(&OURS, true), naming("Glass Harbour"))));
        good.answer(1, Some((words(&OURS, false), naming("Glass Harbour"))));
        assert!(good.leader().unwrap().1 >= WIDEN_BELOW);
        assert!(good.next(0, 6).is_empty(), "nobody else asked");
        let mut missed = Race::new(&tune(), entries.clone(), true, 0);
        missed.next(0, 6);
        missed.answer(0, None);
        assert!(missed.next(1, 6).is_empty(), "not while the first wave is out");
        missed.answer(1, None);
        assert_eq!(missed.next(0, 6), [2], "the first wave missed: the next one, still timed");
        missed.answer(2, None);
        assert_eq!(missed.next(0, 6), [3], "untimed words last, when nobody timed anything");
        let mut low = Race::new(&tune(), entries, true, 0);
        low.next(0, 6);
        low.answer(0, None);
        low.answer(1, Some((words(&OURS, false), Named::default())));
        assert!(low.leader().unwrap().1 < WIDEN_BELOW);
        assert_eq!(low.next(0, 6), [2], "a lone line-timed answer naming nothing: widened");
    }

    #[test]
    fn whats_shown_is_replaced_only_by_a_strictly_better_answer_that_agrees() {
        let mut r = Race::new(&tune(), vec![entry(0.85, true, 3), entry(0.85, true, 3), entry(0.85, true, 3)], true, 0);
        r.next(0, 6);
        r.answer(1, Some((words(&OURS, false), naming("Glass Harbour"))));
        assert_eq!(r.to_show(false).map(|x| x.0), Some(1), "sure, and naming the song: shown at once");
        let mut alike = words(&OURS, false);
        alike.lines[0].text = "first line of this song".into();
        r.answer(0, Some((alike, naming("Glass Harbour"))));
        assert_eq!(r.to_show(false), None, "timed alike and scored alike: no swap");
        r.answer(2, Some((words(&OURS, true), naming("Glass Harbour"))));
        assert_eq!(r.to_show(false).map(|x| x.0), Some(2), "word timing of the same words: strictly better");
    }

    #[test]
    fn the_servers_own_untimed_words_are_never_replaced_by_untimed_ones() {
        let mut r = Race::new(&tune(), vec![entry(0.85, true, 3), entry(0.7, false, 1)], true, 1);
        assert_eq!(r.next(0, 6), [0]);
        r.answer(0, None);
        assert!(r.next(0, 6).is_empty(), "an untimed service cannot beat the server's words");
        let mut plain = words(&OURS, false);
        plain.synced = false;
        let mut s = Race::new(&tune(), vec![entry(0.85, true, 1)], true, 1);
        s.answer(0, Some((plain, Named::default())));
        assert_eq!(s.to_show(true), None);
    }

    // ---- the lookup, through the fake web and cache ----------------------------------------------------

    /// The response cache, in memory; keys in `old` are as old as can be.
    #[derive(Default)]
    struct Kept(Mutex<HashMap<String, Vec<u8>>>, Mutex<HashSet<String>>);

    impl LyricsCache for Kept {
        fn get(&self, key: &str) -> Option<Vec<u8>> {
            self.0.lock().get(key).cloned()
        }
        fn fresh(&self, key: &str, _max_age_ms: i64) -> bool {
            self.0.lock().contains_key(key) && !self.1.lock().contains(key)
        }
        fn put(&self, key: &str, body: Vec<u8>) {
            self.1.lock().remove(key);
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

    fn run_saying(web: &Web, cache: &Kept, s: &Song, server: (bool, bool), l: &LyricsLookup) -> (Vec<LyricsPick>, Option<String>) {
        let screen = Screen::default();
        let said = block(lookup(web, cache, s, server.0, server.1, l, &screen, &|_| {}));
        (screen.0.into_inner(), said)
    }

    fn run(web: &Web, cache: &Kept, s: &Song, server: (bool, bool), l: &LyricsLookup) -> Vec<LyricsPick> {
        run_saying(web, cache, s, server, l).0
    }

    const VERSE: [&str; 6] = OURS;

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
        web.answer("https://unison.boidu.dev/", 200, &json!({"success": true, "data": {"lyrics": body, "format": "lrc", "duration": 239, "title": "Glass Harbour", "artist": "The Lanterns"}}).to_string());
    }

    #[test]
    fn a_song_is_asked_once_and_the_choice_is_kept_with_its_score() {
        let (web, cache) = (Web::default(), Kept::default());
        unison(&web, &lrc(&VERSE, true));
        // LRCLIB has nothing: a 404 to the exact lookup and an empty search (a miss, not a failure).
        web.answer("https://lrclib.net/api/search", 200, "[]");
        web.answer("https://lrclib.net/", 404, r#"{"statusCode":404}"#);
        let s = Song { title: "Remembered".into(), ..song() };
        let l = asked(&[LyricsService::Unison, LyricsService::Lrclib]);
        let s = Song { title: "Glass Harbour".into(), id: "remembered".into(), ..s };
        let (first, said) = run_saying(&web, &cache, &s, (false, false), &l);
        assert_eq!(first.len(), 1);
        assert!(first[0].lyrics.word_timed && first[0].origin == LyricsOrigin::Unison);
        let said = said.unwrap();
        assert!(said.starts_with("lyrics: chose Unison (0.") && said.contains("word-timed"), "{said}");
        let best = read_kept(&cache.get(&best_key(&s)).unwrap()).unwrap();
        assert_eq!((best.source.as_deref(), best.lyrics.lines.len()), (Some("UNISON"), 18));
        assert!(best.score.unwrap() >= LOW);
        let asked_first = web.asked().len();
        let (again, kept) = run_saying(&web, &cache, &s, (false, false), &l);
        assert_eq!(again, first, "the same answer");
        assert!(kept.unwrap().starts_with("lyrics: kept Unison"));
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
    fn word_timed_words_win_over_line_timed_ones() {
        let (web, cache) = (Web::default(), Kept::default());
        unison(&web, &lrc(&VERSE, true));
        lrclib_synced(&web, &lrc(&VERSE, false));
        let s = Song { title: "Glass Harbour".into(), id: "both".into(), ..song() };
        let picks = run(&web, &cache, &s, (true, false), &asked(&[LyricsService::Lrclib, LyricsService::Unison]));
        assert_eq!(picks.last().unwrap().origin, LyricsOrigin::Unison);
        assert!(picks.last().unwrap().lyrics.word_timed);
    }

    #[test]
    fn another_songs_words_never_replace_the_lyrics_shown() {
        let (web, cache) = (Web::default(), Kept::default());
        // LRCLIB's lines; Unison, below it, times words - but of another song, and a fragment of one.
        lrclib_synced(&web, &lrc(&VERSE, false));
        unison(&web, &lrc(&THEIRS[..3], true));
        let s = Song { title: "Glass Harbour".into(), id: "stable".into(), ..song() };
        let picks = run(&web, &cache, &s, (false, false), &asked(&[LyricsService::Lrclib, LyricsService::Unison]));
        assert_eq!(picks.len(), 1, "shown once: {:?}", picks.iter().map(|p| p.origin).collect::<Vec<_>>());
        assert_eq!((picks[0].origin, picks[0].lyrics.lines[0].text.as_str()), (LyricsOrigin::Lrclib, VERSE[0]));
    }

    #[test]
    fn a_fragment_is_a_miss_and_one_kept_before_the_check_is_not_shown() {
        let (web, cache) = (Web::default(), Kept::default());
        let s = Song { title: "Fragment".into(), ..song() };
        let only = asked(&[LyricsService::Unison]);
        unison(&web, "[00:05.00]<00:05.00>la <00:05.50>la\n[00:09.00]<00:09.00>line <00:09.50>two\n[00:13.00]three\n[00:17.00]<00:17.00>la <00:17.50>la\n");
        let picks = run(&web, &cache, &s, (false, false), &only);
        assert_eq!(picks, [LyricsPick { lyrics: Lyrics::default(), origin: LyricsOrigin::Server }], "nothing found");
        assert_eq!(cache.0.lock().values().next(), Some(&Vec::new()), "kept as a miss");
        // The same fragment, kept for good by a build before the check: not shown either.
        let (web, cache) = (Web::default(), Kept::default());
        let junk = crate::lyrics::from_lrc("[00:05.00]la la\n[00:09.00]line two\n[00:13.00]la la\n");
        cache.put(&cache_key(LyricsService::Unison, &s, &only), to_cache(&junk).into_bytes());
        assert_eq!(run(&web, &cache, &s, (false, false), &only)[0].origin, LyricsOrigin::Server);
        assert!(web.asked().is_empty(), "a miss for the week, not asked again");
    }

    #[test]
    fn credits_are_stripped_before_the_answer_is_scored_or_kept() {
        let (web, cache) = (Web::default(), Kept::default());
        let s = Song { title: "Glass Harbour".into(), id: "credited".into(), ..song() };
        let body = format!("[00:00.00]Lyrics by: Some Person\n[00:02.00]Composed by: Another Person\n{}[03:50.00]Transcribed by A. Listener\n", lrc(&VERSE, false));
        unison(&web, &body);
        let picks = run(&web, &cache, &s, (false, false), &asked(&[LyricsService::Unison]));
        let l = &picks.last().unwrap().lyrics;
        assert_eq!((l.lines.len(), l.lines[0].text.as_str(), l.lines[0].start_ms), (18, VERSE[0], 10_000));
        let kept = read_kept(&cache.get(&cache_key(LyricsService::Unison, &s, &asked(&[LyricsService::Unison]))).unwrap()).unwrap();
        assert_eq!(kept.lyrics.lines.len(), 18, "the cached copy is already clean");
    }

    #[test]
    fn the_chosen_lyrics_are_served_without_asking_even_with_more_services_on() {
        let (web, cache) = (Web::default(), Kept::default());
        let s = Song { title: "Glass Harbour".into(), id: "chosen".into(), ..song() };
        unison(&web, &lrc(&VERSE, true));
        run(&web, &cache, &s, (false, false), &asked(&[LyricsService::Unison]));
        let n = web.asked().len();
        lrclib_synced(&web, &lrc(&VERSE, false));
        let picks = run(&web, &cache, &s, (false, false), &asked(&[LyricsService::Lrclib, LyricsService::Unison]));
        assert_eq!(picks.iter().map(|p| p.origin).collect::<Vec<_>>(), [LyricsOrigin::Unison]);
        assert_eq!(web.asked().len(), n, "no network");
        // Its service switched off: the choice no longer stands, and the others are asked.
        let picks = run(&web, &cache, &s, (false, false), &asked(&[LyricsService::Lrclib]));
        assert_eq!(picks.last().unwrap().origin, LyricsOrigin::Lrclib);
    }

    #[test]
    fn a_low_scored_choice_is_asked_about_again_only_after_some_days() {
        let (web, cache) = (Web::default(), Kept::default());
        let s = Song { title: "Glass Harbour".into(), id: "low".into(), ..song() };
        let l = asked(&[LyricsService::Lrclib, LyricsService::Unison]);
        let lines = crate::lyrics::from_lrc(&lrc(&VERSE, false));
        cache.put(&best_key(&s), kept(&lines, &Named::default(), Some((LyricsService::Lrclib, 0.5))));
        unison(&web, &lrc(&VERSE, true));
        let picks = run(&web, &cache, &s, (false, false), &l);
        assert_eq!(picks.iter().map(|p| p.origin).collect::<Vec<_>>(), [LyricsOrigin::Lrclib], "shown at once");
        assert!(web.asked().is_empty(), "low, but kept only lately: not asked again yet");
        cache.1.lock().insert(best_key(&s));
        let (picks, said) = run_saying(&web, &cache, &s, (false, false), &l);
        assert_eq!(picks.iter().map(|p| p.origin).collect::<Vec<_>>(), [LyricsOrigin::Lrclib, LyricsOrigin::Unison], "some days later: better lyrics replace them");
        assert!(said.unwrap().contains("was LRCLIB (0.50)"));
        let best = read_kept(&cache.get(&best_key(&s)).unwrap()).unwrap();
        assert_eq!(best.source.as_deref(), Some("UNISON"), "and the better choice is what is kept");
    }
}
