//! The engine over the core, for a client that links it: the core's queue is the one played, its
//! transition planner and analysis store answer the engine, its settings are the sound, and songs
//! stream from the server's addresses through the client's [`ByteSource`]. With these a desktop client
//! writes no player logic at all.

use std::sync::Arc;

use nori_player::automix::analysis::Analyzer;
use nori_player::dsp::Band;
use nori_player::engine::{Host, Plan};
use nori_player::pipeline::{App, Queue, Sound};
use nori_player::playlist::Playlist;
use nori_player::transitions::WindowSong;
use norimusic::automix::host::CoreHost;
use norimusic::client::Client;
use norimusic::settings::StoredPrefs;

use crate::engine::Settings;
use crate::library::{Library, Located, Source};
use crate::source::ByteSource;

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
}

/// The core's transition planner, analysis store and log, as the engine's app.
pub struct CoreApp {
    host: CoreHost<fn()>,
}

impl CoreApp {
    pub fn new() -> CoreApp {
        fn nothing() {}
        CoreApp { host: CoreHost { now_ms: 0, heard_changed: nothing } }
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

    /// Songs are measured as they play (the engine's analysis tap); measuring the ones coming up ahead
    /// of time is a background job this client does not run yet.
    fn auto_mix(&self) -> bool {
        false
    }

    /// The core keeps the window itself, from its own queue and what it knows of each song.
    fn window(&mut self, _window: Vec<WindowSong>, _shuffling: bool) {
        norimusic::playlist::playlist_window();
    }
}

/// Songs stream from the server the client is logged in to, at the quality the settings ask for.
pub struct CoreLibrary {
    pub client: Arc<Client>,
    pub bytes: Arc<dyn ByteSource>,
    /// The network is metered: the metered quality is streamed.
    pub metered: bool,
}

impl Library for CoreLibrary {
    fn locate(&mut self, id: &str) -> Result<Located, String> {
        let target = self.client.resolve(id.to_string(), false, self.metered);
        let song = norimusic::queue::queue_song(id.to_string());
        let hint = song.as_ref().map(|s| s.suffix.clone()).filter(|s| !s.is_empty());
        let duration_ms = song.map(|s| s.duration as i64 * 1000).filter(|&d| d > 0);
        Ok(Located { source: Source::Url { url: target.url, bytes: self.bytes.clone() }, hint, duration_ms })
    }

    fn about(&self, id: &str) -> WindowSong {
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

    /// Never a provider's song: asking for one makes the server download it.
    fn fetch_ahead(&self, id: &str) -> bool {
        !norimusic::queue::queue_fetchable(vec![id.to_string()]).is_empty()
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
    Settings { sound, speed: s.speed, pitch: s.pitch, skip_silence: s.skip_silence, fade_ms: s.fade_ms }
}
