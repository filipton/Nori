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
//! paused so it can sleep too. Paused long enough (the core's idle release, five minutes on Android),
//! it wakes once more to let the device and the song's bytes go; play opens them again where it was.

use std::collections::VecDeque;
use std::sync::mpsc::{channel, Receiver, Sender, TryRecvError};
use std::sync::Arc;
use std::thread::{JoinHandle, Thread};
use std::time::{Duration, Instant};

use nori_player::pcm::Encoding;
use nori_player::pipeline::{App, Player, Queue, Sound};
use nori_player::policy::{audio_policy, AudioPrefs, OutputState};
use nori_player::queue::previous_restarts;
use nori_player::transport::{load_control, pause_fade, play_fade, skip_plays, switch_dip, Switch, IDLE_RELEASE_MS};
use parking_lot::Mutex;

use crate::library::{Library, Sources};
use crate::output::{AudioOutput, Device, RingTrack, WAKE_LOW_US};

/// What the settings ask of the sound, as the engine applies it.
#[derive(Debug, Clone, PartialEq)]
pub struct Settings {
    pub sound: Sound,
    pub speed: f32,
    pub pitch: f32,
    pub skip_silence: bool,
    /// The fade on play, pause and switches, ms (0 off).
    pub fade_ms: i32,
    /// High quality output: songs decoded to float and taken to a device that plays float as they are,
    /// with nothing touching the samples on the way (no equalizer, transitions or silence skipping,
    /// as `nori_player::policy` says). A device that takes 16-bit only gets the 16-bit chain.
    pub hi_res: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings { sound: Sound::default(), speed: 1.0, pitch: 1.0, skip_silence: false, fade_ms: 0, hi_res: false }
    }
}

/// The settings as the policy let them through to the player.
#[derive(Clone, PartialEq)]
struct Applied {
    sound: Sound,
    speed: (f32, f32),
    skip_silence: bool,
    untouched: bool,
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
    /// The music goes to another output device now, by the name the core keeps devices under.
    Output { name: String },
    /// The music ran out while a song's bytes are on their way (a slow network): `true` when the ear
    /// starts hearing nothing for it, `false` when the music goes on. Nothing is said while the output
    /// still holds music to play through the wait.
    Buffering(bool),
    /// Playback stopped by itself rather than because it was asked to (the queue's rules after a run
    /// of songs that would not play): the `Paused` state that follows is the engine's, and a client that
    /// keeps its own "wants to play" lets it go. A pause that was asked for comes without this.
    Stopped,
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
    /// Times the output was let go after a long pause.
    pub releases: u64,
    /// A jump, skip or seek is waiting out its dip: the place is still the one before it.
    pub switching: bool,
    /// The sound chain is in the samples' path, and what its limiter took off the last buffer, dB.
    pub chain: bool,
    pub gain_reduction_db: f32,
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
    /// Paused this long, the output and the song's bytes are let go, ms
    /// (`nori_player::transport::IDLE_RELEASE_MS`).
    pub idle_release_ms: i64,
}

impl Default for Config {
    fn default() -> Self {
        Config { memory_mb: 256, settings: Settings::default(), idle_release_ms: IDLE_RELEASE_MS }
    }
}

