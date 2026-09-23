//! How a page's data is laid out before it is shown: an album's discs, an artist's releases by kind,
//! the letters down the side of a long list, what a filter keeps. Worked out once per page (or once per
//! keystroke for a filter) here, so the UI only lays out what it is handed.

use crate::model::{Album, AlbumDetail};

/// One disc of an album: its heading ("Disc 2 · Bonus") and the positions of its songs in the album's
/// song list, in the album's order.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DiscGroup {
    pub disc: u32,
    /// Empty when the album has only the one disc, which needs no heading.
    pub heading: String,
    pub songs: Vec<u32>,
}

/// An album's songs by disc, discs in order (a song with no disc number is on disc 1).
#[uniffi::export]
pub fn album_discs(detail: AlbumDetail) -> Vec<DiscGroup> {
    let mut discs: Vec<DiscGroup> = Vec::new();
    for (i, s) in detail.songs.iter().enumerate() {
        let disc = s.disc_number.max(1);
        match discs.iter_mut().find(|d| d.disc == disc) {
            Some(d) => d.songs.push(i as u32),
            None => discs.push(DiscGroup { disc, heading: String::new(), songs: vec![i as u32] }),
        }
    }
    discs.sort_by_key(|d| d.disc);
    if discs.len() > 1 {
        for d in &mut discs {
            let title = detail.disc_titles.iter().find(|t| t.disc == d.disc).map(|t| t.title.as_str());
            d.heading = match title {
                Some(t) if !t.trim().is_empty() => format!("Disc {} · {t}", d.disc),
                _ => format!("Disc {}", d.disc),
            };
        }
    }
    discs
}

/// The kinds of release in the order an artist's page lists them.
const RELEASE_ORDER: [&str; 8] = ["Album", "EP", "Single", "Live", "Compilation", "Soundtrack", "Remix", "Other"];

fn capitalised(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

/// Which kind of release an album is: the first of its types this page knows (other than plain
/// "album", which everything also is), else its first type; a compilation or album when it has none.
fn release_kind(a: &Album) -> String {
    if a.release_types.is_empty() {
        return if a.is_compilation { "Compilation" } else { "Album" }.into();
    }
    let known = a.release_types.iter().find(|t| RELEASE_ORDER.iter().any(|k| k.eq_ignore_ascii_case(t)) && !t.eq_ignore_ascii_case("Album"));
    capitalised(known.unwrap_or(&a.release_types[0]))
}

/// One shelf of an artist's page: "Albums", "Singles", "Eps", and the positions of its releases.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct ReleaseGroup {
    pub heading: String,
    pub albums: Vec<u32>,
}

/// An artist's releases by kind, newest first within each, the kinds in [`RELEASE_ORDER`] (a kind
/// named differently - "Ep" from a lower-case tag - after all of those, in the order they first come).
#[uniffi::export]
pub fn release_groups(albums: Vec<Album>) -> Vec<ReleaseGroup> {
    let mut order: Vec<usize> = (0..albums.len()).collect();
    // Stable, newest first: equal years keep the server's order.
    order.sort_by(|&a, &b| albums[b].year.cmp(&albums[a].year));
    let mut groups: Vec<(usize, String, Vec<u32>)> = Vec::new();
    for i in order {
        let kind = release_kind(&albums[i]);
        match groups.iter_mut().find(|g| g.1 == kind) {
            Some(g) => g.2.push(i as u32),
            None => {
                let rank = RELEASE_ORDER.iter().position(|k| *k == kind).unwrap_or(99);
                groups.push((rank, kind, vec![i as u32]));
            }
        }
    }
    groups.sort_by_key(|g| g.0);
    groups
        .into_iter()
        .map(|(_, kind, albums)| ReleaseGroup { heading: if kind.ends_with('s') { kind } else { format!("{kind}s") }, albums })
        .collect()
}

/// Kotlin's `contains(needle, ignoreCase = true)`: character by character, either case.
fn contains_ignoring_case(hay: &str, needle: &str) -> bool {
    let n: Vec<char> = needle.chars().collect();
    if n.is_empty() {
        return true;
    }
    let h: Vec<char> = hay.chars().collect();
    let same = |a: char, b: char| a == b || upper(a) == upper(b) || lower(a) == lower(b);
    h.windows(n.len()).any(|w| w.iter().zip(&n).all(|(&a, &b)| same(a, b)))
}

