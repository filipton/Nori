//! The settings as they are stored, one value per key (settings_store.rs keeps them in the app's
//! database). Everything here is a free function with no database: what is stored goes in as it is and
//! one checked record comes back (defaults and ranges dealt with); saving is one call that says what to
//! write. The wire formats (the band list, a sound profile's JSON, a server profile's JSON) live here
//! too, so another player reading the same store gets the same settings.

use std::collections::HashMap;

use nori_model::{EqBand, EqKind, EqPreset, NamedPreset, TransitionPrefs};
use serde_json::{Map, Value};

/// One stored value.
#[derive(Debug, Clone, PartialEq)]
pub enum PrefValue {
    Flag { v: bool },
    Number { v: i32 },
    Big { v: i64 },
    Decimal { v: f32 },
    Text { v: String },
    Texts { v: Vec<String> },
}

/// One equalizer filter. `kind` is an `EqKind` ordinal (the order is the wire format, so it must not
/// change), `channel` 0 both, 1 left, 2 right.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SoundBand {
    pub kind: i32,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
    pub channel: i32,
}

/// How many kinds of band there are (`EqKind`), and how many channels a band can apply to.
const BAND_KINDS: i32 = 10;
const BAND_CHANNELS: i32 = 3;

/// One saved server. Each profile has its own index database, so switching is instant and nothing is re-synced.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SavedServer {
    pub id: String,
    pub name: String,
    pub url: String,
    pub alt_url: String,
    pub user: String,
    pub password: String,
    pub api_key: String,
    pub legacy_auth: bool,
    pub headers: HashMap<String, String>,
    pub allow_self_signed: bool,
    pub client_cert: String,
    pub client_cert_password: String,
    pub wifi_only: bool,
    pub music_folder_id: String,
    pub alt_max_bit_rate: i32,
}

/// One stream quality: `bit_rate` 0 and an empty `format` mean the original file.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SavedQuality {
    pub bit_rate: i32,
    pub format: String,
}

/// Every setting. The enums travel as their ordinals; each is in range once it has been through
/// `settings_load`. The names and meanings are the platform's `Prefs`.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StoredPrefs {
    pub servers: Vec<SavedServer>,
    pub active_server_id: String,
    pub wifi: SavedQuality,
    pub mobile: SavedQuality,
    pub download: SavedQuality,
    pub parallel_downloads: i32,
    pub covers_ahead: i32,
    pub cache_mb: i32,
    pub replay_gain: i32,
    pub preamp_db: f32,
    pub untagged_gain_db: f32,
    pub fade_ms: i32,
    pub pitch: f32,
    pub previous_always_skips: bool,
    pub precache_wifi: i32,
    pub precache_mobile: i32,
    pub skip_on_error: bool,
    pub crossfade_keep_albums: bool,
    pub offload: bool,
    pub bit_perfect: bool,
    pub hi_res: bool,
    pub scrobble: bool,
    pub auto_fill: bool,
    pub bridge_offline: bool,
    pub auto_fill_kind: i32,
    pub auto_fill_basis: i32,
    pub eq_enabled: bool,
    pub eq_bands: Vec<SoundBand>,
    pub eq_preamp_db: Option<f32>,
    pub crossfeed_db: f32,
    pub balance: f32,
    pub mono: bool,
    pub limiter: bool,
    pub limiter_threshold_db: f32,
    pub crossfade_sec: i32,
    pub auto_mix: bool,
    pub auto_mix_max_s: i32,
    pub auto_mix_beat_match: bool,
    pub auto_mix_max_tempo_pct: f32,
    pub auto_mix_bass_swap: bool,
    pub auto_mix_filters: bool,
    pub auto_mix_echo_out: bool,
    pub auto_mix_keep_pitch: bool,
    /// "Better beat detection": Beat This!, a neural beat tracker, reads the first and last half minute of the
    /// songs coming up for AutoMix's beat grids, once per song. Only in a build with the `neural-beats` feature;
    /// its model is downloaded once. Off by default.
    pub auto_mix_better_beats: bool,
    /// The beat model may be downloaded over mobile data; otherwise it waits for Wi-Fi.
    pub auto_mix_beats_mobile_data: bool,
    pub speed: f32,
    pub skip_silence: bool,
    pub scrobble_percent: i32,
    pub live_search_delay_ms: i32,
    pub taste_model: bool,
    /// "Look things up online": the switch over everything the app asks a third party for by itself -
    /// missing lyrics, the AutoEQ list, moving covers - each of which has its own switch under it. On for a
    /// new install (lyrics and the AutoEQ list are wanted out of the box; moving covers stay off).
    pub third_party_lookups: bool,
    pub profile_per_output: bool,
    pub auto_eq_auto: bool,
    /// Keep the AutoEQ headphone list on the device: fetched on an unmetered network when it is missing
    /// or a month old (`nori_devices::autoeq::index_due`). Needs `third_party_lookups`. On by default.
    pub auto_eq_download: bool,
    pub lyrics_sweep: bool,
    pub soft_sleeve: bool,
    /// Moving covers: an album's motion artwork from Apple Music plays in the player's sleeve, where it
    /// has one. Needs `third_party_lookups`. Off by default; off, nothing of it is built.
    pub motion_artwork: bool,
    /// Moving covers only on unmetered networks: each is a few megabytes.
    pub motion_artwork_wifi_only: bool,
    pub favourite_notice: bool,
    pub lyrics_keep_screen_on: bool,
    pub lyrics_translation: bool,
    pub lyrics_size: i32,
    /// Look lyrics up online when the server has no timed ones; needs `third_party_lookups`. Stored as
    /// "lyricsLrclib", from when LRCLIB was the only place asked.
    pub lyrics_online: bool,
    /// Every lyrics service by name, in the order they rank (`lyrics_sources`).
    pub lyrics_order: Vec<String>,
    /// The lyrics services switched on, by name.
    pub lyrics_on: Vec<String>,
    /// Keep asking past lyrics timed line by line for lyrics timed word by word, whoever ranks higher.
    pub lyrics_prefer_words: bool,
    /// The user's own PaxSenix key, for its Spotify and Musixmatch lyrics; empty for none.
    pub paxsenix_key: String,
    /// A BetterLyrics key, with which it looks up songs it has not stored yet; empty for none.
    pub better_lyrics_key: String,
    pub theme: i32,
    pub amoled: bool,
    pub player_colours: bool,
    pub dynamic_color: bool,
    pub accent: i64,
    pub cover_colors: bool,
    pub reduce_motion: bool,
    /// Animate even with Android's animations off; on unless the listener turns it off. Stored as
    /// "animateAnyway": the old "ignoreSystemMotion" was written off on every phone before this was the
    /// default, and it would have kept the animations off there.
    pub ignore_system_motion: bool,
    pub ui_scale: f32,
    pub tap_action: i32,
    pub swipe_right: i32,
    pub swipe_left: i32,
    pub skip_explicit: bool,
    /// `HomeRow` ordinals, in order; a row that is not listed is hidden.
    pub home_rows: Vec<i32>,
    pub pinned_playlists: Vec<String>,
    pub list_prefs: HashMap<String, String>,
}

/// The part of the settings a sound profile remembers.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SoundSettings {
    pub eq_enabled: bool,
    pub eq_bands: Vec<SoundBand>,
    pub eq_preamp_db: Option<f32>,
    pub crossfeed_db: f32,
    pub balance: f32,
    pub mono: bool,
    pub limiter: bool,
    pub limiter_threshold_db: f32,
    pub replay_gain: i32,
    pub preamp_db: f32,
    pub crossfade_sec: i32,
    pub hi_res: bool,
    pub bit_perfect: bool,
}

impl StoredPrefs {
    /// The part of the settings a sound profile remembers.
    pub fn sound(&self) -> SoundSettings {
        SoundSettings {
            eq_enabled: self.eq_enabled,
            eq_bands: self.eq_bands.clone(),
            eq_preamp_db: self.eq_preamp_db,
            crossfeed_db: self.crossfeed_db,
            balance: self.balance,
            mono: self.mono,
            limiter: self.limiter,
            limiter_threshold_db: self.limiter_threshold_db,
            replay_gain: self.replay_gain,
            preamp_db: self.preamp_db,
            crossfade_sec: self.crossfade_sec,
            hi_res: self.hi_res,
            bit_perfect: self.bit_perfect,
        }
    }

    /// These settings with the part a sound profile remembers taken from `s`.
    pub fn with_sound(self, s: SoundSettings) -> StoredPrefs {
        StoredPrefs {
            eq_enabled: s.eq_enabled,
            eq_bands: s.eq_bands,
            eq_preamp_db: s.eq_preamp_db,
            crossfeed_db: s.crossfeed_db,
            balance: s.balance,
            mono: s.mono,
            limiter: s.limiter,
            limiter_threshold_db: s.limiter_threshold_db,
            replay_gain: s.replay_gain,
            preamp_db: s.preamp_db,
            crossfade_sec: s.crossfade_sec,
            hi_res: s.hi_res,
            bit_perfect: s.bit_perfect,
            ..self
        }
    }

    /// What the transition planner takes from the settings (`nori_automix::planner::settings_changed`).
    pub fn transition_prefs(&self) -> TransitionPrefs {
        TransitionPrefs {
            auto_mix: self.auto_mix,
            crossfade_s: self.crossfade_sec,
            auto_mix_max_s: self.auto_mix_max_s,
            beat_match: self.auto_mix_beat_match,
            max_tempo_change_pct: self.auto_mix_max_tempo_pct,
            bass_swap: self.auto_mix_bass_swap,
            filter_effects: self.auto_mix_filters,
            echo_out: self.auto_mix_echo_out,
            keep_pitch: self.auto_mix_keep_pitch,
            keep_albums: self.crossfade_keep_albums,
            // AutoMix's loudness matching stands down under ReplayGain.
            replay_gain: self.replay_gain != 0,
        }
    }
}

/// A sound that cannot be made: a preset with no filters in it, or the profiles not reachable. The
/// client words each (the messages here are for the log).
#[derive(Debug, thiserror::Error)]
#[cfg_attr(feature = "ffi", derive(uniffi::Error))]
#[cfg_attr(feature = "ffi", uniffi(flat_error))]
pub enum SoundError {
    #[error("no filters in the preset")]
    NoFilters,
    #[error("database: {0}")]
    Db(String),
}

impl From<rusqlite::Error> for SoundError {
    fn from(e: rusqlite::Error) -> Self {
        SoundError::Db(e.to_string())
    }
}

impl From<nori_model::CoreError> for SoundError {
    fn from(e: nori_model::CoreError) -> Self {
        SoundError::Db(e.to_string())
    }
}

// ---- ranges, shared by loading and by the test bridge's setter ----

/// Songs downloaded at the same time; the rest wait their turn in the order they were asked for.
const PARALLEL_DOWNLOADS: (i32, i32) = (1, 10);
/// Covers of the songs coming up fetched ahead; the one before is always kept.
const COVERS_AHEAD: (i32, i32) = (0, 10);
/// `ReplayGainMode`: off, track, album, auto.
const REPLAY_GAIN: (i32, i32) = (0, 3);
const CACHE_MB: (i32, i32) = (256, 16384);
const RATE: (f32, f32) = (0.25, 4.0);
const FADE_MS: (i32, i32) = (0, 5000);
/// 0 small, 1 medium, 2 large.
const LYRICS_SIZE: (i32, i32) = (0, 2);
/// ReplayGain's overall level, as its slider offers it.
pub(crate) const REPLAY_GAIN_PREAMP: (f32, f32) = (-12.0, 6.0);

