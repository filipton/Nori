//! The player's one thread, and the handle a client drives it with. Everything that decides how music
//! plays is the shared pipeline (`nori_player::pipeline`) the tests run on a virtual clock; this adds
//! the real clock, the controls' fades and the events a screen follows.
//!
//! The thread sleeps whenever it has nothing to do, and wakes only for:
//! - a command (a control, a settings change, the queue changed): the handle unparks it;
//! - the ring running down to its low mark (about 1.75 s left of a 10 s burst): the device's pull
//!   unparks it, once per burst, so while music plays it wakes every eight seconds or so (and at the
//!   end of the queue, when the ring has run empty);
//! - a song's bytes arriving while it waited for them: the loader unparks it;
//! - a moment it has to act at: a fade's end (pause, a seek's dip), the next song reaching the ear
//!   (so the song event comes on time), every 250 ms through a held ending or a mix (the ear moves
//!   into the next song there), the music running out at the end of the queue, and position events
//!   when a screen asked for them. Each is one timed sleep, computed, never a ticking timer.
//!
//! Paused, stopped or at the end of the queue, it sleeps until a command comes, and the device is
//! paused so it can sleep too.

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};

use nori_player::pipeline::{App, Player, Queue, Sound};
use nori_player::queue::previous_restarts;
use nori_player::transport::{load_control, pause_fade, play_fade, switch_dip, Switch};
use parking_lot::Mutex;

use crate::library::{Library, Sources};
use crate::output::{AudioOutput, RingTrack, WAKE_LOW_US};

/// What the settings ask of the sound, as the engine applies it.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub sound: Sound,
    pub speed: f32,
    pub pitch: f32,
    pub skip_silence: bool,
    /// The fade on play, pause and switches, ms (0 off).
    pub fade_ms: i32,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { sound: Sound::default(), speed: 1.0, pitch: 1.0, skip_silence: false, fade_ms: 0 }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Nothing loaded.
    Idle,
    Playing,
    Paused,
    /// Played to the end of the queue.
    Ended,
}

/// What a screen hears from the engine. Each comes once, when it changes; positions only when asked for.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    State(State),
    /// The ear moved to another song: through a mix, the moment the next song is audible.
    Song { index: usize, id: String },
    /// Where the ear is, at the pace asked for with [`Engine::position_updates`].
    Position { index: usize, ms: i64 },
    /// A song would not play (it is skipped, or playback stops, as the queue's rules say), or the
    /// output would not open (`id` empty).
    Error { id: String, message: String },
}

/// The engine as it last looked, for a screen to read at any time without waking it.
#[derive(Debug, Clone)]
pub struct Status {
    pub state: State,
    /// The song heard, as a queue (list) index, and its id.
    pub index: Option<usize>,
    pub id: Option<String>,
    /// Where the ear was in it when `at` was taken, ms.
    pub position_ms: i64,
    pub at: Instant,
    pub speed: f32,
    pub mixing: bool,
    /// Pulls that found the output's buffer short while music was due: a gap each.
    pub underruns: u64,
}

impl Status {
    /// The place in the song now: the last reading, moved on at the playing speed.
    pub fn position_now(&self) -> i64 {
        match self.state {
            State::Playing => self.position_ms + (self.at.elapsed().as_secs_f64() * 1000.0 * self.speed as f64) as i64,
            _ => self.position_ms,
        }
    }
}

/// How the engine is set up.
#[derive(Debug, Clone)]
pub struct Config {
    /// The memory the platform gives the app, MB: it sizes how much of a song is kept loaded
    /// (`nori_player::transport::load_control`).
    pub memory_mb: u32,
    pub settings: Settings,
}

impl Default for Config {
    fn default() -> Self {
        Config { memory_mb: 256, settings: Settings::default() }
    }
}

enum Command {
    PlayAt(usize, i64),
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Seek(i64),
    Settings(Settings),
    Replan,
    QueueChanged,
    Repeat(u8),
    Positions(Option<Duration>),
    Stop,
}

/// What a switch does once its dip is down.
#[derive(Debug, Clone, Copy)]
enum Switched {
    To(usize, i64),
    Next,
    Previous,
    Seek(i64),
}

/// The handle: every call sends a command and wakes the engine's thread, and returns at once.
pub struct Engine {
    tx: Sender<Command>,
    thread: Thread,
    join: Option<JoinHandle<()>>,
    status: Arc<Mutex<Status>>,
}

