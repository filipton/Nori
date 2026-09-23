//! The engine over the core, for a client that links it: the core's queue is the one played, its
//! transition planner and analysis store answer the engine, its settings are the sound, and songs
//! stream from the server's addresses through the client's [`ByteSource`] - or play from the disk, a
//! download or the stream cache, whose order is the core's. The downloads themselves run here too,
//! from the core's bookkeeping. With these a desktop client writes no player logic at all.

use std::collections::HashSet;
use std::io::{Read, Write};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use nori_player::automix::analysis::Analyzer;
use nori_player::dsp::Band;
use nori_player::engine::{Host, Plan};
use nori_player::pipeline::{App, Queue, Sound};
use nori_player::playlist::Playlist;
use nori_player::queue::{OnError, PlaybackError};
use nori_player::transitions::WindowSong;
use norimusic::automix::host::CoreHost;
use norimusic::client::Client;
use norimusic::settings::StoredPrefs;

use norimusic::transfers;
use norimusic::Core;
use parking_lot::Mutex;

use crate::engine::Settings;
use crate::library::{Library, Located, Source};
use crate::source::ByteSource;
use crate::store::{Order, Store};

/// The core's queue (`norimusic::playlist`). Edit it through the core's `playlist_*` calls, then tell
/// the engine ([`crate::Engine::queue_changed`]).
#[derive(Debug, Default, Clone, Copy)]
pub struct CoreQueue;

impl Queue for CoreQueue {
    fn read<R>(&self, f: impl FnOnce(&Playlist) -> R) -> R {
        norimusic::playlist::playlist_read(f)
    }

    fn moved_to(&mut self, index: usize) {
        norimusic::playlist::playlist_moved_to(index as i32);
    }

    fn set_repeat(&mut self, mode: u8) {
        norimusic::playlist::playlist_repeat(mode);
    }

    /// An explicit song with the user's "skip explicit songs" on, as `playlist_transition` decides.
    fn skips(&self, index: usize) -> bool {
        norimusic::playlist::playlist_skips(index)
    }
}

/// The core's transition planner, analysis store and log, as the engine's app.
pub struct CoreApp {
    host: CoreHost<fn()>,
    measurer: Option<Arc<Measurer>>,
    /// For each output device its own sound: the core that keeps the profiles, the outputs seen so far
    /// and the one the music goes to.
    devices: Option<Arc<Core>>,
    known: Vec<String>,
    output: Option<String>,
}

impl CoreApp {
    pub fn new() -> CoreApp {
        fn nothing() {}
        CoreApp { host: CoreHost { now_ms: 0, heard_changed: nothing }, measurer: None, devices: None, known: Vec::new(), output: None }
    }

    /// Output devices get the sound the core keeps for each (a profile bound to it, the sound from
    /// before a bound device took over), as Android's `DeviceSound` has it.
    pub fn per_device(mut self, core: Arc<Core>) -> CoreApp {
        self.devices = Some(core);
        self.known = nori_player::outputs::initial_known(&[]);
        self
    }

    /// With AutoMix on, the songs coming up that are on the disk are measured ahead by `measurer`.
    pub fn measuring(mut self, measurer: Arc<Measurer>) -> CoreApp {
        self.measurer = Some(measurer);
        self
    }
}

impl Default for CoreApp {
    fn default() -> Self {
        CoreApp::new()
    }
}

impl Host for CoreApp {
    fn plan_for(&mut self, outgoing_id: &str) -> Option<Plan> {
        self.host.plan_for(outgoing_id)
    }

    fn wants_analysis(&mut self, song_id: &str) -> Option<u64> {
        self.host.wants_analysis(song_id)
    }

    fn analysed(&mut self, song_id: &str, analyzer: Analyzer, channels: usize, frames: u64, rate: u32) {
        self.host.analysed(song_id, analyzer, channels, frames, rate);
    }

    fn log(&mut self, message: &str) {
        self.host.log(message);
    }

    fn now_ms(&self) -> i64 {
        self.host.now_ms()
    }
}

impl App for CoreApp {
    fn clock(&mut self, now_ms: i64) {
        self.host.now_ms = now_ms;
    }

    /// Songs are measured as they play (the engine's analysis tap), and with a measurer the ones coming
    /// up too, when AutoMix is on (the core's `queue_measure` names none otherwise).
    fn auto_mix(&self) -> bool {
        self.measurer.is_some() && !norimusic::rules::queue_measure().is_empty()
    }