/// Each enum setting's values by name, in ordinal order: `AutoFillKind`, `AutoFillBasis`, `ReplayGainMode`,
/// `ThemeMode`, `TapAction`, `SwipeAction`. A change by name takes these as well as the ordinals.
pub(crate) const AUTO_FILL_KINDS: [&str; 2] = ["SONGS", "ALBUMS"];
pub(crate) const AUTO_FILL_BASES: [&str; 4] = ["SIMILAR", "ARTIST", "GENRE", "ERA"];
pub(crate) const REPLAY_GAIN_MODES: [&str; 4] = ["OFF", "TRACK", "ALBUM", "AUTO"];
pub(crate) const THEME_MODES: [&str; 3] = ["SYSTEM", "LIGHT", "DARK"];
pub(crate) const TAP_ACTION_NAMES: [&str; 4] = ["PLAY_LIST", "PLAY_ONE", "QUEUE", "PLAY_NEXT"];
pub(crate) const SWIPE_ACTION_NAMES: [&str; 5] = ["NONE", "QUEUE", "PLAY_NEXT", "FAVOURITE", "DOWNLOAD"];
const THEMES: i32 = THEME_MODES.len() as i32;
const TAP_ACTIONS: i32 = TAP_ACTION_NAMES.len() as i32;
const SWIPE_ACTIONS: i32 = SWIPE_ACTION_NAMES.len() as i32;
/// `HomeRow`, by the names they are stored under, in their order.
const HOME_ROWS: [&str; 8] = ["PINNED", "PLAYLISTS", "RECENT", "NEWEST", "FREQUENT", "TOP_SONGS", "RANDOM", "STARRED"];

/// The ten default bands: peaking filters an octave apart.
pub fn graphic() -> Vec<SoundBand> {
    [31.0, 62.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0, 16000.0]
        .into_iter()
        .map(|freq| SoundBand { kind: EqKind::Peaking as i32, freq, gain_db: 0.0, q: 1.41, channel: 0 })
        .collect()
}

impl Default for StoredPrefs {
    fn default() -> Self {
        StoredPrefs {
            servers: Vec::new(),
            active_server_id: String::new(),
            wifi: SavedQuality::default(),
            mobile: SavedQuality { bit_rate: 192, format: "opus".to_string() },
            download: SavedQuality::default(),
            parallel_downloads: 5,
            covers_ahead: 3,
            cache_mb: 1024,
            replay_gain: 0,
            preamp_db: 0.0,
            untagged_gain_db: -6.0,
            fade_ms: 0,
            pitch: 1.0,
            previous_always_skips: false,
            precache_wifi: 2,
            precache_mobile: 1,
            skip_on_error: true,
            crossfade_keep_albums: true,
            offload: true,
            bit_perfect: false,
            hi_res: false,
            scrobble: true,
            auto_fill: true,
            bridge_offline: false,
            auto_fill_kind: 0,
            auto_fill_basis: 0,
            eq_enabled: false,
            eq_bands: graphic(),
            eq_preamp_db: None,
            crossfeed_db: 0.0,
            balance: 0.0,
            mono: false,
            limiter: false,
            limiter_threshold_db: -1.0,
            crossfade_sec: 0,
            auto_mix: false,
            auto_mix_max_s: 12,
            auto_mix_beat_match: true,
            auto_mix_max_tempo_pct: 6.0,
            auto_mix_bass_swap: true,
            auto_mix_filters: true,
            auto_mix_echo_out: true,
            auto_mix_keep_pitch: true,
            auto_mix_better_beats: false,
            auto_mix_beats_mobile_data: false,
            speed: 1.0,
            skip_silence: false,
            scrobble_percent: 50,
            live_search_delay_ms: 350,
            taste_model: true,
            third_party_lookups: true,
            profile_per_output: true,
            auto_eq_auto: false,
            auto_eq_download: true,
            lyrics_sweep: true,
            soft_sleeve: true,
            motion_artwork: false,
            motion_artwork_wifi_only: true,
            favourite_notice: true,
            lyrics_keep_screen_on: true,
            lyrics_translation: true,
            lyrics_size: 1,
            lyrics_online: true,
            lyrics_order: crate::lyrics_sources::default_order(),
            lyrics_on: crate::lyrics_sources::default_on(),
            lyrics_prefer_words: true,
            paxsenix_key: String::new(),
            better_lyrics_key: String::new(),
            theme: 0,
            amoled: false,
            player_colours: true,
            dynamic_color: true,
            accent: 0xFF6750A4,
            cover_colors: true,
            reduce_motion: false,
            ignore_system_motion: true,
            ui_scale: 0.0,
            tap_action: 0,
            swipe_right: 1,
            swipe_left: 3,
            skip_explicit: false,
            home_rows: (0..HOME_ROWS.len() as i32).collect(),
            pinned_playlists: Vec::new(),
            list_prefs: HashMap::new(),
        }
    }
}

// ---- the band list: "kind:freq:gain:q:channel" per band, bands joined by ';' ----

/// Reads the stored band list. A band that does not read is dropped; `None` when none is left (the
/// caller falls back to the ten graphic bands).
pub fn decode_bands(s: &str) -> Option<Vec<SoundBand>> {
    let bands: Vec<SoundBand> = s
        .split(';')
        .filter_map(|b| {
            let p: Vec<&str> = b.split(':').collect();
            if p.len() < 4 {
                return None;
            }
            let kind = p[0].parse::<i32>().ok().filter(|k| (0..BAND_KINDS).contains(k))?;
            let channel = match p.get(4) {
                Some(c) => c.parse::<i32>().ok().filter(|c| (0..BAND_CHANNELS).contains(c))?,
                None => 0,
            };
            Some(SoundBand { kind, freq: float(p[1])?, gain_db: float(p[2])?, q: float(p[3])?, channel })
        })
        .collect();
    (!bands.is_empty()).then_some(bands)
}

pub fn encode_bands(bands: &[SoundBand]) -> String {
    let each: Vec<String> =
        bands.iter().map(|b| format!("{}:{}:{}:{}:{}", b.kind, kotlin_float(b.freq), kotlin_float(b.gain_db), kotlin_float(b.q), b.channel)).collect();
    each.join(";")
}

fn float(s: &str) -> Option<f32> {
    s.trim().parse().ok()
}

/// A float written the way the Kotlin side always wrote it ("1000.0", "1.41", "1.0E-5"), so what is
/// stored does not change shape when a different side saves it.
pub(crate) fn kotlin_float(v: f32) -> String {
    if v.is_nan() {
        return "NaN".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 { "Infinity" } else { "-Infinity" }.to_string();
    }
    let a = v.abs();
    if a == 0.0 || (1e-3..1e7).contains(&a) {
        let s = v.to_string();
        if s.contains('.') { s } else { format!("{s}.0") }
    } else {
        let s = format!("{v:e}");
        let (m, e) = s.split_once('e').unwrap_or((&s, "0"));
        if m.contains('.') { format!("{m}E{e}") } else { format!("{m}.0E{e}") }
    }
}

// ---- JSON read with a default for what is missing ----

fn opt_bool(o: &Map<String, Value>, k: &str) -> bool {
    o.get(k).and_then(Value::as_bool).unwrap_or(false)
}

fn opt_f64(o: &Map<String, Value>, k: &str, fallback: f64) -> f64 {
    o.get(k).and_then(Value::as_f64).unwrap_or(fallback)
}

fn opt_i32(o: &Map<String, Value>, k: &str) -> i32 {
    o.get(k).and_then(Value::as_i64).map_or(0, |i| i as i32)
}

fn text(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}

fn opt_string(o: &Map<String, Value>, k: &str) -> String {
    o.get(k).map(text).unwrap_or_default()
}

fn string_map(v: &Value) -> Option<HashMap<String, String>> {
    Some(v.as_object()?.iter().map(|(k, v)| (k.clone(), text(v))).collect())
}

// ---- a sound profile's JSON ----

/// Reads a saved sound; `None` when it is not a sound at all.
pub fn sound_from(json: &str) -> Option<SoundSettings> {
    let v: Value = serde_json::from_str(json).ok()?;
    let o = v.as_object()?;
    // A pre-amp that is there must be a number: null or anything else and the sound is not read.
    let eq_preamp_db = match o.get("eqPreampDb") {
        Some(v) => Some(v.as_f64()? as f32),
        None => None,
    };
    Some(SoundSettings {
        eq_enabled: opt_bool(o, "eqEnabled"),
        eq_bands: decode_bands(&opt_string(o, "eqBands")).unwrap_or_else(graphic),
        eq_preamp_db,
        crossfeed_db: opt_f64(o, "crossfeedDb", 0.0) as f32,
        balance: opt_f64(o, "balance", 0.0) as f32,
        mono: opt_bool(o, "mono"),
        limiter: opt_bool(o, "limiter"),
        limiter_threshold_db: opt_f64(o, "limiterThresholdDb", -1.0) as f32,
        replay_gain: opt_i32(o, "replayGain").clamp(REPLAY_GAIN.0, REPLAY_GAIN.1),
        preamp_db: opt_f64(o, "preampDb", 0.0) as f32,
        crossfade_sec: opt_i32(o, "crossfadeSec"),
        hi_res: opt_bool(o, "hiRes"),
        bit_perfect: opt_bool(o, "bitPerfect"),
    })
}

pub fn sound_json(s: &SoundSettings) -> String {
    let mut o = Map::new();
    o.insert("eqEnabled".into(), s.eq_enabled.into());
    o.insert("eqBands".into(), encode_bands(&s.eq_bands).into());
    if let Some(p) = s.eq_preamp_db {
        o.insert("eqPreampDb".into(), (p as f64).into());
    }
    o.insert("crossfeedDb".into(), (s.crossfeed_db as f64).into());
    o.insert("balance".into(), (s.balance as f64).into());
    o.insert("mono".into(), s.mono.into());
    o.insert("limiter".into(), s.limiter.into());
    o.insert("limiterThresholdDb".into(), (s.limiter_threshold_db as f64).into());
    o.insert("replayGain".into(), s.replay_gain.into());
    o.insert("preampDb".into(), (s.preamp_db as f64).into());
    o.insert("crossfadeSec".into(), s.crossfade_sec.into());
    o.insert("hiRes".into(), s.hi_res.into());
    o.insert("bitPerfect".into(), s.bit_perfect.into());
    Value::Object(o).to_string()
}

// ---- a server profile's JSON ----

/// One server from the stored list; `None` without an id.
fn server_from(v: &Value) -> Option<SavedServer> {
    let o = v.as_object()?;
    Some(SavedServer {
        id: text(o.get("id")?),
        name: opt_string(o, "name"),
        url: opt_string(o, "url"),
        alt_url: opt_string(o, "altUrl"),
        user: opt_string(o, "user"),
        password: opt_string(o, "password"),
        api_key: opt_string(o, "apiKey"),
        legacy_auth: opt_bool(o, "legacyAuth"),
        headers: o.get("headers").and_then(string_map).unwrap_or_default(),
        allow_self_signed: opt_bool(o, "allowSelfSigned"),
        client_cert: opt_string(o, "clientCert"),
        client_cert_password: opt_string(o, "clientCertPassword"),
        wifi_only: opt_bool(o, "wifiOnly"),
        music_folder_id: opt_string(o, "musicFolderId"),
        alt_max_bit_rate: opt_i32(o, "altMaxBitRate"),
    })
}

fn server_json(s: &SavedServer) -> Value {
    serde_json::json!({
        "id": s.id, "name": s.name, "url": s.url, "altUrl": s.alt_url, "user": s.user, "password": s.password,
        "apiKey": s.api_key, "legacyAuth": s.legacy_auth, "headers": s.headers, "allowSelfSigned": s.allow_self_signed,
        "clientCert": s.client_cert, "clientCertPassword": s.client_cert_password, "wifiOnly": s.wifi_only,
        "musicFolderId": s.music_folder_id, "altMaxBitRate": s.alt_max_bit_rate,
    })
}

// ---- loading and saving ----

struct Raw<'a>(&'a HashMap<String, PrefValue>);

