//! A terminal client: logs in to a Navidrome/Subsonic server, searches, queues and plays, with the
//! core deciding everything and `nori-engine` playing it. Everything here is the terminal's own:
//! reading arguments and commands, and printing what the engine says.
//!
//! ```text
//! nori-cli --url http://localhost:4533 --user admin --password admin --search "noise" --crossfade 6
//! nori-cli ... --search "noise" --songs 2 --start 570 --crossfade 6 --mix-albums --wav out.wav
//! ```
//!
//! Without `--wav` it plays on the default sound card and reads commands: `play`, `pause`, `next`,
//! `prev`, `seek <s>`, `crossfade <s>|off`, `automix on|off`, `pos`, `positions on|off`, `search <q>`,
//! `queue`, `quit`. With `--wav` it renders the queue to a file, as a sound card would have played it,
//! and exits at the end.

use std::collections::HashMap;
use std::future::Future;
use std::io::BufRead;
use std::path::PathBuf;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Arc;
use std::task::{Context, Poll, Waker};
use std::time::Duration;

use nori_engine::core::{settings, CoreApp, CoreLibrary, CoreQueue};
use nori_engine::{AudioOutput, Config, Engine, Event, State, WavOutput};
use nori_http::Http;
use nori_output_cpal::CpalOutput;
use norimusic::client::{Client, NetProfile};
use norimusic::settings::StoredPrefs;
use norimusic::settings_store::{settings_open, settings_put, APPLY_AUDIO, REPLAN};
use norimusic::transport::Transport;
use norimusic::{Core, Param, ServerConfig, Song};

/// The core's calls are async over a transport that answers at once: polling them once finishes them.
fn block_on<F: Future>(f: F) -> F::Output {
    let mut f = std::pin::pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    loop {
        if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
            return v;
        }
        std::thread::yield_now();
    }
}

struct Args {
    url: String,
    user: String,
    password: String,
    data: PathBuf,
    search: Option<String>,
    songs: usize,
    start_s: f64,
    crossfade: Option<i32>,
    automix: bool,
    /// Crossfade between songs of one album played in order too (the setting keeps them gapless).
    mix_albums: bool,
    wav: Option<PathBuf>,
    pace: f64,
    device: Option<String>,
}

fn usage() -> ! {
    eprintln!(
        "usage: nori-cli --url URL --user USER --password PASSWORD [--search QUERY] [--songs N] [--start SECONDS]\n\
         \x20               [--crossfade SECONDS] [--automix] [--mix-albums] [--wav OUT.wav [--pace X]] [--device NAME] [--devices]\n\
         \x20               [--data DIR]\n\
         The url, user and password may also come from NORI_URL, NORI_USER and NORI_PASSWORD."
    );
    std::process::exit(2)
}

fn args() -> Args {
    let env = |k: &str| std::env::var(k).ok();
    let mut a = Args {
        url: env("NORI_URL").unwrap_or_default(),
        user: env("NORI_USER").unwrap_or_default(),
        password: env("NORI_PASSWORD").unwrap_or_default(),
        data: std::env::temp_dir().join("nori-cli"),
        search: None,
        songs: usize::MAX,
        start_s: 0.0,
        crossfade: None,
        automix: false,
        mix_albums: false,
        wav: None,
        pace: 16.0,
        device: None,
    };
    let mut it = std::env::args().skip(1);
    while let Some(k) = it.next() {
        let mut v = || it.next().unwrap_or_else(|| usage());
        match k.as_str() {
            "--url" => a.url = v(),
            "--user" => a.user = v(),
            "--password" => a.password = v(),
            "--data" => a.data = v().into(),
            "--search" => a.search = Some(v()),
            "--songs" => a.songs = v().parse().unwrap_or_else(|_| usage()),
            "--start" => a.start_s = v().parse().unwrap_or_else(|_| usage()),
            "--crossfade" => a.crossfade = Some(v().parse().unwrap_or_else(|_| usage())),
            "--automix" => a.automix = true,
            "--mix-albums" => a.mix_albums = true,
            "--wav" => a.wav = Some(v().into()),
            "--pace" => a.pace = v().parse().unwrap_or_else(|_| usage()),
            "--device" => a.device = Some(v()),
            "--devices" => {
                for d in CpalOutput::devices() {
                    println!("{d}");
                }
                std::process::exit(0)
            }
            _ => usage(),
        }
    }
    if a.url.is_empty() || a.user.is_empty() {
        usage();
    }
    a
}