    /// The core picks the songs (`queue_measure`: the next few, never a provider's or a radio stream);
    /// the measurer takes those on the disk, on a thread of its own.
    fn measure_ahead<S: nori_player::pipeline::Songs>(&mut self, _songs: &mut S, _ids: &[String]) {
        if let Some(m) = &self.measurer {
            m.update(norimusic::rules::queue_measure(), std::thread::current());
        }
    }

    /// The core names the device and says what arriving on it means (`Core::device_arrive`); a sound
    /// it applies is put into the settings, which the engine then plays with. AutoEQ curves are the
    /// client's to fetch and offer.
    fn output_changed(&mut self, kind: nori_player::outputs::OutputKind, name: &str) -> Option<(String, Option<Sound>)> {
        let core = self.devices.clone()?;
        let seen = nori_player::outputs::refresh(&[(kind, name)], &self.known, None);
        if let Some(known) = seen.known {
            self.known = known;
        }
        if self.output.as_ref() == Some(&seen.current) {
            return None;
        }
        self.output = Some(seen.current.clone());
        let mut effect = core.device_arrive(seen.current.clone()).effect;
        let mut sound = None;
        // A step may ask for the arrival to be made again once it is done (a sound kept first).
        for _ in 0..2 {
            if let Some(s) = effect.apply.take() {
                let prefs = norimusic::settings_store::settings_current()?.with_sound(s);
                norimusic::settings_store::settings_put(prefs.clone());
                sound = Some(settings(&prefs).sound);
            }
            if !effect.arrive {
                break;
            }
            effect = core.device_arrive(seen.current.clone()).effect;
        }
        Some((seen.current, sound))
    }

    fn measured(&mut self) -> bool {
        self.measurer.as_ref().is_some_and(|m| m.measured.swap(false, std::sync::atomic::Ordering::AcqRel))
    }

    /// The core keeps the window itself, from its own queue and what it knows of each song.
    fn window(&mut self, _window: Vec<WindowSong>, _shuffling: bool) {
        norimusic::playlist::playlist_window();
    }

    /// The core counts the run of songs that would not play, and reads "skip on error" itself.
    fn on_error(&mut self, kind: PlaybackError, _has_next: bool) -> Option<OnError> {
        Some(norimusic::rules::queue_error(kind, false, false))
    }

    fn playing(&mut self) {
        norimusic::rules::queue_playing();
    }

    fn transitions_off(&mut self, off: bool) {
        norimusic::automix::planner::transition_setup(off);
    }

    /// The core's ReplayGain over its own queue and the settings: track, album or automatic, the
    /// pre-amp, and the level for untagged songs.
    fn gain(&mut self, index: usize, _id: &str) -> f32 {
        norimusic::playlist::playlist_gain_of(index, false)
    }
}

/// The stream cache's order is the core's (`norimusic::stream_cache`): what this run never used goes
/// first, then the least recently used.
pub struct CoreOrder;

impl Order for CoreOrder {
    fn touch(&self, key: &str) {
        norimusic::stream_cache::touch(key);
    }

    fn seed(&self, held: &[String]) {
        norimusic::stream_cache::seed(held.iter().map(String::as_str));
    }

    fn next(&self) -> Option<String> {
        norimusic::stream_cache::next()
    }

    fn clear(&self) {
        norimusic::stream_cache::clear();
    }
}

/// Songs stream from the server the client is logged in to, at the quality the settings ask for; with
/// a store, a finished download or a whole cached copy plays from the disk before the network is asked,
/// and what streams is kept in the cache.
pub struct CoreLibrary {
    pub client: Arc<Client>,
    pub bytes: Arc<dyn ByteSource>,
    /// The network is metered: the metered quality is streamed.
    pub metered: bool,
    pub store: Option<Arc<Store>>,
}

/// The container a cache key's quality names (`<id>:192opus` is Opus); none for the original file.
pub fn key_format(key: &str) -> Option<String> {
    let q = key.rsplit_once(':')?.1.trim_start_matches(|c: char| c.is_ascii_digit());
    (!q.is_empty()).then(|| q.to_string())
}

