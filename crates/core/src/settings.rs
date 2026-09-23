//! The settings as they are stored. On Android that is SharedPreferences rather than a database: the
//! playback service needs its settings synchronously when it starts, before any database is open, and
//! this is one small file read once. So everything here is a free function with no database: the
//! platform hands over what it has stored, as it is, and gets one checked record back (defaults,
//! ranges and old layouts dealt with); saving is one call that says what to write. The wire formats
//! (the band list, a sound profile's JSON, a server profile's JSON) live here too, so another player
//! reading the same store gets the same settings.

use std::collections::HashMap;

use serde_json::{Map, Value};

use crate::{EqKind, NamedPreset};

/// One stored value, as the platform's key-value store holds it.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum PrefValue {
    Flag { v: bool },
    Number { v: i32 },
    Big { v: i64 },
    Decimal { v: f32 },
    Text { v: String },
    Texts { v: Vec<String> },
}

/// What to write back: every value, and the keys that go.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PrefsWrite {
    pub put: HashMap<String, PrefValue>,
    pub remove: Vec<String>,
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
    pub speed: f32,
    pub skip_silence: bool,
    pub scrobble_percent: i32,
    pub live_search_delay_ms: i32,
    pub taste_model: bool,
    pub third_party_lookups: bool,
    pub profile_per_output: bool,
    pub auto_eq_auto: bool,
    pub weighted_shuffle: bool,
    pub lyrics_sweep: bool,
    pub soft_sleeve: bool,
    pub favourite_notice: bool,
    pub lyrics_keep_screen_on: bool,
    pub lyrics_translation: bool,
    pub lyrics_size: i32,
    pub lyrics_lrclib: bool,
    pub theme: i32,
    pub amoled: bool,
    pub player_colours: bool,
    pub dynamic_color: bool,
    pub accent: i64,
    pub cover_colors: bool,
    pub reduce_motion: bool,
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
}

/// A sound that cannot be made: a preset with no filters in it, or the profiles not reachable.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(feature = "ffi", derive(uniffi::Error))]
#[cfg_attr(feature = "ffi", uniffi(flat_error))]
pub enum SoundError {
    #[error("{0}")]
    NoFilters(String),
    #[error("database: {0}")]
    Db(String),
}

impl From<rusqlite::Error> for SoundError {
    fn from(e: rusqlite::Error) -> Self {
        SoundError::Db(e.to_string())
    }
}

