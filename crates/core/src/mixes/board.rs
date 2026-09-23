//! The "For you" row: which mixes it offers, today's (or this week's) draw of each, and the favourites.
//!
//! The Home tiles and the mix pages read the same draw, so the covers on a tile, the list on its page and
//! what plays are one list. A mix is drawn once per period (a day, or seven days for Discover Weekly) or
//! when the page asks for another, and never written to the server. The draws are held in memory, one
//! board per core (so per server profile): after a restart the same seed over the same index draws the
//! same mix again, so there is nothing worth storing.
//!
//! The app only says what day it is, which mix a screen wants, and what the server gave when a mix came
//! out empty (the one step here that needs the network).

use std::collections::{HashMap, HashSet};

use parking_lot::Mutex;
use rusqlite::Connection;

use super::{discover, listen_again, quick_picks, top};
use crate::{db, stars, Core, Song};

/// The id of the favourites tile and page; every other id names one of [MIXES].
pub const FAVOURITES_MIX: &str = "favourites";

/// How many songs one draw holds.
const DRAW: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    QuickPicks,
    Discover,
    ListenAgain,
    Top,
}

/// One "For you" mix: `id` is the route, `kind` is what the index draws, `title` is what the tile says.
/// Discover Daily and Discover Weekly both call the same taste-based draw; only the seed period differs.
struct Spec {
    id: &'static str,
    kind: Kind,
    title: &'static str,
    weekly: bool,
    /// The tile's own colour: what it wears before its covers arrive, and the band its name sits on.
    colour: u32,
}

impl Spec {
    /// Top songs is a ranking, not a draw: asking for another would give the same list.
    fn refreshable(&self) -> bool {
        self.kind != Kind::Top
    }
}

/// The mixes "For you" offers, in the order it offers them.
const MIXES: [Spec; 5] = [
    Spec { id: "quick-picks", kind: Kind::QuickPicks, title: "Quick picks", weekly: false, colour: 0xFF8E_3BD6 },
    Spec { id: "discover", kind: Kind::Discover, title: "Discover", weekly: false, colour: 0xFF1E_88E5 },
    Spec { id: "discover-weekly", kind: Kind::Discover, title: "Discover Weekly", weekly: true, colour: 0xFF15_65C0 },
    Spec { id: "listen-again", kind: Kind::ListenAgain, title: "Listen again", weekly: false, colour: 0xFF00_897B },
    Spec { id: "top", kind: Kind::Top, title: "Your top songs", weekly: false, colour: 0xFFE0_662B },
];

fn spec_of(id: &str) -> Option<&'static Spec> {
    MIXES.iter().find(|s| s.id == id)
}

/// The favourites tile's colour, and the one a mix this build does not know wears.
const FAVOURITES_COLOUR: u32 = 0xFFE0_335A;
const OTHER_COLOUR: u32 = 0xFF5C_6BC0;

/// A mix tile's colours: its own, the deeper one its gradient runs to (45 % of the way to black), and
/// that deeper colour at 0, 72 and 94 % for the band rising under the tile's name.
#[uniffi::export]
pub fn mix_tile_colours(id: String) -> Vec<u32> {
    use nori_look::compose::{blend, with_alpha};
    let seed = if id == FAVOURITES_MIX { FAVOURITES_COLOUR } else { spec_of(&id).map_or(OTHER_COLOUR, |s| s.colour) };
    let deep = blend(seed, 0xFF00_0000, 0.45);
    vec![seed, deep, with_alpha(deep, 0.0), with_alpha(deep, 0.72), with_alpha(deep, 0.94)]
}

/// Provider tracks are never queued unasked: a stream request makes octo-fiesta download them.
pub(crate) fn playable(s: &Song) -> bool {
    !s.is_external && !s.id.starts_with("ext-") && !s.id.starts_with("pl-")
}

/// Four different covers, for a tile's collage.
fn cover_ids(songs: &[Song]) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(4);
    for art in songs.iter().filter_map(|s| s.cover_art.as_deref()) {
        if out.len() == 4 {
            break;
        }
        if !out.iter().any(|o| o == art) {
            out.push(art.to_string());
        }
    }
    out
}

