//! The lyrics services the settings can switch on and rank: what each is called and good at, the finest
//! timing it can answer with, whether it needs a key, and whether it is on out of the box. How each is
//! asked and read is nori-lyrics'; which to ask, in what order, is the settings' (`StoredPrefs`'s
//! `lyrics_order`, `lyrics_on`, and [`lyrics_lookup`]).

use nori_words::words::LyricsOrigin;

use crate::settings::StoredPrefs;

/// A key some lyrics services need, entered in Settings, Lyrics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LyricsKey {
    PaxSenix,
    BetterLyrics,
}

/// Somewhere lyrics the server does not have may be looked for. Each is somebody else's service and is
/// sent the artist, title, album and length. Declared in the order they rank out of the box, best first:
/// Apple Music's catalogue timed syllable by syllable, then the others that time words, then those that
/// time lines, then untimed words. The user's own ranking and switches are stored by [`name`](Self::name),
/// so this list can be reordered without disturbing either.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LyricsService {
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

impl LyricsService {
    pub const ALL: [LyricsService; 16] = [
        LyricsService::Binilyrics,
        LyricsService::BetterLyrics,
        LyricsService::Paxsenix,
        LyricsService::LyricsPlus,
        LyricsService::Portato,
        LyricsService::PaxsenixMusixmatch,
        LyricsService::Simpmusic,
        LyricsService::Unison,
        LyricsService::Netease,
        LyricsService::Kugou,
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

    /// Its row's title in Settings.
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

    /// What it is good at, in plain words: the line under its title.
    pub fn about(self) -> &'static str {
        match self {
            LyricsService::Binilyrics => "Apple Music's lyrics, syllable by syllable, from a volunteer's copy. Unofficial.",
            LyricsService::BetterLyrics => "Apple Music's lyrics, syllable by syllable. Finds more with a key. Unofficial.",
            LyricsService::Paxsenix => "Apple Music's lyrics, syllable by syllable, looked up another way. Unofficial.",
            LyricsService::LyricsPlus => "Apple Music's lyrics and others', syllable by syllable, from volunteers. Unofficial.",
            LyricsService::Portato => "QQ Music's lyrics, word by word. Strong on Chinese music. Unofficial.",
            LyricsService::PaxsenixMusixmatch => "Musixmatch's lyrics, often word by word. Needs a PaxSenix key.",
            LyricsService::Simpmusic => "Lyrics timed by listeners, often word by word, matched to the song on YouTube.",
            LyricsService::Unison => "Open, written and timed by listeners. Many songs word by word.",
            LyricsService::Netease => "The Chinese streaming service's own lyrics, often word by word. Unofficial.",
            LyricsService::Kugou => "Strong on Chinese, Japanese and Korean music, often word by word. Unofficial.",
            LyricsService::Lrclib => "Open and run by volunteers. Timed line by line, some songs word by word.",
            LyricsService::PaxsenixSpotify => "Spotify's lyrics, timed line by line. Needs a PaxSenix key.",
            LyricsService::YoutubeCaptions => "The captions of the song on YouTube, timed line by line. Unofficial.",
            LyricsService::Megalobiz => "Lyrics timed line by line by its users, read off its pages.",
            LyricsService::YoutubeMusic => "The words YouTube Music shows for the song. Not timed. Unofficial.",
            LyricsService::Genius => "The biggest catalogue of words, not timed. Asked only when nobody has timed lyrics.",
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

    /// Switched on out of the box, under "Find missing lyrics online": only the open ones, LRCLIB and
    /// Unison. The rest use other companies' lyrics without asking them, or pages never meant to be read
    /// by an app, and any of them can stop answering; they are for the user to switch on.
    pub fn on_by_default(self) -> bool {
        matches!(self, LyricsService::Unison | LyricsService::Lrclib)
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

/// `service` moved `by` places (-1 up, 1 down) among the services switched on; the ones switched off
/// keep their places in the full ranking.
pub fn moved(p: &StoredPrefs, service: LyricsService, by: i32) -> Vec<String> {
    let on = switched_on(p);
    let Some(at) = on.iter().position(|s| *s == service) else { return p.lyrics_order.clone() };
    let to = (at as i64 + by as i64).clamp(0, on.len() as i64 - 1) as usize;
    if to == at {
        return p.lyrics_order.clone();
    }
    let other = on[to];
    // Swapping the two in the full ranking moves this one past its neighbour and leaves the switched-off
    // services between them where they were.
    let mut order = p.lyrics_order.clone();
    let (a, b) = (order.iter().position(|n| n == service.name()), order.iter().position(|n| n == other.name()));
    if let (Some(a), Some(b)) = (a, b) {
        order.swap(a, b);
    }
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
            assert!(!s.title().is_empty() && s.about().ends_with('.'));
        }
        assert_eq!(LyricsService::named("MUSIXMATCH"), None, "a test build's service is not read");
    }

    #[test]
    fn only_the_open_services_are_on_out_of_the_box() {
        assert_eq!(default_on(), names(&["UNISON", "LRCLIB"]));
        let p = StoredPrefs::default();
        assert!(lyrics_lookup(&p).services.is_empty(), "looking things up is off out of the box");
        let on = StoredPrefs { third_party_lookups: true, ..StoredPrefs::default() };
        assert_eq!(lyrics_lookup(&on).services, [LyricsService::Unison, LyricsService::Lrclib]);
        let off = StoredPrefs { lyrics_online: false, ..on };
        assert!(lyrics_lookup(&off).services.is_empty());
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
        assert_eq!(order[..2], names(&["BINILYRICS", "BETTER_LYRICS"]), "the ones ranked above everything stored come first");
        let at = |n: &str| order.iter().position(|o| o == n).unwrap();
        assert!(at("LRCLIB") < at("UNISON"), "the stored order stands");
        assert_eq!(at("PAXSENIX_SPOTIFY"), at("LRCLIB") + 1, "put in after the one above it out of the box");
        assert_eq!(complete_order(&[]), default_order());
    }

    #[test]
    fn a_move_passes_the_next_switched_on_service() {
        let p = StoredPrefs { lyrics_on: names(&["NETEASE", "LRCLIB", "GENIUS"]), ..StoredPrefs::default() };
        let order = moved(&p, LyricsService::Lrclib, -1);
        let q = StoredPrefs { lyrics_order: order, ..p.clone() };
        assert_eq!(switched_on(&q), [LyricsService::Lrclib, LyricsService::Netease, LyricsService::Genius]);
        assert_eq!(moved(&q, LyricsService::Lrclib, -1), q.lyrics_order, "the first stays first");
        assert_eq!(moved(&p, LyricsService::Kugou, 1), p.lyrics_order, "one switched off does not move");
    }
}