impl Raw<'_> {
    fn flag(&self, k: &str, d: bool) -> bool {
        match self.0.get(k) {
            Some(PrefValue::Flag { v }) => *v,
            _ => d,
        }
    }
    fn int(&self, k: &str, d: i32) -> i32 {
        match self.0.get(k) {
            Some(PrefValue::Number { v }) => *v,
            _ => d,
        }
    }
    fn long(&self, k: &str, d: i64) -> i64 {
        match self.0.get(k) {
            Some(PrefValue::Big { v }) => *v,
            _ => d,
        }
    }
    fn float(&self, k: &str, d: f32) -> f32 {
        match self.0.get(k) {
            Some(PrefValue::Decimal { v }) => *v,
            _ => d,
        }
    }
    fn text(&self, k: &str) -> Option<&str> {
        match self.0.get(k) {
            Some(PrefValue::Text { v }) => Some(v),
            _ => None,
        }
    }
    /// Names kept as one comma-separated text; none when nothing was stored.
    fn list(&self, k: &str) -> Option<Vec<String>> {
        self.text(k).map(names)
    }
    /// An enum stored as its ordinal; one out of range (a value from a newer version) is the default.
    fn ordinal(&self, k: &str, count: i32, d: i32) -> i32 {
        Some(self.int(k, d)).filter(|i| (0..count).contains(i)).unwrap_or(d)
    }
    fn quality(&self, name: &str, d: &SavedQuality) -> SavedQuality {
        SavedQuality {
            bit_rate: self.int(&format!("{name}BitRate"), d.bit_rate),
            format: self.text(&format!("{name}Format")).map_or_else(|| d.format.clone(), str::to_string),
        }
    }

    /// The server profiles; a list that does not read, or a server without an id, loses the whole list.
    fn servers(&self) -> Vec<SavedServer> {
        let Some(json) = self.text("servers") else { return Vec::new() };
        let list: Option<Vec<SavedServer>> =
            serde_json::from_str::<Value>(json).ok().and_then(|v| v.as_array().map(|a| a.iter().map(server_from).collect())).flatten();
        list.unwrap_or_default()
    }
}

/// Everything stored, as it is, into the settings: defaults for what is missing or of the wrong type,
/// ranges enforced.
pub fn load(raw: &HashMap<String, PrefValue>) -> StoredPrefs {
    let r = Raw(raw);
    let d = StoredPrefs::default();
    StoredPrefs {
        servers: r.servers(),
        active_server_id: r.text("activeServerId").map(str::to_string).unwrap_or_default(),
        wifi: r.quality("wifi", &d.wifi),
        mobile: r.quality("mobile", &d.mobile),
        download: r.quality("download", &d.download),
        cache_mb: r.int("cacheMb", d.cache_mb),
        parallel_downloads: r.int("parallelDownloads", d.parallel_downloads).clamp(PARALLEL_DOWNLOADS.0, PARALLEL_DOWNLOADS.1),
        covers_ahead: r.int("coversAhead", d.covers_ahead).clamp(COVERS_AHEAD.0, COVERS_AHEAD.1),
        replay_gain: r.int("replayGain", 0).clamp(REPLAY_GAIN.0, REPLAY_GAIN.1),
        preamp_db: r.float("preampDb", 0.0),
        untagged_gain_db: r.float("untaggedGainDb", d.untagged_gain_db),
        fade_ms: r.int("fadeMs", 0),
        pitch: r.float("pitch", 1.0),
        previous_always_skips: r.flag("previousAlwaysSkips", false),
        precache_wifi: r.int("precacheWifi", d.precache_wifi),
        precache_mobile: r.int("precacheMobile", d.precache_mobile),
        skip_on_error: r.flag("skipOnError", true),
        crossfade_keep_albums: r.flag("crossfadeKeepAlbums", true),
        offload: r.flag("offload", true),
        bit_perfect: r.flag("bitPerfect", false),
        hi_res: r.flag("hiRes", false),
        scrobble: r.flag("scrobble", true),
        auto_fill: r.flag("autoFill", true),
        bridge_offline: r.flag("bridgeOffline", false),
        auto_fill_kind: r.ordinal("autoFillKind", AUTO_FILL_KINDS.len() as i32, d.auto_fill_kind),
        auto_fill_basis: r.ordinal("autoFillBasis", AUTO_FILL_BASES.len() as i32, d.auto_fill_basis),
        eq_enabled: r.flag("eqEnabled", false),
        eq_bands: r.text("eqBands").and_then(decode_bands).unwrap_or(d.eq_bands),
        eq_preamp_db: match raw.get("eqPreampDb") {
            Some(PrefValue::Decimal { v }) => Some(*v),
            _ => None,
        },
        crossfeed_db: r.float("crossfeedDb", 0.0),
        balance: r.float("balance", 0.0),
        mono: r.flag("mono", false),
        limiter: r.flag("limiter", false),
        limiter_threshold_db: r.float("limiterThresholdDb", -1.0),
        crossfade_sec: r.int("crossfadeSec", 0),
        auto_mix: r.flag("autoMix", false),
        auto_mix_max_s: r.int("autoMixMaxS", 12),
        auto_mix_beat_match: r.flag("autoMixBeatMatch", true),
        auto_mix_max_tempo_pct: r.float("autoMixMaxTempoPct", 6.0),
        auto_mix_bass_swap: r.flag("autoMixBassSwap", true),
        auto_mix_filters: r.flag("autoMixFilters", true),
        auto_mix_echo_out: r.flag("autoMixEchoOut", true),
        auto_mix_keep_pitch: r.flag("autoMixKeepPitch", true),
        auto_mix_better_beats: r.flag("autoMixBetterBeats", false),
        auto_mix_beats_mobile_data: r.flag("autoMixBeatsMobileData", false),
        speed: r.float("speed", 1.0),
        skip_silence: r.flag("skipSilence", false),
        scrobble_percent: r.int("scrobblePercent", 50),
        live_search_delay_ms: r.int("liveSearchDelayMs", d.live_search_delay_ms),
        profile_per_output: r.flag("profilePerOutput", true),
        auto_eq_auto: r.flag("autoEqAuto", false),
        auto_eq_download: r.flag("autoEqDownload", true),
        taste_model: r.flag("tasteModel", true),
        third_party_lookups: r.flag("thirdPartyLookups", true),
        lyrics_sweep: r.flag("lyricsSweep", true),
        soft_sleeve: r.flag("softSleeve", true),
        motion_artwork: r.flag("motionArtwork", false),
        motion_artwork_wifi_only: r.flag("motionArtworkWifiOnly", true),
        favourite_notice: r.flag("favouriteNotice", true),
        lyrics_keep_screen_on: r.flag("lyricsKeepScreenOn", true),
        lyrics_translation: r.flag("lyricsTranslation", true),
        lyrics_size: r.int("lyricsSize", 1),
        lyrics_online: r.flag("lyricsLrclib", true),
        lyrics_order: crate::lyrics_sources::complete_order(&r.list("lyricsOrder").unwrap_or_default()),
        lyrics_on: r.list("lyricsOn").map_or(d.lyrics_on, |on| crate::lyrics_sources::known(&on)),
        lyrics_prefer_words: r.flag("lyricsPreferWords", true),
        paxsenix_key: r.text("paxSenixKey").unwrap_or_default().to_string(),
        better_lyrics_key: r.text("betterLyricsKey").unwrap_or_default().to_string(),
        theme: r.ordinal("theme", THEMES, 0),
        amoled: r.flag("amoled", false),
        dynamic_color: r.flag("dynamicColor", true),
        accent: r.long("accent", d.accent),
        cover_colors: r.flag("coverColors", true),
        reduce_motion: r.flag("reduceMotion", false),
        ignore_system_motion: r.flag("animateAnyway", true),
        ui_scale: r.float("uiScale", 0.0),
        player_colours: r.flag("playerColours", true),
        tap_action: r.ordinal("tapAction", TAP_ACTIONS, d.tap_action),
        swipe_right: r.ordinal("swipeRight", SWIPE_ACTIONS, d.swipe_right),
        swipe_left: r.ordinal("swipeLeft", SWIPE_ACTIONS, d.swipe_left),
        skip_explicit: r.flag("skipExplicit", false),
        home_rows: r.text("homeRows").map_or(d.home_rows, |s| {
            s.split(',').filter_map(|n| HOME_ROWS.iter().position(|r| *r == n)).map(|i| i as i32).collect()
        }),
        pinned_playlists: r.text("pinnedPlaylists").map_or_else(Vec::new, |s| s.split('\n').filter(|p| !p.is_empty()).map(str::to_string).collect()),
        list_prefs: r.text("listPrefs").and_then(|j| serde_json::from_str::<Value>(j).ok()).and_then(|v| string_map(&v)).unwrap_or_default(),
    }
}

/// Everything to write for these settings.
pub fn save(p: &StoredPrefs) -> HashMap<String, PrefValue> {
    let mut put = HashMap::with_capacity(96);
    let mut flag = |k: &str, v: bool| put.insert(k.to_string(), PrefValue::Flag { v });
    flag("previousAlwaysSkips", p.previous_always_skips);
    flag("skipOnError", p.skip_on_error);
    flag("crossfadeKeepAlbums", p.crossfade_keep_albums);
    flag("offload", p.offload);
    flag("bitPerfect", p.bit_perfect);
    flag("scrobble", p.scrobble);
    flag("hiRes", p.hi_res);
    flag("autoFill", p.auto_fill);
    flag("bridgeOffline", p.bridge_offline);
    flag("eqEnabled", p.eq_enabled);
    flag("mono", p.mono);
    flag("limiter", p.limiter);
    flag("autoMix", p.auto_mix);
    flag("autoMixBeatMatch", p.auto_mix_beat_match);
    flag("autoMixBassSwap", p.auto_mix_bass_swap);
    flag("autoMixFilters", p.auto_mix_filters);
    flag("autoMixEchoOut", p.auto_mix_echo_out);
    flag("autoMixKeepPitch", p.auto_mix_keep_pitch);
    flag("autoMixBetterBeats", p.auto_mix_better_beats);
    flag("autoMixBeatsMobileData", p.auto_mix_beats_mobile_data);
    flag("skipSilence", p.skip_silence);
    flag("profilePerOutput", p.profile_per_output);
    flag("autoEqAuto", p.auto_eq_auto);
    flag("autoEqDownload", p.auto_eq_download);
    flag("tasteModel", p.taste_model);
    flag("thirdPartyLookups", p.third_party_lookups);
    flag("lyricsSweep", p.lyrics_sweep);
    flag("softSleeve", p.soft_sleeve);
    flag("motionArtwork", p.motion_artwork);
    flag("motionArtworkWifiOnly", p.motion_artwork_wifi_only);
    flag("favouriteNotice", p.favourite_notice);
    flag("lyricsKeepScreenOn", p.lyrics_keep_screen_on);
    flag("lyricsTranslation", p.lyrics_translation);
    flag("lyricsLrclib", p.lyrics_online);
    flag("lyricsPreferWords", p.lyrics_prefer_words);
    flag("amoled", p.amoled);
    flag("dynamicColor", p.dynamic_color);
    flag("coverColors", p.cover_colors);
    flag("reduceMotion", p.reduce_motion);
    flag("animateAnyway", p.ignore_system_motion);
    flag("playerColours", p.player_colours);
    flag("skipExplicit", p.skip_explicit);
    let mut int = |k: &str, v: i32| put.insert(k.to_string(), PrefValue::Number { v });
    for (n, q) in [("wifi", &p.wifi), ("mobile", &p.mobile), ("download", &p.download)] {
        int(&format!("{n}BitRate"), q.bit_rate);
    }
    int("cacheMb", p.cache_mb);
    int("parallelDownloads", p.parallel_downloads);
    int("coversAhead", p.covers_ahead);
    int("replayGain", p.replay_gain);
    int("fadeMs", p.fade_ms);
    int("precacheWifi", p.precache_wifi);
    int("precacheMobile", p.precache_mobile);
    int("autoFillKind", p.auto_fill_kind);
    int("autoFillBasis", p.auto_fill_basis);
    int("crossfadeSec", p.crossfade_sec);
    int("autoMixMaxS", p.auto_mix_max_s);
    int("scrobblePercent", p.scrobble_percent);
    int("liveSearchDelayMs", p.live_search_delay_ms);
    int("lyricsSize", p.lyrics_size);
    int("theme", p.theme);
    int("tapAction", p.tap_action);
    int("swipeRight", p.swipe_right);
    int("swipeLeft", p.swipe_left);
    let mut float = |k: &str, v: f32| put.insert(k.to_string(), PrefValue::Decimal { v });
    float("preampDb", p.preamp_db);
    float("untaggedGainDb", p.untagged_gain_db);
    float("pitch", p.pitch);
    float("crossfeedDb", p.crossfeed_db);
    float("balance", p.balance);
    float("limiterThresholdDb", p.limiter_threshold_db);
    float("autoMixMaxTempoPct", p.auto_mix_max_tempo_pct);
    float("speed", p.speed);
    float("uiScale", p.ui_scale);
    if let Some(v) = p.eq_preamp_db {
        float("eqPreampDb", v);
    }
    let mut text = |k: &str, v: String| put.insert(k.to_string(), PrefValue::Text { v });
    text("servers", Value::Array(p.servers.iter().map(server_json).collect()).to_string());
    text("activeServerId", p.active_server_id.clone());
    for (n, q) in [("wifi", &p.wifi), ("mobile", &p.mobile), ("download", &p.download)] {
        text(&format!("{n}Format"), q.format.clone());
    }
    text("eqBands", encode_bands(&p.eq_bands));
    let rows: Vec<&str> = p.home_rows.iter().filter_map(|i| usize::try_from(*i).ok().and_then(|i| HOME_ROWS.get(i)).copied()).collect();
    text("homeRows", rows.join(","));
    text("pinnedPlaylists", p.pinned_playlists.join("\n"));
    text("listPrefs", serde_json::to_string(&p.list_prefs).unwrap_or_else(|_| "{}".to_string()));
    text("lyricsOrder", p.lyrics_order.join(","));
    text("lyricsOn", p.lyrics_on.join(","));
    text("paxSenixKey", p.paxsenix_key.clone());
    text("betterLyricsKey", p.better_lyrics_key.clone());
    put.insert("accent".to_string(), PrefValue::Big { v: p.accent });
    put
}