/// Keeps the first of each id: lists are keyed by id.
fn distinct(songs: impl IntoIterator<Item = Song>) -> Vec<Song> {
    let mut seen = HashSet::new();
    songs.into_iter().filter(|s| seen.insert(s.id.clone())).collect()
}

// ---- what crosses to the app -----------------------------------------------

/// One entry of the catalogue, for a player that lists the mixes itself.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MixSpec {
    pub id: String,
    pub title: String,
    pub weekly: bool,
    pub refreshable: bool,
}

/// A "For you" tile: what it is called and up to four cover ids of what is in it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MixTile {
    pub id: String,
    pub title: String,
    pub covers: Vec<String>,
    pub favourites: bool,
}

/// A mix page: exactly the songs that play, in the order they play.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MixSheet {
    pub id: String,
    pub title: String,
    pub songs: Vec<Song>,
    /// Up to four cover ids.
    pub covers: Vec<String>,
    /// False for favourites (they follow the hearts) and for top songs (there is only one draw of those).
    pub refreshable: bool,
    pub favourites: bool,
}

#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum MixLookup {
    /// No mix has this id; `message` says so in words.
    Unknown { message: String },
    /// Not drawn yet (or, for favourites, not handed over yet): the page keeps its loader.
    NotDrawn,
    Ready { sheet: MixSheet },
}

#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum MixDraw {
    Unknown,
    /// This period's draw was already there.
    Kept,
    Drawn,
    /// The index gave nothing (no listening history yet): call again with what the server thinks is
    /// random (`getRandomSongs`, 50), which then stands in for the mix. Nothing was stored.
    NeedsFallback,
}

/// What warming the row did: whether any tile changed, and which mixes need the server's random songs.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct MixWarm {
    pub changed: bool,
    pub needs_fallback: Vec<String>,
}

// ---- the boards ------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
struct Drawn {
    songs: Vec<Song>,
    period: i64,
    generation: i64,
}

#[derive(Default)]
struct Board {
    drawn: HashMap<&'static str, Drawn>,
    /// None until the app has handed the starred songs over once.
    favourites: Option<Vec<Song>>,
}

/// One board per core, told apart by the core's address and its database file. A core that is gone
/// leaves its address free: the next one to get it replaces its board rather than inheriting it.
static BOARDS: Mutex<Vec<(usize, String, Board)>> = Mutex::new(Vec::new());

impl Core {
    fn board<R>(&self, f: impl FnOnce(&mut Board) -> R) -> R {
        let address = self as *const Core as usize;
        let path = self.db.lock().path().unwrap_or_default().to_string();
        let mut boards = BOARDS.lock();
        let at = match boards.iter().position(|(a, p, _)| *a == address && *p == path) {
            Some(i) => i,
            None => {
                boards.retain(|(a, _, _)| *a != address);
                boards.push((address, path, Board::default()));
                boards.len() - 1
            }
        };
        f(&mut boards[at].2)
    }
}

/// A draw that fails reads as an empty one, which then falls back to the server's random songs.
fn draw(c: &Connection, kind: Kind, seed: u64, now_ms: i64) -> Vec<Song> {
    match kind {
        Kind::QuickPicks => quick_picks(c, DRAW, seed, now_ms),
        Kind::Discover => discover(c, DRAW, seed, now_ms),
        Kind::ListenAgain => listen_again(c, DRAW, seed, now_ms),
        Kind::Top => top(c, DRAW, now_ms),
    }
    .unwrap_or_default()
}

/// Every tile that can be named without the index: favourites first, then the mixes when the taste model
/// is on (switched off, it draws nothing and offers only favourites). No covers yet.
#[uniffi::export]
pub fn mix_tiles(taste: bool) -> Vec<MixTile> {
    let mut out = vec![MixTile { id: FAVOURITES_MIX.into(), title: "Favourites".into(), covers: vec![], favourites: true }];
    if taste {
        out.extend(MIXES.iter().map(|s| MixTile { id: s.id.into(), title: s.title.into(), covers: vec![], favourites: false }));
    }
    out
}

