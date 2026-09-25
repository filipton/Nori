//! Every key binding, in one table: dispatch reads it and the help overlay (`?`) lists it, so the two
//! cannot disagree.

use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// What a key does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Action {
    Quit,
    Help,
    TogglePlay,
    Next,
    Previous,
    SeekBack,
    SeekForward,
    SeekBackLong,
    SeekForwardLong,
    VolumeUp,
    VolumeDown,
    Search,
    NextPane,
    PreviousPane,
    Mouse,
    Images,
    Shuffle,
    Repeat,
    Screen(u8),
    NextScreen,
    PreviousScreen,
    Back,
    Refresh,
    // Lists
    Up,
    Down,
    Top,
    Bottom,
    PageUp,
    PageDown,
    Open,
    Enqueue,
    PlayNext,
    Download,
    Star,
    PlayAll,
    ShuffleAll,
    TabLeft,
    TabRight,
    // The queue
    Remove,
    MoveUp,
    MoveDown,
    // Lyrics
    Sooner,
    Later,
    Unnudge,
    // Values (settings, the equalizer)
    Decrease,
    Increase,
}

/// Where a binding applies. Screens are looked through first, then lists, then everything.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
    Global,
    List,
    Queue,
    Lyrics,
    Values,
}

impl Scope {
    pub fn title(self) -> &'static str {
        match self {
            Scope::Global => "Everywhere",
            Scope::List => "Lists and pages",
            Scope::Queue => "Queue",
            Scope::Lyrics => "Lyrics",
            Scope::Values => "Settings and equalizer",
        }
    }
}

pub struct Binding {
    pub scope: Scope,
    pub keys: &'static [(KeyCode, KeyModifiers)],
    pub label: &'static str,
    pub action: Action,
    pub help: &'static str,
}

const N: KeyModifiers = KeyModifiers::NONE;
const S: KeyModifiers = KeyModifiers::SHIFT;
const C: KeyModifiers = KeyModifiers::CONTROL;

macro_rules! b {
    ($scope:ident, $label:expr, $action:expr, $help:expr, [$($k:expr),*]) => {
        Binding { scope: Scope::$scope, keys: &[$($k),*], label: $label, action: $action, help: $help }
    };
}

use KeyCode::*;