impl Engine {
    /// Starts the engine's thread over `library`'s songs, `queue` and `app` (the transition planner
    /// and log), playing through `output`. `events` is called on the engine's thread; it should only
    /// hand the event on.
    pub fn start<L, A, Q, E>(library: L, app: A, queue: Q, output: Box<dyn AudioOutput>, config: Config, events: E) -> Engine
    where
        L: Library,
        A: App + Send + 'static,
        Q: Queue + Send + 'static,
        E: FnMut(Event) + Send + 'static,
    {
        let (tx, rx) = channel();
        let status = Arc::new(Mutex::new(Status {
            state: State::Idle,
            index: None,
            id: None,
            position_ms: 0,
            at: Instant::now(),
            speed: 1.0,
            mixing: false,
            underruns: 0,
        }));
        let shared = status.clone();
        let join = std::thread::Builder::new()
            .name("nori-engine".into())
            .spawn(move || {
                let me = std::thread::current();
                let songs = Sources::new(library, load_control(config.memory_mb), me);
                let player = Player::build(songs, queue, app, RingTrack::new(output));
                Worker::new(player, rx, events, shared, config.settings).run();
            })
            .expect("a thread for the engine");
        Engine { tx, thread: join.thread().clone(), join: Some(join), status }
    }

    fn send(&self, c: Command) {
        if self.tx.send(c).is_ok() {
            self.thread.unpark();
        }
    }

    /// Plays queue (list) index `index` from `ms`.
    pub fn play_at(&self, index: usize, ms: i64) {
        self.send(Command::PlayAt(index, ms));
    }

    pub fn play(&self) {
        self.send(Command::Play);
    }

    pub fn pause(&self) {
        self.send(Command::Pause);
    }

    pub fn toggle(&self) {
        self.send(Command::Toggle);
    }

    pub fn next(&self) {
        self.send(Command::Next);
    }

    /// Previous, as the button does it: back to the start of the song a few seconds in.
    pub fn previous(&self) {
        self.send(Command::Previous);
    }

    pub fn seek(&self, ms: i64) {
        self.send(Command::Seek(ms));
    }

    /// The sound and the controls' fades, as the settings are now.
    pub fn set_settings(&self, settings: Settings) {
        self.send(Command::Settings(settings));
    }

    /// Something a transition plan depends on changed (the transition settings, an analysis): the plan
    /// out of the song playing is asked for again.
    pub fn replan(&self) {
        self.send(Command::Replan);
    }

    /// The queue was edited where it is kept: the planner's window and what plays next follow.
    pub fn queue_changed(&self) {
        self.send(Command::QueueChanged);
    }

    /// Repeat off, one or all (`nori_player::playlist::REPEAT_*`).
    pub fn set_repeat(&self, mode: u8) {
        self.send(Command::Repeat(mode));
    }

    /// Position events this often while music plays, or none (the default: an idle screen is not woken).
    pub fn position_updates(&self, every: Option<Duration>) {
        self.send(Command::Positions(every));
    }

    pub fn status(&self) -> Status {
        self.status.lock().clone()
    }

    /// Stops the thread and lets the output go.
    pub fn stop(&mut self) {
        self.send(Command::Stop);
        if let Some(j) = self.join.take() {
            let _ = j.join();
        }
    }
}

impl Drop for Engine {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Worker<L: Library, A: App, Q: Queue, E: FnMut(Event)> {
    p: Player<Sources<L>, RingTrack, A, Q>,
    rx: Receiver<Command>,
    events: E,
    status: Arc<Mutex<Status>>,
    started: Instant,
    settings: Settings,
    state: State,
    /// A pause waiting for its fade to end.
    pause_at: Option<i64>,
    /// Switches waiting out their dip, when the dip is down, and how fast the music comes back.
    switches: VecDeque<Switched>,
    switch_at: Option<i64>,
    up_ms: i64,
    /// The song last reported.
    heard: Option<usize>,
    positions: Option<i64>,
    next_position: i64,
}

impl<L: Library, A: App, Q: Queue, E: FnMut(Event)> Worker<L, A, Q, E> {
    fn new(p: Player<Sources<L>, RingTrack, A, Q>, rx: Receiver<Command>, events: E, status: Arc<Mutex<Status>>, settings: Settings) -> Self {
        let mut w = Worker {
            p,
            rx,
            events,
            status,
            started: Instant::now(),
            settings: Settings::default(),
            state: State::Idle,
            pause_at: None,
            switches: VecDeque::new(),
            switch_at: None,
            up_ms: 0,
            heard: None,
            positions: None,
            next_position: 0,
        };
        w.apply(settings);
        w
    }

    fn now(&self) -> i64 {
        self.started.elapsed().as_millis() as i64 + 1_000
    }