/// The mixes "For you" offers, in order.
#[uniffi::export]
pub fn mix_catalogue() -> Vec<MixSpec> {
    MIXES.iter().map(|s| MixSpec { id: s.id.into(), title: s.title.into(), weekly: s.weekly, refreshable: s.refreshable() }).collect()
}

/// Drawn off the main thread: a mix takes a few milliseconds of the index.
#[uniffi::export]
impl Core {
    /// Draws mix `id` unless this period's draw is already here; `again` asks for a different one.
    /// `today_epoch_day` is the local date as days since 1970-01-01. `fallback` is None on the first call
    /// and the server's random songs on the call after [MixDraw::NeedsFallback].
    pub fn mix_draw(&self, id: String, today_epoch_day: i64, again: bool, fallback: Option<Vec<Song>>) -> MixDraw {
        let Some(spec) = spec_of(&id) else { return MixDraw::Unknown };
        // Weekly mixes share one seed for seven days so the tile does not churn every midnight.
        let period = if spec.weekly { today_epoch_day / 7 } else { today_epoch_day };
        let generation = match self.board(|b| b.drawn.get(spec.id).map(|d| (d.period, d.generation))) {
            Some((p, _)) if p == period && !again => return MixDraw::Kept,
            Some((p, g)) if p == period => g + 1,
            _ => 0,
        };
        // Offset weekly seeds so they never collide with the same day's Discover draw.
        let seed = period * 1_000 + generation + if spec.weekly { 7_000_000 } else { 0 };
        let songs: Vec<Song> = match fallback {
            Some(random) => random.into_iter().filter(playable).collect(),
            None => {
                let songs: Vec<Song> = draw(&self.db.lock(), spec.kind, seed as u64, db::now_ms()).into_iter().filter(playable).collect();
                // With no listening history yet the personal mixes are empty: what the server thinks is random stands in.
                if songs.is_empty() {
                    return MixDraw::NeedsFallback;
                }
                songs
            }
        };
        let drawn = Drawn { songs: distinct(songs), period, generation };
        self.board(|b| b.drawn.insert(spec.id, drawn));
        MixDraw::Drawn
    }

    /// Draws whichever mixes are missing or from the last period, in the row's order.
    pub fn mix_warm(&self, today_epoch_day: i64) -> MixWarm {
        let mut out = MixWarm { changed: false, needs_fallback: vec![] };
        for spec in &MIXES {
            match self.mix_draw(spec.id.into(), today_epoch_day, false, None) {
                MixDraw::Drawn => out.changed = true,
                MixDraw::NeedsFallback => out.needs_fallback.push(spec.id.into()),
                MixDraw::Kept | MixDraw::Unknown => {}
            }
        }
        out
    }

    /// The starred songs, as the server last listed them, with this session's marks applied (an unstarred
    /// song leaves the list under the finger instead of after the server's answer). Provider tracks are
    /// left out like everywhere a list is played unasked. True when the favourites changed.
    pub fn mix_favourites(&self, starred_songs: Vec<Song>) -> bool {
        stars::with_marks(|marks| self.favourites_with(starred_songs, marks))
    }

    /// The "For you" row with the covers of what is drawn: favourites first, then the mixes when `taste`.
    pub fn mix_cards(&self, taste: bool) -> Vec<MixTile> {
        let mut tiles = mix_tiles(taste);
        self.board(|b| {
            tiles[0].covers = cover_ids(b.favourites.as_deref().unwrap_or_default());
            for (tile, spec) in tiles.iter_mut().skip(1).zip(&MIXES) {
                tile.covers = b.drawn.get(spec.id).map(|d| cover_ids(&d.songs)).unwrap_or_default();
            }
        });
        tiles
    }

    /// One mix page. Its list does not change while it is open unless asked to (favourites follow the hearts).
    pub fn mix_page(&self, id: String) -> MixLookup {
        if id == FAVOURITES_MIX {
            return self.board(|b| match &b.favourites {
                Some(songs) => MixLookup::Ready {
                    sheet: MixSheet { id: id.clone(), title: "Favourites".into(), covers: cover_ids(songs), songs: songs.clone(), refreshable: false, favourites: true },
                },
                None => MixLookup::NotDrawn,
            });
        }
        let Some(spec) = spec_of(&id) else { return MixLookup::Unknown { message: format!("There is no mix called {id}") } };
        self.board(|b| match b.drawn.get(spec.id) {
            Some(d) => MixLookup::Ready {
                sheet: MixSheet { id: spec.id.into(), title: spec.title.into(), songs: d.songs.clone(), covers: cover_ids(&d.songs), refreshable: spec.refreshable(), favourites: false },
            },
            None => MixLookup::NotDrawn,
        })
    }
}