impl Library for CoreLibrary {
    fn locate(&mut self, id: &str) -> Result<Located, String> {
        let song = norimusic::queue::queue_song(id.to_string());
        let duration_ms = song.as_ref().map(|s| s.duration as i64 * 1000).filter(|&d| d > 0);
        // A download may have been transcoded: the file says what it is.
        let kept = self.store.as_ref().filter(|_| transfers::held(id) == 2).and_then(|s| s.downloaded(id));
        if let Some(path) = kept {
            return Ok(Located { source: Source::File(path), hint: None, duration_ms });
        }
        let target = self.client.resolve(id.to_string(), false, self.metered);
        let hint = key_format(&target.key).or_else(|| song.as_ref().map(|s| s.suffix.clone())).filter(|s| !s.is_empty());
        let (url, bytes) = (target.url, self.bytes.clone());
        let source = match &self.store {
            Some(store) => Source::Cached { url, bytes, store: store.clone(), key: target.key },
            None => Source::Url { url, bytes },
        };
        Ok(Located { source, hint, duration_ms })
    }

    fn about(&self, id: &str) -> WindowSong {
        about(id)
    }

    /// Never a provider's song: asking for one makes the server download it.
    fn fetch_ahead(&self, id: &str) -> bool {
        fetch_ahead(id)
    }
}

/// What the transition planner and the seek bar know of `id`, from the core's queue: for a client
/// that writes its own [`Library`] around the core's.
pub fn about(id: &str) -> WindowSong {
    match norimusic::queue::queue_song(id.to_string()) {
        Some(s) => WindowSong {
            id: s.id,
            title: s.title,
            duration_ms: s.duration as i64 * 1000,
            album_id: s.album_id,
            disc: s.disc_number as i32,
            track: s.track as i32,
            tag_bpm: s.bpm as f32,
            radio: false,
        },
        None => WindowSong { id: id.to_string(), title: id.to_string(), ..Default::default() },
    }
}

/// Whether `id` may be fetched before anyone asked to hear it: never a provider's song.
pub fn fetch_ahead(id: &str) -> bool {
    !norimusic::queue::queue_fetchable(vec![id.to_string()]).is_empty()
}

/// Downloads, as the core keeps them (`Core::download_queue` and the downloads table): the songs still
/// to come are fetched whole through the client's [`ByteSource`] into the store, a few at a time as the
/// settings say, oldest first; progress, phases and the notification's words are the core's
/// (`transfers`). Its threads exist only while there is something to fetch.
pub struct Downloader {
    core: Arc<Core>,
    client: Arc<Client>,
    bytes: Arc<dyn ByteSource>,
    store: Arc<Store>,
    work: Mutex<Work>,
}

#[derive(Default)]
struct Work {
    running: usize,
    /// Being fetched now, and failed in this run (tried again only when asked again).
    busy: HashSet<String>,
    failed: HashSet<String>,
    threads: Vec<JoinHandle<()>>,
}

/// A download interrupted this many times in a row counts as failed.
const DOWNLOAD_TRIES: u32 = 3;
const DOWNLOAD_CHUNK: usize = 64 * 1024;

impl Downloader {
    pub fn new(core: Arc<Core>, client: Arc<Client>, bytes: Arc<dyn ByteSource>, store: Arc<Store>) -> Arc<Downloader> {
        Arc::new(Downloader { core, client, bytes, store, work: Mutex::new(Work::default()) })
    }

    /// Fetches what is queued, `slots` songs at a time; a call while it runs only fills free slots.
    pub fn start(self: &Arc<Self>, slots: usize) {
        let mut w = self.work.lock();
        w.failed.clear();
        w.threads.retain(|t| !t.is_finished());
        let pending = self.pending();
        for id in &pending {
            transfers::followed(id, transfers::QUEUED, norimusic::db::now_ms());
        }
        let want = slots.max(1).min(pending.len());
        while w.running < want {
            w.running += 1;
            let me = self.clone();
            match std::thread::Builder::new().name("nori-download".into()).spawn(move || me.run()) {
                Ok(t) => w.threads.push(t),
                Err(_) => w.running -= 1,
            }
        }
    }

    /// Waits until nothing is left to fetch.
    pub fn wait(&self) {
        loop {
            let Some(t) = self.work.lock().threads.pop() else { return };
            let _ = t.join();
        }
    }

    /// The unfinished downloads in the order the queue runs them: oldest first.
    fn pending(&self) -> Vec<String> {
        let mut ids: Vec<String> = self.core.downloads(false).unwrap_or_default().into_iter().map(|s| s.id).collect();
        ids.reverse();
        ids
    }

