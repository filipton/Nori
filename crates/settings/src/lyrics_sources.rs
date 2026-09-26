//! The lyrics services the settings can switch on and rank: what each is stored as, the finest
//! timing it can answer with, whether it needs a key, and whether it is on out of the box. How each is
//! asked and read is nori-lyrics'; which to ask, in what order, is the settings' (`StoredPrefs`'s
//! `lyrics_order`, `lyrics_on`, and [`lyrics_lookup`]).

use crate::settings::StoredPrefs;

/// Where lyrics came from, for the credit line under them: the server, or the lyrics service that
/// answered ([`LyricsService`], one for one). The client names each ("your server", "LRCLIB").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum LyricsOrigin {
    Server,
    Binilyrics,
    BetterLyrics,
    Paxsenix,
    LyricsPlus,
    Portato,
    PaxsenixMusixmatch,
    Simpmusic,
    Unison,
    Netease,
    Kugou,
    Lrclib,
    PaxsenixSpotify,
    YoutubeCaptions,
    Megalobiz,
    YoutubeMusic,
    Genius,
}

/// A key some lyrics services need, entered in Settings, Lyrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsKey {
    PaxSenix,
    BetterLyrics,
}

/// Somewhere lyrics the server does not have may be looked for. Each is somebody else's service and is
/// sent the artist, title, album and length. The server's own lyrics (OpenSubsonic's structured lyrics)
/// always come before all of these. Declared in the order they rank out of the box, best first (see
/// docs/features.md, "Lyrics sources", for why each is where it is and whether it is on): Apple Music's
/// lyrics timed syllable by syllable, matched through Apple's own catalogue; the open database that times
/// words; the other services that time words; LRCLIB, the open one that times lines; the others that time
/// lines; untimed words. The user's own ranking and switches are stored by [`name`](Self::name), so this
/// list can be reordered without disturbing either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LyricsService {
    Paxsenix,
    Binilyrics,
    Unison,
    BetterLyrics,
    Kugou,
    Netease,
    LyricsPlus,
    Simpmusic,
    Portato,
    PaxsenixMusixmatch,
    Lrclib,
    PaxsenixSpotify,
    YoutubeCaptions,
    Megalobiz,
    YoutubeMusic,
    Genius,
}

impl LyricsService {
    pub const ALL: [LyricsService; 16] = [
        LyricsService::Paxsenix,
        LyricsService::Binilyrics,
        LyricsService::Unison,
        LyricsService::BetterLyrics,
        LyricsService::Kugou,
        LyricsService::Netease,
        LyricsService::LyricsPlus,
        LyricsService::Simpmusic,
        LyricsService::Portato,
        LyricsService::PaxsenixMusixmatch,
        LyricsService::Lrclib,
        LyricsService::PaxsenixSpotify,
        LyricsService::YoutubeCaptions,
        LyricsService::Megalobiz,
        LyricsService::YoutubeMusic,
        LyricsService::Genius,
    ];