/// What a change by name did: the settings after it, whether the stream cache has to shrink to a new
/// limit now, and whether the active server's own profile changed (its connection is set up again).
/// `effect` is what the player has to apply again (`settings_store`'s bits), once the change is kept.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingChange {
    pub prefs: StoredPrefs,
    pub apply_cache_limit: bool,
    pub server: bool,
    pub effect: u32,
}

/// Comma-separated names, each trimmed, the empty ones left out.
fn names(value: &str) -> Vec<String> {
    value.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect()
}

/// Stream quality as a value: "0:" is the original file, "320:mp3" a bitrate and a format.
pub(crate) fn quality_name(q: &SavedQuality) -> String {
    format!("{}:{}", q.bit_rate, q.format)
}

fn quality_named(value: &str) -> Option<SavedQuality> {
    let (rate, format) = value.split_once(':')?;
    Some(SavedQuality { bit_rate: rate.trim().parse().ok()?, format: format.trim().to_string() })
}

/// Changes one setting by name: the settings screen's rows (each row says which name it sets) and the
/// debug test bridge (`tools/app.sh set <name> <value>`). A switch reads "true"/"1" as on; an enum reads
/// its name or its ordinal; a number that does not read leaves the setting as it is. Anything that is
/// not a setting is `None`, so a typo in a script fails loudly instead of silently doing nothing.
pub fn set_by_name(p: &StoredPrefs, name: &str, value: &str) -> Option<SettingChange> {
    let on = value.eq_ignore_ascii_case("true") || value == "1";
    let int = value.trim().parse::<i32>().ok();
    let float = value.trim().parse::<f32>().ok();
    let clamp = |v: Option<i32>, (lo, hi): (i32, i32), keep: i32| v.map_or(keep, |v| v.clamp(lo, hi));
    let named = |names: &[&str]| {
        names.iter().position(|n| n.eq_ignore_ascii_case(value)).map(|i| i as i32).or(int.filter(|i| (0..names.len() as i32).contains(i)))
    };
    let mut n = p.clone();
    let mut server = false;
    match name {
        // "raw" or "<format>:<kbps>" (e.g. "opus:128"): what streams on Wi-Fi, for checks of a codec.
        "wifiQuality" => {
            n.wifi = match value.split_once(':') {
                Some((format, kbps)) => SavedQuality { bit_rate: kbps.parse().ok()?, format: format.to_string() },
                None if value == "raw" => SavedQuality::default(),
                None => return None,
            }
        }
        "limiter" => n.limiter = on,
        "eq" => n.eq_enabled = on,
        "mono" => n.mono = on,
        "hiRes" => n.hi_res = on,
        "bitPerfect" => n.bit_perfect = on,
        "offload" => n.offload = on,
        "autoMix" => n.auto_mix = on,
        "amoled" => n.amoled = on,
        "ignoreSystemMotion" => n.ignore_system_motion = on,
        "reduceMotion" => n.reduce_motion = on,
        "playerColours" => n.player_colours = on,
        "coverColors" => n.cover_colors = on,
        "dynamicColor" => n.dynamic_color = on,
        // The lookups switch covers the lyrics services too, so it takes the lyrics half with it both ways.
        "thirdPartyLookups" => (n.third_party_lookups, n.lyrics_online) = (on, on),
        // Lyrics online need lookups, so switching them on switches lookups on; off leaves the lookups
        // (the AutoEQ list, moving covers) as they are. Stored as "lyricsLrclib", and still answers to it.
        "lyricsOnline" | "lyricsLrclib" => (n.lyrics_online, n.third_party_lookups) = (on, on || p.third_party_lookups),
        "lyricsPreferWords" => n.lyrics_prefer_words = on,
        "paxSenixKey" => n.paxsenix_key = value.trim().to_string(),
        "betterLyricsKey" => n.better_lyrics_key = value.trim().to_string(),
        // The whole ranking, by name (the test bridge).
        "lyricsOrder" => n.lyrics_order = crate::lyrics_sources::complete_order(&names(value)),
        // Back to how they come out of the box (the test bridge).
        "lyricsSources" if value.trim().eq_ignore_ascii_case("default") => {
            n.lyrics_order = crate::lyrics_sources::default_order();
            n.lyrics_on = crate::lyrics_sources::default_on();
        }
        // The services asked, in this order, and no others (the test bridge): `lyricsSources lrclib,unison`.
        "lyricsSources" => {
            let on = crate::lyrics_sources::known(&names(value));
            let rest = p.lyrics_order.iter().filter(|s| !on.contains(s)).cloned();
            n.lyrics_order = on.iter().cloned().chain(rest).collect();
            n.lyrics_on = on;
        }
        // One service dropped at a place in the ranking, held by its handle and dragged: `NETEASE:3`.
        "lyricsPlace" => {
            let (service, to) = value.split_once(':')?;
            let service = crate::lyrics_sources::LyricsService::named(service)?;
            n.lyrics_order = crate::lyrics_sources::placed(p, service, to.trim().parse().ok()?);
        }
        // One service a place up or down in the ranking (the terminal's keys): `NETEASE:-1`.
        "lyricsMove" => {
            let (service, by) = value.split_once(':')?;
            let service = crate::lyrics_sources::LyricsService::named(service)?;
            n.lyrics_order = crate::lyrics_sources::moved(p, service, by.trim().parse().ok()?);
        }
        "crossfadeKeepAlbums" => n.crossfade_keep_albums = on,
        "lyricsSweep" => n.lyrics_sweep = on,
        "lyricsTranslation" => n.lyrics_translation = on,
        "lyricsKeepScreenOn" => n.lyrics_keep_screen_on = on,
        "lyricsSize" => n.lyrics_size = clamp(int, LYRICS_SIZE, p.lyrics_size),
        "softSleeve" => n.soft_sleeve = on,
        // Moving covers need lookups, so switching them on switches lookups on, as lyrics online do.
        "motionArtwork" => (n.motion_artwork, n.third_party_lookups) = (on, on || p.third_party_lookups),
        "motionArtworkWifiOnly" => n.motion_artwork_wifi_only = on,
        // The row says "on mobile data", the setting "Wi-Fi only": the one is the other turned round.
        "motionArtworkMobile" => n.motion_artwork_wifi_only = !on,
        "favouriteNotice" => n.favourite_notice = on,
        "crossfadeSec" => n.crossfade_sec = int.unwrap_or(p.crossfade_sec),
        "autoMixMaxS" => n.auto_mix_max_s = int.unwrap_or(p.auto_mix_max_s),
        "autoMixBeatMatch" => n.auto_mix_beat_match = on,
        "autoMixMaxTempoPct" => n.auto_mix_max_tempo_pct = float.unwrap_or(p.auto_mix_max_tempo_pct),
        "autoMixKeepPitch" => n.auto_mix_keep_pitch = on,
        "autoMixBassSwap" => n.auto_mix_bass_swap = on,
        "autoMixFilters" => n.auto_mix_filters = on,
        "autoMixEchoOut" => n.auto_mix_echo_out = on,
        "autoMixBetterBeats" => n.auto_mix_better_beats = on,
        "autoMixBeatsMobileData" => n.auto_mix_beats_mobile_data = on,
        "coversAhead" => n.covers_ahead = clamp(int, COVERS_AHEAD, p.covers_ahead),
        "cacheMb" => n.cache_mb = clamp(int, CACHE_MB, p.cache_mb),
        "parallelDownloads" => n.parallel_downloads = clamp(int, PARALLEL_DOWNLOADS, p.parallel_downloads),
        "precacheWifi" => n.precache_wifi = int.unwrap_or(p.precache_wifi),
        "precacheMobile" => n.precache_mobile = int.unwrap_or(p.precache_mobile),
        "wifi" => n.wifi = quality_named(value)?,
        "mobile" => n.mobile = quality_named(value)?,
        "download" => n.download = quality_named(value)?,
        "speed" => n.speed = float.map_or(p.speed, |v| v.clamp(RATE.0, RATE.1)),
        "pitch" => n.pitch = float.map_or(p.pitch, |v| v.clamp(RATE.0, RATE.1)),
        "skipSilence" => n.skip_silence = on,
        "fadeMs" => n.fade_ms = clamp(int, FADE_MS, p.fade_ms),
        "previousAlwaysSkips" => n.previous_always_skips = on,
        "crossfeedDb" => n.crossfeed_db = float.unwrap_or(p.crossfeed_db),
        // The equalizer's own pre-amp, the one in front of the limiter: a number within its range, or "auto".
        "eqPreampDb" => n.eq_preamp_db = match float {
            Some(v) => Some(EQ_RANGES.preamp.hold(v)),
            None if value.trim().eq_ignore_ascii_case("auto") => None,
            None => p.eq_preamp_db,
        },
        "limiterThresholdDb" => n.limiter_threshold_db = float.unwrap_or(p.limiter_threshold_db),
        "replayGain" => n.replay_gain = named(&REPLAY_GAIN_MODES)?,
        "preampDb" => n.preamp_db = float.map_or(p.preamp_db, |v| v.clamp(REPLAY_GAIN_PREAMP.0, REPLAY_GAIN_PREAMP.1)),
        "untaggedGainDb" => n.untagged_gain_db = float.unwrap_or(p.untagged_gain_db),
        "autoFill" => n.auto_fill = on,
        "bridgeOffline" => n.bridge_offline = on,
        "autoFillKind" => n.auto_fill_kind = named(&AUTO_FILL_KINDS)?,
        "autoFillBasis" => n.auto_fill_basis = named(&AUTO_FILL_BASES)?,
        "autoEqAuto" => n.auto_eq_auto = on,
        // The list comes from a third party, so switching it on switches lookups on, as lyrics online do.
        "autoEqDownload" => (n.auto_eq_download, n.third_party_lookups) = (on, on || p.third_party_lookups),
        "profilePerOutput" => n.profile_per_output = on,
        "skipExplicit" => n.skip_explicit = on,
        "skipOnError" => n.skip_on_error = on,
        // One lyrics service switched on or off: `lyricsService:NETEASE`.
        _ if name.starts_with("lyricsService:") => {
            let service = crate::lyrics_sources::LyricsService::named(&name["lyricsService:".len()..])?;
            n.lyrics_on.retain(|s| s != service.name());
            if on {
                n.lyrics_on.push(service.name().to_string());
            }
        }
        "theme" => n.theme = named(&THEME_MODES)?,
        "accent" => n.accent = value.trim().parse::<i64>().unwrap_or(p.accent),
        "uiScale" => n.ui_scale = float.unwrap_or(p.ui_scale),
        "tapAction" => n.tap_action = named(&TAP_ACTION_NAMES)?,
        "swipeRight" => n.swipe_right = named(&SWIPE_ACTION_NAMES)?,
        "swipeLeft" => n.swipe_left = named(&SWIPE_ACTION_NAMES)?,
        "liveSearchDelayMs" => n.live_search_delay_ms = int.unwrap_or(p.live_search_delay_ms),
        "tasteModel" => n.taste_model = on,
        "scrobble" => n.scrobble = on,
        "scrobblePercent" => n.scrobble_percent = int.unwrap_or(p.scrobble_percent),
        // The active server's own settings: which music folder it browses, and the bitrate cap on its
        // second address.
        "musicFolder" | "altMaxBitRate" => {
            let s = n.servers.iter_mut().find(|s| s.id == p.active_server_id)?;
            if name == "musicFolder" {
                s.music_folder_id = value.to_string();
            } else {
                s.alt_max_bit_rate = int.map_or(s.alt_max_bit_rate, |v| v.max(0));
            }
            server = true;
        }
        _ => return None,
    }
    // "Space for streamed music" is applied at once instead of at the next track.
    Some(SettingChange { prefs: n, apply_cache_limit: name == "cacheMb", server, effect: 0 })
}