    fn take(&self) -> Option<String> {
        let pending = self.pending();
        let mut w = self.work.lock();
        let id = pending.into_iter().find(|id| !w.busy.contains(id) && !w.failed.contains(id));
        match &id {
            Some(id) => {
                w.busy.insert(id.clone());
            }
            None => w.running -= 1,
        }
        id
    }

    fn run(&self) {
        while let Some(id) = self.take() {
            let ok = self.fetch(&id);
            let now = norimusic::db::now_ms();
            let mut w = self.work.lock();
            w.busy.remove(&id);
            if ok {
                drop(w);
                transfers::followed(&id, transfers::COMPLETED, now);
                let _ = self.core.download_settle(vec![id.clone()], vec![true]);
                // The streamed copy is the same bytes twice now.
                self.store.drop_cached(&norimusic::stream_cache::copies(&id));
            } else {
                w.failed.insert(id.clone());
                drop(w);
                transfers::followed(&id, transfers::FAILED, now);
            }
        }
    }

    /// Fetches `id` whole into the store, taking up a download left half way where it stopped.
    fn fetch(&self, id: &str) -> bool {
        let now = norimusic::db::now_ms();
        transfers::followed(id, transfers::DOWNLOADING, now);
        let slot = transfers::open(id, now);
        let url = self.client.resolve(id.to_string(), true, false).url;
        let part = self.store.download_part(id);
        let mut chunk = vec![0u8; DOWNLOAD_CHUNK];
        let mut tries = 0;
        loop {
            let have = std::fs::metadata(&part).map_or(0, |m| m.len());
            let body = match self.bytes.open(&url, have) {
                Ok(b) => b,
                Err(_) => {
                    tries += 1;
                    if tries >= DOWNLOAD_TRIES {
                        return false;
                    }
                    std::thread::sleep(Duration::from_millis(500 << tries));
                    continue;
                }
            };
            // A server that would not do ranges sends it all again: the file starts again too.
            let (mut at, append) = if body.start == have { (have, true) } else { (0, false) };
            let file = std::fs::OpenOptions::new().create(true).write(true).append(append).truncate(!append).open(&part);
            let Ok(mut file) = file else { return false };
            let mut reader = body.reader;
            let broke = loop {
                match reader.read(&mut chunk) {
                    Ok(0) => break false,
                    Ok(n) => {
                        if file.write_all(&chunk[..n]).is_err() {
                            return false;
                        }
                        at += n as u64;
                        transfers::note(slot, body.len.unwrap_or(0) as i64, at as i64, norimusic::db::now_ms());
                    }
                    Err(_) => break true,
                }
            };
            if !broke && body.len.is_none_or(|l| at == l) && file.flush().is_ok() {
                drop(file);
                return std::fs::rename(&part, self.store.download_path(id)).is_ok();
            }
            tries += 1;
            if tries >= DOWNLOAD_TRIES {
                return false;
            }
        }
    }
}

/// AutoMix's measuring ahead, as Android's `AutoMixPrefetch` does it: the songs coming up that have no
/// analysis yet are decoded whole on a low-priority thread and measured, and the result stored through
/// the core, so a transition has both songs' tempo and beats the first time they meet. Only songs
/// already on the disk (downloaded, or whole in the stream cache) are measured: it costs the network
/// nothing, and a song not there yet is left for the next time the queue moves. The thread exists only
/// while there is something to measure.
pub struct Measurer {
    core: Arc<Core>,
    client: Arc<Client>,
    store: Arc<Store>,
    /// The songs to measure now, and a count that moves when they are replaced.
    want: Mutex<(Vec<String>, u64, bool, Option<std::thread::Thread>)>,
    /// Something was stored since the engine last asked.
    measured: std::sync::atomic::AtomicBool,
}

impl Measurer {
    pub fn new(core: Arc<Core>, client: Arc<Client>, store: Arc<Store>) -> Arc<Measurer> {
        Arc::new(Measurer { core, client, store, want: Mutex::new((Vec::new(), 0, false, None)), measured: Default::default() })
    }

    /// Measures `ids` (the songs coming up) from now on, dropping what was asked before: the point is
    /// the next boundary, not completeness. `engine` is woken when something was stored.
    pub fn update(self: &Arc<Self>, ids: Vec<String>, engine: std::thread::Thread) {
        let mut w = self.want.lock();
        w.0 = ids;
        w.1 += 1;
        w.3 = Some(engine);
        if w.2 || w.0.is_empty() {
            return;
        }
        w.2 = true;
        let me = self.clone();
        if std::thread::Builder::new().name("nori-measure".into()).spawn(move || me.run()).is_err() {
            w.2 = false;
        }
    }