/// A character's one-character upper case, as Java's `Character.toUpperCase` gives it ('ß' stays).
fn upper(c: char) -> char {
    let mut u = c.to_uppercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

fn lower(c: char) -> char {
    let mut u = c.to_lowercase();
    match (u.next(), u.next()) {
        (Some(x), None) => x,
        _ => c,
    }
}

/// A letter down the side of a long list and the first row that starts with it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct IndexLetter {
    pub letter: String,
    pub row: u32,
}

/// What a filter keeps of a list (positions in the whole list), and its letters.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct IndexView {
    pub rows: Vec<u32>,
    /// The first row of each initial, in the kept rows' positions, by letter; '#' for anything that
    /// does not start with a letter.
    pub letters: Vec<IndexLetter>,
    /// Whether the letters are worth an index down the side: more than three of them.
    pub show_letters: bool,
}

/// A list with this many initials or fewer has no index down its side: there is nothing to jump over.
const INDEX_FROM: usize = 3;

/// A list's text, held once for the life of the page, so a filter typed into it sends only what was
/// typed. Each row is one or more fields (a song's title and artist); a row is kept when any field
/// contains the filter, in either case.
#[derive(uniffi::Object)]
pub struct TextIndex {
    rows: Vec<Vec<String>>,
}

#[uniffi::export]
impl TextIndex {
    #[uniffi::constructor]
    pub fn new(rows: Vec<Vec<String>>) -> std::sync::Arc<Self> {
        std::sync::Arc::new(Self { rows })
    }

    /// A search rather than a filter: the rows whose first field contains `query`, then those where
    /// only a later field does, each in list order. Nothing for a blank query.
    pub fn ranked(&self, query: String) -> Vec<u32> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let hit = |i: usize, first: bool| {
            let f = &self.rows[i];
            let head = f.first().is_some_and(|h| contains_ignoring_case(h, &query));
            if first { head } else { !head && f.iter().skip(1).any(|x| contains_ignoring_case(x, &query)) }
        };
        let n = self.rows.len();
        (0..n).filter(|&i| hit(i, true)).chain((0..n).filter(|&i| hit(i, false))).map(|i| i as u32).collect()
    }

    /// The rows `filter` keeps (all of them for a blank one), and the letters of the first field.
    pub fn view(&self, filter: String) -> IndexView {
        let blank = filter.trim().is_empty();
        let rows: Vec<u32> = (0..self.rows.len())
            .filter(|&i| blank || self.rows[i].iter().any(|f| contains_ignoring_case(f, &filter)))
            .map(|i| i as u32)
            .collect();
        let mut letters: Vec<(char, u32)> = Vec::new();
        for (at, &i) in rows.iter().enumerate() {
            let first = self.rows[i as usize].first().and_then(|f| f.chars().next());
            let c = first.map(upper).filter(|c| c.is_alphabetic()).unwrap_or('#');
            if !letters.iter().any(|l| l.0 == c) {
                letters.push((c, at as u32));
            }
        }
        letters.sort_by_key(|l| l.0);
        let show_letters = letters.len() > INDEX_FROM;
        IndexView { rows, letters: letters.into_iter().map(|(c, row)| IndexLetter { letter: c.to_string(), row }).collect(), show_letters }
    }
}

/// What makes a page's queue the one playing: its songs (an album also claims a song of its own record
/// that some other queue carried along), or for an artist, whose page has no song list of its own, a
/// song of one of its albums.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum PageOwn {
    Songs { ids: Vec<String>, album_id: Option<String> },
    Albums { ids: Vec<String> },
}

/// A page's own queue, held for the life of the page so that asking whether it is playing sends only
/// the song playing.
#[derive(uniffi::Object)]
pub struct PageQueue {
    songs: std::collections::HashSet<String>,
    album: Option<String>,
    albums: std::collections::HashSet<String>,
}

#[uniffi::export]
impl PageQueue {
    #[uniffi::constructor]
    pub fn new(own: PageOwn) -> std::sync::Arc<Self> {
        std::sync::Arc::new(match own {
            PageOwn::Songs { ids, album_id } => PageQueue { songs: ids.into_iter().collect(), album: album_id, albums: Default::default() },
            PageOwn::Albums { ids } => PageQueue { songs: Default::default(), album: None, albums: ids.into_iter().collect() },
        })
    }

    /// Whether the song playing (its id and album) is this page's.
    pub fn plays(&self, song_id: Option<String>, album_id: Option<String>) -> bool {
        let Some(id) = song_id else { return false };
        self.songs.contains(&id) || album_id.is_some_and(|a| self.album.as_deref() == Some(a.as_str()) || self.albums.contains(&a))
    }
}