    fn run(mut self) {
        loop {
            loop {
                match self.rx.try_recv() {
                    Ok(Command::Stop) | Err(TryRecvError::Disconnected) => return,
                    Ok(c) => self.command(c),
                    Err(TryRecvError::Empty) => break,
                }
            }
            let now = self.now();
            self.due(now);
            // The burst's count of what the device holds is kept from the clock it reads, and misses
            // whatever played before its first reading; the ring knows exactly. At the low mark the
            // count starts again, so a burst always begins when the ring says it is time.
            if self.p.playing() && !self.p.source_ended() && self.p.sink.track.filled_us() <= WAKE_LOW_US {
                self.p.burst.restart();
            }
            self.p.turn(now);
            self.check();
            self.report(now);
            let w = self.wake_in(now);
            match w {
                Some(0) => continue,
                Some(ms) => std::thread::park_timeout(Duration::from_millis(ms as u64)),
                None => std::thread::park(),
            }
        }
    }

    fn set_state(&mut self, s: State) {
        if self.state != s {
            self.state = s;
            (self.events)(Event::State(s));
        }
    }

    fn command(&mut self, c: Command) {
        let now = self.now();
        match c {
            Command::PlayAt(i, ms) => self.switch(Switched::To(i, ms), Switch::ToSong, now),
            Command::Play => self.play(),
            Command::Pause => self.pause(now),
            Command::Toggle => {
                if self.state == State::Playing {
                    self.pause(now)
                } else {
                    self.play()
                }
            }
            Command::Next => self.switch(Switched::Next, Switch::Skip, now),
            Command::Previous => self.switch(Switched::Previous, Switch::Skip, now),
            Command::Seek(ms) => self.switch(Switched::Seek(ms), Switch::Seek, now),
            Command::Settings(s) => self.apply(s),
            Command::Replan => self.p.engine.replan(),
            Command::QueueChanged => self.p.queue_changed(),
            Command::Repeat(m) => self.p.set_repeat(m),
            Command::Positions(every) => {
                self.positions = every.map(|d| d.as_millis().max(1) as i64);
                self.next_position = now;
            }
            Command::Stop => {}
        }
    }

    fn apply(&mut self, s: Settings) {
        if s.sound != self.settings.sound {
            self.p.set_sound(s.sound.clone());
        }
        if (s.speed, s.pitch) != (self.settings.speed, self.settings.pitch) {
            self.p.set_speed(s.speed, s.pitch);
        }
        if s.skip_silence != self.settings.skip_silence {
            self.p.set_skip_silence(s.skip_silence);
        }
        self.settings = s;
    }

    /// Music from where the player is: fading in from silence when the settings say so.
    fn resume(&mut self) {
        self.p.resume();
        match play_fade(self.settings.fade_ms, false) {
            Some(ms) => self.p.sink.track.ramp(Some(0.0), 1.0, ms as i64),
            None => self.p.sink.track.ramp(None, 1.0, 0),
        }
        self.set_state(State::Playing);
    }

    fn play(&mut self) {
        if self.pause_at.take().is_some() {
            // Pressed again inside the fade out: the music comes back from where the fade got to.
            self.p.sink.track.ramp(None, 1.0, self.settings.fade_ms.max(0) as i64);
            self.set_state(State::Playing);
            return;
        }
        if self.p.playing() && self.state == State::Playing {
            return;
        }
        if self.p.current().is_none() || self.state == State::Ended {
            let at = self.p.queue.read(|q| q.current()).or(self.p.current()).unwrap_or(0);
            if self.p.queue.read(|q| q.is_empty()) {
                return;
            }
            self.p.jump(at, 0);
        }
        self.resume();
    }

    fn pause(&mut self, now: i64) {
        if !self.p.playing() || self.pause_at.is_some() {
            return;
        }
        // A pause never swallows the switch it interrupts.
        self.run_switches();
        match pause_fade(self.settings.fade_ms, true) {
            Some(ms) => {
                self.p.sink.track.ramp(None, 0.0, ms as i64);
                self.pause_at = Some(now + ms as i64);
            }
            None => self.p.pause(),
        }
        self.set_state(State::Paused);
    }

    fn switch(&mut self, s: Switched, kind: Switch, now: i64) {
        let playing = self.p.playing() && self.pause_at.is_none();
        match switch_dip(self.settings.fade_ms, kind, playing) {
            Some(dip) => {
                if self.switch_at.is_none() {
                    self.p.sink.track.ramp(None, 0.0, dip.down_ms as i64);
                    self.switch_at = Some(now + dip.down_ms as i64);
                }
                self.switches.push_back(s);
                self.up_ms = dip.up_ms as i64;
            }
            None => {
                self.switches.push_back(s);
                self.run_switches();
            }
        }
    }