    /// Where `id` is on the disk, whole, if it is.
    fn on_disk(&self, id: &str) -> Option<std::path::PathBuf> {
        if transfers::held(id) == 2 {
            if let Some(p) = self.store.downloaded(id) {
                return Some(p);
            }
        }
        self.store.peek(&self.client.resolve(id.to_string(), false, false).key)
    }

    fn run(&self) {
        lower_priority();
        loop {
            let (ids, generation) = {
                let mut w = self.want.lock();
                if w.0.is_empty() {
                    w.2 = false;
                    return;
                }
                (std::mem::take(&mut w.0), w.1)
            };
            let missing = self.core.analysis_missing(ids.clone()).unwrap_or_default();
            let waiting = missing.iter().filter(|id| self.on_disk(id).is_none()).count();
            norimusic::alog::info(&format!("measuring ahead: {} of {} unmeasured, {waiting} not on the device yet", missing.len(), ids.len()));
            for id in missing {
                if self.want.lock().1 != generation {
                    break;
                }
                let Some(path) = self.on_disk(&id) else { continue };
                if self.measure(&id, &path) {
                    self.measured.store(true, std::sync::atomic::Ordering::Release);
                    if let Some(t) = &self.want.lock().3 {
                        t.unpark();
                    }
                }
            }
        }
    }

    /// Decodes `id` whole into the streaming analyser and stores what comes out; whether it was stored.
    fn measure(&self, id: &str, path: &std::path::Path) -> bool {
        let song = norimusic::queue::queue_song(id.to_string());
        let expected_ms = song.as_ref().map_or(0, |s| s.duration as i64 * 1000);
        let hint = song.map(|s| s.suffix).filter(|s| !s.is_empty());
        let mut stream = None;
        let whole = crate::demux::decode_whole(path, hint.as_deref(), |rate, channels, samples| {
            stream.get_or_insert_with(|| norimusic::automix::store::AnalysisStream::new(rate, channels, expected_ms.max(0) as u64)).feed_f32(samples);
        });
        let Some(stream) = stream else { return false };
        if !matches!(whole, Ok(true)) {
            norimusic::alog::info(&format!("measuring {id} ahead stopped before its end: not stored"));
            return false;
        }
        let handle = stream.into_handle();
        let stored = self.core.analysis_finish_whole(id.to_string(), handle, expected_ms);
        // SAFETY: the handle was made just above and is handed to nobody else.
        unsafe { norimusic::automix::store::AnalysisStream::free_handle(handle) };
        let a = stored.ok().flatten();
        norimusic::alog::info(&match &a {
            Some(t) => format!("analysed {id} ahead: {:.2} bpm (conf {:.2}, stab {:.2})", t.bpm, t.bpm_confidence, t.stability),
            None => format!("analysed {id} ahead: not stored: not the whole song, or too short"),
        });
        a.is_some()
    }
}

/// The measuring thread yields to everything else: the lowest priority the system has.
fn lower_priority() {
    #[cfg(target_os = "linux")]
    // SAFETY: a plain system call about the calling thread (on Linux, `0` with PRIO_PROCESS is the
    // thread itself).
    unsafe {
        libc::setpriority(libc::PRIO_PROCESS, 0, 19);
    }
}

/// The sound and the controls as the core's settings ask for them.
pub fn settings(s: &StoredPrefs) -> Settings {
    let bands = if s.eq_enabled { s.eq_bands.iter().map(|b| Band { kind: b.kind, freq: b.freq as f64, gain_db: b.gain_db as f64, q: b.q as f64, channel: b.channel }).collect() } else { Vec::new() };
    let sound = Sound {
        bands,
        preamp_db: norimusic::dsp::effective_preamp_db(s) as f64,
        crossfeed_db: s.crossfeed_db as f64,
        balance: s.balance as f64,
        mono: s.mono,
        limiter: s.limiter,
        threshold_db: s.limiter_threshold_db as f64,
    };
    Settings { sound, speed: s.speed, pitch: s.pitch, skip_silence: s.skip_silence, fade_ms: s.fade_ms, hi_res: s.hi_res }
}