    /// What it is stored and cached under, and what the test bridge calls it.
    pub fn name(self) -> &'static str {
        match self {
            LyricsService::Binilyrics => "BINILYRICS",
            LyricsService::BetterLyrics => "BETTER_LYRICS",
            LyricsService::Paxsenix => "PAXSENIX",
            LyricsService::LyricsPlus => "LYRICS_PLUS",
            LyricsService::Portato => "PORTATO",
            LyricsService::PaxsenixMusixmatch => "PAXSENIX_MUSIXMATCH",
            LyricsService::Simpmusic => "SIMPMUSIC",
            LyricsService::Unison => "UNISON",
            LyricsService::Netease => "NETEASE",
            LyricsService::Kugou => "KUGOU",
            LyricsService::Lrclib => "LRCLIB",
            LyricsService::PaxsenixSpotify => "PAXSENIX_SPOTIFY",
            LyricsService::YoutubeCaptions => "YOUTUBE_CAPTIONS",
            LyricsService::Megalobiz => "MEGALOBIZ",
            LyricsService::YoutubeMusic => "YOUTUBE_MUSIC",
            LyricsService::Genius => "GENIUS",
        }
    }

    /// The service stored under `name`, in any case; none for a name no service has any more.
    pub fn named(name: &str) -> Option<LyricsService> {
        LyricsService::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(name.trim()))
    }

    /// What it is called in the core's own log lines. A client names it in its own words.
    pub fn title(self) -> &'static str {
        match self {
            LyricsService::Binilyrics => "BiniLyrics",
            LyricsService::BetterLyrics => "BetterLyrics",
            LyricsService::Paxsenix => "PaxSenix",
            LyricsService::LyricsPlus => "LyricsPlus",
            LyricsService::Portato => "BetterLyrics Portato",
            LyricsService::PaxsenixMusixmatch => "PaxSenix: Musixmatch",
            LyricsService::Simpmusic => "SimpMusic",
            LyricsService::Unison => "Unison",
            LyricsService::Netease => "NetEase Cloud Music",
            LyricsService::Kugou => "KuGou",
            LyricsService::Lrclib => "LRCLIB",
            LyricsService::PaxsenixSpotify => "PaxSenix: Spotify",
            LyricsService::YoutubeCaptions => "YouTube captions",
            LyricsService::Megalobiz => "Megalobiz",
            LyricsService::YoutubeMusic => "YouTube Music",
            LyricsService::Genius => "Genius",
        }
    }

    /// The finest timing it can answer with: 3 word by word, 2 line by line, 1 not timed. A lookup does
    /// not wait on a service that cannot beat what it has, and one with untimed words only is asked last.
    pub fn best(self) -> u8 {
        match self {
            LyricsService::YoutubeCaptions | LyricsService::Megalobiz => 2,
            LyricsService::YoutubeMusic | LyricsService::Genius => 1,
            _ => 3,
        }
    }

    /// A key it cannot be asked without; with none given it is skipped quietly.
    pub fn needs(self) -> Option<LyricsKey> {
        match self {
            LyricsService::PaxsenixMusixmatch | LyricsService::PaxsenixSpotify => Some(LyricsKey::PaxSenix),
            _ => None,
        }
    }

    /// Switched on out of the box, under "Find missing lyrics online": the ones that answer with one or
    /// two quick requests and check the song they found against this one's title, artist and length
    /// before answering. Apple Music's lyrics through PaxSenix (the song found in Apple's own catalogue
    /// first), BiniLyrics and BetterLyrics, the open Unison database and LRCLIB; and, asked only when
    /// those miss or answer poorly ([`first_wave`](Self::first_wave)), KuGou, NetEase, LyricsPlus and
    /// SimpMusic, each of which names the song it found, so another song's words score too low to be
    /// shown (nori-lyrics' trust.rs). Off: the ones that need a key, QQ Music through BetterLyrics (a
    /// key for most songs), and the ones that read web pages or YouTube never meant for an app
    /// (YouTube's captions and lyrics tab, Megalobiz, Genius).
    pub fn on_by_default(self) -> bool {
        // Every service, the owner's call (2026-09-26): with the nine above alone most songs had no
        // word timing, and word-timed lyrics came from the rest. The waves keep the cost down: the others
        // are asked only when the first wave misses, scores low, or has no word timing
        // (nori-lyrics' race.rs `wants_words`); the ones needing a key stay silent until it is given.
        let _ = self;
        true
    }

    /// Asked in the first wave of a lookup: cheap (one or two requests), quick and good. The others that
    /// are on are asked only when the first wave misses or its best answer scores low, so a song that
    /// PaxSenix, BiniLyrics, Unison or LRCLIB have costs no more than those few requests.
    pub fn first_wave(self) -> bool {
        // KuGou and SimpMusic too: in the owner's library they, PaxSenix and BiniLyrics gave most of the
        // word-timed lyrics shown (2026-09-26), so waiting for a first wave without them only delayed them.
        matches!(
            self,
            LyricsService::Paxsenix | LyricsService::Binilyrics | LyricsService::Unison | LyricsService::Kugou | LyricsService::Simpmusic | LyricsService::Lrclib
        )
    }

    /// How far its answers are trusted before anything else is known, 0 to 1: the services that match
    /// the song in a catalogue by its exact length and answer with that catalogue's own lyrics highest,
    /// the open databases next, the unofficial catalogues after them, and captions and scraped pages
    /// lowest. One part of an answer's score (nori-lyrics' trust.rs).
    pub fn prior(self) -> f64 {
        match self {
            LyricsService::Paxsenix => 0.95,
            LyricsService::Binilyrics | LyricsService::BetterLyrics => 0.9,
            LyricsService::Unison | LyricsService::Lrclib | LyricsService::PaxsenixMusixmatch | LyricsService::PaxsenixSpotify => 0.85,
            LyricsService::LyricsPlus => 0.8,
            LyricsService::Kugou | LyricsService::Netease | LyricsService::Simpmusic | LyricsService::Portato => 0.75,
            LyricsService::Genius => 0.7,
            LyricsService::YoutubeMusic => 0.6,
            LyricsService::Megalobiz => 0.55,
            LyricsService::YoutubeCaptions => 0.45,
        }
    }

    /// The credit line's name for it.
    pub fn origin(self) -> LyricsOrigin {
        match self {
            LyricsService::Binilyrics => LyricsOrigin::Binilyrics,
            LyricsService::BetterLyrics => LyricsOrigin::BetterLyrics,
            LyricsService::Paxsenix => LyricsOrigin::Paxsenix,
            LyricsService::LyricsPlus => LyricsOrigin::LyricsPlus,
            LyricsService::Portato => LyricsOrigin::Portato,
            LyricsService::PaxsenixMusixmatch => LyricsOrigin::PaxsenixMusixmatch,
            LyricsService::Simpmusic => LyricsOrigin::Simpmusic,
            LyricsService::Unison => LyricsOrigin::Unison,
            LyricsService::Netease => LyricsOrigin::Netease,
            LyricsService::Kugou => LyricsOrigin::Kugou,
            LyricsService::Lrclib => LyricsOrigin::Lrclib,
            LyricsService::PaxsenixSpotify => LyricsOrigin::PaxsenixSpotify,
            LyricsService::YoutubeCaptions => LyricsOrigin::YoutubeCaptions,
            LyricsService::Megalobiz => LyricsOrigin::Megalobiz,
            LyricsService::YoutubeMusic => LyricsOrigin::YoutubeMusic,
            LyricsService::Genius => LyricsOrigin::Genius,
        }
    }
}

