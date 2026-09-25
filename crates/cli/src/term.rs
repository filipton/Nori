//! The terminal itself: raw mode and the alternate screen on the way in, everything put back on the way
//! out - on a clean exit, on an error and on a panic - and stderr sent to a log file while the screen is
//! ours, so the core's log lines (and ALSA's) cannot scribble over it.

use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture};
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{cursor, execute};
use ratatui::Terminal;

pub type Term = Terminal<CrosstermBackend<io::Stdout>>;

/// The terminal's stderr from before it was sent to the log, for a panic's message; -1 while it was not.
static STDERR: AtomicI32 = AtomicI32::new(-1);
static MOUSE: AtomicBool = AtomicBool::new(false);
static ON: AtomicBool = AtomicBool::new(false);

/// Sends stderr to `log` (truncated), keeping the terminal's own for a panic.
pub fn stderr_to(log: &Path) {
    let Ok(file) = std::fs::File::create(log) else { return };
    // SAFETY: plain descriptor calls on descriptors this process owns; the file stays open as fd 2.
    unsafe {
        let saved = libc::dup(2);
        if saved >= 0 && libc::dup2(file.as_raw_fd(), 2) >= 0 {
            STDERR.store(saved, Ordering::SeqCst);
        }
    }
}

/// Writes to the terminal's own stderr, or the log's when it was never moved.
fn to_terminal(text: &str) {
    let fd: RawFd = match STDERR.load(Ordering::SeqCst) {
        -1 => 2,
        fd => fd,
    };
    // SAFETY: a write to a descriptor this process owns.
    unsafe {
        libc::write(fd, text.as_ptr().cast(), text.len());
    }
}

/// Raw mode, the alternate screen, bracketed paste and (when asked) the mouse.
pub fn enter(mouse: bool) -> io::Result<Term> {
    enable_raw_mode()?;
    let mut out = io::stdout();
    execute!(out, EnterAlternateScreen, EnableBracketedPaste, cursor::Hide)?;
    ON.store(true, Ordering::SeqCst);
    set_mouse(mouse);
    let mut t = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    t.clear()?;
    Ok(t)
}

/// Mouse capture on or off. Off, the terminal's own selection and copy work again.
pub fn set_mouse(on: bool) {
    let mut out = io::stdout();
    let _ = if on { execute!(out, EnableMouseCapture) } else { execute!(out, DisableMouseCapture) };
    MOUSE.store(on, Ordering::SeqCst);
}

/// Everything [`enter`] changed, put back. Safe to call twice, and from the panic hook.
pub fn leave() {
    if !ON.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = io::stdout();
    if MOUSE.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, DisableMouseCapture);
    }
    let _ = execute!(out, DisableBracketedPaste, LeaveAlternateScreen, cursor::Show);
    let _ = disable_raw_mode();
    let _ = out.flush();
}

/// A panic puts the terminal back before it says anything, and says it where it can be read.
pub fn hook_panics() {
    let default = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        // Only the screen's own thread ends the program; a worker's panic (a cover that would not decode,
        // which the loader catches and goes on from) is only logged.
        if std::thread::current().name() == Some("main") {
            leave();
            to_terminal(&format!("nori: {info}\n"));
        }
        default(info);
    }));
}
