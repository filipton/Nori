//! The queue the app plays, owned here (nori_player::playlist). The platform's player mirrors it: a
//! change to the queue is made here first and the answer says where it lands and, while shuffling,
//! the play order to write. Everything that asks what the queue is - what comes next, the transition
//! planner's window, ReplayGain, the queue as the app lists it, the queue saved for next time - reads
//! it here, without the player's list crossing over.

use nori_player::playlist::{Hand, Playlist, Splice};
use nori_player::queue::Onto;
use parking_lot::Mutex;

use crate::queue;
use crate::{alog, Core, Song};

static LIST: Mutex<Playlist> = Mutex::new(Playlist::new());
/// The planner's window as it was last handed over, so it is handed over only when it changes.
static WINDOW: Mutex<(Vec<String>, bool)> = Mutex::new((Vec::new(), false));

pub(crate) fn with<R>(f: impl FnOnce(&Playlist) -> R) -> R {
    f(&LIST.lock())
}

fn seed() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(1, |d| d.as_nanos() as u64)
}

/// Where a change landed, for the player to make the same change.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct QueueChange {
    /// The list index: the song to start at for a new queue, where the songs went for an insert; -1 none.
    pub at: i32,
    /// The play order to give the player while shuffling (list indexes); none when not shuffling.
    pub order: Option<Vec<u32>>,
}

fn change(p: &Playlist, at: Option<usize>) -> QueueChange {
    QueueChange { at: at.map_or(-1, |a| a as i32), order: p.shuffle_order().map(|o| o.iter().map(|&i| i as u32).collect()) }
}

fn edit(f: impl FnOnce(&mut Playlist) -> Option<usize>) -> QueueChange {
    let mut p = LIST.lock();
    let at = f(&mut p);
    change(&p, at)
}

/// A change the player makes as given: `remove` holds (from, to) pairs, last first, flattened; then
/// `songs` go in at `at`; then, when not -1, a jump to `seek`; and while shuffling the play order.
#[derive(Debug, Clone, uniffi::Record)]
pub struct QueueEdit {
    pub remove: Vec<u32>,
    pub at: u32,
    pub songs: Vec<Song>,
    pub seek: i32,
    pub order: Option<Vec<u32>>,
}

pub(crate) fn edit_splice(f: impl FnOnce(&mut Playlist) -> Option<Splice>, songs: Vec<Song>) -> Option<QueueEdit> {
    let mut p = LIST.lock();
    let s = f(&mut p)?;
    Some(QueueEdit {
        remove: s.remove.iter().flat_map(|&(a, b)| [a as u32, b as u32]).collect(),
        at: s.at as u32,
        songs,
        seek: s.seek.map_or(-1, |i| i as i32),
        order: change(&p, None).order,
    })
}

/// The server is back: the offline bridge's songs go and the parked song plays. None when not bridging.
#[uniffi::export]
pub fn playlist_unbridge() -> Option<QueueEdit> {
    edit_splice(|p| p.unbridge(), Vec::new())
}