/// The client and everything it keeps.
struct Cli {
    core: Arc<Core>,
    http: Arc<Http>,
    prefs: StoredPrefs,
    engine: Engine,
    /// The queue as it was last set, for titles.
    songs: Vec<Song>,
}

impl Cli {
    fn search(&self, query: &str) -> Result<Vec<Song>, String> {
        let p = |k: &str, v: &str| Param { key: k.into(), value: v.into() };
        let url = self.core.url("search3".into(), vec![p("query", query), p("songCount", "50"), p("albumCount", "0"), p("artistCount", "0")]);
        let r = block_on(self.http.get(url, 0)).map_err(|e| e.to_string())?;
        let found = self.core.parse_search(r.body).map_err(|e| e.to_string())?;
        // A provider's song (octo-fiesta's `ext-`) is downloaded by the server the moment it is asked
        // for: never queued from a search.
        Ok(found.songs.into_iter().filter(|s| !s.id.starts_with("ext-") && !s.is_external).collect())
    }

    fn queue(&mut self, songs: Vec<Song>, start_ms: i64) {
        norimusic::queue::queue_register(songs.clone());
        norimusic::playlist::playlist_set(songs.iter().map(|s| s.id.clone()).collect(), 0, false);
        self.songs = songs;
        self.engine.queue_changed();
        self.engine.play_at(0, start_ms);
    }

    /// New settings: kept by the core, and whatever they change applied.
    fn put(&mut self, prefs: StoredPrefs) {
        let effects = settings_put(prefs.clone());
        self.prefs = prefs;
        if effects & APPLY_AUDIO != 0 {
            self.engine.set_settings(settings(&self.prefs));
        }
        if effects & REPLAN != 0 {
            self.engine.replan();
        }
    }
}

fn title(songs: &[Song], index: usize) -> String {
    songs.get(index).map_or_else(|| format!("#{index}"), |s| format!("{} - {} ({}:{:02})", s.artist, s.title, s.duration / 60, s.duration % 60))
}

fn clock(ms: i64) -> String {
    let s = ms.max(0) / 1000;
    format!("{}:{:02}", s / 60, s % 60)
}

