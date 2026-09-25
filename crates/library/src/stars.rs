//! This session's star changes, kept here, and laid over a list the server sent earlier.
//!
//! A heart changes the moment it is pressed, long before the server has answered and the favourites
//! list has been asked for again. Until then the list on screen is a snapshot from before the press, so
//! the marks (`"<param>:<id>"` -> starred, where `param` is the Subsonic parameter of the kind: `id`,
//! `albumId`, `artistId`) are applied on top of it: an item whose mark says it is no longer starred leaves
//! the list under the finger. A mark that says "starred" adds nothing, because the snapshot does not hold
//! the item's details; the re-asked list brings it.

use std::collections::HashMap;

use nori_model::{Album, Artist, Song};
use nori_net::requests::Starrable;
use parking_lot::Mutex;

use crate::pages::Starred;

static MARKS: Mutex<Option<HashMap<String, bool>>> = Mutex::new(None);

fn key(kind: Starrable, id: &str) -> String {
    format!("{}:{id}", kind.param())
}

/// This session's marks, one map per kind keyed by the item's id: a screen asks "is this song
/// starred" by id alone and never needs to know how the marks are keyed here.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StarMarks {
    pub songs: HashMap<String, bool>,
    pub albums: HashMap<String, bool>,
    pub artists: HashMap<String, bool>,
}

fn marks_now(m: &Option<HashMap<String, bool>>) -> StarMarks {
    let mut out = StarMarks::default();
    for (k, on) in m.iter().flatten() {
        let Some((param, id)) = k.split_once(':') else { continue };
        let into = match param {
            p if p == Starrable::Song.param() => &mut out.songs,
            p if p == Starrable::Album.param() => &mut out.albums,
            p if p == Starrable::Artist.param() => &mut out.artists,
            _ => continue,
        };
        into.insert(id.to_string(), *on);
    }
    out
}

/// The marks as they are now.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn star_marks() -> StarMarks {
    marks_now(&MARKS.lock())
}

/// This session's marks as they are now, read without copying them.
pub fn with_marks<R>(f: impl FnOnce(&HashMap<String, bool>) -> R) -> R {
    f(MARKS.lock().get_or_insert_with(HashMap::new))
}

/// A heart pressed: the mark goes up at once, before the server is asked, so the heart fills under
/// the finger. `previous` is what [`star_restore`] puts back should the server refuse it.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StarMarked {
    pub previous: Option<bool>,
    pub marks: StarMarks,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn star_mark(kind: Starrable, id: String, on: bool) -> StarMarked {
    let mut m = MARKS.lock();
    let previous = m.get_or_insert_with(HashMap::new).insert(key(kind, &id), on);
    StarMarked { previous, marks: marks_now(&m) }
}

/// The server refused a star change (being offline is not refusing: those are kept and sent later), so
/// the heart must not keep showing it: the mark from before comes back. Returns the marks as they are now.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn star_restore(kind: Starrable, id: String, previous: Option<bool>) -> StarMarks {
    let mut m = MARKS.lock();
    let all = m.get_or_insert_with(HashMap::new);
    match previous {
        Some(on) => all.insert(key(kind, &id), on),
        None => all.remove(&key(kind, &id)),
    };
    marks_now(&m)
}

/// Built once per list into one buffer, so looking up a mark allocates nothing per item.
pub struct Marks<'a> {
    marks: &'a HashMap<String, bool>,
    /// False when no mark says "unstarred": then nothing can leave, and no key is built at all.
    any_removed: bool,
    key: String,
}

impl<'a> Marks<'a> {
    pub fn new(marks: &'a HashMap<String, bool>) -> Self {
        Marks { marks, any_removed: marks.values().any(|on| !on), key: String::new() }
    }

