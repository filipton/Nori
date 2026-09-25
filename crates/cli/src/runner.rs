//! The program's loop: one thread blocked on the terminal's input, the engine's events and the workers'
//! answers all arriving on one channel, and the screen drawn only after something arrived. The loop
//! sleeps on that channel until the next thing that is due by itself (the clock's next second while
//! music plays, the lyrics' next change); with nothing playing it sleeps until something happens.

use std::sync::mpsc::{channel, Receiver, RecvTimeoutError, Sender};
use std::sync::Arc;
use std::time::Instant;

use nori_core::settings::SavedServer;
use nori_core::settings_store;
use nori_engine::Event;
use nori_http::Http;
use ratatui::crossterm::event::{self, Event as TermEvent};
use ratatui_image::picker::Picker;

use crate::app::{App, Cmd, Screen};
use crate::art::{protocol_name, Art, COVER_PX};
use crate::backend::{own, Msg, Open, Session};
use crate::Options;

/// Reads the terminal on a thread of its own, blocked until a key, a click, a paste or a resize.
fn read_input(tx: Sender<Msg>) {
    let _ = std::thread::Builder::new().name("nori-input".into()).spawn(move || loop {
        let msg = match event::read() {
            Ok(TermEvent::Key(k)) => Msg::Key(k),
            Ok(TermEvent::Mouse(m)) => Msg::Mouse(m),
            Ok(TermEvent::Paste(p)) => Msg::Paste(p),
            Ok(TermEvent::Resize(..)) => Msg::Resize,
            Ok(_) => continue,
            Err(_) => return,
        };
        if tx.send(msg).is_err() {
            return;
        }
    });
}

/// How long the terminal is given to say which pictures it draws.
const QUERY_MS: u64 = 1000;