    fn run_switches(&mut self) {
        let dipped = self.switch_at.take().is_some();
        while let Some(s) = self.switches.pop_front() {
            let wants_music = !matches!(s, Switched::Seek(_));
            match s {
                Switched::To(i, ms) => {
                    if i < self.p.queue.read(|q| q.len()) {
                        self.p.jump(i, ms);
                    }
                }
                Switched::Next => {
                    self.p.next();
                }
                Switched::Previous => {
                    let has_previous = self.p.queue.read(|q| q.previous().is_some());
                    if previous_restarts(self.p.position_ms(), has_previous, false) {
                        self.p.seek(0);
                    } else {
                        self.p.previous();
                    }
                }
                Switched::Seek(ms) => self.p.seek(ms),
            }
            // A skip while paused is a request for music.
            if wants_music && !self.p.playing() && self.p.current().is_some() {
                self.resume();
            }
        }
        if dipped {
            self.p.sink.track.ramp(None, 1.0, self.up_ms);
        }
        if self.p.playing() {
            self.set_state(State::Playing);
        }
    }

    fn due(&mut self, now: i64) {
        if self.pause_at.is_some_and(|t| now >= t) {
            self.pause_at = None;
            self.p.pause();
        }
        if self.switch_at.is_some_and(|t| now >= t) {
            self.run_switches();
        }
    }

    fn check(&mut self) {
        if let Some(why) = self.p.sink.track.failed.take() {
            (self.events)(Event::Error { id: String::new(), message: format!("the output would not open: {why}") });
            self.p.pause();
            self.set_state(State::Idle);
        }
        for (id, message) in std::mem::take(&mut self.p.failures) {
            (self.events)(Event::Error { id, message });
        }
        self.p.changes.clear();
        if self.p.source_ended() {
            self.p.sink.track.set_ended(true);
        }
        if self.p.playing() && self.p.ended() {
            self.p.pause();
            self.set_state(State::Ended);
        } else if !self.p.playing() && self.state == State::Playing && self.pause_at.is_none() && self.switch_at.is_none() {
            // The queue's rules stopped playback (a run of songs that would not play).
            self.set_state(State::Paused);
        }
    }

    fn report(&mut self, now: i64) {
        if self.p.current().is_none() {
            return;
        }
        let seen = self.p.bar();
        let index = seen.index.or(self.p.current());
        let ms = if seen.index.is_some() { seen.ms } else { self.p.position_ms() };
        let id = index.map(|i| self.p.id_at(i));
        if index != self.heard {
            self.heard = index;
            if let (Some(i), Some(id)) = (index, id.clone()) {
                (self.events)(Event::Song { index: i, id });
            }
        }
        {
            let mut s = self.status.lock();
            s.state = self.state;
            if s.index != index {
                s.index = index;
                s.id = id;
            }
            s.position_ms = ms;
            s.at = Instant::now();
            s.speed = self.p.speed().0;
            s.mixing = self.p.mixing();
            s.underruns = self.p.sink.track.underruns();
        }
        if let (Some(every), Some(i), State::Playing) = (self.positions, index, self.state) {
            if now >= self.next_position {
                self.next_position = now + every;
                (self.events)(Event::Position { index: i, ms });
            }
        }
    }

    /// How long the thread may sleep: `None` until a command, `Some(0)` not at all.
    fn wake_in(&self, now: i64) -> Option<i64> {
        let mut d: Option<i64> = None;
        let mut at = |ms: i64| d = Some(d.map_or(ms, |x| x.min(ms)));
        if let Some(t) = self.pause_at {
            at(t - now);
        }
        if let Some(t) = self.switch_at {
            at(t - now);
        }
        if !self.p.playing() {
            return d.map(|x| x.max(1));
        }
        if self.p.hungry() {
            return Some(0);
        }
        let track = &self.p.sink.track;
        let speed = self.p.speed().0.max(0.1) as f64;
        let fill = track.filled_us();
        if self.p.source_ended() {
            // The end of the queue: the device's pull says when the last of it has gone.
            track.wake_at(0);
            at((fill + track.latency_us()) / 1000 + 5);
        } else if self.p.starved() {
            // The loader wakes the thread as soon as the bytes are there; this only guards against a
            // loader that never answers.
            at(1_000);
        } else if fill > WAKE_LOW_US {
            track.wake_at(WAKE_LOW_US);
            // The device's pull wakes the thread at the low mark; this is in case its clock runs slow.
            at((fill - WAKE_LOW_US) / 1000 + 250);
        } else {
            // The burst's own count (which sees what the device holds too) did not agree yet.
            at(200);
        }
        if let Some(u) = self.p.until_next_song_us() {
            at((u as f64 / speed / 1000.0) as i64 + 5);
        }
        let h = self.p.heard();
        if h.id.is_some() || h.mixing || h.next_id.is_some() {
            at(250);
        }
        if let (Some(_), State::Playing) = (self.positions, self.state) {
            at(self.next_position - now);
        }
        d.map(|x| x.max(1))
    }
}