/// Whether the offline bridge is playing, and whether it has run out before the parked song.
#[uniffi::export]
pub fn playlist_bridge_state() -> BridgeState {
    with(|p| BridgeState { bridging: p.bridging(), next_is_parked: p.next_is_parked(), parked: p.parked_id().map(str::to_string), current: p.current_id().map(str::to_string) })
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct BridgeState {
    pub bridging: bool,
    pub next_is_parked: bool,
    /// The song the queue picks up at once the server is back, and the one playing.
    pub parked: Option<String>,
    pub current: Option<String>,
}

/// A new queue (`start` -1: wherever shuffle starts).
#[uniffi::export]
pub fn playlist_set(ids: Vec<String>, start: i32, shuffle: bool) -> QueueChange {
    edit(|p| p.set(ids, usize::try_from(start).ok(), shuffle, seed()))
}

/// A new queue already in the order it plays, shown as shuffled (a weighted shuffle).
#[uniffi::export]
pub fn playlist_set_ordered(ids: Vec<String>) -> QueueChange {
    edit(|p| p.set_ordered(ids))
}

#[uniffi::remote(Enum)]
pub enum Hand {
    No,
    Next,
    Last,
    Bridge,
}

/// Songs a controller adds at `at`, each marked with how it came (`hands`, one per song: Play next, Add
/// to queue, or neither). Where they go is `nori_player::playlist::Playlist::take`'s call.
#[uniffi::export]
pub fn playlist_take(at: u32, ids: Vec<String>, hands: Vec<Hand>) -> QueueChange {
    edit(|p| Some(p.take(at as usize, ids, &hands)))
}

#[uniffi::export]
pub fn playlist_remove(from: u32, to: u32) -> QueueChange {
    edit(|p| {
        p.remove(from as usize, to as usize);
        p.current()
    })
}

#[uniffi::export]
pub fn playlist_move(from: u32, to: u32, new_index: u32) -> QueueChange {
    edit(|p| {
        p.move_range(from as usize, to as usize, new_index as usize);
        p.current()
    })
}

#[uniffi::export]
pub fn playlist_shuffle(on: bool) -> QueueChange {
    edit(|p| {
        p.set_shuffle(on, seed());
        p.current()
    })
}

/// The user asked for shuffle on or off (a plain Play, Shuffle, the shuffle button): shown so at once,
/// before the queue's own change arrives, which then says the same.
#[uniffi::export]
pub fn playlist_show_shuffle(on: bool) {
    LIST.lock().show_shuffle(on);
}

/// Whether shuffle is shown as on: the queue is shuffled, or was put in a shuffled order before it was
/// queued (a weighted shuffle), and the user has not turned it off since.
#[uniffi::export]
pub fn playlist_shuffle_shown() -> bool {
    with(|p| p.lit())
}

#[uniffi::export]
pub fn playlist_repeat(mode: u8) {
    LIST.lock().set_repeat(mode);
}

/// The player moved to `index` by itself: a song ended, a seek to another song.
#[uniffi::export]
pub fn playlist_moved_to(index: i32) {
    if let Ok(i) = usize::try_from(index) {
        LIST.lock().moved_to(i);
    }
}

#[uniffi::remote(Enum)]
pub enum Onto {
    Skip,
    Loop,
    Song,
}

/// The player moved onto `index` (-1: onto nothing), `looped` by its own repeat. What that means is
/// `nori_player::queue::arrival`'s call over this queue and the user's "skip explicit songs"; a new song
/// also breaks a run of songs that would not play.
#[uniffi::export]
pub fn playlist_transition(index: i32, looped: bool) -> Onto {
    playlist_moved_to(index);
    let skip_explicit = crate::rules::prefs(|p| p.skip_explicit);
    let (current, has_next) = with(|p| (p.current_id().map(str::to_string), p.next().is_some()));
    let explicit = current.clone().is_some_and(|id| queue::queue_flags(id) & queue::EXPLICIT != 0);
    let a = nori_player::queue::arrival(current.is_some(), skip_explicit, explicit, has_next, looped);
    if a == Onto::Song {
        crate::rules::song_started();
    }
    a
}

/// The player's list after it changed, checked against this one. Every change is meant to be made here
/// first; one that was not (a path that edits the player directly) is taken as it is, and said in the
/// log so that path can be found. True when it had to be taken.
#[uniffi::export]
pub fn playlist_follow(ids: Vec<String>, current: i32, shuffling: bool, order: Vec<u32>) -> bool {
    let mut p = LIST.lock();
    let order: Vec<usize> = order.into_iter().map(|i| i as usize).collect();
    let same = p.ids() == ids.as_slice()
        && p.current() == usize::try_from(current).ok()
        && p.shuffling() == shuffling
        && (!shuffling || p.shuffle_order() == Some(order.as_slice()));
    if same {
        return false;
    }
    alog::info(&format!("queue: the player's list differs ({} vs {} songs), following it", ids.len(), p.len()));
    p.adopt(ids, usize::try_from(current).ok(), shuffling.then_some(order));
    true
}

/// How many songs still follow the current one in play order, repeat left out.
#[uniffi::export]
pub fn playlist_after() -> u32 {
    with(|p| p.songs_after() as u32)
}

/// The songs coming up, the current one first, at most `n`, in play order.
#[uniffi::export]
pub fn playlist_upcoming(n: u32) -> Vec<String> {
    with(|p| p.upcoming().take(n as usize).map(|i| p.ids()[i].clone()).collect())
}

/// How many songs the planner's window holds after the one before the current one.
const WINDOW_LEN: usize = 8;

/// Hands the transition planner its window - the song before the current one, then the current one
/// and those after it as the player will walk them, repeat included - when it changed. True when it
/// did, so the platform asks for a new plan.
#[uniffi::export]
pub fn playlist_window() -> bool {
    let (ids, shuffling) = with(|p| {
        let mut ids = Vec::with_capacity(WINDOW_LEN + 1);
        if let Some(c) = p.current() {
            if let Some(b) = p.previous_of(c, p.repeat()) {
                ids.push(p.ids()[b].clone());
            }
            let mut i = Some(c);
            let mut n = 0;
            while let (Some(x), true) = (i, n < WINDOW_LEN) {
                ids.push(p.ids()[x].clone());
                i = p.next_of(x, p.repeat());
                n += 1;
            }
        }
        (ids, p.shuffling())
    });
    let mut w = WINDOW.lock();
    if w.0 == ids && w.1 == shuffling {
        return false;
    }
    queue::queue_window(ids.clone(), shuffling);
    *w = (ids, shuffling);
    true
}

/// The volume the current song plays at under ReplayGain (see `queue::queue_gain`), as the settings
/// say; `bit_perfect` whether the output takes the samples untouched.
#[uniffi::export]
pub fn playlist_gain(bit_perfect: bool) -> f32 {
    let Some(s) = crate::settings_store::current() else { return 1.0 };
    let mode = match s.replay_gain {
        1 => crate::GainMode::Track,
        2 => crate::GainMode::Album,
        3 => crate::GainMode::Auto,
        _ => crate::GainMode::Off,
    };
    let (preamp_db, untagged_db) = (s.preamp_db, s.untagged_gain_db);
    let (before, current, after, shuffling) = with(|p| {
        let id = |i: Option<usize>| i.map(|i| p.ids()[i].clone());
        (id(p.previous()), p.current_id().map(str::to_string), id(p.next()), p.shuffling())
    });
    queue::queue_gain(before, current, after, mode, preamp_db, untagged_db, bit_perfect, shuffling)
}

/// The queue as the app lists it.
#[derive(Debug, Clone, uniffi::Record)]
pub struct PlaylistView {
    pub songs: Vec<Song>,
    /// The list indexes in the order they play.
    pub order: Vec<u32>,
    /// The songs added by hand.
    pub queued: Vec<u32>,
    pub index: i32,
    /// Shuffle shown as on.
    pub shuffle: bool,
    pub repeat: u8,
    /// The offline bridge is playing.
    pub bridging: bool,
    /// Changes whenever the list or its order does.
    pub rev: u64,
}

#[uniffi::export]
pub fn playlist_view() -> PlaylistView {
    let (ids, order, queued, index, shuffle, repeat, bridging, rev) = with(|p| {
        (
            p.ids().to_vec(),
            p.play_order().map(|i| i as u32).collect(),
            p.by_hand().map(|i| i as u32).collect(),
            p.current().map_or(-1, |c| c as i32),
            p.lit(),
            p.repeat(),
            p.bridging(),
            p.rev(),
        )
    });
    PlaylistView { songs: queue::queue_songs(ids), order, queued, index, shuffle, repeat, bridging, rev }
}

/// What the list looks like now, cheaply: changes whenever the list or its order does.
#[uniffi::export]
pub fn playlist_rev() -> u64 {
    with(|p| p.rev())
}

#[uniffi::export]
impl Core {
    /// Saves the queue for next time (radio streams are left out: they do not come back).
    pub fn playlist_save(&self, position_ms: u64) -> crate::Result<()> {
        let (ids, index) = with(|p| (p.ids().to_vec(), p.current().unwrap_or(0) as u32));
        self.queue_save(ids, index, position_ms)
    }
}

/// The queue to hand the server (its "play queue", for picking up on another device): the songs, radio
/// streams left out, and only while the user lets plays be sent to the server at all.
#[uniffi::export]
pub fn playlist_to_push() -> Vec<String> {
    if !crate::rules::prefs(|p| p.scrobble) {
        return Vec::new();
    }
    with(|p| p.ids().iter().filter(|id| !id.starts_with(queue::RADIO_PREFIX)).cloned().collect())
}

/// The ids queued, and the current one, for autofill.
pub(crate) fn snapshot() -> (Option<String>, Vec<String>) {
    with(|p| (p.current_id().map(str::to_string), p.ids().to_vec()))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// The one queue is the process's: tests that use it take turns.
    static TURN: Mutex<()> = Mutex::new(());

    pub(crate) fn hold(ids: &[&str], start: i32) -> parking_lot::MutexGuard<'static, ()> {
        let g = TURN.lock();
        playlist_set(ids.iter().map(|s| s.to_string()).collect(), start, false);
        playlist_repeat(0);
        g
    }

    fn ids(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_window_is_handed_over_only_when_it_changes() {
        let _g = hold(&["w1", "w2", "w3"], 1);
        playlist_window();
        assert!(!playlist_window(), "nothing changed");
        playlist_moved_to(2);
        assert!(playlist_window());
        assert_eq!(WINDOW.lock().0, ids(&["w2", "w3"]));
        playlist_repeat(2);
        assert!(playlist_window());
        assert_eq!(WINDOW.lock().0, ids(&["w2", "w3", "w1", "w2", "w3", "w1", "w2", "w3", "w1"]));
    }

    #[test]
    fn a_change_made_behind_the_queues_back_is_followed() {
        let _g = hold(&["f1", "f2"], 0);
        assert!(!playlist_follow(ids(&["f1", "f2"]), 0, false, vec![0, 1]));
        assert!(playlist_follow(ids(&["f1", "f2", "f3"]), 1, false, vec![0, 1, 2]));
        assert_eq!((playlist_after(), playlist_upcoming(5)), (1, ids(&["f2", "f3"])));
    }

    #[test]
    fn edits_say_where_the_songs_went() {
        let _g = hold(&["e1", "e2", "e3"], 0);
        assert_eq!(playlist_take(9, ids(&["n"]), vec![Hand::Next]), QueueChange { at: 1, order: None });
        assert_eq!(playlist_take(9, ids(&["i"]), vec![Hand::No]).at, 4, "a controller's own insert, clamped to the end");
        let c = playlist_shuffle(true);
        assert_eq!(c.order.as_ref().unwrap()[..2], [0, 1], "the current song, then the one added by hand");
        let v = playlist_view();
        assert_eq!((v.index, v.queued.as_slice(), v.shuffle), (0, &[1u32][..], true));
        assert_eq!(v.songs[1].id, "n");
    }

    #[test]
    fn shuffle_is_shown_as_asked_until_turned_off() {
        let _g = hold(&["s1", "s2"], 0);
        assert!(!playlist_shuffle_shown());
        playlist_show_shuffle(true);
        assert!(playlist_shuffle_shown(), "at once, before the queue changes");
        playlist_set_ordered(ids(&["s2", "s1"]));
        assert!(playlist_shuffle_shown(), "a weighted shuffle stays lit");
        playlist_take(0, ids(&["s3"]), vec![Hand::Last]);
        assert!(playlist_shuffle_shown());
        playlist_show_shuffle(false);
        playlist_shuffle(false);
        assert!(!playlist_shuffle_shown());
    }

    #[test]
    fn arriving_on_a_song() {
        let _g = hold(&["t1", "t2", "radio:9"], 0);
        assert_eq!(playlist_transition(1, false), Onto::Song);
        assert_eq!(playlist_bridge_state().current.as_deref(), Some("t2"));
        assert_eq!(playlist_transition(1, true), Onto::Loop);
        assert_eq!(playlist_to_push(), ids(&["t1", "t2"]), "radio streams are not handed to the server");
    }
}