/// What a server is called in lists: its name, or else the host of its address.
pub fn label(name: &str, url: &str) -> String {
    if !name.trim().is_empty() {
        return name.to_string();
    }
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    rest.split('/').next().unwrap_or_default().to_string()
}

// ---- equalizer edits ----

fn with_bands(s: SoundSettings, eq_bands: Vec<SoundBand>) -> SoundSettings {
    SoundSettings { eq_bands, ..s }
}

fn band_of(b: &nori_model::EqBand) -> SoundBand {
    SoundBand { kind: b.kind as i32, freq: b.freq, gain_db: b.gain_db, q: b.q, channel: 0 }
}

/// A built-in curve, switched on. Its pre-amp of 0 means automatic; "Flat" has no bands and gets the
/// ten graphic ones back.
pub fn apply_preset(s: SoundSettings, p: &NamedPreset) -> SoundSettings {
    let bands: Vec<SoundBand> = p.bands.iter().map(band_of).collect();
    SoundSettings {
        eq_enabled: true,
        eq_preamp_db: (p.preamp_db != 0.0).then_some(p.preamp_db),
        eq_bands: if bands.is_empty() { graphic() } else { bands },
        ..s
    }
}

/// An AutoEQ "ParametricEQ.txt" / Equalizer APO preset, switched on with its own pre-amp. A file with no
/// filters in it is refused ([`SoundError::NoFilters`]).
pub fn import(s: SoundSettings, text: &str) -> Result<SoundSettings, SoundError> {
    let preset = parse_eq_preset(text.to_string());
    if preset.bands.is_empty() {
        return Err(SoundError::NoFilters);
    }
    Ok(SoundSettings { eq_enabled: true, eq_preamp_db: Some(preset.preamp_db), eq_bands: preset.bands.iter().map(band_of).collect(), ..s })
}

/// A new band: a neutral peak in the middle of the range.
pub fn add_band(s: SoundSettings) -> SoundSettings {
    let mut bands = s.eq_bands.clone();
    bands.push(SoundBand { kind: EqKind::Peaking as i32, freq: 1000.0, gain_db: 0.0, q: 1.0, channel: 0 });
    with_bands(s, bands)
}

/// Removing the last band gives the ten graphic ones back rather than an empty equalizer.
pub fn remove_band(s: SoundSettings, index: u32) -> SoundSettings {
    let bands: Vec<SoundBand> = s.eq_bands.iter().enumerate().filter(|(i, _)| *i != index as usize).map(|(_, b)| *b).collect();
    with_bands(s, if bands.is_empty() { graphic() } else { bands })
}

/// How far each equalizer control goes. The screen's sliders span exactly this, and every edit made
/// through the core is held inside it, so a value from anywhere else cannot leave the range either.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Span {
    pub min: f32,
    pub max: f32,
}

impl Span {
    fn hold(self, v: f32) -> f32 {
        if v.is_nan() { self.min.max(0.0).min(self.max) } else { v.clamp(self.min, self.max) }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct EqRanges {
    /// A band's boost or cut, dB.
    pub gain: Span,
    /// The equalizer's own pre-amp when it is not automatic, dB.
    pub preamp: Span,
    /// -1 hard left, +1 hard right.
    pub balance: Span,
    /// The limiter's ceiling, dB.
    pub limiter: Span,
    /// Crossfeed, dB; 0 is off.
    pub crossfeed: Span,
    /// A band's width (or a shelf's slope).
    pub q: Span,
    /// A band's frequency, Hz: the frequency slider's 20 Hz to 20 kHz.
    pub freq: Span,
    /// ReplayGain's overall level, dB.
    pub replay_gain_preamp: Span,
}

pub const EQ_RANGES: EqRanges = EqRanges {
    gain: Span { min: -12.0, max: 12.0 },
    preamp: Span { min: -20.0, max: 6.0 },
    balance: Span { min: -1.0, max: 1.0 },
    limiter: Span { min: -12.0, max: 0.0 },
    crossfeed: Span { min: 0.0, max: 9.0 },
    q: Span { min: 0.2, max: 8.0 },
    freq: Span { min: 20.0, max: 20_000.0 },
    replay_gain_preamp: Span { min: REPLAY_GAIN_PREAMP.0, max: REPLAY_GAIN_PREAMP.1 },
};

/// One kind of band (`EqKind`, by its ordinal) as the editor needs it: whether it has a gain to set, and
/// whether its width is a slope. The client names each kind.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct BandKindInfo {
    pub uses_gain: bool,
    /// A shelf given by its slope rather than a Q.
    pub slope: bool,
}

/// What the equalizer editor needs of the core besides the bands: each band kind's facts, in `EqKind`'s
/// order, and the sliders' ranges; asked once. Every word on the screen is the client's.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct EqModel {
    pub band_kinds: Vec<BandKindInfo>,
    pub eq_ranges: EqRanges,
}

pub fn eq_model() -> EqModel {
    EqModel {
        band_kinds: (0..BAND_KINDS)
            .map(|k| BandKindInfo {
                uses_gain: nori_player::dsp::uses_gain(k),
                slope: k == EqKind::LowShelfSlope as i32 || k == EqKind::HighShelfSlope as i32,
            })
            .collect(),
        eq_ranges: EQ_RANGES,
    }
}

/// A band as the edit leaves it: a kind and a channel that exist, and gain, width and frequency held
/// inside [`EQ_RANGES`].
fn held(b: SoundBand) -> SoundBand {
    let r = EQ_RANGES;
    SoundBand {
        kind: if (0..BAND_KINDS).contains(&b.kind) { b.kind } else { EqKind::Peaking as i32 },
        freq: r.freq.hold(b.freq),
        gain_db: r.gain.hold(b.gain_db),
        q: r.q.hold(b.q),
        channel: if (0..BAND_CHANNELS).contains(&b.channel) { b.channel } else { 0 },
    }
}

/// One band changed. An index past the end changes nothing.
pub fn set_band(s: SoundSettings, index: u32, band: SoundBand) -> SoundSettings {
    let mut bands = s.eq_bands.clone();
    match bands.get_mut(index as usize) {
        Some(b) => *b = held(band),
        None => return s,
    }
    with_bands(s, bands)
}

impl SoundSettings {
    /// The pre-amp in effect: the one set, or the automatic one for these bands; none with the
    /// equalizer off.
    pub fn effective_preamp_db(&self) -> f32 {
        effective_preamp_db(self.eq_enabled, self.eq_preamp_db, self.eq_bands.iter().map(|b| (b.kind, b.gain_db)))
    }
}

/// [`SoundSettings::effective_preamp_db`] from its parts: whether the equalizer is on, the pre-amp set
/// (none for automatic) and each band's kind and gain.
pub fn effective_preamp_db(eq_enabled: bool, eq_preamp_db: Option<f32>, bands: impl IntoIterator<Item = (i32, f32)>) -> f32 {
    if !eq_enabled {
        return 0.0;
    }
    eq_preamp_db.unwrap_or_else(|| nori_player::dsp::auto_preamp_db(bands))
}

/// The automatic pre-amp switched on, or off - and then it starts from the level it was at, so the
/// sound does not jump when the switch is flipped.
pub fn set_auto_preamp(s: SoundSettings, automatic: bool) -> SoundSettings {
    let eq_preamp_db = if automatic { None } else { Some(EQ_RANGES.preamp.hold(s.effective_preamp_db())) };
    SoundSettings { eq_preamp_db, ..s }
}

/// One of the equalizer screen's other controls, or the settings' "Overall level" slider: a level a
/// slider drags, edited in place on every step (`settings_store::edit_level`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum EqLevel {
    Preamp,
    Balance,
    Limiter,
    Crossfeed,
    /// The level ReplayGain plays at (`preamp_db`), not the equalizer's pre-amp.
    ReplayGainPreamp,
}

/// A balance near the middle is the middle: within 4 % of it the slider snaps to 0.
pub fn balance_snap(v: f32) -> f32 {
    if v.abs() < 0.04 { 0.0 } else { v }
}

/// Crossfeed under a decibel is none at all.
pub fn crossfeed_snap(db: f32) -> f32 {
    if db < 1.0 { 0.0 } else { db }
}

/// A level moved on the equalizer screen, held in its range; balance near the middle and crossfeed
/// under a decibel snap to none ([`balance_snap`], [`crossfeed_snap`]).
pub fn set_level(s: SoundSettings, level: EqLevel, value: f32) -> SoundSettings {
    let r = EQ_RANGES;
    match level {
        EqLevel::Preamp => SoundSettings { eq_preamp_db: Some(r.preamp.hold(value)), ..s },
        EqLevel::Balance => SoundSettings { balance: balance_snap(r.balance.hold(value)), ..s },
        EqLevel::Limiter => SoundSettings { limiter_threshold_db: r.limiter.hold(value), ..s },
        EqLevel::Crossfeed => SoundSettings { crossfeed_db: crossfeed_snap(r.crossfeed.hold(value)), ..s },
        EqLevel::ReplayGainPreamp => SoundSettings { preamp_db: r.replay_gain_preamp.hold(value), ..s },
    }
}

/// Why nothing on the equalizer screen reaches the sound; the client says it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
#[repr(u8)]
pub enum EqBypass {
    /// Bit-perfect USB output is active.
    BitPerfect,
    /// High quality output is on (and can be turned off in the settings).
    HiRes,
}

/// Why nothing on the equalizer screen reaches the sound, or `None` when it does. Bit-perfect output
/// and high quality output both hand the file's samples to the DAC untouched, so the whole chain is
/// out of the path; without this the screen looks broken.
pub fn eq_bypass(hi_res: bool, bit_perfect: bool) -> Option<EqBypass> {
    if bit_perfect {
        Some(EqBypass::BitPerfect)
    } else if hi_res {
        Some(EqBypass::HiRes)
    } else {
        None
    }
}

/// What a band's label marks after its frequency (the client draws it: "1k L", "63 ↙").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum BandMark {
    None,
    /// One channel only.
    Left,
    Right,
    LowShelf,
    HighShelf,
    /// A band with no gain (a notch, a pass).
    NoGain,
}

/// A band's mark: its channel, a shelf, or no gain, in that order of precedence.
pub fn band_mark(kind: i32, channel: i32) -> BandMark {
    let low = kind == EqKind::LowShelf as i32 || kind == EqKind::LowShelfSlope as i32;
    let high = kind == EqKind::HighShelf as i32 || kind == EqKind::HighShelfSlope as i32;
    match channel {
        1 => BandMark::Left,
        2 => BandMark::Right,
        _ if low => BandMark::LowShelf,
        _ if high => BandMark::HighShelf,
        _ if !nori_player::dsp::uses_gain(kind) => BandMark::NoGain,
        _ => BandMark::None,
    }
}

// ---- server profiles ----

/// The saved servers and which one is in use.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct ServerList {
    pub servers: Vec<SavedServer>,
    pub active_server_id: String,
}