enum Command {
    PlayAt(usize, i64),
    GoTo(usize, i64),
    PauseAtEnd(bool),
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
    Gain,
    Tuning(bool),
    Positions(Option<Duration>),
    Device(Device),
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
    join: Mutex<Option<JoinHandle<()>>>,
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
            releases: 0,
            switching: false,
            chain: false,
            gain_reduction_db: 0.0,
        }));
        let shared = status.clone();
        let devices = tx.clone();
        let join = std::thread::Builder::new()
            .name("nori-engine".into())
            .spawn(move || {
                let me = std::thread::current();
                let mut output = output;
                // The output says where the music goes whenever that changes, on a thread of its own.
                let wake = me.clone();
                output.watch(Box::new(move |d| {
                    if devices.send(Command::Device(d)).is_ok() {
                        wake.unpark();
                    }
                }));
                let songs = Sources::new(library, load_control(config.memory_mb), me);
                let player = Player::build(songs, queue, app, RingTrack::new(output));
                Worker::new(player, rx, events, shared, config.settings, config.idle_release_ms).run();
            })
            .expect("a thread for the engine");
        Engine { tx, thread: join.thread().clone(), join: Mutex::new(Some(join)), status }
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

    /// Goes to queue (list) index `index` at `ms` and leaves playing or paused as it was: paused, the
    /// place is held and nothing is fetched for it until play, as a skip, a previous or a seek is.
    pub fn go_to(&self, index: usize, ms: i64) {
        self.send(Command::GoTo(index, ms));
    }

    /// Pauses when the song playing ends (the sleep timer's "end of this song"), on the next one, from
    /// its start: nothing after this song is read or mixed into. `false` takes it back.
    pub fn pause_at_end(&self, on: bool) {
        self.send(Command::PauseAtEnd(on));
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

    /// The next song, as the button does it: paused, a skip starts the music
    /// (`nori_player::transport::skip_plays`).
    pub fn next(&self) {
        self.send(Command::Next);
    }

    /// Previous, as the button does it: back to the start of the song a few seconds in; paused, it
    /// starts the music, as the next button does.
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

    /// The ReplayGain settings changed: the volume of the song playing is asked for again.
    pub fn gain_changed(&self) {
        self.send(Command::Gain);
    }

    /// The equalizer's screen is open (`true`) or closed: a band moved is heard at once while it is,
    /// the sink trading its deep buffer for a shallow one (`nori_player::transport::Chain::tuning`),
    /// and the deep buffer is back at the next boundary after.
    pub fn set_tuning(&self, on: bool) {
        self.send(Command::Tuning(on));
    }

    /// Position events this often while music plays, or none (the default: an idle screen is not woken).
    pub fn position_updates(&self, every: Option<Duration>) {
        self.send(Command::Positions(every));
    }

    pub fn status(&self) -> Status {
        self.status.lock().clone()
    }

    /// Reads the status in place, without copying it (and the song's id with it): for a question asked
    /// every frame.
    pub fn status_with<R>(&self, f: impl FnOnce(&Status) -> R) -> R {
        f(&self.status.lock())
    }

    /// Stops the thread and lets the output go.
    pub fn stop(&self) {
        self.send(Command::Stop);
        let join = self.join.lock().take();
        if let Some(j) = join {
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
    applied: Option<Applied>,
    state: State,
    /// A pause waiting for its fade to end.
    pause_at: Option<i64>,
    /// Switches waiting out their dip, when the dip is down, and how fast the music comes back.
    switches: VecDeque<Switched>,
    switch_at: Option<i64>,
    up_ms: i64,
    /// The song last reported.
    heard: Option<usize>,
    /// The ReplayGain settings changed: the song playing's volume is asked for again.
    gain_changed: bool,
    positions: Option<i64>,
    next_position: i64,
    /// Paused: when the output is let go, and after that, where the player was.
    idle_release_ms: i64,
    idle_at: Option<i64>,
    released: Option<(usize, i64)>,
    releases: u64,
    /// The music ran out waiting for a song's bytes, as last said.
    stalled: bool,
    /// Where a jump, a skip or a seek made while paused goes (the queue index, the place, the song's
    /// id): held until play, so nothing is fetched for a place that may move again before anyone
    /// listens. The screen is told it at once.
    held: Option<(usize, i64, String)>,
}

/// Less than this left to play while a song's bytes are on their way is a stall a screen shows.
const STALL_US: i64 = 200_000;

impl<L: Library, A: App, Q: Queue, E: FnMut(Event)> Worker<L, A, Q, E> {
    fn new(p: Player<Sources<L>, RingTrack, A, Q>, rx: Receiver<Command>, events: E, status: Arc<Mutex<Status>>, settings: Settings, idle_release_ms: i64) -> Self {
        let mut w = Worker {
            p,
            rx,
            events,
            status,
            started: Instant::now(),
            settings: Settings::default(),
            applied: None,
            state: State::Idle,
            pause_at: None,
            switches: VecDeque::new(),
            switch_at: None,
            up_ms: 0,
            heard: None,
            gain_changed: false,
            positions: None,
            next_position: 0,
            idle_release_ms,
            idle_at: None,
            released: None,
            releases: 0,
            stalled: false,
            held: None,
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
            self.follow_gain();
            if self.p.app.measured() {
                self.p.engine.replan();
            }
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
            self.idle_at = (s == State::Paused).then(|| self.now() + self.idle_release_ms);
            (self.events)(Event::State(s));
        }
    }

    /// Paused a long while: the device and the song's bytes go, so nothing holds the sound card or the
    /// memory; the place in the queue and the song stays.
    fn release(&mut self) {
        self.idle_at = None;
        if self.p.playing() || self.released.is_some() {
            return;
        }
        self.released = self.p.release();
        self.p.sink.track.release();
        self.p.tracks.let_go();
        self.releases += 1;
    }

    /// After a release, the song is opened again where it was.
    fn reopen(&mut self) {
        if let Some((i, ms)) = self.released.take() {
            self.p.jump(i, ms);
        }
    }

    /// The output device changed: the core picks its sound (a profile bound to it), which is applied.
    fn device(&mut self, d: Device) {
        let Some((name, sound)) = self.p.app.output_changed(d.kind, &d.name) else { return };
        if let Some(sound) = sound {
            let s = Settings { sound, ..self.settings.clone() };
            self.apply(s);
        }
        (self.events)(Event::Output { name });
    }

    fn command(&mut self, c: Command) {
        let now = self.now();
        match c {
            Command::PlayAt(i, ms) => {
                self.held = None;
                self.switch(Switched::To(i, ms), Switch::ToSong, now)
            }
            Command::GoTo(i, ms) => self.go(Switched::To(i, ms), Switch::ToSong, now),
            Command::PauseAtEnd(on) => self.p.pause_at_end(on),
            Command::Play => self.play(),
            Command::Pause => self.pause(now),
            Command::Toggle => {
                if self.state == State::Playing {
                    self.pause(now)
                } else {
                    self.play()
                }
            }
            // The skip buttons: paused, a skip is a request for music (nori_player::transport::skip_plays).
            Command::Next => self.skip(Switched::Next, now),
            Command::Previous => self.skip(Switched::Previous, now),
            Command::Seek(ms) => self.go(Switched::Seek(ms), Switch::Seek, now),
            Command::Settings(s) => self.apply(s),
            Command::Replan => self.p.engine.replan(),
            Command::QueueChanged => {
                self.p.queue_changed();
                self.follow_held();
            }
            Command::Repeat(m) => self.p.set_repeat(m),
            Command::Gain => self.gain_changed = true,
            Command::Tuning(on) => self.p.set_tuning(on),
            Command::Positions(every) => {
                self.positions = every.map(|d| d.as_millis().max(1) as i64);
                self.next_position = now;
            }
            Command::Device(d) => self.device(d),
            Command::Stop => {}
        }
    }

    /// The settings, through the audio policy Android applies: high quality output on a device that
    /// plays float keeps the samples untouched, which stands the sound chain, silence skipping, the
    /// pinned output format and every transition down. Songs opened from now on are decoded for it.
    fn apply(&mut self, s: Settings) {
        let hi_res = s.hi_res && self.p.sink.track.takes_float();
        let prefs = AudioPrefs { dsp: s.sound.on(), skip_silence: s.skip_silence, offload: false, crossfade_s: 0, auto_mix: false, speed: s.speed, pitch: s.pitch };
        let policy = audio_policy(&prefs, &OutputState { hi_res, ..Default::default() });
        let now = Applied {
            sound: if policy.untouched { Sound::default() } else { s.sound.clone() },
            speed: (s.speed, s.pitch),
            skip_silence: policy.skip_silence,
            untouched: policy.untouched,
        };
        // The player starts out with the defaults' sound; the output's say is given once at least.
        let first = self.applied.is_none();
        let was = self.applied.take().unwrap_or(Applied { sound: Sound::default(), speed: (1.0, 1.0), skip_silence: false, untouched: false });
        if first || was.untouched != now.untouched {
            self.p.tracks.encoding = if hi_res { Encoding::Float } else { Encoding::Pcm16 };
            self.p.engine.lock_rate = policy.lock_rate;
            self.p.app.transitions_off(policy.transitions_off);
            self.p.engine.replan();
        }
        if was.sound != now.sound {
            self.p.set_sound(now.sound.clone());
        }
        if was.speed != now.speed {
            self.p.set_speed(s.speed, s.pitch);
        }
        if was.skip_silence != now.skip_silence {
            self.p.set_skip_silence(now.skip_silence);
        }
        self.applied = Some(now);
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
        if let Some((i, ms, _)) = self.held.take() {
            // The place a skip or a seek left while paused: fetched now that the music is wanted.
            self.released = None;
            self.p.jump(i, ms);
            self.resume();
            return;
        }
        self.reopen();
        if let Some(i) = self.p.stopped_at() {
            // Stopped at a song that would not play, nothing is being read: resuming would run the
            // clock over silence. It is tried again.
            self.p.jump(i, 0);
        } else if self.p.current().is_none() || self.state == State::Ended {
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

    /// A skip button: made from the place held, if one is, and a request for music when paused.
    fn skip(&mut self, s: Switched, now: i64) {
        if !(self.p.playing() && self.pause_at.is_none()) {
            self.hold(s);
            if skip_plays(false) {
                self.play();
            }
            return;
        }
        self.switch(s, Switch::Skip, now);
    }

    /// A jump or a seek that leaves playing or paused as it was: while music plays it is made (with its
    /// dip), paused it is held until play.
    fn go(&mut self, s: Switched, kind: Switch, now: i64) {
        if self.p.playing() && self.pause_at.is_none() {
            self.switch(s, kind, now);
        } else {
            self.hold(s);
        }
    }

    /// Works out where `s` goes from the place held or the player's, and holds it there.
    fn hold(&mut self, s: Switched) {
        let len = self.p.queue.read(|q| q.len());
        if len == 0 {
            return;
        }
        let (at, ms) = match &self.held {
            Some((i, ms, _)) => (*i, *ms),
            None => (self.p.current().or(self.p.queue.read(|q| q.current())).unwrap_or(0), self.p.position_ms()),
        };
        let (i, ms) = match s {
            Switched::To(i, ms) => (i.min(len - 1), ms.max(0)),
            Switched::Seek(ms) => (at, ms.max(0)),
            Switched::Next => match self.p.queue.read(|q| q.next_of(at, q.repeat())) {
                Some(n) => (n, 0),
                None => return,
            },
            Switched::Previous => {
                let before = self.p.queue.read(|q| q.previous_of(at, q.repeat()));
                if previous_restarts(ms, before.is_some(), false) {
                    (at, 0)
                } else {
                    (before.unwrap_or(at), 0)
                }
            }
        };
        self.held = Some((i, ms, self.p.id_at(i)));
    }

    /// The queue was edited: a held place stays on its song, wherever that is now.
    fn follow_held(&mut self) {
        let Some((i, _, id)) = self.held.as_mut() else { return };
        let found = self.p.queue.read(|q| q.ids().iter().enumerate().filter(|(_, s)| **s == *id).map(|(k, _)| k).min_by_key(|k| k.abs_diff(*i)));
        match found {
            Some(k) => *i = k,
            None => self.held = None,
        }
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
        if !self.switches.is_empty() {
            self.reopen();
        }
        while let Some(s) = self.switches.pop_front() {
            // Only a play_at comes through here paused (a skip or a go_to is held instead): music is wanted.
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
        if self.idle_at.is_some_and(|t| now >= t) {
            self.release();
        }
        if self.pause_at.is_some_and(|t| now >= t) {
            self.pause_at = None;
            self.p.pause();
        }
        if self.switch_at.is_some_and(|t| now >= t) {
            self.run_switches();
        }
    }

    /// Each song's ReplayGain volume is put on the frame it starts at by the player itself (the ring
    /// takes it before any of the song's frames). Only a change of the settings is applied here, to
    /// the song playing, at once, as Android sets the player's volume.
    fn follow_gain(&mut self) {
        if !std::mem::take(&mut self.gain_changed) {
            return;
        }
        let level = self.p.current().map_or(1.0, |i| self.gain_of(i));
        self.p.sink.track.set_level(level);
    }

    fn gain_of(&mut self, i: usize) -> f32 {
        let id = self.p.id_at(i);
        self.p.app.gain(i, &id)
    }

    fn check(&mut self) {
        if let Some(message) = self.p.sink.track.take_failure() {
            (self.events)(Event::Error { id: String::new(), message });
            self.p.pause();
            self.set_state(State::Idle);
            // The device is let go as a long pause lets it go: the next play opens a new one where the
            // music was, rather than resuming into one that takes nothing.
            if self.released.is_none() {
                self.released = self.p.release();
                self.p.sink.track.release();
            }
        }
        for (id, message) in std::mem::take(&mut self.p.failures) {
            (self.events)(Event::Error { id, message });
        }
        self.p.changes.clear();
        if self.p.source_ended() {
            self.p.sink.track.set_ended(true);
        }
        if self.p.playing() && self.p.ended() && self.p.stopping_after().is_some() {
            // Heard to the end of the song the sleep timer stops at: paused on the next one, from its
            // start, which is fetched when play is pressed.
            let i = self.p.stopping_after().expect("checked");
            self.p.pause_at_end(false);
            self.p.pause();
            let next = self.p.queue.read(|q| q.next_of(i, q.repeat()));
            if let Some(n) = next {
                self.held = Some((n, 0, self.p.id_at(n)));
            }
            (self.events)(Event::Stopped);
            self.set_state(if next.is_some() { State::Paused } else { State::Ended });
        } else if self.p.playing() && self.p.ended() {
            self.p.pause();
            self.set_state(State::Ended);
        } else if !self.p.playing() && self.state == State::Playing && self.pause_at.is_none() && self.switch_at.is_none() {
            // The queue's rules stopped playback (a run of songs that would not play).
            (self.events)(Event::Stopped);
            self.set_state(State::Paused);
        }
    }

    fn report(&mut self, now: i64) {
        if let Some((i, ms)) = self.held.as_ref().map(|h| (h.0, h.1)) {
            // A place held while paused is where the player is, to the screen. Its id is copied only
            // when it changes.
            let id = || self.held.as_ref().map(|h| h.2.clone()).unwrap_or_default();
            if self.heard != Some(i) {
                self.heard = Some(i);
                (self.events)(Event::Song { index: i, id: id() });
            }
            let mut s = self.status.lock();
            s.state = self.state;
            if s.index != Some(i) {
                s.index = Some(i);
                s.id = Some(id());
            }
            s.position_ms = ms;
            s.at = Instant::now();
            s.switching = false;
            return;
        }
        if self.released.is_some() {
            // Let go: the place last reported stands.
            self.status.lock().releases = self.releases;
            return;
        }
        if self.p.current().is_none() {
            return;
        }
        let track = &self.p.sink.track;
        let stalled = self.state == State::Playing && self.p.starved() && track.filled_us() < STALL_US && track.latency_us() < STALL_US;
        if stalled != self.stalled {
            self.stalled = stalled;
            (self.events)(Event::Buffering(stalled));
        }
        let seen = self.p.bar();
        let index = seen.index.or(self.p.current());
        let ms = if seen.index.is_some() { seen.ms } else { self.p.position_ms() };
        // The song's id is copied only when the song changes: this runs on every wake.
        if index != self.heard {
            self.heard = index;
            if let Some(i) = index {
                (self.events)(Event::Song { index: i, id: self.p.id_at(i) });
            }
        }
        {
            let mut s = self.status.lock();
            s.state = self.state;
            if s.index != index {
                s.index = index;
                s.id = index.map(|i| self.p.id_at(i));
            }
            s.position_ms = ms;
            s.at = Instant::now();
            s.speed = self.p.speed().0;
            s.mixing = self.p.mixing();
            s.underruns = self.p.sink.track.underruns();
            s.releases = self.releases;
            s.switching = self.switch_at.is_some();
            s.chain = self.p.sink.chain_in();
            s.gain_reduction_db = self.p.sink.meter_db;
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
        if let Some(t) = self.idle_at {
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
            // The device's pull wakes the thread at the low mark; this is in case its clock runs slow. A
            // device that pulls in bursts leaves the ring standing between them, so a timer from the
            // ring's fill would only wake the thread for nothing once a burst.
            if !track.bursts() {
                at((fill - WAKE_LOW_US) / 1000 + 250);
            }
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