impl From<crate::CoreError> for SoundError {
    fn from(e: crate::CoreError) -> Self {
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
const AUTO_FILL_KINDS: [&str; 2] = ["SONGS", "ALBUMS"];
const AUTO_FILL_BASES: [&str; 4] = ["SIMILAR", "ARTIST", "GENRE", "ERA"];
const REPLAY_GAIN_MODES: [&str; 4] = ["OFF", "TRACK", "ALBUM", "AUTO"];
const THEME_MODES: [&str; 3] = ["SYSTEM", "LIGHT", "DARK"];
const TAP_ACTION_NAMES: [&str; 4] = ["PLAY_LIST", "PLAY_ONE", "QUEUE", "PLAY_NEXT"];
const SWIPE_ACTION_NAMES: [&str; 5] = ["NONE", "QUEUE", "PLAY_NEXT", "FAVOURITE", "DOWNLOAD"];
const THEMES: i32 = THEME_MODES.len() as i32;
const TAP_ACTIONS: i32 = TAP_ACTION_NAMES.len() as i32;
const SWIPE_ACTIONS: i32 = SWIPE_ACTION_NAMES.len() as i32;
/// `HomeRow`, by the names they are stored under, in their order.
const HOME_ROWS: [&str; 8] = ["PINNED", "PLAYLISTS", "RECENT", "NEWEST", "FREQUENT", "TOP_SONGS", "RANDOM", "STARRED"];

/// What each is called on screen, in the same order.
pub(crate) const AUTO_FILL_KIND_LABELS: [&str; 2] = ["Songs", "Albums"];
pub(crate) const AUTO_FILL_BASIS_LABELS: [&str; 4] = ["Similar music", "The same artist", "The same genre", "The same era"];
const HOME_ROW_TITLES: [&str; 8] = [
    "Favourite playlists",
    "Playlists",
    "Recently played",
    "Recently added",
    "Most played albums",
    "Most played songs",
    "Random",
    "Favourite albums",
];
/// `EqKind`, in its order, as the band editor names each kind.
const BAND_KIND_LABELS: [&str; BAND_KINDS as usize] =
    ["Peak", "Low shelf", "High shelf", "Low pass", "High pass", "Band pass", "Notch", "All pass", "Low shelf (slope)", "High shelf (slope)"];
const BAND_CHANNEL_LABELS: [&str; BAND_CHANNELS as usize] = ["Both", "Left", "Right"];

/// Where the left swipe is stored. It used to default to Play next and be saved along with everything
/// else, so a new key is what gives existing installs the new default (the left swipe favourites).
const SWIPE_LEFT: &str = "swipeLeft3";

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
            speed: 1.0,
            skip_silence: false,
            scrobble_percent: 50,
            live_search_delay_ms: 350,
            taste_model: true,
            third_party_lookups: false,
            profile_per_output: true,
            auto_eq_auto: false,
            weighted_shuffle: true,
            lyrics_sweep: true,
            soft_sleeve: true,
            favourite_notice: true,
            lyrics_keep_screen_on: true,
            lyrics_translation: true,
            lyrics_size: 1,
            lyrics_lrclib: true,
            theme: 0,
            amoled: false,
            player_colours: true,
            dynamic_color: true,
            accent: 0xFF6750A4,
            cover_colors: true,
            reduce_motion: false,
            ignore_system_motion: false,
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

// ---- JSON read the forgiving way the stored JSON was always read ----

fn opt_bool(o: &Map<String, Value>, k: &str) -> bool {
    match o.get(k) {
        Some(Value::Bool(b)) => *b,
        Some(Value::String(s)) => s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn num(v: &Value) -> Option<f64> {
    match v {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.trim().parse().ok(),
        _ => None,
    }
}

fn opt_f64(o: &Map<String, Value>, k: &str, fallback: f64) -> f64 {
    o.get(k).and_then(num).unwrap_or(fallback)
}

fn opt_i32(o: &Map<String, Value>, k: &str) -> i32 {
    match o.get(k) {
        Some(Value::Number(n)) => n.as_i64().map(|i| i as i32).or_else(|| n.as_f64().map(|f| f as i32)).unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse::<f64>().map(|f| f as i32).unwrap_or(0),
        _ => 0,
    }
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
        Some(v) => Some(num(v)? as f32),
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

    /// Before profiles existed there was one server in three keys; it becomes the profile "default".
    fn servers(&self) -> Vec<SavedServer> {
        if let Some(json) = self.text("servers") {
            let list: Option<Vec<SavedServer>> =
                serde_json::from_str::<Value>(json).ok().and_then(|v| v.as_array().map(|a| a.iter().map(server_from).collect())).flatten();
            return list.unwrap_or_default();
        }
        let url = self.text("serverUrl").unwrap_or_default();
        if url.is_empty() {
            return Vec::new();
        }
        vec![SavedServer {
            id: "default".to_string(),
            url: url.to_string(),
            user: self.text("user").unwrap_or_default().to_string(),
            password: self.text("password").unwrap_or_default().to_string(),
            ..SavedServer::default()
        }]
    }
}

/// Everything stored, as it is, into the settings: defaults for what is missing or of the wrong type,
/// ranges enforced, older layouts carried over.
pub fn load(raw: &HashMap<String, PrefValue>) -> StoredPrefs {
    let r = Raw(raw);
    let d = StoredPrefs::default();
    let legacy = r.text("serverUrl").is_some_and(|u| !u.is_empty());
    StoredPrefs {
        servers: r.servers(),
        active_server_id: r.text("activeServerId").map(str::to_string).unwrap_or_else(|| if legacy { "default".to_string() } else { String::new() }),
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
        speed: r.float("speed", 1.0),
        skip_silence: r.flag("skipSilence", false),
        scrobble_percent: r.int("scrobblePercent", 50),
        live_search_delay_ms: r.int("liveSearchDelayMs", d.live_search_delay_ms),
        profile_per_output: r.flag("profilePerOutput", true),
        auto_eq_auto: r.flag("autoEqAuto", false),
        taste_model: r.flag("tasteModel", true),
        third_party_lookups: r.flag("thirdPartyLookups", false),
        weighted_shuffle: r.flag("weightedShuffle", true),
        lyrics_sweep: r.flag("lyricsSweep", true),
        soft_sleeve: r.flag("softSleeve", true),
        favourite_notice: r.flag("favouriteNotice", true),
        lyrics_keep_screen_on: r.flag("lyricsKeepScreenOn", true),
        lyrics_translation: r.flag("lyricsTranslation", true),
        lyrics_size: r.int("lyricsSize", 1),
        lyrics_lrclib: r.flag("lyricsLrclib", true),
        theme: r.ordinal("theme", THEMES, 0),
        amoled: r.flag("amoled", false),
        dynamic_color: r.flag("dynamicColor", true),
        accent: r.long("accent", d.accent),
        cover_colors: r.flag("coverColors", true),
        reduce_motion: r.flag("reduceMotion", false),
        ignore_system_motion: r.flag("ignoreSystemMotion", false),
        ui_scale: r.float("uiScale", 0.0),
        player_colours: r.flag("playerColours", true),
        tap_action: r.ordinal("tapAction", TAP_ACTIONS, d.tap_action),
        swipe_right: r.ordinal("swipeRight", SWIPE_ACTIONS, d.swipe_right),
        swipe_left: r.ordinal(SWIPE_LEFT, SWIPE_ACTIONS, d.swipe_left),
        skip_explicit: r.flag("skipExplicit", false),
        home_rows: r.text("homeRows").map_or(d.home_rows, |s| {
            s.split(',').filter_map(|n| HOME_ROWS.iter().position(|r| *r == n)).map(|i| i as i32).collect()
        }),
        pinned_playlists: r.text("pinnedPlaylists").map_or_else(Vec::new, |s| s.split('\n').filter(|p| !p.is_empty()).map(str::to_string).collect()),
        list_prefs: r.text("listPrefs").and_then(|j| serde_json::from_str::<Value>(j).ok()).and_then(|v| string_map(&v)).unwrap_or_default(),
    }
}

/// Everything to write for these settings. The single-server keys from before profiles go.
pub fn save(p: &StoredPrefs) -> PrefsWrite {
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
    flag("skipSilence", p.skip_silence);
    flag("profilePerOutput", p.profile_per_output);
    flag("autoEqAuto", p.auto_eq_auto);
    flag("tasteModel", p.taste_model);
    flag("thirdPartyLookups", p.third_party_lookups);
    flag("weightedShuffle", p.weighted_shuffle);
    flag("lyricsSweep", p.lyrics_sweep);
    flag("softSleeve", p.soft_sleeve);
    flag("favouriteNotice", p.favourite_notice);
    flag("lyricsKeepScreenOn", p.lyrics_keep_screen_on);
    flag("lyricsTranslation", p.lyrics_translation);
    flag("lyricsLrclib", p.lyrics_lrclib);
    flag("amoled", p.amoled);
    flag("dynamicColor", p.dynamic_color);
    flag("coverColors", p.cover_colors);
    flag("reduceMotion", p.reduce_motion);
    flag("ignoreSystemMotion", p.ignore_system_motion);
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
    int(SWIPE_LEFT, p.swipe_left);
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
    put.insert("accent".to_string(), PrefValue::Big { v: p.accent });
    let mut remove: Vec<String> = ["serverUrl", "user", "password"].into_iter().map(str::to_string).collect();
    if p.eq_preamp_db.is_none() {
        remove.push("eqPreampDb".to_string());
    }
    PrefsWrite { put, remove }
}

/// What a change by name did: the settings after it, whether the stream cache has to shrink to a new
/// limit now, and whether the active server's own profile changed (its connection is set up again).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingChange {
    pub prefs: StoredPrefs,
    pub apply_cache_limit: bool,
    pub server: bool,
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
        // The lookups switch covers LRCLIB too, so it takes the lyrics half with it both ways.
        "thirdPartyLookups" => (n.third_party_lookups, n.lyrics_lrclib) = (on, on),
        // Lyrics from LRCLIB need lookups, so switching them on switches lookups on; off leaves the
        // lookups (update checks) as they are.
        "lyricsLrclib" => (n.lyrics_lrclib, n.third_party_lookups) = (on, on || p.third_party_lookups),
        "crossfadeKeepAlbums" => n.crossfade_keep_albums = on,
        "lyricsSweep" => n.lyrics_sweep = on,
        "lyricsTranslation" => n.lyrics_translation = on,
        "lyricsKeepScreenOn" => n.lyrics_keep_screen_on = on,
        "lyricsSize" => n.lyrics_size = clamp(int, LYRICS_SIZE, p.lyrics_size),
        "softSleeve" => n.soft_sleeve = on,
        "favouriteNotice" => n.favourite_notice = on,
        "crossfadeSec" => n.crossfade_sec = int.unwrap_or(p.crossfade_sec),
        "autoMixMaxS" => n.auto_mix_max_s = int.unwrap_or(p.auto_mix_max_s),
        "autoMixBeatMatch" => n.auto_mix_beat_match = on,
        "autoMixMaxTempoPct" => n.auto_mix_max_tempo_pct = float.unwrap_or(p.auto_mix_max_tempo_pct),
        "autoMixKeepPitch" => n.auto_mix_keep_pitch = on,
        "autoMixBassSwap" => n.auto_mix_bass_swap = on,
        "autoMixFilters" => n.auto_mix_filters = on,
        "autoMixEchoOut" => n.auto_mix_echo_out = on,
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
        "limiterThresholdDb" => n.limiter_threshold_db = float.unwrap_or(p.limiter_threshold_db),
        "replayGain" => n.replay_gain = named(&REPLAY_GAIN_MODES)?,
        "preampDb" => n.preamp_db = float.map_or(p.preamp_db, |v| v.clamp(REPLAY_GAIN_PREAMP.0, REPLAY_GAIN_PREAMP.1)),
        "untaggedGainDb" => n.untagged_gain_db = float.unwrap_or(p.untagged_gain_db),
        "autoFill" => n.auto_fill = on,
        "bridgeOffline" => n.bridge_offline = on,
        "autoFillKind" => n.auto_fill_kind = named(&AUTO_FILL_KINDS)?,
        "autoFillBasis" => n.auto_fill_basis = named(&AUTO_FILL_BASES)?,
        "autoEqAuto" => n.auto_eq_auto = on,
        "profilePerOutput" => n.profile_per_output = on,
        "weightedShuffle" => n.weighted_shuffle = on,
        "skipExplicit" => n.skip_explicit = on,
        "skipOnError" => n.skip_on_error = on,
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
    Some(SettingChange { prefs: n, apply_cache_limit: name == "cacheMb", server })
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

fn band_of(b: &crate::EqBand) -> SoundBand {
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
/// filters in it is refused with `empty`, what to tell the user.
pub fn import(s: SoundSettings, text: &str, empty: &str) -> Result<SoundSettings, SoundError> {
    let preset = crate::parse_eq_preset(text.to_string());
    if preset.bands.is_empty() {
        return Err(SoundError::NoFilters(empty.to_string()));
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

/// One kind of band as the editor shows it: its name, whether it has a gain to set, and which marks
/// its label gets.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct BandKindInfo {
    pub label: String,
    pub uses_gain: bool,
    /// A shelf given by its slope rather than a Q.
    pub slope: bool,
}

/// The words every enum setting is shown with, in ordinal order; asked once.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingLabels {
    pub auto_fill_kinds: Vec<String>,
    pub auto_fill_bases: Vec<String>,
    pub home_rows: Vec<String>,
    pub band_kinds: Vec<BandKindInfo>,
    pub band_channels: Vec<String>,
    pub eq_ranges: EqRanges,
}

fn strings(v: &[&str]) -> Vec<String> {
    v.iter().map(|s| s.to_string()).collect()
}

pub fn labels() -> SettingLabels {
    SettingLabels {
        auto_fill_kinds: strings(&AUTO_FILL_KIND_LABELS),
        auto_fill_bases: strings(&AUTO_FILL_BASIS_LABELS),
        home_rows: strings(&HOME_ROW_TITLES),
        band_kinds: BAND_KIND_LABELS
            .iter()
            .enumerate()
            .map(|(k, l)| BandKindInfo {
                label: l.to_string(),
                uses_gain: nori_player::dsp::uses_gain(k as i32),
                slope: k as i32 == EqKind::LowShelfSlope as i32 || k as i32 == EqKind::HighShelfSlope as i32,
            })
            .collect(),
        band_channels: strings(&BAND_CHANNEL_LABELS),
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

/// One of the equalizer screen's other controls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum EqLevel {
    Preamp,
    Balance,
    Limiter,
    Crossfeed,
}

/// A level moved on the equalizer screen, held in its range; balance near the middle and crossfeed
/// under a decibel snap to none (see `fmt::eq_balance_snap`, `fmt::eq_crossfeed_snap`).
pub fn set_level(s: SoundSettings, level: EqLevel, value: f32) -> SoundSettings {
    let r = EQ_RANGES;
    match level {
        EqLevel::Preamp => SoundSettings { eq_preamp_db: Some(r.preamp.hold(value)), ..s },
        EqLevel::Balance => SoundSettings { balance: crate::fmt::eq_balance_snap(r.balance.hold(value)), ..s },
        EqLevel::Limiter => SoundSettings { limiter_threshold_db: r.limiter.hold(value), ..s },
        EqLevel::Crossfeed => SoundSettings { crossfeed_db: crate::fmt::eq_crossfeed_snap(r.crossfeed.hold(value)), ..s },
    }
}

/// Why nothing on the equalizer screen reaches the sound, or `None` when it does. Bit-perfect output
/// and high quality output both hand the file's samples to the DAC untouched, so the whole chain is
/// out of the path; without this the screen looks broken.
pub fn eq_bypass(hi_res: bool, bit_perfect: bool) -> Option<String> {
    if bit_perfect {
        Some("Bit-perfect USB output is active, so nothing here touches the audio.".into())
    } else if hi_res {
        Some("High quality output is on, so nothing here changes the sound. Turn it off in Settings, under Sound.".into())
    } else {
        None
    }
}

/// A band's label for the band list: its frequency and a mark for its channel or kind.
pub fn band_label(b: &SoundBand) -> String {
    let k = b.kind;
    let low = k == EqKind::LowShelf as i32 || k == EqKind::LowShelfSlope as i32;
    let high = k == EqKind::HighShelf as i32 || k == EqKind::HighShelfSlope as i32;
    crate::fmt::eq_band_label(b.freq, b.channel == 1, b.channel == 2, low, high, nori_player::dsp::uses_gain(k))
}

/// Under a saved profile: which devices use it, by name, or that a tap loads it.
pub fn profile_use(devices: &[String]) -> String {
    if devices.is_empty() { "Tap to load".into() } else { format!("Used for {}", devices.join(", ")) }
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

/// Whose rows in the app's database are open: the active profile's, and "default" before there is
/// one (the id the single server from before profiles was given).
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

/// Every enum setting's words, the band kinds and the equalizer's ranges; asked once.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn setting_labels() -> SettingLabels {
    labels()
}

/// The settings as a fresh install has them.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn settings_defaults() -> StoredPrefs {
    StoredPrefs::default()
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_set_band(sound: SoundSettings, index: u32, band: SoundBand) -> SoundSettings {
    set_band(sound, index, band)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_set_auto_preamp(sound: SoundSettings, automatic: bool) -> SoundSettings {
    set_auto_preamp(sound, automatic)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_set_level(sound: SoundSettings, level: EqLevel, value: f32) -> SoundSettings {
    set_level(sound, level, value)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_effective_preamp_db(sound: SoundSettings) -> f32 {
    sound.effective_preamp_db()
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_bypass_reason(hi_res: bool, bit_perfect: bool) -> Option<String> {
    eq_bypass(hi_res, bit_perfect)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_band_name(band: SoundBand) -> String {
    band_label(&band)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn words_profile_use(devices: Vec<String>) -> String {
    profile_use(&devices)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn servers_activated(list: ServerList, profile: SavedServer) -> ServerList {
    servers_activate(list, profile)
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

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_graphic() -> Vec<SoundBand> {
    graphic()
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_apply_preset(sound: SoundSettings, preset: NamedPreset) -> SoundSettings {
    apply_preset(sound, &preset)
}

/// A preset file the user picked or downloaded; an error, with what to say, when it has no filters.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_import(sound: SoundSettings, text: String) -> Result<SoundSettings, SoundError> {
    import(sound, &text, "that file had no filters in it")
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_add_band(sound: SoundSettings) -> SoundSettings {
    add_band(sound)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_remove_band(sound: SoundSettings, index: u32) -> SoundSettings {
    remove_band(sound, index)
}

/// The ten graphic bands back, with the automatic pre-amp.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_reset_bands(sound: SoundSettings) -> SoundSettings {
    SoundSettings { eq_bands: graphic(), eq_preamp_db: None, ..sound }
}

/// Which of the app's own files are the app's database: `nori.db` (and an old `nori-<id>.db`) with its write-ahead
/// log and shared memory. Indices into `names`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn storage_index_files(names: Vec<String>) -> Vec<u32> {
    names
        .iter()
        .enumerate()
        .filter(|(_, n)| n.starts_with("nori") && (n.ends_with(".db") || n.ends_with("-wal") || n.ends_with("-shm")))
        .map(|(i, _)| i as u32)
        .collect()
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
        let w = save(&p);
        assert!(w.remove.contains(&"serverUrl".to_string()) && !w.remove.contains(&"eqPreampDb".to_string()));
        assert_eq!(load(&w.put), p);
        let w = save(&StoredPrefs { eq_preamp_db: None, ..p });
        assert!(w.remove.contains(&"eqPreampDb".to_string()) && !w.put.contains_key("eqPreampDb"));
    }

    #[test]
    fn values_out_of_range_are_brought_back() {
        let p = load(&raw(&[("parallelDownloads", n(40)), ("coversAhead", n(-1)), ("replayGain", n(9)), ("theme", n(7)), ("tapAction", n(-2)), (SWIPE_LEFT, n(5))]));
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
    fn the_old_single_server_becomes_the_default_profile() {
        let p = load(&raw(&[("serverUrl", t("http://10.0.2.2:4533")), ("user", t("admin")), ("password", t("pw"))]));
        assert_eq!(p.active_server_id, "default");
        assert_eq!(p.servers.len(), 1);
        assert_eq!((p.servers[0].id.as_str(), p.servers[0].user.as_str(), p.servers[0].password.as_str()), ("default", "admin", "pw"));
        // Once there is a list, the old keys are ignored.
        let p = load(&raw(&[("serverUrl", t("http://old")), ("servers", t("[]")), ("activeServerId", t(""))]));
        assert!(p.servers.is_empty());
        assert_eq!(p.active_server_id, "");
        // A list that does not read, or a server without an id, loses the whole list.
        assert!(load(&raw(&[("servers", t(r#"[{"id":"a"},{"name":"x"}]"#))])).servers.is_empty());
        assert!(load(&raw(&[("servers", t("nope"))])).servers.is_empty());
        let old = load(&raw(&[("servers", t(r#"[{"id":"a","legacyAuth":"true","altMaxBitRate":"128"}]"#))]));
        assert!(old.servers[0].legacy_auth);
        assert_eq!(old.servers[0].alt_max_bit_rate, 128);
    }

    #[test]
    fn the_left_swipe_moved_to_a_new_key() {
        assert_eq!(load(&raw(&[("swipeLeft", n(2))])).swipe_left, 3);
        assert_eq!(load(&raw(&[(SWIPE_LEFT, n(2))])).swipe_left, 2);
        assert!(save(&StoredPrefs::default()).put.contains_key(SWIPE_LEFT));
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
    fn a_sound_is_read_forgivingly() {
        let s = sound_from("{}").unwrap();
        assert_eq!(s.eq_bands, graphic());
        assert_eq!(s.limiter_threshold_db, -1.0);
        assert_eq!(s.eq_preamp_db, None);
        assert_eq!(sound_from(r#"{"replayGain":7}"#).unwrap().replay_gain, 3);
        assert_eq!(sound_from(r#"{"replayGain":-1}"#).unwrap().replay_gain, 0);
        assert_eq!(sound_from(r#"{"crossfeedDb":"2.5","eqEnabled":"TRUE"}"#).unwrap().crossfeed_db, 2.5);
        assert!(sound_from(r#"{"eqEnabled":"TRUE"}"#).unwrap().eq_enabled);
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
    fn the_lyrics_lookup_and_the_lookups_switch_go_together() {
        let p = StoredPrefs::default();
        let on = set_by_name(&p, "lyricsLrclib", "true").unwrap().prefs;
        assert!(on.lyrics_lrclib && on.third_party_lookups, "lyrics online switches lookups on");
        let off = set_by_name(&on, "lyricsLrclib", "false").unwrap().prefs;
        assert!(!off.lyrics_lrclib && off.third_party_lookups, "and off leaves the lookups alone");
        let all_off = set_by_name(&on, "thirdPartyLookups", "false").unwrap().prefs;
        assert!(!all_off.lyrics_lrclib && !all_off.third_party_lookups);
        let all_on = set_by_name(&all_off, "thirdPartyLookups", "true").unwrap().prefs;
        assert!(all_on.lyrics_lrclib && all_on.third_party_lookups);
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
    fn labels_are_the_screens_words() {
        let l = labels();
        assert_eq!(l.auto_fill_kinds, ["Songs", "Albums"]);
        assert_eq!(l.auto_fill_bases, ["Similar music", "The same artist", "The same genre", "The same era"]);
        assert_eq!(l.home_rows[0], "Favourite playlists");
        assert_eq!(l.home_rows[7], "Favourite albums");
        assert_eq!(l.band_channels, ["Both", "Left", "Right"]);
        let kinds: Vec<(&str, bool, bool)> = l.band_kinds.iter().map(|k| (k.label.as_str(), k.uses_gain, k.slope)).collect();
        assert_eq!(
            kinds,
            [
                ("Peak", true, false),
                ("Low shelf", true, false),
                ("High shelf", true, false),
                ("Low pass", false, false),
                ("High pass", false, false),
                ("Band pass", false, false),
                ("Notch", false, false),
                ("All pass", false, false),
                ("Low shelf (slope)", true, true),
                ("High shelf (slope)", true, true),
            ]
        );
    }

    #[test]
    fn the_equalizer_ranges_are_the_sliders() {
        let r = EQ_RANGES;
        assert_eq!((r.gain.min, r.gain.max), (-12.0, 12.0));
        assert_eq!((r.preamp.min, r.preamp.max), (-20.0, 6.0));
        assert_eq!((r.balance.min, r.balance.max), (-1.0, 1.0));
        assert_eq!((r.limiter.min, r.limiter.max), (-12.0, 0.0));
        assert_eq!((r.crossfeed.min, r.crossfeed.max), (0.0, 9.0));
        assert_eq!((r.q.min, r.q.max), (0.2, 8.0));
        assert_eq!((r.replay_gain_preamp.min, r.replay_gain_preamp.max), (-12.0, 6.0));
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
        assert_eq!(eq_bypass(true, true).unwrap(), "Bit-perfect USB output is active, so nothing here touches the audio.");
        assert_eq!(eq_bypass(true, false).unwrap(), "High quality output is on, so nothing here changes the sound. Turn it off in Settings, under Sound.");
    }

    #[test]
    fn band_labels_and_profile_lines() {
        let b = |kind: i32, channel: i32| band_label(&SoundBand { kind, freq: 1000.0, gain_db: 0.0, q: 1.0, channel });
        assert_eq!(b(0, 0), "1k");
        assert_eq!(b(1, 1), "1k L");
        assert_eq!(b(8, 0), "1k ↙");
        assert_eq!(b(2, 0), "1k ↗");
        assert_eq!(b(9, 2), "1k R");
        assert_eq!(b(6, 0), "1k ∿");
        assert_eq!(profile_use(&[]), "Tap to load");
        assert_eq!(profile_use(&["Qudelix".into(), "Phone speaker".into()]), "Used for Qudelix, Phone speaker");
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
        let flat = NamedPreset { name: "Flat".into(), preamp_db: 0.0, bands: vec![] };
        let s = apply_preset(SoundSettings { eq_preamp_db: Some(-4.0), ..sound() }, &flat);
        assert!(s.eq_enabled);
        assert_eq!(s.eq_preamp_db, None, "a pre-amp of 0 is automatic");
        assert_eq!(s.eq_bands, graphic());
        let bass = NamedPreset { name: "Bass".into(), preamp_db: -6.0, bands: vec![crate::EqBand { kind: EqKind::LowShelf, freq: 100.0, gain_db: 6.0, q: 0.7 }] };
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
        let s = import(sound(), "Preamp: -6.2 dB\nFilter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70\n", "empty").unwrap();
        assert!(s.eq_enabled);
        assert_eq!(s.eq_preamp_db, Some(-6.2));
        assert_eq!(s.eq_bands.len(), 1);
        assert_eq!(import(sound(), "Preamp: 0 dB\n", "empty").unwrap_err().to_string(), "empty");
        let zero = import(sound(), "Filter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70\n", "e").unwrap();
        assert_eq!(zero.eq_preamp_db, Some(0.0), "an imported pre-amp is kept even at 0");
    }

    #[test]
    fn index_files_are_the_databases() {
        let names = ["nori-abc.db", "nori-abc.db-wal", "nori-abc.db-shm", "nori.db-journal", "certs", "other.db", "nori-x.db.bak"].map(String::from).to_vec();
        assert_eq!(storage_index_files(names), [0, 1, 2]);
    }
}