/// `profile` made the server in use: it replaces the saved one with its id, or joins the end.
pub fn servers_activate(list: ServerList, profile: SavedServer) -> ServerList {
    let id = profile.id.clone();
    let mut servers: Vec<SavedServer> = list.servers.into_iter().filter(|s| s.id != id).collect();
    servers.push(profile);
    ServerList { servers, active_server_id: id }
}

/// The profile a login keeps: the form as filled in, under the id of a saved profile for the same address
/// and user when there is one, so that logging in to a server again (the add-server form, the test bridge)
/// takes up that profile, its library and downloads with it, instead of saving a copy with none. The saved
/// one keeps what the form does not ask (the music folder, the second address's bitrate cap) and its name
/// when the form's is empty. A profile already saved (edited in place) is left as it is.
pub fn servers_login(list: &ServerList, profile: SavedServer) -> SavedServer {
    if list.servers.iter().any(|s| s.id == profile.id) {
        return profile;
    }
    let address = |url: &str| url.trim().trim_end_matches('/').to_ascii_lowercase();
    let same = |s: &&SavedServer| address(&s.url) == address(&profile.url) && s.user.trim().eq_ignore_ascii_case(profile.user.trim());
    match list.servers.iter().find(same) {
        Some(saved) => SavedServer {
            id: saved.id.clone(),
            name: if profile.name.trim().is_empty() { saved.name.clone() } else { profile.name },
            music_folder_id: saved.music_folder_id.clone(),
            alt_max_bit_rate: saved.alt_max_bit_rate,
            ..profile
        },
        None => profile,
    }
}

/// A saved profile changed in place; which one is in use does not change.
pub fn servers_update(list: ServerList, profile: SavedServer) -> ServerList {
    let servers = list.servers.into_iter().map(|s| if s.id == profile.id { profile.clone() } else { s }).collect();
    ServerList { servers, ..list }
}

/// A profile removed. Removing the one in use puts the first one left in its place, or none.
pub fn servers_remove(list: ServerList, id: &str) -> ServerList {
    let was_active = list.active_server_id == id;
    let servers: Vec<SavedServer> = list.servers.into_iter().filter(|s| s.id != id).collect();
    let active_server_id = if was_active { servers.first().map(|s| s.id.clone()).unwrap_or_default() } else { list.active_server_id };
    ServerList { servers, active_server_id }
}

/// Whose rows in the app's database are open: the active profile's, and "default" before there is one.
pub fn server_db_id(active_server_id: &str) -> String {
    if active_server_id.is_empty() { "default".into() } else { active_server_id.to_string() }
}

/// A fresh profile id: eight hex digits, like the start of a random UUID.
pub fn new_server_id() -> String {
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};
    static N: AtomicU64 = AtomicU64::new(0);
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u64(N.fetch_add(1, Ordering::Relaxed));
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_nanos()));
    format!("{:08x}", h.finish() as u32)
}

/// Extra HTTP headers as typed, one per line, "Name: value". A line without a colon or a name is
/// skipped; name and value are trimmed; a name given twice keeps its last value.
pub fn parse_headers(text: &str) -> HashMap<String, String> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            (!k.trim().is_empty()).then(|| (k.trim().to_string(), v.trim().to_string()))
        })
        .collect()
}

/// The headers back as the field shows them, by name so the order is always the same.
pub fn format_headers(headers: &HashMap<String, String>) -> String {
    let mut h: Vec<(&String, &String)> = headers.iter().collect();
    h.sort();
    h.iter().map(|(k, v)| format!("{k}: {v}")).collect::<Vec<_>>().join("\n")
}

/// An address typed without its scheme: the schemes offered in front of it, https first.
pub fn url_schemes(url: &str) -> Vec<String> {
    if url.is_empty() || url.contains("://") { Vec::new() } else { vec!["https://".into(), "http://".into()] }
}

/// Whether the form can be sent: an address, and a user or an API key.
pub fn profile_ready(p: &SavedServer) -> bool {
    !p.url.trim().is_empty() && (!p.user.trim().is_empty() || !p.api_key.trim().is_empty())
}

/// The profile as the form sends it: addresses trimmed, headers read from what was typed.
pub fn profile_from_form(p: SavedServer, headers: &str) -> SavedServer {
    SavedServer { url: p.url.trim().to_string(), alt_url: p.alt_url.trim().to_string(), headers: parse_headers(headers), ..p }
}

// ---- the doors ----