    /// False only when this session unstarred the item; an item without a mark keeps whatever the list says.
    pub fn kept(&mut self, param: &str, id: &str) -> bool {
        if !self.any_removed {
            return true;
        }
        self.key.clear();
        self.key.push_str(param);
        self.key.push(':');
        self.key.push_str(id);
        self.marks.get(&self.key) != Some(&false)
    }
}

pub(crate) fn songs(list: Vec<Song>, marks: &mut Marks) -> Vec<Song> {
    list.into_iter().filter(|s| marks.kept("id", &s.id)).collect()
}

pub(crate) fn albums(list: Vec<Album>, marks: &mut Marks) -> Vec<Album> {
    list.into_iter().filter(|a| marks.kept("albumId", &a.id)).collect()
}

fn artists(list: Vec<Artist>, marks: &mut Marks) -> Vec<Artist> {
    list.into_iter().filter(|a| marks.kept("artistId", &a.id)).collect()
}

fn overlay(starred: Starred, marks: &HashMap<String, bool>) -> Starred {
    let mut m = Marks::new(marks);
    Starred::new(artists(starred.artists, &mut m), albums(starred.albums, &mut m), songs(starred.songs, &mut m))
}

/// The favourites answer as it is read: artists, albums and songs this session unstarred are left out.
pub fn star_overlay(starred: Starred) -> Starred {
    with_marks(|m| overlay(starred, m))
}

/// The favourite albums shelf of the home page, which is an album list rather than a favourites answer.
pub fn star_overlay_albums(albums: Vec<Album>) -> Vec<Album> {
    with_marks(|m| self::albums(albums, &mut Marks::new(m)))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn marks(pairs: &[(&str, bool)]) -> HashMap<String, bool> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    #[test]
    fn only_an_unstarred_mark_removes() {
        let s = |id: &str| Song { id: id.into(), ..Default::default() };
        let a = |id: &str| Album { id: id.into(), ..Default::default() };
        let r = |id: &str| Artist { id: id.into(), ..Default::default() };
        let starred = Starred::new(vec![r("1"), r("2")], vec![a("1"), a("2")], vec![s("1"), s("2"), s("3")]);
        // The same id under another kind's key does not count: an album "1" unstarred is not song "1".
        let out = overlay(starred, &marks(&[("id:2", false), ("id:3", true), ("albumId:1", false), ("artistId:9", false)]));
        assert_eq!(out.songs.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["1", "3"]);
        assert_eq!(out.albums.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["2"]);
        assert_eq!(out.artists.len(), 2);
        assert_eq!(out.library_songs, 2, "counted after the overlay");
        assert_eq!(albums(vec![a("1"), a("2")], &mut Marks::new(&marks(&[("albumId:2", false)]))).len(), 1);
        assert_eq!(albums(vec![a("1")], &mut Marks::new(&HashMap::new())).len(), 1);
    }

    #[test]
    fn a_refused_star_puts_the_mark_from_before_back() {
        let first = star_mark(Starrable::Album, "sm-1".into(), true);
        assert_eq!(first.previous, None);
        assert_eq!(first.marks.albums.get("sm-1"), Some(&true));
        let second = star_mark(Starrable::Album, "sm-1".into(), false);
        assert_eq!(second.previous, Some(true));
        assert_eq!(star_restore(Starrable::Album, "sm-1".into(), second.previous).albums.get("sm-1"), Some(&true));
        assert_eq!(star_restore(Starrable::Album, "sm-1".into(), first.previous).albums.get("sm-1"), None);
        // Split by kind, keyed by the id alone: a song and an album may share an id.
        star_mark(Starrable::Song, "sm-1".into(), true);
        let m = star_marks();
        assert_eq!((m.songs.get("sm-1"), m.albums.get("sm-1"), m.artists.get("sm-1")), (Some(&true), None, None));
        star_mark(Starrable::Album, "sm-2".into(), false);
        assert!(star_overlay_albums(vec![Album { id: "sm-2".into(), ..Default::default() }]).is_empty(), "the overlay reads the marks kept here");
    }
}