/// What the page's two big buttons press.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum HeroPress {
    /// Start this page's songs (played or shuffled).
    Start,
    /// Pause or resume the queue this page started.
    Toggle,
    /// Turn shuffle off for the queue this page started.
    ShuffleOff,
}

/// A page's Shuffle and Play as they stand.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct HeroButtons {
    /// Shuffle lit: this page's queue is shuffling.
    pub shuffle_lit: bool,
    pub shuffle_enabled: bool,
    pub shuffle_press: HeroPress,
    /// "Pause" while this page's queue sounds, else "Play".
    pub play_label: String,
    pub pausing: bool,
    pub play_enabled: bool,
    pub play_press: HeroPress,
}

/// The two big buttons answer for the queue the page started rather than for the player in general:
/// Play becomes Pause while it sounds (and picks it up where it stopped rather than starting the record
/// again), Shuffle lights while it is shuffling, and a second press on Shuffle turns shuffle off instead
/// of drawing the same songs into a new queue. `here` is [`PageQueue::plays`]; `can_play` and
/// `can_shuffle` say the page has songs to start.
#[uniffi::export]
pub fn hero_buttons(here: bool, shuffle: bool, playing: bool, buffering: bool, can_play: bool, can_shuffle: bool) -> HeroButtons {
    let lit = shuffle && here;
    let pausing = here && (playing || buffering);
    HeroButtons {
        shuffle_lit: lit,
        shuffle_enabled: lit || can_shuffle,
        shuffle_press: if lit { HeroPress::ShuffleOff } else { HeroPress::Start },
        play_label: (if pausing { "Pause" } else { "Play" }).into(),
        pausing,
        play_enabled: here || can_play,
        play_press: if here { HeroPress::Toggle } else { HeroPress::Start },
    }
}

/// The offer on a provider's album or playlist page, which octo-fiesta fetches whole into the library
/// when it is starred: "Add the whole album to the library (Deezer)". None for the library's own.
#[uniffi::export]
pub fn library_offer(id: String, is_external: bool) -> Option<String> {
    let playlist = id.starts_with("pl-");
    if !is_external && !playlist {
        return None;
    }
    let provider = crate::fmt::provider_of(id).unwrap_or_else(|| "provider".into());
    Some(format!("Add the whole {} to the library ({provider})", if playlist { "playlist" } else { "album" }))
}

/// A long list wants a way to narrow itself; one short enough to see whole does not. Kept while a
/// filter is typed, so emptying the list does not take the field away under the finger.
#[uniffi::export]
pub fn filter_offered(songs: u32, filtering: bool) -> bool {
    songs > FILTER_FROM || filtering
}

const FILTER_FROM: u32 = 12;

/// The similar artists worth a link: the ones the server can open (last.fm names some it does not have).
#[uniffi::export]
pub fn similar_artists(similar: Vec<crate::Artist>) -> Vec<crate::Artist> {
    similar.into_iter().filter(|a| !a.id.is_empty()).collect()
}

/// An artist's biography as the page shows it: the text before the link the server appends to it.
#[uniffi::export]
pub fn biography(text: String) -> String {
    match text.find("<a ") {
        Some(i) => text[..i].to_string(),
        None => text,
    }
}

