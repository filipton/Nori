//! nori's terminal client. By default a full-screen player (ratatui over crossterm): log in, browse the
//! library, search, play, edit the queue, read lyrics, change every setting. With `--script` (or any of
//! the old flags: `--search`, `--play`, `--wav`, `--devices`) it is the small non-interactive player it
//! used to be, for scripts and renders.
//!
//! Everything a player decides is the core's and nori-engine's; this crate only draws and reads keys.

mod app;
mod art;
mod backend;
mod keys;
mod lyrics;
mod runner;
mod script;
mod settings_view;
mod term;
mod ui;
#[cfg(test)]
mod tests;

use std::path::PathBuf;

/// What the full-screen client is started with.
pub struct Options {
    /// Where the database, covers, downloads and the stream cache live.
    pub data: PathBuf,
    /// An output device by name (as `--devices` lists them).
    pub device: Option<String>,
    /// Covers in the terminal; None: as the client's own setting says.
    pub images: Option<bool>,
    /// Mouse capture; None: as the client's own setting says.
    pub mouse: Option<bool>,
    /// A server to add and use, from the command line or NORI_URL/NORI_USER/NORI_PASSWORD.
    pub login: Option<(String, String, String)>,
    /// Plays the downloads, asks nothing of the server.
    pub offline: bool,
    /// Serve the desktop's media controls (MPRIS).
    pub mpris: bool,
}

fn usage() -> ! {
    eprintln!(
        "usage: nori-cli [--data DIR] [--device NAME] [--no-images] [--no-mouse] [--no-mpris] [--offline]\n\
         \x20               [--url URL --user USER --password PASSWORD]\n\
         \x20      nori-cli --script ... (the non-interactive player; nori-cli --script --help)\n\
         \x20      nori-cli --devices\n\
         The url, user and password may also come from NORI_URL, NORI_USER and NORI_PASSWORD."
    );
    std::process::exit(2)
}

/// Where the client keeps its files: $XDG_DATA_HOME/nori, else ~/.local/share/nori.
fn data_dir() -> PathBuf {
    if let Some(d) = std::env::var_os("XDG_DATA_HOME").filter(|d| !d.is_empty()) {
        return PathBuf::from(d).join("nori");
    }
    match std::env::var_os("HOME") {
        Some(h) => PathBuf::from(h).join(".local/share/nori"),
        None => std::env::temp_dir().join("nori"),
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let script = args.iter().any(|a| matches!(a.as_str(), "--script" | "--search" | "--play" | "--wav" | "--devices" | "--songs" | "--download"));
    if script {
        let rest = args.into_iter().filter(|a| a != "--script").collect();
        script::main(rest);
        return;
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let mut o = Options { data: data_dir(), device: None, images: None, mouse: None, login: None, offline: false, mpris: true };
    let (mut url, mut user, mut password) = (env("NORI_URL"), env("NORI_USER"), env("NORI_PASSWORD"));
    let mut it = args.into_iter();
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| usage());
        match k.as_str() {
            "--data" => o.data = v().into(),
            "--device" => o.device = Some(v()),
            "--no-images" => o.images = Some(false),
            "--images" => o.images = Some(true),
            "--no-mouse" => o.mouse = Some(false),
            "--mouse" => o.mouse = Some(true),
            "--no-mpris" => o.mpris = false,
            "--offline" => o.offline = true,
            "--url" => url = Some(v()),
            "--user" => user = Some(v()),
            "--password" => password = Some(v()),
            "-h" | "--help" => usage(),
            _ => usage(),
        }
    }
    if let (Some(u), Some(n)) = (url, user) {
        o.login = Some((u, n, password.unwrap_or_default()));
    }
    if let Err(e) = std::fs::create_dir_all(&o.data) {
        eprintln!("nori: {}: {e}", o.data.display());
        std::process::exit(1);
    }
    if let Err(e) = runner::run(o) {
        eprintln!("nori: {e}");
        std::process::exit(1);
    }
}
