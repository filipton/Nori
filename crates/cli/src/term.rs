//! The terminal itself: raw mode and the alternate screen on the way in, everything put back on the way
//! out - on a clean exit, on an error and on a panic - and stderr sent to a log file while the screen is
//! ours, so the core's log lines (and ALSA's) cannot scribble over it.

use std::io::{self, Write};
use std::os::fd::{AsRawFd, RawFd};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};

use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste, EnableFocusChange, EnableMouseCapture};
use ratatui::crossterm::terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen};
use ratatui::crossterm::{cursor, execute};
use ratatui::Terminal;

pub type Term = Terminal<CrosstermBackend<io::Stdout>>;

/// The terminal's stderr from before it was sent to the log, for a panic's message; -1 while it was not.
static STDERR: AtomicI32 = AtomicI32::new(-1);
static MOUSE: AtomicBool = AtomicBool::new(false);
static ON: AtomicBool = AtomicBool::new(false);
static DEBUG: AtomicBool = AtomicBool::new(false);

/// Every message the loop takes, every frame drawn and every picture sent written to nori.log too
/// (`--debug`, or NORI_DEBUG=1).
pub fn set_debug(on: bool) {
    DEBUG.store(on, Ordering::Relaxed);
}

pub fn debugging() -> bool {
    DEBUG.load(Ordering::Relaxed)
}

/// A line for nori.log when debugging, made only then.
macro_rules! debug {
    ($($t:tt)*) => {
        if $crate::term::debugging() {
            eprintln!("nori debug {:?}: {}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() % 100_000), format!($($t)*));
        }
    };
}
pub(crate) use debug;

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
    // Focus reports: coming back to the terminal (or, under tmux, to the pane's window) draws the whole
    // screen again, pictures included.
    execute!(out, EnterAlternateScreen, EnableBracketedPaste, EnableFocusChange, cursor::Hide)?;
    ON.store(true, Ordering::SeqCst);
    set_mouse(mouse);
    let mut t = Terminal::new(CrosstermBackend::new(io::stdout()))?;
    t.clear()?;
    Ok(t)
}

/// The next draw writes every cell again. Not `Terminal::clear`, which asks the terminal where its
/// cursor is and waits for the answer, while the input thread takes that answer as a key and the ask
/// fails (the program ended). A blank frame first leaves ratatui's last frame blank, the screen is then
/// erased (pictures too), and the next draw writes everything that is not blank.
pub fn repaint(t: &mut Term) -> io::Result<()> {
    use ratatui::backend::Backend;
    t.draw(|_| {})?;
    t.backend_mut().clear_region(ratatui::backend::ClearType::All)?;
    Ok(())
}

/// Whether the program runs inside tmux.
pub fn in_tmux() -> bool {
    std::env::var_os("TMUX").is_some_and(|v| !v.is_empty())
}

/// A tmux command's answer, if tmux ran.
fn tmux(args: &[&str]) -> Option<String> {
    let o = std::process::Command::new("tmux").args(args).stdin(std::process::Stdio::null()).stderr(std::process::Stdio::null()).output().ok()?;
    o.status.success().then(|| String::from_utf8_lossy(&o.stdout).trim_end_matches('\n').to_string())
}

/// Whether tmux draws sixel pictures itself: only on a terminal that tmux found draws them (its client's
/// `sixel` feature). tmux built with sixel says it draws them when asked (the device attributes), whatever
/// the terminal outside is; on one that does not (kitty, Ghostty, WezTerm, alacritty) it shows a box of
/// `+` signs saying SIXEL IMAGE where the picture was sent.
pub fn tmux_draws_sixel() -> bool {
    let features = tmux(&["display", "-p", "#{client_termfeatures}"]);
    eprintln!("nori: tmux's terminal features: {}", features.as_deref().unwrap_or("(tmux did not say)"));
    features.is_some_and(|f| sixel_feature(&f))
}

/// `sixel` among tmux's comma-separated terminal features.
pub fn sixel_feature(features: &str) -> bool {
    features.split(',').any(|f| f.trim() == "sixel")
}

/// `f` run with tmux's marks taken out of the environment (put back after), so that ratatui-image talks
/// to tmux itself rather than wrapping everything to pass through it. Called before any other thread
/// that reads the environment is started. Its kitty query, not passed through, is taken by tmux for a
/// pane title (an APC sequence): the pane's title is put back.
pub fn not_tmux<R>(f: impl FnOnce() -> R) -> R {
    let pane = std::env::var("TMUX_PANE").unwrap_or_default();
    let title = tmux(&["display", "-p", "-t", &pane, "#{pane_title}"]);
    let (term, program) = (std::env::var_os("TERM"), std::env::var_os("TERM_PROGRAM"));
    std::env::set_var("TERM", "xterm-256color");
    std::env::remove_var("TERM_PROGRAM");
    let r = f();
    match term {
        Some(t) => std::env::set_var("TERM", t),
        None => std::env::remove_var("TERM"),
    }
    if let Some(p) = program {
        std::env::set_var("TERM_PROGRAM", p);
    }
    if let Some(t) = title {
        tmux(&["select-pane", "-t", &pane, "-T", &t]);
    }
    r
}

/// Under tmux, pictures in a graphics protocol are passed through to the terminal tmux runs in (the
/// picker turns the pane's `allow-passthrough` on), and tmux keeps no copy: one sent while the pane's
/// window is not on screen is lost, and switching back redraws only the text. tmux tells a pane that its
/// window came back only with `focus-events` on (off by default), so it is switched on for the server,
/// as tmux's own manual asks of programs that need it.
pub fn tmux_focus_events() {
    if !in_tmux() {
        return;
    }
    if tmux(&["show", "-sv", "focus-events"]).is_some_and(|v| v.trim() != "on") {
        let set = tmux(&["set", "-s", "focus-events", "on"]).is_some();
        eprintln!("nori: tmux focus-events switched on ({set}), so covers are sent again when the window comes back");
    }
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
    let _ = execute!(out, DisableFocusChange, DisableBracketedPaste, LeaveAlternateScreen, cursor::Show);
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