/// An artist's MusicBrainz page.
#[uniffi::export]
pub fn musicbrainz_artist_url(id: String) -> String {
    format!("https://musicbrainz.org/artist/{id}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{DiscTitle, Song};

    fn song(disc: u32) -> Song {
        Song { disc_number: disc, ..Default::default() }
    }

    #[test]
    fn discs_in_order_with_their_titles() {
        let d = AlbumDetail {
            songs: vec![song(2), song(0), song(1), song(2)],
            disc_titles: vec![DiscTitle { disc: 2, title: "Bonus".into() }, DiscTitle { disc: 1, title: " ".into() }],
            ..Default::default()
        };
        let g = album_discs(d);
        assert_eq!(g.iter().map(|x| (x.disc, x.heading.as_str(), x.songs.clone())).collect::<Vec<_>>(), [(1, "Disc 1", vec![1, 2]), (2, "Disc 2 · Bonus", vec![0, 3])]);
        let one = album_discs(AlbumDetail { songs: vec![song(1), song(1)], ..Default::default() });
        assert_eq!((one.len(), one[0].heading.as_str()), (1, ""));
    }

    #[test]
    fn releases_by_kind_newest_first() {
        let a = |year: u32, types: &[&str], comp: bool| Album { year, release_types: types.iter().map(|t| t.to_string()).collect(), is_compilation: comp, ..Default::default() };
        let albums = vec![a(2001, &[], false), a(2010, &["album", "live"], false), a(2005, &["single"], false), a(2003, &["ep"], false), a(2020, &[], false), a(1999, &[], true)];
        let g = release_groups(albums);
        let got: Vec<(&str, Vec<u32>)> = g.iter().map(|x| (x.heading.as_str(), x.albums.clone())).collect();
        assert_eq!(got, [("Albums", vec![4, 0]), ("Singles", vec![2]), ("Lives", vec![1]), ("Compilations", vec![5]), ("Eps", vec![3])]);
    }

    #[test]
    fn filters_and_letters() {
        let idx = TextIndex::new(vec![vec!["abba".into()], vec!["Björk".into()], vec!["!!!".into()], vec!["Beck".into(), "x".into()], vec!["ßuper".into()]]);
        let all = idx.view("  ".into());
        assert_eq!(all.rows, [0, 1, 2, 3, 4]);
        let l: Vec<(&str, u32)> = all.letters.iter().map(|x| (x.letter.as_str(), x.row)).collect();
        assert_eq!(l, [("#", 2), ("A", 0), ("B", 1), ("ß", 4)]);
        assert!(all.show_letters);
        assert!(!idx.view("b".into()).show_letters, "three letters or fewer");
        assert_eq!(idx.view("BJÖ".into()).rows, [1]);
        assert_eq!(idx.view("X".into()).rows, [3]);
        assert_eq!(biography("Hello <a href=x>more</a>".into()), "Hello ");
    }

    #[test]
    fn a_page_knows_its_own_queue() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let album = PageQueue::new(PageOwn::Songs { ids: s(&["1", "2"]), album_id: Some("al".into()) });
        assert!(album.plays(Some("1".into()), None));
        assert!(album.plays(Some("9".into()), Some("al".into())), "a song of the record carried by another queue");
        assert!(!album.plays(Some("9".into()), Some("other".into())));
        assert!(!album.plays(None, Some("al".into())), "nothing playing");
        let artist = PageQueue::new(PageOwn::Albums { ids: s(&["a1", "a2"]) });
        assert!(artist.plays(Some("x".into()), Some("a2".into())));
        assert!(!artist.plays(Some("x".into()), None));
    }

    #[test]
    fn the_big_buttons_answer_for_the_pages_own_queue() {
        let away = hero_buttons(false, true, true, false, true, true);
        assert_eq!((away.shuffle_lit, away.pausing, away.play_label.as_str(), away.play_press, away.shuffle_press), (false, false, "Play", HeroPress::Start, HeroPress::Start));
        let here = hero_buttons(true, true, false, true, true, true);
        assert_eq!((here.shuffle_lit, here.pausing, here.play_label.as_str(), here.play_press, here.shuffle_press), (true, true, "Pause", HeroPress::Toggle, HeroPress::ShuffleOff));
        let waiting = hero_buttons(false, false, false, false, false, false);
        assert!(!waiting.play_enabled && !waiting.shuffle_enabled);
        assert!(hero_buttons(true, false, false, false, false, false).play_enabled, "its own queue can always be resumed");
    }

    #[test]
    fn provider_pages_offer_the_library_and_long_lists_a_filter() {
        assert_eq!(library_offer("ext-deezer-album-1".into(), true).as_deref(), Some("Add the whole album to the library (Deezer)"));
        assert_eq!(library_offer("pl-deezer-1".into(), false).as_deref(), Some("Add the whole playlist to the library (Deezer)"));
        assert_eq!(library_offer("al-1".into(), false), None);
        assert_eq!((filter_offered(12, false), filter_offered(13, false), filter_offered(3, true)), (false, true, true));
        let a = |id: &str| crate::Artist { id: id.into(), ..Default::default() };
        assert_eq!(similar_artists(vec![a(""), a("x")]).len(), 1);
    }

    #[test]
    fn a_search_puts_names_before_descriptions() {
        let idx = TextIndex::new(vec![
            vec!["Keep albums gapless".into(), "No mixing".into()],
            vec!["AMOLED black".into(), "Pixels off".into()],
            vec!["Crossfade".into(), "Songs fade, gapless otherwise".into()],
            vec!["Gapless".into(), "".into()],
        ]);
        assert_eq!(idx.ranked("gapless".into()), [0, 3, 2]);
        assert_eq!(idx.ranked("OLED".into()), [1]);
        assert!(idx.ranked(" ".into()).is_empty());
    }
}