/// Asks the terminal which graphics protocol it speaks and how large its cells are (ratatui-image).
/// A terminal that does not answer leaves the query's reader waiting on stdin, where it would take
/// the first key pressed: a device-attributes query, which every terminal answers, lets it finish.
/// Whatever arrives late is read and dropped before the screen starts reading keys.
fn query_picker() -> Picker {
    use std::io::Write;
    let t0 = Instant::now();
    let mut o = ratatui_image::picker::cap_parser::QueryStdioOptions::default();
    o.timeout = std::time::Duration::from_millis(QUERY_MS);
    let picker = Picker::from_query_stdio_with_options(o).unwrap_or_else(|_| Picker::halfblocks());
    if t0.elapsed() >= std::time::Duration::from_millis(QUERY_MS) {
        let mut out = std::io::stdout();
        let _ = out.write_all(b"\x1b[c");
        let _ = out.flush();
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    while event::poll(std::time::Duration::from_millis(30)).unwrap_or(false) {
        let _ = event::read();
    }
    eprintln!("nori: pictures drawn with {:?}, cells {:?} px, in {:?}", picker.protocol_type(), picker.font_size(), t0.elapsed());
    picker
}

struct Runner {
    o: Options,
    http: Arc<Http>,
    tx: Sender<Msg>,
    session: Option<Session>,
    art: Option<Art>,
    picker: Option<Picker>,
    /// The tickets of covers on their way: dropped, a cover no longer wanted is not fetched.
    tickets: Vec<nori_covers::loader::Ticket>,
    /// The song the screen last showed as heard, so the engine is only asked about it when it moved.
    heard: Option<String>,
}

pub fn run(o: Options) -> Result<(), String> {
    crate::term::stderr_to(&o.data.join("nori.log"));
    crate::term::hook_panics();
    let db = crate::backend::db_path(&o.data);
    crate::backend::set_core_db_path(db.clone());
    let mut prefs = settings_store::settings_open(db).map_err(|e| format!("the settings: {e}"))?;
    // A server given on the command line is added (or found) and used.
    if let Some((url, user, password)) = o.login.clone() {
        let found = prefs.servers.iter().find(|s| s.url == url && s.user == user).map(|s| s.id.clone());
        let id = found.unwrap_or_else(|| {
            let s = SavedServer { id: nori_core::settings::new_server_id(), url: url.clone(), user: user.clone(), password: password.clone(), ..Default::default() };
            prefs.servers.push(s.clone());
            s.id
        });
        prefs.servers.iter_mut().filter(|s| s.id == id && !password.is_empty()).for_each(|s| s.password = password.clone());
        prefs.active_server_id = id;
        settings_store::settings_put(prefs.clone());
    }
    let mouse = o.mouse.unwrap_or_else(|| own::flag(own::MOUSE, true));
    let images = o.images.unwrap_or_else(|| own::flag(own::IMAGES, true));
    let mut terminal = crate::term::enter(mouse).map_err(|e| e.to_string())?;
    // Asked before anything reads the terminal: the answer to the query comes in on stdin.
    let picker = if images { Some(query_picker()) } else { None };
    let (tx, rx) = channel();
    read_input(tx.clone());
    let mut app = App::new(prefs.clone());
    app.mouse = mouse;
    app.images = images;
    app.offline = o.offline;
    app.volume = own::number(own::VOLUME, 1.0);
    app.protocol = picker.as_ref().map_or("off", |p| protocol_name(p.protocol_type()));
    app.settings.own.data = o.data.display().to_string();
    let art = picker.clone().map(Art::new);
    let mut r = Runner { http: Http::new(), tx, session: None, art, picker, tickets: Vec::new(), heard: None, o };
    match prefs.servers.iter().find(|s| s.id == prefs.active_server_id).cloned() {
        Some(p) => r.open(&mut app, p),
        None => app.screen = Screen::Login,
    }
    if app.screen != Screen::Login {
        app.go(Screen::Home);
    }
    let result = r.run(&mut app, &mut terminal, rx);
    if let Some(s) = r.session.take() {
        s.close();
    }
    crate::term::leave();
    result
}

impl Runner {
    fn open(&mut self, app: &mut App, profile: SavedServer) {
        if let Some(s) = self.session.take() {
            s.close();
        }
        let name = nori_core::settings::label(&profile.name, &profile.url);
        let o = Open {
            data: &self.o.data,
            http: self.http.clone(),
            profile,
            device: self.o.device.clone(),
            images: app.images,
            offline: self.o.offline,
            mpris: self.o.mpris,
            tx: self.tx.clone(),
        };
        match Session::open(o) {
            Ok(s) => {
                s.check();
                app.server = name;
                app.unreachable = None;
                self.session = Some(s);
                self.heard = None;
                // Every screen starts again for the new server.
                let prefs = app.prefs.clone();
                let keep = (app.mouse, app.images, app.volume, app.protocol, app.offline, app.server.clone(), app.settings.own.data.clone());
                *app = App::new(prefs);
                (app.mouse, app.images, app.volume, app.protocol, app.offline, app.server, app.settings.own.data) = keep;
                self.follow(app);
            }
            Err(e) => {
                app.screen = Screen::Login;
                app.login.error = Some(e);
            }
        }
    }

    fn run(&mut self, app: &mut App, terminal: &mut crate::term::Term, rx: Receiver<Msg>) -> Result<(), String> {
        loop {
            self.carry_out(app);
            if app.quit {
                return Ok(());
            }
            if app.dirty {
                self.follow(app);
                let art = self.art.as_mut();
                terminal.draw(|f| crate::ui::draw(f, app, art)).map_err(|e| e.to_string())?;
                app.dirty = false;
            }
            let now = Instant::now();
            let msg = match app.next_wake(now) {
                Some(at) => rx.recv_timeout(at.saturating_duration_since(now)),
                None => rx.recv().map_err(|_| RecvTimeoutError::Disconnected),
            };
            match msg {
                Ok(m) => {
                    self.take(app, m);
                    // Whatever else has arrived meanwhile is taken before drawing once, each carried out
                    // before the next is read (a key held down steps from where the last one left).
                    while let Ok(m) = rx.try_recv() {
                        self.carry_out(app);
                        self.take(app, m);
                    }
                }
                Err(RecvTimeoutError::Timeout) => app.tick(Instant::now()),
                Err(RecvTimeoutError::Disconnected) => return Ok(()),
            }
        }
    }

    fn take(&mut self, app: &mut App, m: Msg) {
        match &m {
            Msg::Engine(e @ (Event::Song { .. } | Event::State(_) | Event::Looped { .. })) => {
                if let Some(s) = &self.session {
                    s.desktop_changed();
                    s.followed(e);
                }
            }
            Msg::Cover { art, image, .. } => {
                if let Some(a) = &mut self.art {
                    a.put(art.clone(), image);
                }
            }
            Msg::LoggedIn(Ok(p)) => {
                // Kept, made the one in use, and opened.
                let mut prefs = settings_store::settings_current().unwrap_or_default();
                prefs.servers.retain(|s| !(s.url == p.url && s.user == p.user));
                prefs.servers.push(p.clone());
                prefs.active_server_id = p.id.clone();
                settings_store::settings_put(prefs.clone());
                app.prefs_changed(prefs);
                self.open(app, p.clone());
                app.go(Screen::Home);
                return;
            }
            _ => {}
        }
        app.handle(m);
    }

    /// What the screen shows of the engine and the queue, read where they are kept (no copy of a string
    /// unless the song heard changed).
    fn follow(&mut self, app: &mut App) {
        let Some(s) = &self.session else { return };
        let (now, id_changed) = s.engine.status_with(|st| {
            let changed = st.id.as_deref() != self.heard.as_deref();
            (
                crate::app::Now { state: st.state, position_ms: st.position_ms, at: st.at, speed: st.speed, mixing: st.mixing, buffering: app.now.buffering },
                changed.then(|| st.id.clone()),
            )
        });
        app.now = now;
        if let Some(id) = id_changed {
            self.heard = id.clone();
            app.heard(id.and_then(nori_core::queue::queue_song));
        }
        // The queue, copied again only when it changed.
        let (rev, repeat) = nori_core::playlist::with(|p| (p.rev(), p.repeat()));
        if app.queue.as_ref().is_none_or(|q| q.rev != rev || q.repeat != repeat) {
            let held = app.queue.as_ref().map_or(u64::MAX, |q| q.list_rev);
            let mut v = nori_core::playlist::playlist_view(held);
            if v.songs.is_empty() && v.len > 0 {
                if let Some(q) = &app.queue {
                    v.songs = q.songs.clone();
                }
            }
            app.queue = Some(v);
        }
        if app.screen == Screen::Playing {
            if let Some(id) = &self.heard {
                let note = nori_core::automix::planner::transition_note(id);
                if note != app.transition {
                    app.transition = note;
                }
                app.mixed_in = if app.now.mixing { nori_core::automix::planner::transition_into(id) } else { None };
            }
        }
        if app.screen == Screen::Lyrics {
            self.lyrics_step(app);
        }
    }

    /// The lyrics' clock asked where the music is, and when to look again.
    fn lyrics_step(&self, app: &mut App) {
        let Some(l) = &app.lyrics else {
            app.lyrics_wake = None;
            return;
        };
        let now = Instant::now();
        let force = app.lyrics_wake.is_some_and(|t| t <= now);
        let (_, wait) = l.advance(app.now.position(now), app.prefs.lyrics_sweep, force);
        app.lyrics_wake = wait.map(|ms| now + std::time::Duration::from_millis(ms));
    }

    fn carry_out(&mut self, app: &mut App) {
        let cmds = std::mem::take(&mut app.cmds);
        for c in cmds {
            self.carry(app, c);
        }
    }

    fn carry(&mut self, app: &mut App, c: Cmd) {
        match c {
            Cmd::Quit => {
                app.quit = true;
                return;
            }
            Cmd::Mouse(on) => {
                crate::term::set_mouse(on);
                own::keep(own::MOUSE, on.to_string());
                app.mouse = on;
                app.settings.invalidate();
                return;
            }
            Cmd::Images(on) => {
                own::keep(own::IMAGES, on.to_string());
                app.images = on;
                app.settings.invalidate();
                if on && self.art.is_none() {
                    let picker = self.picker.clone().unwrap_or_else(Picker::halfblocks);
                    app.protocol = protocol_name(picker.protocol_type());
                    self.art = Some(Art::new(picker));
                }
                if !on {
                    self.art = None;
                    self.tickets.clear();
                }
                app.say(if on { "Covers on (from the next song or page; restart to fetch covers again)" } else { "Covers off" }, false);
                return;
            }
            Cmd::Login(draft) => {
                let (data, http, tx) = (self.o.data.clone(), self.http.clone(), self.tx.clone());
                std::thread::spawn(move || {
                    let _ = tx.send(Msg::LoggedIn(crate::backend::check_login(&data, http, draft)));
                });
                return;
            }
            Cmd::SwitchServer(id) => {
                let mut prefs = settings_store::settings_current().unwrap_or_default();
                let Some(p) = prefs.servers.iter().find(|s| s.id == id).cloned() else { return };
                prefs.active_server_id = id;
                settings_store::settings_put(prefs.clone());
                app.prefs_changed(prefs);
                self.open(app, p);
                app.go(Screen::Home);
                return;
            }
            _ => {}
        }
        let Some(s) = &self.session else { return };
        match c {
            Cmd::Load(req) => s.load(req),
            Cmd::Play { songs, start, shuffle } => s.play(songs, start, shuffle),
            Cmd::PlayFetch(what, shuffle) => s.play_later(what, shuffle),
            Cmd::Enqueue(songs, next) => s.enqueue(songs, next),
            Cmd::EnqueueFetch(what, next) => s.enqueue_later(what, next),
            Cmd::Toggle => {
                // Nothing loaded: the queue kept from last time starts where it was.
                if app.now.state == nori_engine::State::Idle && app.queue.as_ref().is_some_and(|q| q.len > 0) {
                    let at = app.queue.as_ref().map_or(0, |q| q.index.max(0) as usize);
                    s.engine.play_at(at, app.now.position_ms);
                } else {
                    s.engine.toggle();
                }
            }
            Cmd::Next => s.next(),
            Cmd::Previous => {
                s.engine.previous();
            }
            Cmd::Seek(ms) => s.engine.seek(ms),
            Cmd::Volume(v) => {
                s.volume.set(v);
                own::keep(own::VOLUME, v.to_string());
                app.volume = v;
                app.settings.invalidate();
            }
            Cmd::Jump(i) => {
                s.engine.play_at(i, 0);
            }
            Cmd::Remove(i) => s.remove(i),
            Cmd::Move(from, to) => s.move_song(from, to),
            Cmd::Shuffle(on) => s.shuffle(on),
            Cmd::Repeat(m) => {
                // Kept in the queue at once, so the screen shows it; the engine is told as well.
                nori_core::playlist::playlist_repeat(m);
                s.repeat(m);
            }
            Cmd::Download(songs) => s.download(songs),
            Cmd::DownloadFetch(what) => s.download_later(what),
            Cmd::DownloadRemove(id) => {
                s.download_remove(&id);
                s.load(crate::backend::Req::Downloads);
            }
            Cmd::Star(kind, id, on) => s.star(kind, id, on),
            Cmd::Setting(name, value) => {
                if s.setting(&name, &value).is_none() {
                    app.say(format!("{name}: not a setting"), true);
                }
                self.prefs_changed(app);
            }
            Cmd::Level(level, v) => {
                if let Some((effect, _)) = settings_store::edit_level(level, v) {
                    s.applied(effect);
                }
                self.prefs_changed(app);
            }
            Cmd::Band(i, band) => {
                if let Some((effect, _)) = settings_store::edit_band(i, band) {
                    s.applied(effect);
                }
                self.prefs_changed(app);
            }
            Cmd::Sound(tool) => {
                if let Some(t) = tool.tool() {
                    match settings_store::settings_sound_tool(t) {
                        Ok(Some(change)) => s.applied(change.effect),
                        Ok(None) => {}
                        Err(e) => app.say(format!("{e:?}"), true),
                    }
                }
                self.prefs_changed(app);
            }
            Cmd::Action(a) => s.action(&a),
            Cmd::Tuning(on) => s.engine.set_tuning(on),
            Cmd::SearchTyped(text) => {
                let v = s.search_typed(&text);
                app.search.view = Some(v);
            }
            Cmd::SearchServer(q) => {
                let _ = s.core.search_remember_recent(q.clone());
                s.search_server(q);
            }
            Cmd::Lyrics(id) => s.lyrics(id),
            Cmd::Cover { art, colours } => {
                if self.art.as_ref().is_some_and(|a| a.has(&art)) && !colours {
                    return;
                }
                if let Some(t) = s.cover(art, COVER_PX, colours) {
                    self.tickets.push(t);
                    if self.tickets.len() > 4 {
                        drop(self.tickets.remove(0));
                    }
                }
            }
            Cmd::Quit | Cmd::Mouse(_) | Cmd::Images(_) | Cmd::Login(_) | Cmd::SwitchServer(_) => {}
        }
    }

    fn prefs_changed(&self, app: &mut App) {
        if let Some(p) = settings_store::settings_current() {
            app.prefs_changed(p);
        }
    }
}