/// Every service's name in the order they rank out of the box.
pub fn default_order() -> Vec<String> {
    LyricsService::ALL.iter().map(|s| s.name().to_string()).collect()
}

/// The names of the services on out of the box.
pub fn default_on() -> Vec<String> {
    LyricsService::ALL.iter().filter(|s| s.on_by_default()).map(|s| s.name().to_string()).collect()
}

/// A stored ranking, read: names no service has any more left out, each service once, and any service
/// added since it was saved put in where it ranks out of the box (after the one above it there).
pub fn complete_order(stored: &[String]) -> Vec<String> {
    let mut order: Vec<LyricsService> = Vec::new();
    for s in stored.iter().filter_map(|n| LyricsService::named(n)) {
        if !order.contains(&s) {
            order.push(s);
        }
    }
    for (i, s) in LyricsService::ALL.into_iter().enumerate() {
        if !order.contains(&s) {
            let above = LyricsService::ALL[..i].iter().rev().find_map(|a| order.iter().position(|o| o == a));
            order.insert(above.map_or(0, |at| at + 1), s);
        }
    }
    order.into_iter().map(|s| s.name().to_string()).collect()
}

/// A stored set of names, read: known services only, each once, in their stored order.
pub fn known(stored: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for s in stored.iter().filter_map(|n| LyricsService::named(n)) {
        if !out.iter().any(|o| o == s.name()) {
            out.push(s.name().to_string());
        }
    }
    out
}

/// The services switched on, in the order they rank.
pub fn switched_on(p: &StoredPrefs) -> Vec<LyricsService> {
    p.lyrics_order.iter().filter(|n| p.lyrics_on.contains(n)).filter_map(|n| LyricsService::named(n)).collect()
}

/// `service` moved `by` places (-1 up, 1 down) in the ranking, past its neighbours whether they are
/// switched on or not: the list is one, and a switch never moves a service in it.
pub fn moved(p: &StoredPrefs, service: LyricsService, by: i32) -> Vec<String> {
    let Some(at) = p.lyrics_order.iter().position(|n| n == service.name()) else { return p.lyrics_order.clone() };
    placed(p, service, (at as i64 + by as i64).max(0) as usize)
}