fn main() {
    let a = args();
    std::fs::create_dir_all(&a.data).unwrap_or_else(|e| panic!("{}: {e}", a.data.display()));
    let db = a.data.join("nori.db").to_string_lossy().into_owned();
    let core = Core::new(db.clone(), "cli".into()).unwrap_or_else(|e| panic!("the database: {e}"));
    let config = ServerConfig { url: a.url.clone(), user: a.user.clone(), password: a.password.clone(), api_key: None, legacy_auth: false };
    core.configure(config.clone()).unwrap_or_else(|e| panic!("the server: {e}"));
    let http = Http::new();
    let client = Client::new(core.clone(), http.clone());
    client.set_profile(NetProfile { url: a.url.clone(), ..Default::default() });
    if let Err(e) = block_on(client.login(config, String::new())) {
        eprintln!("login failed: {e}");
        std::process::exit(1);
    }
    let mut prefs = settings_open(db, HashMap::new()).unwrap_or_else(|e| panic!("the settings: {e}"));
    prefs.crossfade_sec = a.crossfade.unwrap_or(prefs.crossfade_sec);
    prefs.auto_mix = a.automix;
    prefs.crossfade_keep_albums = !a.mix_albums;
    settings_put(prefs.clone());

    let output: Box<dyn AudioOutput> = match (&a.wav, &a.device) {
        (Some(path), _) => Box::new(WavOutput::new(path, a.pace)),
        (None, Some(name)) => Box::new(CpalOutput::with_device(name)),
        (None, None) => Box::new(CpalOutput::new()),
    };
    let (tx, events): (_, Receiver<Event>) = channel();
    let library = CoreLibrary { client: client.clone(), bytes: http.clone(), metered: false };
    let engine = Engine::start(library, CoreApp::new(), CoreQueue, output, Config { memory_mb: 256, settings: settings(&prefs) }, move |e| {
        let _ = tx.send(e);
    });
    let mut cli = Cli { core, http, prefs, engine, songs: Vec::new() };
    println!("logged in to {} as {}; crossfade {} s, AutoMix {}", a.url, a.user, cli.prefs.crossfade_sec, if cli.prefs.auto_mix { "on" } else { "off" });

    if let Some(q) = &a.search {
        match cli.search(q) {
            Ok(found) if !found.is_empty() => {
                let found: Vec<Song> = found.into_iter().take(a.songs).collect();
                for i in 0..found.len() {
                    println!("  {i}: {}", title(&found, i));
                }
                cli.queue(found, (a.start_s * 1000.0) as i64);
            }
            Ok(_) => println!("nothing found for {q:?}"),
            Err(e) => println!("search failed: {e}"),
        }
    }

    if a.wav.is_some() {
        // Rendering: follow the engine to the end of the queue, then let the file be written.
        loop {
            match events.recv_timeout(Duration::from_secs(600)) {
                Ok(Event::Song { index, .. }) => println!("now: {}", title(&cli.songs, index)),
                Ok(Event::Error { id, message }) => println!("error: {id} {message}"),
                Ok(Event::State(State::Ended)) => break,
                Ok(Event::State(State::Idle)) if cli.songs.is_empty() => break,
                Ok(_) => {}
                Err(_) => {
                    println!("timed out");
                    break;
                }
            }
        }
        let s = cli.engine.status();
        println!("ended; underruns {}", s.underruns);
        cli.engine.stop();
        return;
    }

    // Playing: events are printed as they come, commands read from the terminal.
    let songs = Arc::new(shown::Titles::default());
    let shown = songs.clone();
    std::thread::spawn(move || {
        for e in events {
            match e {
                Event::Song { index, .. } => println!("now: {}", shown.title(index)),
                Event::State(s) => println!("{s:?}"),
                Event::Position { index, ms } => println!("  {} at {}", shown.title(index), clock(ms)),
                Event::Error { id, message } => println!("error: {id} {message}"),
            }
        }
    });
    songs.set(&cli.songs);
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let (cmd, rest) = line.trim().split_once(' ').unwrap_or((line.trim(), ""));
        match cmd {
            "play" => cli.engine.play(),
            "pause" => cli.engine.pause(),
            "p" | "toggle" => cli.engine.toggle(),
            "next" | "n" => cli.engine.next(),
            "prev" | "previous" => cli.engine.previous(),
            "seek" => match rest.parse::<f64>() {
                Ok(s) => cli.engine.seek((s * 1000.0) as i64),
                Err(_) => println!("seek <seconds>"),
            },
            "crossfade" => {
                let secs = if rest == "off" { 0 } else { rest.parse().unwrap_or(6) };
                let p = StoredPrefs { crossfade_sec: secs, ..cli.prefs.clone() };
                cli.put(p);
                println!("crossfade {secs} s");
            }
            "automix" => {
                let p = StoredPrefs { auto_mix: rest != "off", ..cli.prefs.clone() };
                cli.put(p);
                println!("AutoMix {}", if cli.prefs.auto_mix { "on" } else { "off" });
            }
            "pos" => {
                let s = cli.engine.status();
                let name = s.index.map_or_else(|| "nothing".into(), |i| songs.title(i));
                println!("{:?}: {} at {}{}", s.state, name, clock(s.position_now()), if s.mixing { " (mixing)" } else { "" });
            }
            "positions" => cli.engine.position_updates((rest != "off").then(|| Duration::from_secs(1))),
            "search" => match cli.search(rest) {
                Ok(found) if !found.is_empty() => {
                    for i in 0..found.len() {
                        println!("  {i}: {}", title(&found, i));
                    }
                    cli.queue(found, 0);
                    songs.set(&cli.songs);
                }
                Ok(_) => println!("nothing found"),
                Err(e) => println!("search failed: {e}"),
            },
            "queue" => {
                for i in 0..cli.songs.len() {
                    println!("  {i}: {}", title(&cli.songs, i));
                }
            }
            "q" | "quit" | "exit" => break,
            "" => {}
            _ => println!("play, pause, next, prev, seek <s>, crossfade <s>|off, automix on|off, pos, positions on|off, search <q>, queue, quit"),
        }
    }
    cli.engine.stop();
}

/// The titles the event printer shows, shared with it.
mod shown {
    use std::sync::Mutex;

    #[derive(Default)]
    pub struct Titles(Mutex<Vec<norimusic::Song>>);

    impl Titles {
        pub fn set(&self, songs: &[norimusic::Song]) {
            *self.0.lock().unwrap() = songs.to_vec();
        }

        pub fn title(&self, index: usize) -> String {
            super::title(&self.0.lock().unwrap(), index)
        }
    }
}