impl Core {
    fn favourites_with(&self, starred_songs: Vec<Song>, marks: &HashMap<String, bool>) -> bool {
        let mut m = stars::Marks::new(marks);
        let kept = distinct(starred_songs.into_iter().filter(|s| playable(s) && m.kept("id", &s.id)));
        self.board(|b| {
            let changed = b.favourites.as_ref() != Some(&kept);
            b.favourites = Some(kept);
            changed
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::song;

    fn ids(l: &[Song]) -> Vec<&str> {
        l.iter().map(|s| s.id.as_str()).collect()
    }

    fn sheet(core: &Core, id: &str) -> MixSheet {
        match core.mix_page(id.into()) {
            MixLookup::Ready { sheet } => sheet,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn tiles_wear_their_own_colour_deepened_towards_black() {
        let c = mix_tile_colours("top".into());
        // Compose's blend(Color(0xFFE0662B), Color.Black, 0.45f), and the band's alphas in 8 bits.
        assert_eq!(c, [0xFFE0_662B, 0xFF7B_3818, 0x007B_3818, 0xB87B_3818, 0xF07B_3818]);
        assert_eq!(mix_tile_colours(FAVOURITES_MIX.into())[0], FAVOURITES_COLOUR);
        assert_eq!(mix_tile_colours("gone".into())[0], OTHER_COLOUR);
    }

    #[test]
    fn playable_leaves_out_provider_items() {
        let mut s = song("1", "a", "b", "c", "", 0);
        assert!(playable(&s));
        s.is_external = true;
        assert!(!playable(&s));
        assert!(!playable(&song("ext-deezer-1", "a", "b", "c", "", 0)));
        assert!(!playable(&song("pl-1", "a", "b", "c", "", 0)));
    }

    #[test]
    fn covers_are_four_distinct() {
        let mut l: Vec<Song> = (0..8).map(|i| song(&i.to_string(), "t", "a", "b", "", 0)).collect();
        l[1].cover_art = l[0].cover_art.clone();
        l[2].cover_art = None;
        assert_eq!(cover_ids(&l), ["cv-0", "cv-3", "cv-4", "cv-5"]);
    }

    #[test]
    fn catalogue_and_tiles() {
        assert_eq!(mix_catalogue().iter().map(|m| m.id.as_str()).collect::<Vec<_>>(), ["quick-picks", "discover", "discover-weekly", "listen-again", "top"]);
        assert!(mix_catalogue().iter().all(|m| m.refreshable == (m.id != "top")));
        assert_eq!(mix_tiles(false).len(), 1);
        assert_eq!(mix_tiles(true).len(), 6);
        assert!(mix_tiles(true)[0].favourites);
    }

    #[test]
    fn empty_index_asks_for_the_fallback_and_keeps_the_period() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        assert_eq!(core.mix_draw("nope".into(), 100, false, None), MixDraw::Unknown);
        assert!(matches!(core.mix_page("nope".into()), MixLookup::Unknown { message } if message == "There is no mix called nope"));
        assert_eq!(core.mix_page("discover".into()), MixLookup::NotDrawn);
        assert_eq!(core.mix_draw("discover".into(), 100, false, None), MixDraw::NeedsFallback);
        let random = vec![song("r1", "t", "a", "b", "", 0), song("ext-r2", "t", "a", "b", "", 0), song("r1", "t", "a", "b", "", 0)];
        assert_eq!(core.mix_draw("discover".into(), 100, false, Some(random)), MixDraw::Drawn);
        assert_eq!(ids(&sheet(&core, "discover").songs), ["r1"]);
        assert_eq!(core.mix_draw("discover".into(), 100, false, None), MixDraw::Kept);
        // A failed request for random songs still counts as this period's draw.
        assert_eq!(core.mix_draw("top".into(), 100, false, None), MixDraw::NeedsFallback);
        assert_eq!(core.mix_draw("top".into(), 100, false, Some(vec![])), MixDraw::Drawn);
        assert_eq!(core.mix_draw("top".into(), 100, false, None), MixDraw::Kept);
        assert!(!sheet(&core, "top").refreshable);
        let warm = core.mix_warm(100);
        assert_eq!(warm.needs_fallback, ["quick-picks", "discover-weekly", "listen-again"]);
        assert!(!warm.changed);
    }

    #[test]
    fn draws_follow_the_period_and_again_redraws() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let all: Vec<Song> = (0..60).map(|i| song(&format!("s{i}"), "t", &format!("Artist {}", i % 12), &format!("LP {i}"), "Rock", 1990)).collect();
        db::index(&mut core.db.lock(), &[], &[], &all).unwrap();
        let today = 20_000;
        // With no history Discover is a seeded walk through the library, which is all this needs.
        let expected = |seed: i64| ids(&discover(&core.db.lock(), DRAW, seed as u64, db::now_ms()).unwrap()).into_iter().map(String::from).collect::<Vec<_>>();
        assert_eq!(core.mix_draw("discover".into(), today, false, None), MixDraw::Drawn);
        let first = sheet(&core, "discover").songs;
        assert!(!first.is_empty());
        assert_eq!(ids(&first), expected(today * 1000));
        assert_eq!(core.mix_draw("discover".into(), today, false, None), MixDraw::Kept);
        assert_eq!(core.mix_draw("discover".into(), today, true, None), MixDraw::Drawn);
        assert_eq!(ids(&sheet(&core, "discover").songs), expected(today * 1000 + 1));
        assert_eq!(core.mix_draw("discover".into(), today, true, None), MixDraw::Drawn);
        assert_eq!(ids(&sheet(&core, "discover").songs), expected(today * 1000 + 2));
        // A new day starts again from generation 0.
        assert_eq!(core.mix_draw("discover".into(), today + 1, false, None), MixDraw::Drawn);
        assert_eq!(ids(&sheet(&core, "discover").songs), expected((today + 1) * 1000));
        // Weekly: the same seven days are one period, offset from the daily seeds.
        assert_eq!(core.mix_draw("discover-weekly".into(), 7 * 3000, false, None), MixDraw::Drawn);
        assert_eq!(core.mix_draw("discover-weekly".into(), 7 * 3000 + 6, false, None), MixDraw::Kept);
        assert_eq!(ids(&sheet(&core, "discover-weekly").songs), expected(3000 * 1000 + 7_000_000));
        let cards = core.mix_cards(true);
        assert_eq!(cards.len(), 6);
        assert_eq!(cards[2].id, "discover");
        assert_eq!(cards[2].covers.len(), 4);
        assert!(cards[1].covers.is_empty());
    }

    #[test]
    fn favourites_follow_the_marks_and_each_core_has_its_own_board() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let other = Core::new(String::new(), "t".into()).unwrap();
        assert_eq!(core.mix_page(FAVOURITES_MIX.into()), MixLookup::NotDrawn);
        let starred = vec![song("1", "t", "a", "b", "", 0), song("2", "t", "a", "b", "", 0), song("ext-3", "t", "a", "b", "", 0), song("1", "t", "a", "b", "", 0)];
        assert!(core.favourites_with(starred.clone(), &HashMap::new()));
        assert!(!core.favourites_with(starred.clone(), &HashMap::new()));
        assert_eq!(ids(&sheet(&core, FAVOURITES_MIX).songs), ["1", "2"]);
        assert!(core.favourites_with(starred, &HashMap::from([("id:1".to_string(), false)])));
        let fav = sheet(&core, FAVOURITES_MIX);
        assert_eq!(ids(&fav.songs), ["2"]);
        assert!(fav.favourites && !fav.refreshable);
        assert_eq!(core.mix_cards(false)[0].covers, ["cv-2"]);
        assert_eq!(other.mix_page(FAVOURITES_MIX.into()), MixLookup::NotDrawn);
    }
}