pub const BINDINGS: &[Binding] = &[
    b!(Global, "space", Action::TogglePlay, "Play or pause", [(Char(' '), N)]),
    b!(Global, "n", Action::Next, "Next song", [(Char('n'), N)]),
    b!(Global, "p", Action::Previous, "Previous song (or the start of this one)", [(Char('p'), N)]),
    b!(Global, "← / ,", Action::SeekBack, "Back 5 seconds", [(Left, N), (Char(','), N)]),
    b!(Global, "→ / .", Action::SeekForward, "Forward 5 seconds", [(Right, N), (Char('.'), N)]),
    b!(Global, "shift ← / <", Action::SeekBackLong, "Back 30 seconds", [(Left, S), (Char('<'), N), (Char('<'), S)]),
    b!(Global, "shift → / >", Action::SeekForwardLong, "Forward 30 seconds", [(Right, S), (Char('>'), N), (Char('>'), S)]),
    b!(Global, "+ / =", Action::VolumeUp, "Volume up", [(Char('+'), N), (Char('+'), S), (Char('='), N)]),
    b!(Global, "-", Action::VolumeDown, "Volume down", [(Char('-'), N)]),
    b!(Global, "s", Action::Shuffle, "Shuffle the queue on or off", [(Char('s'), N)]),
    b!(Global, "r", Action::Repeat, "Repeat: off, all, one", [(Char('r'), N)]),
    b!(Global, "/", Action::Search, "Search", [(Char('/'), N)]),
    b!(Global, "tab", Action::NextPane, "Next pane", [(Tab, N)]),
    b!(Global, "shift tab", Action::PreviousPane, "Previous pane", [(BackTab, S), (BackTab, N)]),
    b!(Global, "1 … 9", Action::Screen(0), "Home, Library, Search, Queue, Playing, Lyrics, Downloads, Equalizer, Settings", [(Char('1'), N)]),
    b!(Global, "", Action::Screen(1), "", [(Char('2'), N)]),
    b!(Global, "", Action::Screen(2), "", [(Char('3'), N)]),
    b!(Global, "", Action::Screen(3), "", [(Char('4'), N)]),
    b!(Global, "", Action::Screen(4), "", [(Char('5'), N)]),
    b!(Global, "", Action::Screen(5), "", [(Char('6'), N)]),
    b!(Global, "", Action::Screen(6), "", [(Char('7'), N)]),
    b!(Global, "", Action::Screen(7), "", [(Char('8'), N)]),
    b!(Global, "", Action::Screen(8), "", [(Char('9'), N)]),
    b!(Global, "H / L", Action::PreviousScreen, "Previous or next screen", [(Char('H'), S), (Char('H'), N)]),
    b!(Global, "", Action::NextScreen, "", [(Char('L'), S), (Char('L'), N)]),
    b!(Global, "esc / h / ⌫", Action::Back, "Back out of a page", [(Esc, N), (Char('h'), N), (Backspace, N)]),
    b!(Global, "R", Action::Refresh, "Ask the server again", [(Char('R'), S), (Char('R'), N)]),
    b!(Global, "m", Action::Mouse, "Mouse on or off (off: the terminal selects text)", [(Char('m'), N)]),
    b!(Global, "I", Action::Images, "Covers on or off", [(Char('I'), S), (Char('I'), N)]),
    b!(Global, "?", Action::Help, "This help", [(Char('?'), N), (Char('?'), S), (F(1), N)]),
    b!(Global, "q / ctrl c", Action::Quit, "Quit", [(Char('q'), N), (Char('c'), C)]),
    b!(List, "↑ ↓ / k j", Action::Up, "Move", [(Up, N), (Char('k'), N)]),
    b!(List, "", Action::Down, "", [(Down, N), (Char('j'), N)]),
    b!(List, "g / G", Action::Top, "First or last", [(Home, N), (Char('g'), N)]),
    b!(List, "", Action::Bottom, "", [(End, N), (Char('G'), S), (Char('G'), N)]),
    b!(List, "pgup pgdn / ctrl u d", Action::PageUp, "A page up or down", [(PageUp, N), (Char('u'), C)]),
    b!(List, "", Action::PageDown, "", [(PageDown, N), (Char('d'), C)]),
    b!(List, "enter / l", Action::Open, "Open, or play from here", [(Enter, N), (Char('l'), N)]),
    b!(List, "a", Action::Enqueue, "Add to the queue", [(Char('a'), N)]),
    b!(List, "A", Action::PlayNext, "Play next", [(Char('A'), S), (Char('A'), N)]),
    b!(List, "x / X", Action::PlayAll, "Play the whole page, or shuffle it", [(Char('x'), N)]),
    b!(List, "", Action::ShuffleAll, "", [(Char('X'), S), (Char('X'), N)]),
    b!(List, "D", Action::Download, "Download", [(Char('D'), S), (Char('D'), N)]),
    b!(List, "f", Action::Star, "Favourite or not", [(Char('f'), N)]),
    b!(List, "[ ]", Action::TabLeft, "Albums, artists, playlists, songs", [(Char('['), N)]),
    b!(List, "", Action::TabRight, "", [(Char(']'), N)]),
    b!(Queue, "enter", Action::Open, "Play this song", [(Enter, N)]),
    b!(Queue, "d / delete", Action::Remove, "Take it out of the queue (a download: off this computer; the equalizer: the band)", [(Char('d'), N), (Delete, N)]),
    b!(Queue, "K / J", Action::MoveUp, "Move it up or down", [(Char('K'), S), (Char('K'), N)]),
    b!(Queue, "", Action::MoveDown, "", [(Char('J'), S), (Char('J'), N)]),
    b!(Lyrics, "[ ]", Action::Later, "Words later or sooner, for lyrics timed wrong", [(Char('['), N)]),
    b!(Lyrics, "", Action::Sooner, "", [(Char(']'), N)]),
    b!(Lyrics, "0", Action::Unnudge, "Back to their own timing", [(Char('0'), N)]),
    b!(Lyrics, "enter", Action::Open, "Play from that line", [(Enter, N)]),
    b!(Values, "← → / h l", Action::Decrease, "Change the value (seek with , and . here)", [(Left, N), (Char('h'), N)]),
    b!(Values, "", Action::Increase, "", [(Right, N), (Char('l'), N)]),
    b!(Values, "enter", Action::Open, "Switch, choose or open", [(Enter, N)]),
];

/// The action `key` has in the first of `scopes` that binds it.
pub fn action(key: &KeyEvent, scopes: &[Scope]) -> Option<Action> {
    // Shift is part of the character for letters and symbols; a terminal may or may not report it.
    let mods = key.modifiers & (KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT);
    for scope in scopes {
        for b in BINDINGS.iter().filter(|b| b.scope == *scope) {
            if b.keys.iter().any(|(code, m)| *code == key.code && *m == mods) {
                return Some(b.action);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(code: KeyCode, m: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, m)
    }

    #[test]
    fn a_screen_binding_wins_over_a_global_one() {
        let right = key(Right, N);
        assert_eq!(action(&right, &[Scope::Global]), Some(Action::SeekForward));
        assert_eq!(action(&right, &[Scope::Values, Scope::List, Scope::Global]), Some(Action::Increase));
        assert_eq!(action(&key(Char('h'), N), &[Scope::List, Scope::Global]), Some(Action::Back));
        assert_eq!(action(&key(Char('d'), N), &[Scope::Queue, Scope::List, Scope::Global]), Some(Action::Remove));
        assert_eq!(action(&key(Char('G'), S), &[Scope::List, Scope::Global]), Some(Action::Bottom));
    }

    #[test]
    fn every_action_bound_is_listed_in_the_help() {
        // A row with no label continues the one above it, so every binding is reachable from the help.
        let mut labelled = false;
        for b in BINDINGS {
            labelled |= !b.label.is_empty();
            assert!(labelled, "{:?} has no help row", b.action);
            assert!(!b.keys.is_empty());
        }
    }

    #[test]
    fn no_key_is_bound_twice_in_one_scope() {
        for (i, a) in BINDINGS.iter().enumerate() {
            for b in &BINDINGS[i + 1..] {
                if a.scope == b.scope {
                    for k in a.keys {
                        assert!(!b.keys.contains(k), "{k:?} twice in {:?}", a.scope);
                    }
                }
            }
        }
    }
}
