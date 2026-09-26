//! A look at the engine as it runs, for a client that keeps watch over it (the Android perf build's
//! invariant watchdogs). The engine says what it sees once per wake of its own thread, after it has
//! reported where the ear is: no timer, no thread and no wake of its own.
//!
//! Nothing is looked at unless a hook is installed and wants it: without one, a wake costs one
//! `OnceLock` read; with one that does not want it (the watch switched off), a call that reads an
//! atomic. What the hook makes of what it sees is the client's.

use std::sync::OnceLock;

/// What the engine's thread saw at the end of one wake.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Seen {
    /// The engine's clock, ms: only differences between two looks mean anything.
    pub now_ms: i64,
    /// Music is wanted and should be moving: playing, no jump waiting out its dip, and no song's bytes
    /// awaited with nothing left to play.
    pub playing: bool,
    /// The songs go to the output's own decoder.
    pub offloaded: bool,
    /// The song heard (queue index, and its id) and where the ear is in it, ms, as the engine last read
    /// the output.
    pub index: Option<usize>,
    pub id: Option<String>,
    pub position_ms: i64,
    /// Music written to the output and not yet heard, ms: what it holds.
    pub in_output_ms: i64,
    /// How long the place heard has stood still while music should be moving (playing, no pause or
    /// jump under way), ms; 0 while it moves or waits for bytes on their way.
    pub quiet_ms: i64,
    /// The song the music waits for has its bytes on their way (its loader still fetching).
    pub bytes_coming: bool,
    /// An output is open for the music: the CPU's device, or the offloaded track.
    pub output_open: bool,
    /// Where the engine stands, in words (the song, what it reads and waits for, the transition engine,
    /// the loaders): what a client quotes when it finds the music stalled. Made only when wanted.
    pub state: String,
}

/// What a client hands the engine to be told what it sees.
pub struct Hook {
    /// Whether the client wants to be told now: asked on every wake, so it should cost next to nothing.
    pub wanted: fn() -> bool,
    /// Told what the engine saw.
    pub seen: fn(&Seen),
}

static HOOK: OnceLock<Hook> = OnceLock::new();

/// Installs the hook for the life of the process; a second one is not taken.
pub fn install(hook: Hook) {
    let _ = HOOK.set(hook);
}

/// Tells the hook what [`Seen`] `make` makes, only when there is one and it wants it.
#[inline]
pub(crate) fn look(make: impl FnOnce() -> Seen) {
    if let Some(h) = HOOK.get() {
        if (h.wanted)() {
            (h.seen)(&make());
        }
    }
}