/// `service` taken out of the ranking and put back at place `to` (0 is first; past the end is last):
/// a service held by its handle and dropped somewhere else.
pub fn placed(p: &StoredPrefs, service: LyricsService, to: usize) -> Vec<String> {
    let mut order = complete_order(&p.lyrics_order);
    let Some(at) = order.iter().position(|n| n == service.name()) else { return order };
    let name = order.remove(at);
    order.insert(to.min(order.len()), name);
    order
}

/// One lookup, as the settings have it: the services to ask in the order they rank (switched on, and
/// with the key they need), whether a lyric timed by line is held back while a service ranked lower may
/// still time the words, and the keys.
#[derive(Debug, Clone, PartialEq)]
pub struct LyricsLookup {
    pub services: Vec<LyricsService>,
    pub prefer_words: bool,
    pub paxsenix_key: String,
    pub better_lyrics_key: String,
}

/// The lookup to make now: no service unless both switches over them are on (looking things up at all,
/// and lyrics online), and none that needs a key nobody has given.
pub fn lyrics_lookup(p: &StoredPrefs) -> LyricsLookup {
    let paxsenix_key = p.paxsenix_key.trim().to_string();
    let better_lyrics_key = p.better_lyrics_key.trim().to_string();
    let has = |k: LyricsKey| match k {
        LyricsKey::PaxSenix => !paxsenix_key.is_empty(),
        LyricsKey::BetterLyrics => !better_lyrics_key.is_empty(),
    };
    let services = if p.third_party_lookups && p.lyrics_online { switched_on(p).into_iter().filter(|s| s.needs().is_none_or(has)).collect() } else { Vec::new() };
    LyricsLookup { services, prefer_words: p.lyrics_prefer_words, paxsenix_key, better_lyrics_key }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn every_service_has_a_name_that_reads_back() {
        for s in LyricsService::ALL {
            assert_eq!(LyricsService::named(s.name()), Some(s));
            assert_eq!(LyricsService::named(&s.name().to_lowercase()), Some(s));
            assert!(!s.title().is_empty());
        }
        assert_eq!(LyricsService::named("MUSIXMATCH"), None, "a test build's service is not read");
    }

    #[test]
    fn the_reputable_services_are_on_out_of_the_box_best_first() {
        assert_eq!(default_on(), default_order(), "every service is on out of the box, in its order");
        let on = StoredPrefs::default();
        assert!(on.third_party_lookups && on.lyrics_online, "looking lyrics up is on out of the box");
        assert!(lyrics_lookup(&StoredPrefs { third_party_lookups: false, ..on.clone() }).services.is_empty(), "and off under the lookups switch");
        let first: Vec<LyricsService> = lyrics_lookup(&on).services.into_iter().filter(|s| s.first_wave()).collect();
        let mut want = vec![LyricsService::Paxsenix, LyricsService::Binilyrics, LyricsService::Unison, LyricsService::Kugou, LyricsService::Simpmusic, LyricsService::Lrclib];
        want.sort_by_key(|s| default_order().iter().position(|n| n == s.name()));
        assert_eq!(first, want, "the cheap and good ones first");
        assert!(lyrics_lookup(&on).services.iter().all(|s| s.needs().is_none()), "the ones that need a key are skipped until it is given");
        let off = StoredPrefs { lyrics_online: false, ..on };
        assert!(lyrics_lookup(&off).services.is_empty());
    }

    #[test]
    fn the_default_order_goes_from_word_timing_to_line_timing_to_none() {
        let order: Vec<LyricsService> = default_order().iter().filter_map(|n| LyricsService::named(n)).collect();
        assert_eq!(order.len(), 16);
        let rank = |s: LyricsService| order.iter().position(|o| *o == s).unwrap();
        // Word timing first, LRCLIB the first of those that time lines, untimed words last.
        let lrclib = rank(LyricsService::Lrclib);
        assert!(order[..lrclib].iter().all(|s| s.best() == 3), "only services that time words rank above LRCLIB");
        assert!(order[lrclib..].windows(2).all(|w| w[0].best() >= w[1].best() || w[0] == LyricsService::Lrclib), "then by line, then untimed");
        assert_eq!(order[14..], [LyricsService::YoutubeMusic, LyricsService::Genius]);
        assert!(LyricsService::ALL.into_iter().all(|s| s.on_by_default()), "every service on out of the box");
        assert!(LyricsService::ALL.into_iter().filter(|s| s.first_wave()).all(|s| s.on_by_default() && s.best() == 3 || s == LyricsService::Lrclib));
        assert!(LyricsService::ALL.into_iter().all(|s| (0.0..=1.0).contains(&s.prior())));
    }

    #[test]
    fn a_service_that_needs_a_key_waits_for_one() {
        let p = StoredPrefs { third_party_lookups: true, lyrics_on: names(&["PAXSENIX_SPOTIFY", "LRCLIB"]), ..StoredPrefs::default() };
        assert_eq!(lyrics_lookup(&p).services, [LyricsService::Lrclib]);
        let keyed = StoredPrefs { paxsenix_key: " abc ".into(), ..p };
        let l = lyrics_lookup(&keyed);
        assert_eq!(l.services, [LyricsService::Lrclib, LyricsService::PaxsenixSpotify]);
        assert_eq!(l.paxsenix_key, "abc");
    }

    #[test]
    fn a_stored_ranking_gains_new_services_where_they_rank() {
        let order = complete_order(&names(&["LRCLIB", "UNISON", "MUSIXMATCH", "LRCLIB"]));
        assert_eq!(order.len(), 16);
        assert_eq!(order[..2], names(&["PAXSENIX", "BINILYRICS"]), "the ones ranked above everything stored come first");
        let at = |n: &str| order.iter().position(|o| o == n).unwrap();
        assert!(at("LRCLIB") < at("UNISON"), "the stored order stands");
        assert_eq!(at("PAXSENIX_SPOTIFY"), at("LRCLIB") + 1, "put in after the one above it out of the box");
        assert_eq!(complete_order(&[]), default_order());
    }

    #[test]
    fn a_move_passes_the_next_service_whether_it_is_on_or_not() {
        let p = StoredPrefs { lyrics_on: names(&["NETEASE", "LRCLIB", "GENIUS"]), ..StoredPrefs::default() };
        let at = |o: &[String], n: &str| o.iter().position(|x| x == n).unwrap();
        let order = moved(&p, LyricsService::Lrclib, -1);
        assert_eq!(at(&order, "LRCLIB"), at(&p.lyrics_order, "LRCLIB") - 1, "one place, past a service that is off");
        assert_eq!(at(&order, "PAXSENIX_MUSIXMATCH"), at(&p.lyrics_order, "LRCLIB"));
        let first = StoredPrefs { lyrics_order: placed(&p, LyricsService::Lrclib, 0), ..p.clone() };
        assert_eq!(first.lyrics_order[0], "LRCLIB");
        assert_eq!(moved(&first, LyricsService::Lrclib, -1), first.lyrics_order, "the first stays first");
        assert_eq!(switched_on(&first), [LyricsService::Lrclib, LyricsService::Netease, LyricsService::Genius]);
        let off = moved(&p, LyricsService::Kugou, 1);
        assert_eq!(at(&off, "KUGOU"), at(&p.lyrics_order, "KUGOU") + 1, "one switched off moves too");
        assert_eq!(p.lyrics_on, names(&["NETEASE", "LRCLIB", "GENIUS"]), "moving switches nothing");
    }

    #[test]
    fn a_service_dropped_anywhere_takes_that_place_and_the_rest_close_up() {
        let p = StoredPrefs::default();
        let last = placed(&p, LyricsService::Paxsenix, 99);
        assert_eq!(last.len(), 16);
        assert_eq!(last[15], "PAXSENIX");
        assert_eq!(last[..15], p.lyrics_order[1..]);
        let back = StoredPrefs { lyrics_order: last, ..p.clone() };
        assert_eq!(placed(&back, LyricsService::Paxsenix, 0), p.lyrics_order);
        let mid = placed(&p, LyricsService::Genius, 3);
        assert_eq!(mid[3], "GENIUS");
        assert_eq!(mid.iter().filter(|n| *n == "GENIUS").count(), 1);
    }
}