/// The band kinds' facts and the equalizer's ranges; asked once.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_model_get() -> EqModel {
    eq_model()
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_bypass_reason(hi_res: bool, bit_perfect: bool) -> Option<EqBypass> {
    eq_bypass(hi_res, bit_perfect)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn servers_activated(list: ServerList, profile: SavedServer) -> ServerList {
    servers_activate(list, profile)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn server_for_login(list: ServerList, profile: SavedServer) -> SavedServer {
    servers_login(&list, profile)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn servers_updated(list: ServerList, profile: SavedServer) -> ServerList {
    servers_update(list, profile)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn servers_removed(list: ServerList, id: String) -> ServerList {
    servers_remove(list, &id)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn server_db(active_server_id: String) -> String {
    server_db_id(&active_server_id)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn server_new_id() -> String {
    new_server_id()
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn server_headers_text(headers: HashMap<String, String>) -> String {
    format_headers(&headers)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn server_url_schemes(url: String) -> Vec<String> {
    url_schemes(&url)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn server_ready(profile: SavedServer) -> bool {
    profile_ready(&profile)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn server_from_form(profile: SavedServer, headers: String) -> SavedServer {
    profile_from_form(profile, &headers)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn server_label(name: String, url: String) -> String {
    label(&name, &url)
}

/// A saved profile's sound; `None` when the JSON is not one.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn sound_from_json(json: String) -> Option<SoundSettings> {
    sound_from(&json)
}

/// The ten graphic bands back, with the automatic pre-amp.
pub fn eq_reset_bands(sound: SoundSettings) -> SoundSettings {
    SoundSettings { eq_bands: graphic(), eq_preamp_db: None, ..sound }
}

/// Which of the app's own files are the app's database: `nori.db` with its write-ahead log and shared
/// memory. Indices into `names`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn storage_index_files(names: Vec<String>) -> Vec<u32> {
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| n.strip_prefix(nori_db::DB_FILE).is_some_and(|rest| ["", "-wal", "-shm"].contains(&rest)))
        .map(|(i, _)| i as u32)
        .collect()
}

/// Reads an AutoEQ "ParametricEQ.txt" / Equalizer APO preset:
/// `Preamp: -6.2 dB` and `Filter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70` lines; anything else is ignored.
/// A file with no filters but a `GraphicEQ:` curve (AutoEQ's "GraphicEQ.txt", Wavelet's) is fitted here,
/// once, with ten parametric filters (`nori_player::eqfit`), so it plays as any parametric preset does.
pub fn parse_eq_preset(text: String) -> EqPreset {
    let mut preset = EqPreset::default();
    for line in text.lines() {
        let t: Vec<&str> = line.split_whitespace().collect();
        let after = |key: &str| t.iter().position(|w| w.eq_ignore_ascii_case(key)).and_then(|i| t.get(i + 1)).and_then(|v| v.parse::<f32>().ok());
        if t.first().is_some_and(|w| w.eq_ignore_ascii_case("preamp:")) {
            preset.preamp_db = t.get(1).and_then(|v| v.parse().ok()).unwrap_or(0.0);
        } else if t.first().is_some_and(|w| w.eq_ignore_ascii_case("filter")) {
            let Some(on) = t.iter().position(|w| w.eq_ignore_ascii_case("ON")) else { continue };
            let token = t.get(on + 1).map(|k| k.to_ascii_uppercase()).unwrap_or_default();
            let kind = match token.as_str() {
                "PK" | "PEQ" | "MODAL" => EqKind::Peaking,
                "LS" | "LSC" | "LSQ" => EqKind::LowShelf,
                "HS" | "HSC" | "HSQ" => EqKind::HighShelf,
                "LSC 6DB" | "LS 6DB" | "LS6" => EqKind::LowShelfSlope,
                "HSC 6DB" | "HS 6DB" | "HS6" => EqKind::HighShelfSlope,
                "LP" | "LPQ" => EqKind::LowPass,
                "HP" | "HPQ" => EqKind::HighPass,
                "BP" => EqKind::BandPass,
                "NO" | "NOTCH" => EqKind::Notch,
                "AP" => EqKind::AllPass,
                _ => continue,
            };
            // Only the shelving and peaking kinds carry a gain; a pass filter line has none.
            let Some(freq) = after("Fc") else { continue };
            let gain_db = after("Gain").unwrap_or(0.0);
            if matches!(kind, EqKind::Peaking | EqKind::LowShelf | EqKind::HighShelf | EqKind::LowShelfSlope | EqKind::HighShelfSlope) && after("Gain").is_none() {
                continue;
            }
            preset.bands.push(EqBand { kind, freq, gain_db, q: after("Q").unwrap_or(0.71) });
        }
    }
    if preset.bands.is_empty() {
        if let Some(points) = nori_player::eqfit::parse_graphic(&text) {
            let fit = nori_player::eqfit::fit_graphic(&points);
            return EqPreset { preamp_db: fit.preamp_db, bands: fit.bands };
        }
    }
    preset
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(entries: &[(&str, PrefValue)]) -> HashMap<String, PrefValue> {
        entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect()
    }
    fn t(v: &str) -> PrefValue {
        PrefValue::Text { v: v.to_string() }
    }
    fn n(v: i32) -> PrefValue {
        PrefValue::Number { v }
    }

    #[test]
    fn nothing_stored_is_the_defaults() {
        let p = load(&HashMap::new());
        assert_eq!(p, StoredPrefs::default());
        assert_eq!(p.eq_bands.len(), 10);
        assert_eq!(p.home_rows, (0..8).collect::<Vec<_>>());
        assert_eq!(p.swipe_left, 3, "the left swipe favourites");
    }

    #[test]
    fn what_is_saved_loads_back_the_same() {
        let mut p = StoredPrefs::default();
        p.servers = vec![SavedServer {
            id: "a1".into(),
            name: "Home".into(),
            url: "https://music.example.com".into(),
            headers: [("X-Auth".to_string(), "t".to_string())].into(),
            alt_max_bit_rate: 320,
            legacy_auth: true,
            ..SavedServer::default()
        }];
        p.active_server_id = "a1".into();
        p.eq_preamp_db = Some(-3.5);
        p.eq_bands = vec![SoundBand { kind: 2, freq: 1234.5, gain_db: -2.25, q: 0.7, channel: 1 }];
        p.home_rows = vec![5, 0];
        p.pinned_playlists = vec!["p1".into(), "p2".into()];
        p.list_prefs = [("albums".to_string(), "grid".to_string())].into();
        p.accent = 0xFF112233;
        p.theme = 2;
        assert_eq!(load(&save(&p)), p);
        assert!(!save(&StoredPrefs { eq_preamp_db: None, ..p }).contains_key("eqPreampDb"));
    }

    #[test]
    fn values_out_of_range_are_brought_back() {
        let p = load(&raw(&[("parallelDownloads", n(40)), ("coversAhead", n(-1)), ("replayGain", n(9)), ("theme", n(7)), ("tapAction", n(-2)), ("swipeLeft", n(5))]));
        assert_eq!(p.parallel_downloads, 10);
        assert_eq!(p.covers_ahead, 0);
        assert_eq!(p.replay_gain, 3);
        assert_eq!(p.theme, 0);
        assert_eq!(p.tap_action, 0);
        assert_eq!(p.swipe_left, 3);
        assert_eq!(load(&raw(&[("parallelDownloads", n(0))])).parallel_downloads, 1);
        // A value of the wrong type is as good as missing.
        assert_eq!(load(&raw(&[("cacheMb", t("big"))])).cache_mb, 1024);
    }

    #[test]
    fn the_server_list_reads_whole_or_not_at_all() {
        let p = load(&raw(&[("servers", t(r#"[{"id":"a","legacyAuth":true,"altMaxBitRate":128}]"#)), ("activeServerId", t("a"))]));
        assert_eq!((p.servers.len(), p.active_server_id.as_str()), (1, "a"));
        assert!(p.servers[0].legacy_auth);
        assert_eq!(p.servers[0].alt_max_bit_rate, 128);
        // A list that does not read, or a server without an id, loses the whole list.
        assert!(load(&raw(&[("servers", t(r#"[{"id":"a"},{"name":"x"}]"#))])).servers.is_empty());
        assert!(load(&raw(&[("servers", t("nope"))])).servers.is_empty());
    }

    #[test]
    fn home_rows_pins_and_list_prefs() {
        let p = load(&raw(&[("homeRows", t("RANDOM,NOPE,PINNED")), ("pinnedPlaylists", t("a\n\nb")), ("listPrefs", t(r#"{"x":"1","y":2}"#))]));
        assert_eq!(p.home_rows, [6, 0]);
        assert_eq!(p.pinned_playlists, ["a", "b"]);
        assert_eq!(p.list_prefs.get("y").map(String::as_str), Some("2"));
        assert!(load(&raw(&[("homeRows", t(""))])).home_rows.is_empty(), "every row hidden");
        assert!(load(&raw(&[("listPrefs", t("{"))])).list_prefs.is_empty());
    }

    #[test]
    fn bands_keep_their_wire_format() {
        let b = [SoundBand { kind: 0, freq: 1000.0, gain_db: -2.5, q: 1.41, channel: 0 }, SoundBand { kind: 9, freq: 31.0, gain_db: 0.0, q: 0.00001, channel: 2 }];
        assert_eq!(encode_bands(&b), "0:1000.0:-2.5:1.41:0;9:31.0:0.0:1.0E-5:2");
        assert_eq!(decode_bands(&encode_bands(&b)).unwrap(), b);
        // Four fields is a band on both channels; anything that does not read is dropped.
        assert_eq!(decode_bands("1:100:3:0.7").unwrap(), [SoundBand { kind: 1, freq: 100.0, gain_db: 3.0, q: 0.7, channel: 0 }]);
        assert_eq!(decode_bands("1:100:3:0.7;12:1:1:1;x:1:1:1;1:1:1;2:1:1:1:3").unwrap().len(), 1);
        assert_eq!(decode_bands(""), None);
        assert_eq!(decode_bands("1:1:1"), None);
        assert_eq!(kotlin_float(12_345_678.0), "1.2345678E7");
        assert_eq!(kotlin_float(-0.5), "-0.5");
    }

    #[test]
    fn a_sound_round_trips_through_its_json() {
        let s = SoundSettings {
            eq_enabled: true,
            eq_bands: vec![SoundBand { kind: 1, freq: 105.0, gain_db: -3.5, q: 0.7, channel: 0 }],
            eq_preamp_db: Some(-6.2),
            crossfeed_db: 3.0,
            balance: -0.25,
            mono: true,
            limiter: true,
            limiter_threshold_db: -2.0,
            replay_gain: 2,
            preamp_db: 1.5,
            crossfade_sec: 4,
            hi_res: true,
            bit_perfect: false,
        };
        assert_eq!(sound_from(&sound_json(&s)).unwrap(), s);
        assert_eq!(sound_from(&sound_json(&SoundSettings { eq_preamp_db: None, ..s.clone() })).unwrap().eq_preamp_db, None);
    }

    #[test]
    fn a_sound_takes_defaults_for_what_is_missing() {
        let s = sound_from("{}").unwrap();
        assert_eq!(s.eq_bands, graphic());
        assert_eq!(s.limiter_threshold_db, -1.0);
        assert_eq!(s.eq_preamp_db, None);
        assert_eq!(sound_from(r#"{"replayGain":7}"#).unwrap().replay_gain, 3);
        assert_eq!(sound_from(r#"{"replayGain":-1}"#).unwrap().replay_gain, 0);
        assert_eq!(sound_from(r#"{"eqPreampDb":"x"}"#), None, "a pre-amp that is not a number");
        assert_eq!(sound_from(r#"{"eqPreampDb":null}"#), None);
        assert_eq!(sound_from("not json"), None);
        assert_eq!(sound_from("[]"), None);
    }

    #[test]
    fn server_labels() {
        assert_eq!(label("Home", "https://x"), "Home");
        assert_eq!(label(" ", "https://music.example.com:4533/navidrome"), "music.example.com:4533");
        assert_eq!(label("", "music.example.com/x"), "music.example.com");
        assert_eq!(label("", ""), "");
    }

    #[test]
    fn the_test_bridge_sets_within_the_same_ranges() {
        let p = StoredPrefs::default();
        assert!(set_by_name(&p, "limiter", "TRUE").unwrap().prefs.limiter);
        assert_eq!(set_by_name(&p, "eqPreampDb", "12").unwrap().prefs.eq_preamp_db, Some(6.0), "held to the equalizer's range");
        assert_eq!(set_by_name(&StoredPrefs { eq_preamp_db: Some(2.0), ..p.clone() }, "eqPreampDb", "auto").unwrap().prefs.eq_preamp_db, None);
        assert!(set_by_name(&p, "mono", "1").unwrap().prefs.mono);
        assert!(!set_by_name(&StoredPrefs { mono: true, ..p.clone() }, "mono", "yes").unwrap().prefs.mono);
        assert_eq!(set_by_name(&p, "parallelDownloads", "99").unwrap().prefs.parallel_downloads, 10);
        assert_eq!(set_by_name(&p, "coversAhead", "x").unwrap().prefs.covers_ahead, 3, "unreadable keeps the value");
        let cache = set_by_name(&p, "cacheMb", "10").unwrap();
        assert_eq!(cache.prefs.cache_mb, 256);
        assert!(cache.apply_cache_limit);
        assert!(!set_by_name(&p, "speed", "9").unwrap().apply_cache_limit);
        assert_eq!(set_by_name(&p, "speed", "9").unwrap().prefs.speed, 4.0);
        assert_eq!(set_by_name(&p, "fadeMs", "-5").unwrap().prefs.fade_ms, 0);
        assert_eq!(set_by_name(&p, "autoFillKind", "albums").unwrap().prefs.auto_fill_kind, 1);
        assert_eq!(set_by_name(&p, "autoFillBasis", "era").unwrap().prefs.auto_fill_basis, 3);
        assert_eq!(set_by_name(&p, "autoFillBasis", "mood"), None);
        assert_eq!(set_by_name(&p, "nope", "1"), None);
    }

    #[test]
    fn every_row_of_the_settings_screen_sets_by_name() {
        let p = StoredPrefs::default();
        let set = |name: &str, v: &str| set_by_name(&p, name, v).unwrap().prefs;
        assert_eq!(set("replayGain", "ALBUM").replay_gain, 2);
        assert_eq!(set("replayGain", "3").replay_gain, 3);
        assert_eq!(set_by_name(&p, "replayGain", "9"), None, "an ordinal out of range is not a value");
        assert_eq!(set("theme", "dark").theme, 2);
        assert_eq!(set("tapAction", "PLAY_NEXT").tap_action, 3);
        assert_eq!(set("swipeLeft", "DOWNLOAD").swipe_left, 4);
        assert_eq!(set("wifi", "320:mp3").wifi, SavedQuality { bit_rate: 320, format: "mp3".into() });
        assert_eq!(set("mobile", "0:").mobile, SavedQuality::default());
        assert_eq!(set_by_name(&p, "download", "flac"), None);
        assert_eq!(set("preampDb", "20").preamp_db, 6.0);
        assert_eq!(set("accent", "4280191205").accent, 0xFF1E88E5);
        assert_eq!(set("speed", "0.75").speed, 0.75);
        assert_eq!(set("lyricsSize", "7").lyrics_size, 2);
        assert_eq!(set("autoMixMaxTempoPct", "2.0").auto_mix_max_tempo_pct, 2.0);
    }

    #[test]
    fn a_new_install_finds_lyrics_and_keeps_the_autoeq_list_but_no_moving_covers() {
        let fresh = load(&HashMap::new());
        assert!(fresh.third_party_lookups && fresh.lyrics_online && fresh.auto_eq_download);
        assert!(!fresh.motion_artwork, "moving covers are heavier and stay off");
        let asked: Vec<&str> = crate::lyrics_sources::lyrics_lookup(&fresh).services.iter().map(|s| s.name()).collect();
        let keyless: Vec<String> = crate::lyrics_sources::default_order().into_iter().filter(|n| crate::lyrics_sources::LyricsService::named(n).is_some_and(|s| s.needs().is_none())).collect();
        assert_eq!(asked, keyless, "every service on; the ones that need a key wait for it");
        assert_eq!(fresh, StoredPrefs::default());
        // An install that stored the lookups off keeps them off: no migration.
        let kept = load(&save(&StoredPrefs { third_party_lookups: false, auto_eq_download: false, ..StoredPrefs::default() }));
        assert!(!kept.third_party_lookups && !kept.auto_eq_download);
        let p = StoredPrefs { third_party_lookups: false, auto_eq_download: false, ..StoredPrefs::default() };
        let on = set_by_name(&p, "autoEqDownload", "true").unwrap().prefs;
        assert!(on.auto_eq_download && on.third_party_lookups, "the AutoEQ list switches lookups on");
        assert!(!set_by_name(&on, "autoEqDownload", "false").unwrap().prefs.auto_eq_download);
    }

    #[test]
    fn the_lyrics_lookup_and_the_lookups_switch_go_together() {
        let p = StoredPrefs::default();
        let on = set_by_name(&p, "lyricsOnline", "true").unwrap().prefs;
        assert!(on.lyrics_online && on.third_party_lookups, "lyrics online switches lookups on");
        let off = set_by_name(&on, "lyricsLrclib", "false").unwrap().prefs;
        assert!(!off.lyrics_online && off.third_party_lookups, "and off leaves the lookups alone");
        let all_off = set_by_name(&on, "thirdPartyLookups", "false").unwrap().prefs;
        assert!(!all_off.lyrics_online && !all_off.third_party_lookups);
        let all_on = set_by_name(&all_off, "thirdPartyLookups", "true").unwrap().prefs;
        assert!(all_on.lyrics_online && all_on.third_party_lookups);
    }

    #[test]
    fn lyrics_services_are_switched_ranked_and_kept() {
        let p = StoredPrefs::default();
        let all = p.lyrics_on.len();
        let off = set_by_name(&p, "lyricsService:portato", "false").unwrap().prefs;
        assert!(!off.lyrics_on.iter().any(|n| n == "PORTATO") && off.lyrics_on.len() == all - 1);
        let on = set_by_name(&off, "lyricsService:portato", "true").unwrap().prefs;
        assert!(on.lyrics_on.iter().any(|n| n == "PORTATO") && on.lyrics_on.len() == all);
        assert_eq!(on.lyrics_order, p.lyrics_order, "a switch never moves a service");
        let off = set_by_name(&on, "lyricsService:paxsenix", "false").unwrap().prefs;
        assert_eq!((off.lyrics_order.clone(), off.lyrics_on.len()), (p.lyrics_order.clone(), all - 1));
        assert!(set_by_name(&p, "lyricsService:nobody", "true").is_none(), "no such service");
        let at = |o: &[String], n: &str| o.iter().position(|x| x == n).unwrap();
        let moved = set_by_name(&on, "lyricsMove", "LRCLIB:-1").unwrap().prefs;
        assert_eq!(at(&moved.lyrics_order, "LRCLIB"), at(&on.lyrics_order, "LRCLIB") - 1, "one place, whoever is above it");
        let placed = set_by_name(&on, "lyricsPlace", "LRCLIB:0").unwrap().prefs;
        assert_eq!(crate::lyrics_sources::switched_on(&placed)[0].name(), "LRCLIB", "dropped first, asked first");
        assert_eq!(placed.lyrics_on, on.lyrics_on);
        assert!(set_by_name(&on, "lyricsPlace", "LRCLIB:x").is_none());
        let only = set_by_name(&p, "lyricsSources", "lrclib, kugou").unwrap().prefs;
        assert_eq!(only.lyrics_on, ["LRCLIB", "KUGOU"]);
        assert_eq!(only.lyrics_order[..2], ["LRCLIB", "KUGOU"]);
        let back = set_by_name(&only, "lyricsSources", "default").unwrap().prefs;
        assert_eq!((back.lyrics_on, back.lyrics_order), (p.lyrics_on.clone(), p.lyrics_order.clone()));
        let keyed = set_by_name(&placed, "paxSenixKey", "  k  ").unwrap().prefs;
        let back = load(&save(&keyed));
        assert_eq!((back.lyrics_on, back.lyrics_order, back.paxsenix_key), (keyed.lyrics_on.clone(), keyed.lyrics_order.clone(), "k".to_string()));
        assert_eq!(load(&HashMap::new()).lyrics_on, crate::lyrics_sources::default_order(), "nothing stored: the defaults, every service");
        assert_eq!(load(&HashMap::new()).lyrics_order, crate::lyrics_sources::default_order());
    }

    #[test]
    fn the_active_servers_own_settings() {
        let a = SavedServer { id: "a".into(), ..SavedServer::default() };
        let b = SavedServer { id: "b".into(), ..SavedServer::default() };
        let p = StoredPrefs { servers: vec![a, b], active_server_id: "b".into(), ..StoredPrefs::default() };
        let c = set_by_name(&p, "musicFolder", "7").unwrap();
        assert!(c.server);
        assert_eq!((c.prefs.servers[0].music_folder_id.as_str(), c.prefs.servers[1].music_folder_id.as_str()), ("", "7"));
        assert_eq!(set_by_name(&p, "altMaxBitRate", "128").unwrap().prefs.servers[1].alt_max_bit_rate, 128);
        assert_eq!(set_by_name(&StoredPrefs::default(), "musicFolder", "7"), None, "no server in use");
        assert!(!set_by_name(&p, "mono", "1").unwrap().server);
    }

    #[test]
    fn the_band_kinds_facts() {
        let kinds: Vec<(bool, bool)> = eq_model().band_kinds.iter().map(|k| (k.uses_gain, k.slope)).collect();
        assert_eq!(
            kinds,
            [(true, false), (true, false), (true, false), (false, false), (false, false), (false, false), (false, false), (false, false), (true, true), (true, true)]
        );
    }

    #[test]
    fn a_band_edit_stays_in_range() {
        let s = sound();
        let b = set_band(s.clone(), 3, SoundBand { kind: 42, freq: 5.0, gain_db: 30.0, q: 0.0, channel: 7 });
        assert_eq!(b.eq_bands[3], SoundBand { kind: 0, freq: 20.0, gain_db: 12.0, q: 0.2, channel: 0 });
        let ok = SoundBand { kind: 1, freq: 120.0, gain_db: -3.5, q: 0.7, channel: 2 };
        assert_eq!(set_band(s.clone(), 0, ok).eq_bands[0], ok);
        assert_eq!(set_band(s.clone(), 99, ok), s, "no such band");
    }

    #[test]
    fn switching_the_automatic_pre_amp_off_keeps_the_level() {
        let mut s = sound();
        s.eq_enabled = true;
        s.eq_bands[2].gain_db = 4.5;
        assert_eq!(s.effective_preamp_db(), -4.5);
        let manual = set_auto_preamp(s.clone(), false);
        assert_eq!(manual.eq_preamp_db, Some(-4.5));
        assert_eq!(set_auto_preamp(manual, true).eq_preamp_db, None);
        assert_eq!(SoundSettings { eq_enabled: false, ..s.clone() }.effective_preamp_db(), 0.0);
        assert_eq!(set_auto_preamp(SoundSettings { eq_enabled: false, ..s }, false).eq_preamp_db, Some(0.0));
    }

    #[test]
    fn levels_snap_and_hold() {
        let s = sound();
        assert_eq!(set_level(s.clone(), EqLevel::Balance, 0.03).balance, 0.0);
        assert_eq!(set_level(s.clone(), EqLevel::Balance, -3.0).balance, -1.0);
        assert_eq!(set_level(s.clone(), EqLevel::Crossfeed, 0.5).crossfeed_db, 0.0);
        assert_eq!(set_level(s.clone(), EqLevel::Crossfeed, 12.0).crossfeed_db, 9.0);
        assert_eq!(set_level(s.clone(), EqLevel::Limiter, 2.0).limiter_threshold_db, 0.0);
        assert_eq!(set_level(s, EqLevel::Preamp, -30.0).eq_preamp_db, Some(-20.0));
    }

    #[test]
    fn why_the_equalizer_does_nothing() {
        assert_eq!(eq_bypass(false, false), None);
        assert_eq!(eq_bypass(true, true), Some(EqBypass::BitPerfect));
        assert_eq!(eq_bypass(true, false), Some(EqBypass::HiRes));
    }

    #[test]
    fn band_marks_and_snaps() {
        assert_eq!(band_mark(0, 0), BandMark::None);
        assert_eq!(band_mark(1, 1), BandMark::Left);
        assert_eq!(band_mark(8, 0), BandMark::LowShelf);
        assert_eq!(band_mark(2, 0), BandMark::HighShelf);
        assert_eq!(band_mark(9, 2), BandMark::Right);
        assert_eq!(band_mark(6, 0), BandMark::NoGain);
        assert_eq!((balance_snap(0.03), crossfeed_snap(0.9), crossfeed_snap(2.0)), (0.0, 0.0, 2.0));
    }

    #[test]
    fn server_list_edits() {
        let s = |id: &str, name: &str| SavedServer { id: id.into(), name: name.into(), ..SavedServer::default() };
        let list = ServerList { servers: vec![s("a", "A"), s("b", "B")], active_server_id: "b".into() };
        let l = servers_activate(list.clone(), s("a", "A2"));
        assert_eq!(l.servers.iter().map(|x| x.name.as_str()).collect::<Vec<_>>(), ["B", "A2"], "replaced, and moved to the end");
        assert_eq!(l.active_server_id, "a");
        let l = servers_update(list.clone(), s("a", "A3"));
        assert_eq!((l.servers[0].name.as_str(), l.active_server_id.as_str()), ("A3", "b"));
        assert_eq!(servers_update(list.clone(), s("z", "Z")).servers.len(), 2, "an unknown profile is not added");
        let l = servers_remove(list.clone(), "b");
        assert_eq!((l.servers.len(), l.active_server_id.as_str()), (1, "a"), "the first one left takes over");
        assert_eq!(servers_remove(list.clone(), "a").active_server_id, "b");
        assert_eq!(servers_remove(ServerList { servers: vec![s("a", "")], active_server_id: "a".into() }, "a").active_server_id, "");
        assert_eq!((server_db_id(""), server_db_id("x1")), ("default".into(), "x1".into()));
        let id = new_server_id();
        assert_eq!(id.len(), 8);
        assert!(id.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(new_server_id(), new_server_id());
    }

    #[test]
    fn a_login_to_a_server_already_saved_takes_up_its_profile() {
        let saved = SavedServer { id: "a1".into(), name: "Home".into(), url: "http://10.0.2.2:4534/".into(), user: "admin".into(), password: "old".into(), music_folder_id: "3".into(), ..SavedServer::default() };
        let other = SavedServer { id: "b2".into(), url: "http://10.0.2.2:4534".into(), user: "guest".into(), ..SavedServer::default() };
        let list = ServerList { servers: vec![saved.clone(), other], active_server_id: "b2".into() };
        let form = SavedServer { id: new_server_id(), url: " HTTP://10.0.2.2:4534 ".into(), user: "Admin".into(), password: "new".into(), ..SavedServer::default() };
        let kept = servers_login(&list, form.clone());
        assert_eq!((kept.id.as_str(), kept.password.as_str(), kept.name.as_str(), kept.music_folder_id.as_str()), ("a1", "new", "Home", "3"), "{kept:?}");
        let l = servers_activate(list.clone(), kept);
        assert_eq!((l.servers.len(), l.active_server_id.as_str()), (2, "a1"), "activated, not copied");
        // Another user of the same server, or another server, is a profile of its own.
        let stranger = SavedServer { user: "someone".into(), ..form.clone() };
        assert_eq!(servers_login(&list, stranger.clone()), stranger);
        let elsewhere = SavedServer { url: "https://music.example".into(), ..form.clone() };
        assert_eq!(servers_login(&list, elsewhere.clone()), elsewhere);
        // A saved profile edited keeps its own id, whatever the others are.
        let edited = SavedServer { id: "b2".into(), ..form };
        assert_eq!(servers_login(&list, edited.clone()), edited);
    }

    #[test]
    fn the_login_form() {
        let h = parse_headers("X-Auth: a:b\n: nope\nno colon\n  CF-Id :  x  \nX-Auth: c");
        assert_eq!(h.len(), 2);
        assert_eq!(h["X-Auth"], "c", "the last one wins");
        assert_eq!(h["CF-Id"], "x");
        assert_eq!(format_headers(&parse_headers("b: 2\na: 1")), "a: 1\nb: 2");
        assert_eq!(url_schemes("music.local"), ["https://", "http://"]);
        assert!(url_schemes("").is_empty() && url_schemes("http://x").is_empty());
        let p = SavedServer { url: " https://x ".into(), alt_url: " y ".into(), ..SavedServer::default() };
        assert!(!profile_ready(&p), "a user or a key");
        assert!(profile_ready(&SavedServer { user: "u".into(), ..p.clone() }));
        assert!(profile_ready(&SavedServer { api_key: "k".into(), ..p.clone() }));
        assert!(!profile_ready(&SavedServer { url: "  ".into(), user: "u".into(), ..p.clone() }));
        let f = profile_from_form(p, "A: 1");
        assert_eq!((f.url.as_str(), f.alt_url.as_str(), f.headers["A"].as_str()), ("https://x", "y", "1"));
    }

    fn sound() -> SoundSettings {
        sound_from("{}").unwrap()
    }

    #[test]
    fn equalizer_edits() {
        let flat = NamedPreset { kind: nori_model::PresetKind::Flat, preamp_db: 0.0, bands: vec![] };
        let s = apply_preset(SoundSettings { eq_preamp_db: Some(-4.0), ..sound() }, &flat);
        assert!(s.eq_enabled);
        assert_eq!(s.eq_preamp_db, None, "a pre-amp of 0 is automatic");
        assert_eq!(s.eq_bands, graphic());
        let bass = NamedPreset { kind: nori_model::PresetKind::BassBoost, preamp_db: -6.0, bands: vec![nori_model::EqBand { kind: EqKind::LowShelf, freq: 100.0, gain_db: 6.0, q: 0.7 }] };
        let s = apply_preset(sound(), &bass);
        assert_eq!(s.eq_preamp_db, Some(-6.0));
        assert_eq!(s.eq_bands, [SoundBand { kind: 1, freq: 100.0, gain_db: 6.0, q: 0.7, channel: 0 }]);

        let added = add_band(sound());
        assert_eq!(added.eq_bands.len(), 11);
        assert_eq!(added.eq_bands[10], SoundBand { kind: 0, freq: 1000.0, gain_db: 0.0, q: 1.0, channel: 0 });
        assert_eq!(remove_band(added.clone(), 10).eq_bands, graphic());
        assert_eq!(remove_band(added, 99).eq_bands.len(), 11);
        let one = SoundSettings { eq_bands: vec![SoundBand { kind: 2, freq: 5.0, gain_db: 1.0, q: 1.0, channel: 0 }], ..sound() };
        assert_eq!(remove_band(one, 0).eq_bands, graphic(), "never an empty equalizer");
    }

    #[test]
    fn importing_a_preset() {
        let s = import(sound(), "Preamp: -6.2 dB\nFilter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70\n").unwrap();
        assert!(s.eq_enabled);
        assert_eq!(s.eq_preamp_db, Some(-6.2));
        assert_eq!(s.eq_bands.len(), 1);
        assert!(matches!(import(sound(), "Preamp: 0 dB\n"), Err(SoundError::NoFilters)));
        let zero = import(sound(), "Filter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70\n").unwrap();
        assert_eq!(zero.eq_preamp_db, Some(0.0), "an imported pre-amp is kept even at 0");
    }

    #[test]
    fn index_files_are_the_databases() {
        let names = ["nori.db", "nori.db-wal", "nori.db-shm", "nori.db-journal", "certs", "other.db", "nori.db.bak"].map(String::from).to_vec();
        assert_eq!(storage_index_files(names), [0, 1, 2]);
    }
}
