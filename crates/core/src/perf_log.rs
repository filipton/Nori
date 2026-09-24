//! What the perf build measured, stretch by stretch: one row per stretch of the app's life (screen off
//! and playing, the player open, charging, ...), kept in the app's database for a couple of weeks so
//! a battery test of several days can be read back and shared. The platform reads its own counters
//! (CPU time, context switches, the battery, the frames it drew); which state a stretch is filed under,
//! what two readings make, how the stretches add up and how the page and the shared report say it are
//! here, so a desktop client's recorder files, sums and reports the same way.
//!
//! Each stretch also keeps why it cost what it did: the threads that woke most, with their names and CPU
//! time, the audio output as it stood at the end (the track's format, the buffer asked for and given,
//! the performance mode asked for and applied, offload, the route, underruns), and the bytes the app
//! moved over the network. Read at the stretch's two ends only, as everything else is.
//!
//! Only the perf build calls these. The table is made by the first row, so the database of every
//! other build never has it.

use std::collections::HashMap;

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

use crate::settings_store;

/// How long a stretch is kept after it ended.
pub const KEEP_MS: i64 = 14 * 24 * 3600 * 1000;

/// Stretches shorter than this are the blinks between two states (the screen going off stops the
/// activity too): they are not kept.
pub const SHORTEST_MS: i64 = 3_000;

/// Every state a stretch is filed under, in the order the page lists them, with its name.
const STATES: [(&str, &str); 7] = [
    ("off-playing", "Screen off, playing"),
    ("off-paused", "Screen off, paused"),
    ("on-playing-player", "Screen on, playing, player open"),
    ("on-playing-app", "Screen on, playing, other page"),
    ("on-playing-away", "Screen on, playing, another app"),
    ("on-paused", "Screen on, paused"),
    (CHARGING, "Charging"),
];

/// Charging runs the battery the other way, so it is left out of every battery figure.
const CHARGING: &str = "charging";

/// One stretch as it is kept: the state it was in, how long, and what it cost. `uah` is the charge used
/// by the battery's own counter, none where the phone does not keep one; `pct` is the drop in the battery
/// level, which every phone reports. Kept as JSON under the short names the rows have always had.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfStretch {
    #[serde(rename = "s")]
    pub state: String,
    #[serde(rename = "t0")]
    pub start_wall: i64,
    pub ms: i64,
    #[serde(rename = "cpu")]
    pub cpu_ms: i64,
    #[serde(rename = "wk")]
    pub wakeups: i64,
    #[serde(rename = "al")]
    pub alloc_bytes: i64,
    #[serde(rename = "gc")]
    pub gcs: i64,
    #[serde(rename = "pss")]
    pub pss_kb: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uah: Option<i64>,
    pub pct: i32,
    #[serde(rename = "gma", default, skip_serializing_if = "Option::is_none")]
    pub gauge_ma: Option<f64>,
    #[serde(rename = "tmin")]
    pub temp_min: i32,
    #[serde(rename = "tmax")]
    pub temp_max: i32,
    #[serde(rename = "fr")]
    pub frames: i64,
    #[serde(rename = "jk")]
    pub janky: i64,
    #[serde(rename = "worst")]
    pub worst_ms: f64,
    pub cfg: String,
    /// The threads that woke most over the stretch, the most first ([`TOP_THREADS`] of them); none in
    /// rows from before they were kept.
    #[serde(rename = "th", default, skip_serializing_if = "Vec::is_empty")]
    pub threads: Vec<PerfThreadUse>,
    /// The audio output as the stretch ended; none with no player, or in rows from before.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub out: Option<PerfOutput>,
    /// Bytes the app received and sent over the network during the stretch, where the platform counts them.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rx: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tx: Option<i64>,
}

/// How many of the threads that woke most a stretch keeps.
pub const TOP_THREADS: usize = 6;

/// One thread of the app at one reading: its name as the system has it, and what it has done since it
/// started.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfThread {
    pub tid: i32,
    pub name: String,
    /// CPU time, user and system.
    pub cpu_ms: i64,
    /// Voluntary context switches: each is the thread going to sleep and being woken again.
    pub switches: i64,
}

/// What one thread did over a stretch. `born` is a thread that started during it, whose whole count
/// is the stretch's; one that ended during it is not seen at all.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfThreadUse {
    #[serde(rename = "n")]
    pub name: String,
    #[serde(rename = "cpu")]
    pub cpu_ms: i64,
    #[serde(rename = "wk")]
    pub wakeups: i64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub born: bool,
}

/// The audio output as the platform describes it, read when a stretch ends. What was asked of the track
/// against what the platform made of it: a buffer smaller than asked, or a power-saving mode not
/// applied, is what makes a writer wake more than the design says. -1 for a figure the platform does
/// not give.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfOutput {
    /// The player that opened the track, "rust" or "exoplayer".
    #[serde(rename = "e")]
    pub engine: String,
    pub rate: i32,
    #[serde(rename = "ch")]
    pub channels: i32,
    /// `AudioFormat.ENCODING_*`.
    #[serde(rename = "enc")]
    pub encoding: i32,
    /// The buffer asked for, bytes.
    #[serde(rename = "ask")]
    pub asked_bytes: i64,
    /// The buffer the track uses (`getBufferSizeInFrames`) and the most it could (`getBufferCapacityInFrames`).
    #[serde(rename = "size")]
    pub size_frames: i64,
    #[serde(rename = "cap")]
    pub capacity_frames: i64,
    /// `AudioTrack.PERFORMANCE_MODE_*`: asked for, and what the track got.
    #[serde(rename = "pma")]
    pub mode_asked: i32,
    #[serde(rename = "pm")]
    pub mode: i32,
    /// Played by the audio chip rather than mixed on the CPU (`isOffloadedPlayback`).
    #[serde(rename = "off")]
    pub offloaded: bool,
    /// Where the track is routed: `AudioDeviceInfo.TYPE_*` (0 unknown) and the device's own name.
    #[serde(rename = "dt")]
    pub device_type: i32,
    #[serde(rename = "dn", default, skip_serializing_if = "String::is_empty")]
    pub device_name: String,
    /// `getUnderrunCount`: times the track ran dry since it was made.
    #[serde(rename = "ur")]
    pub underruns: i32,
    /// `getPlayState`: 1 stopped, 2 paused, 3 playing.
    #[serde(rename = "st")]
    pub play_state: i32,
}

/// One reading of every counter the platform keeps.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfCounters {
    /// A clock that counts deep sleep (Android's elapsedRealtime).
    pub elapsed_ms: i64,
    pub wall_ms: i64,
    /// The process's CPU time, user and system.
    pub cpu_ms: i64,
    /// Every thread of the app: its name, CPU time and voluntary context switches.
    pub threads: Vec<PerfThread>,
    pub alloc_bytes: i64,
    pub gcs: i64,
    pub pss_kb: i64,
    /// What is left in the battery, where the phone counts it (µAh).
    pub charge_uah: Option<i64>,
    pub capacity_pct: i32,
    /// The fuel gauge's current, its own average where it keeps one (µA, either sign by maker).
    pub gauge_ua: Option<i64>,
    /// Battery temperature in tenths of a degree.
    pub temp_deci: i32,
    /// Bytes the app has received and sent over the network since boot, where the platform counts them.
    pub rx_bytes: Option<i64>,
    pub tx_bytes: Option<i64>,
}

/// What the frames drawn over a stretch came to, counted by the platform as they were drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfFrames {
    pub frames: i64,
    pub janky: i64,
    pub worst_ns: i64,
}

/// One row of the Performance page: what it is, and its figures on the line under it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfFigures {
    pub title: String,
    pub detail: String,
}

/// The Performance page's figures.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfPage {
    /// Each state's stretches added up ("By state"); empty when nothing is recorded yet.
    pub totals: Vec<PerfFigures>,
    /// Every frame counted; none before the app has been on screen.
    pub frames: Option<PerfFigures>,
    /// The stretch under way.
    pub live: Option<PerfFigures>,
    /// The stretches kept, newest first.
    pub stretches: Vec<PerfFigures>,
}

/// The phone and the build, for the report's head.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfDevice {
    pub manufacturer: String,
    pub model: String,
    pub device: String,
    pub release: String,
    pub sdk: i32,
    pub version: String,
    pub sha: String,
    pub build_type: String,
}

// ---- keeping them ----

fn table(c: &Connection) -> rusqlite::Result<()> {
    c.execute_batch("CREATE TABLE IF NOT EXISTS perf_stretches(ended_ms INTEGER NOT NULL, row TEXT NOT NULL)")
}

fn add(c: &Connection, ended_ms: i64, row: &str) -> rusqlite::Result<()> {
    table(c)?;
    c.execute("INSERT INTO perf_stretches(ended_ms, row) VALUES(?1, ?2)", params![ended_ms, row])?;
    c.execute("DELETE FROM perf_stretches WHERE ended_ms < ?1", [ended_ms - KEEP_MS])?;
    Ok(())
}

fn rows(c: &Connection, since_ms: i64) -> rusqlite::Result<Vec<String>> {
    table(c)?;
    let mut st = c.prepare("SELECT row FROM perf_stretches WHERE ended_ms >= ?1 ORDER BY ended_ms, rowid")?;
    let out = st.query_map([since_ms], |r| r.get(0))?.collect();
    out
}

/// A stretch that ended at `ended_ms` (wall clock). Stretches older than [`KEEP_MS`] go at the same
/// time. Written on the calling thread; nothing before the settings are open.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_log_add(ended_ms: i64, stretch: PerfStretch) {
    let Some(db) = settings_store::app_db() else { return };
    let Ok(row) = serde_json::to_string(&stretch) else { return };
    let written = add(&db.lock(), ended_ms, &row);
    if let Err(e) = written {
        crate::alog::info(&format!("perf log: could not write: {e}"));
    }
}

/// The stretches that ended at or after `since_ms`, oldest first; a row that does not read is passed over.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_log_rows(since_ms: i64) -> Vec<PerfStretch> {
    let Some(db) = settings_store::app_db() else { return Vec::new() };
    let read = rows(&db.lock(), since_ms);
    parsed(read.unwrap_or_default())
}

fn parsed(rows: Vec<String>) -> Vec<PerfStretch> {
    rows.iter().filter_map(|r| serde_json::from_str(r).ok()).collect()
}

/// Every stretch forgotten: "Start fresh".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_log_clear() {
    let Some(db) = settings_store::app_db() else { return };
    let c = db.lock();
    if table(&c).is_ok() {
        let _ = c.execute("DELETE FROM perf_stretches", []);
    }
}

// ---- measuring them ----

/// The state a stretch is filed under: charging first, then the screen, then the music, then where
/// the app is.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_state(charging: bool, screen_on: bool, playing: bool, foreground: bool, player_open: bool) -> String {
    let key = if charging {
        CHARGING
    } else if !screen_on {
        if playing { "off-playing" } else { "off-paused" }
    } else if !playing {
        "on-paused"
    } else if !foreground {
        "on-playing-away"
    } else if player_open {
        "on-playing-player"
    } else {
        "on-playing-app"
    };
    key.into()
}

/// The settings that change what playing costs, in one line, the playback path first: `engine` is the
/// player the running service built ("exoplayer" or "rust"); with no service yet, the one the setting
/// will start. A change of this line ends a stretch as a change of state does.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_config(engine: Option<String>) -> String {
    let p = settings_store::current().unwrap_or_default();
    config(engine, &p)
}

fn config(engine: Option<String>, p: &crate::settings::StoredPrefs) -> String {
    let on = |b: bool| if b { "on" } else { "off" };
    let engine = engine.unwrap_or_else(|| if p.playback_engine == 1 { "rust" } else { "exoplayer" }.into());
    format!(
        "engine {engine}, eq {}, automix {}, crossfade {} s, offload {}, hi-res {}, bit-perfect {}",
        on(p.eq_enabled),
        on(p.auto_mix),
        p.crossfade_sec,
        on(p.offload),
        on(p.hi_res),
        on(p.bit_perfect)
    )
}

/// The difference between two readings, filed under `state` with the settings line `cfg` it began
/// with and the frames drawn meanwhile; `offloaded` is whether the music went to the audio chip when it
/// ended, `output` the audio output as it stood then. None for a blink shorter than [`SHORTEST_MS`],
/// unless it is the `live` one, which is shown however short.
#[cfg_attr(feature = "ffi", uniffi::export)]
#[allow(clippy::too_many_arguments)]
pub fn perf_stretch(
    state: String,
    cfg: String,
    a: PerfCounters,
    b: PerfCounters,
    drawn: PerfFrames,
    offloaded: bool,
    output: Option<PerfOutput>,
    live: bool,
) -> Option<PerfStretch> {
    let ms = b.elapsed_ms - a.elapsed_ms;
    if ms < if live { 0 } else { SHORTEST_MS } {
        return None;
    }
    let before: HashMap<i32, &PerfThread> = a.threads.iter().map(|t| (t.tid, t)).collect();
    // Only threads alive at both ends: one that ended in between would take its whole count with it.
    let wakeups = b.threads.iter().filter_map(|t| before.get(&t.tid).map(|m| t.switches - m.switches)).sum();
    let gauge: Vec<i64> = [a.gauge_ua, b.gauge_ua].into_iter().flatten().collect();
    let gauge_ma = (!gauge.is_empty()).then(|| (gauge.iter().map(|g| g.abs()).sum::<i64>() / gauge.len() as i64) as f64 / 1000.0);
    Some(PerfStretch {
        state,
        start_wall: a.wall_ms,
        ms,
        cpu_ms: b.cpu_ms - a.cpu_ms,
        wakeups,
        alloc_bytes: b.alloc_bytes - a.alloc_bytes,
        gcs: b.gcs - a.gcs,
        pss_kb: b.pss_kb,
        uah: a.charge_uah.zip(b.charge_uah).map(|(x, y)| x - y),
        pct: a.capacity_pct - b.capacity_pct,
        gauge_ma,
        temp_min: a.temp_deci.min(b.temp_deci),
        temp_max: a.temp_deci.max(b.temp_deci),
        frames: drawn.frames,
        janky: drawn.janky,
        worst_ms: drawn.worst_ns as f64 / 1e6,
        cfg: format!("{cfg}, {}", if offloaded { "offloaded" } else { "on the CPU" }),
        threads: busiest(&before, &b.threads),
        out: output,
        rx: a.rx_bytes.zip(b.rx_bytes).map(|(x, y)| y - x),
        tx: a.tx_bytes.zip(b.tx_bytes).map(|(x, y)| y - x),
    })
}

/// The threads that woke most between two readings, the most first, then by CPU time. A thread only in
/// the second reading started in between, and all it did is the stretch's; a thread id taken again by
/// another thread (the name says so) counts as that other thread's start.
fn busiest(before: &HashMap<i32, &PerfThread>, after: &[PerfThread]) -> Vec<PerfThreadUse> {
    let mut used: Vec<PerfThreadUse> = after
        .iter()
        .map(|t| match before.get(&t.tid).filter(|a| a.name == t.name) {
            Some(a) => PerfThreadUse { name: t.name.clone(), cpu_ms: t.cpu_ms - a.cpu_ms, wakeups: t.switches - a.switches, born: false },
            None => PerfThreadUse { name: t.name.clone(), cpu_ms: t.cpu_ms, wakeups: t.switches, born: true },
        })
        .filter(|u| u.wakeups > 0 || u.cpu_ms > 0)
        .collect();
    used.sort_by(|x, y| y.wakeups.cmp(&x.wakeups).then(y.cpu_ms.cmp(&x.cpu_ms)));
    used.truncate(TOP_THREADS);
    used
}

// ---- adding them up ----

/// Every stretch of one state added up. Battery figures leave charging out: it runs the other way.
#[derive(Debug, Default)]
struct Totals {
    state: String,
    count: i64,
    ms: i64,
    cpu_ms: i64,
    wakeups: i64,
    alloc_bytes: i64,
    gcs: i64,
    /// The last stretch's: memory is a level, not something spent.
    pss_kb: i64,
    /// Charge used, over the stretches the counter measured, and how long those were.
    uah: i64,
    uah_ms: i64,
    pct: i64,
    frames: i64,
    janky: i64,
}

impl Totals {
    fn add(&mut self, s: &PerfStretch) {
        self.count += 1;
        self.ms += s.ms;
        self.cpu_ms += s.cpu_ms;
        self.wakeups += s.wakeups;
        self.alloc_bytes += s.alloc_bytes;
        self.gcs += s.gcs;
        self.pss_kb = s.pss_kb;
        self.pct += s.pct as i64;
        self.frames += s.frames;
        self.janky += s.janky;
        if let Some(u) = s.uah {
            self.uah += u;
            self.uah_ms += s.ms;
        }
    }

    fn battery(&self) -> bool {
        self.state != CHARGING
    }

    fn mah(&self) -> f64 {
        self.uah as f64 / 1000.0
    }

    fn mah_per_h(&self) -> Option<f64> {
        (self.uah_ms > 0).then(|| self.mah() / (self.uah_ms as f64 / 3_600_000.0))
    }

    fn pct_per_h(&self) -> f64 {
        if self.ms > 0 { self.pct as f64 / (self.ms as f64 / 3_600_000.0) } else { 0.0 }
    }

    fn jank_pct(&self) -> Option<f64> {
        (self.frames > 0).then(|| self.janky as f64 * 100.0 / self.frames as f64)
    }

    /// One state's figures on one line, as the page and the report show them.
    fn line(&self) -> String {
        let mut out = duration(self.ms);
        out.push_str(&format!(
            ", CPU {} %, {} wakeups/s, {} KB/min allocated, {} GCs, PSS {} MB",
            fixed(cpu_pct(self.cpu_ms, self.ms), 2),
            fixed(per_s(self.wakeups, self.ms), 1),
            fixed(kb_per_min(self.alloc_bytes, self.ms), 0),
            self.gcs,
            self.pss_kb / 1024
        ));
        if self.battery() {
            match self.mah_per_h() {
                Some(h) => out.push_str(&format!(", {} mAh ({} mAh/h)", fixed(self.mah(), 1), fixed(h, 1))),
                None => out.push_str(&format!(", {} % ({} %/h)", self.pct, fixed(self.pct_per_h(), 2))),
            }
        }
        if let Some(j) = self.jank_pct() {
            out.push_str(&format!(", {} frames, {} % janky", self.frames, fixed(j, 1)));
        }
        out
    }
}

/// The stretches added up by state, in [`STATES`]' order, then any state not listed there in the order
/// it came; states never seen left out.
fn totals(stretches: &[PerfStretch]) -> Vec<Totals> {
    let mut by: Vec<Totals> = STATES.iter().map(|(k, _)| Totals { state: k.to_string(), ..Totals::default() }).collect();
    for s in stretches {
        match by.iter_mut().find(|t| t.state == s.state) {
            Some(t) => t.add(s),
            None => {
                let mut t = Totals { state: s.state.clone(), ..Totals::default() };
                t.add(s);
                by.push(t);
            }
        }
    }
    by.retain(|t| t.count > 0);
    by
}

fn state_name(key: &str) -> &str {
    STATES.iter().find(|(k, _)| *k == key).map_or(key, |(_, name)| name)
}

// ---- saying them ----

/// Java's `"%.{places}f"` as `Locale.ROOT` writes it: the report reads the same on every phone.
fn fixed(v: f64, places: i32) -> String {
    let v = if v.is_finite() { v } else { 0.0 };
    nori_text::fixed_in(v, places, false, '.')
}

fn cpu_pct(cpu_ms: i64, ms: i64) -> f64 {
    if ms > 0 { cpu_ms as f64 * 100.0 / ms as f64 } else { 0.0 }
}

fn per_s(n: i64, ms: i64) -> f64 {
    if ms > 0 { n as f64 * 1000.0 / ms as f64 } else { 0.0 }
}

fn kb_per_min(bytes: i64, ms: i64) -> f64 {
    if ms > 0 { bytes as f64 / 1024.0 / (ms as f64 / 60_000.0) } else { 0.0 }
}

/// "1 h 05 min", "3 min 07 s", "42 s".
fn duration(ms: i64) -> String {
    let s = ms / 1000;
    if s >= 3600 {
        format!("{} h {:02} min", s / 3600, s / 60 % 60)
    } else if s >= 60 {
        format!("{} min {:02} s", s / 60, s % 60)
    } else {
        format!("{s} s")
    }
}

/// "09-24 21:05": when a stretch began, by the phone's own clock.
fn when(wall_ms: i64) -> String {
    let local = wall_ms + crate::library::local_offset_s(wall_ms.div_euclid(1000)) * 1000;
    let (_, m, d, secs) = crate::smart::civil_from_ms(local);
    format!("{m:02}-{d:02} {:02}:{:02}", secs / 3600, secs / 60 % 60)
}

/// "12.3 MB", "840 KB": bytes moved over the network.
fn bytes(n: i64) -> String {
    if n.abs() >= 1024 * 1024 {
        format!("{} MB", fixed(n as f64 / 1024.0 / 1024.0, 1))
    } else {
        format!("{} KB", n / 1024)
    }
}

/// The threads that woke most, on one line: "nori-track 2.1/s 40 ms, ...". A thread that started
/// during the stretch is marked so.
fn threads_line(s: &PerfStretch) -> Option<String> {
    if s.threads.is_empty() {
        return None;
    }
    let each: Vec<String> = s
        .threads
        .iter()
        .map(|t| format!("{} {}/s {} ms{}", t.name, fixed(per_s(t.wakeups, s.ms), 1), t.cpu_ms, if t.born { " (new)" } else { "" }))
        .collect();
    Some(format!("threads by wakeups: {}", each.join(", ")))
}

/// `AudioFormat.ENCODING_*` as a person says it, and the bytes one sample of it takes (none when it is
/// not PCM).
fn encoding(e: i32) -> (String, Option<i64>) {
    match e {
        2 => ("16-bit".into(), Some(2)),
        3 => ("8-bit".into(), Some(1)),
        4 => ("float".into(), Some(4)),
        21 => ("24-bit".into(), Some(3)),
        22 => ("32-bit".into(), Some(4)),
        5 => ("AC-3".into(), None),
        6 => ("E-AC-3".into(), None),
        9 => ("MP3".into(), None),
        10 => ("AAC".into(), None),
        20 => ("Opus".into(), None),
        _ => (format!("encoding {e}"), None),
    }
}

/// `AudioTrack.PERFORMANCE_MODE_*` as a person says it.
fn mode(m: i32) -> &'static str {
    match m {
        0 => "none",
        1 => "low latency",
        2 => "power saving",
        _ => "unknown",
    }
}

/// The audio output on one line: what was asked of the track against what the platform made of it.
fn output_line(o: &PerfOutput) -> String {
    let (enc, width) = encoding(o.encoding);
    let channels = match o.channels {
        1 => "mono".to_string(),
        2 => "stereo".to_string(),
        n => format!("{n} channels"),
    };
    let mut out = format!("output: {}, {} Hz {channels} {enc}", o.engine, o.rate);
    let frame_ms = |f: i64| if o.rate > 0 { f * 1000 / o.rate as i64 } else { 0 };
    let asked = width.filter(|_| o.asked_bytes > 0 && o.channels > 0).map(|w| frame_ms(o.asked_bytes / (w * o.channels as i64)));
    if o.size_frames >= 0 {
        out.push_str(&format!(", buffer {} ms", frame_ms(o.size_frames)));
        match asked {
            Some(a) => out.push_str(&format!(" of {a} ms asked")),
            None if o.asked_bytes > 0 => out.push_str(&format!(" ({} KB asked)", o.asked_bytes / 1024)),
            None => {}
        }
        if o.capacity_frames > o.size_frames {
            out.push_str(&format!(", up to {} ms", frame_ms(o.capacity_frames)));
        }
    }
    out.push_str(&format!(", mode {} asked, {} given", mode(o.mode_asked), mode(o.mode)));
    if o.offloaded {
        out.push_str(", offloaded");
    }
    if o.device_type != 0 || !o.device_name.is_empty() {
        out.push_str(&format!(", to {}", nori_player::outputs::key(crate::outputs::kind(o.device_type), &o.device_name)));
    }
    out.push_str(&format!(", {} underruns", o.underruns));
    out.push_str(match o.play_state {
        1 => ", stopped",
        2 => ", paused",
        3 => ", playing",
        _ => "",
    });
    out
}

/// The lines that say why a stretch cost what it did, under its figures: the threads, the output.
fn why_lines(s: &PerfStretch) -> Vec<String> {
    threads_line(s).into_iter().chain(s.out.as_ref().map(output_line)).collect()
}

/// One stretch's figures on one line.
fn stretch_line(s: &PerfStretch) -> String {
    let mut out = format!("{}  {}", when(s.start_wall), duration(s.ms));
    out.push_str(&format!(
        ", CPU {} %, {} wakeups/s, {} KB/min, {} GCs, PSS {} MB",
        fixed(cpu_pct(s.cpu_ms, s.ms), 2),
        fixed(per_s(s.wakeups, s.ms), 1),
        fixed(kb_per_min(s.alloc_bytes, s.ms), 0),
        s.gcs,
        s.pss_kb / 1024
    ));
    if s.state != CHARGING {
        match s.uah {
            Some(u) => {
                let per_h = if s.ms > 0 { u as f64 / 1000.0 / (s.ms as f64 / 3_600_000.0) } else { 0.0 };
                out.push_str(&format!(", {} mAh ({} mAh/h)", fixed(u as f64 / 1000.0, 1), fixed(per_h, 1)));
            }
            None => out.push_str(&format!(", {} %", s.pct)),
        }
    }
    if let (Some(rx), Some(tx)) = (s.rx, s.tx) {
        out.push_str(&format!(", network {} in, {} out", bytes(rx), bytes(tx)));
    }
    if let Some(g) = s.gauge_ma {
        out.push_str(&format!(", gauge {} mA", fixed(g, 0)));
    }
    out.push_str(&format!(", {}-{} °C", fixed(s.temp_min as f64 / 10.0, 1), fixed(s.temp_max as f64 / 10.0, 1)));
    if s.frames > 0 {
        out.push_str(&format!(", {} frames, {} janky, worst {} ms", s.frames, s.janky, fixed(s.worst_ms, 0)));
    }
    out.push_str(&format!(" [{}]", s.cfg));
    out
}

/// The kept stretches and the one under way, oldest first.
fn all(kept: Vec<PerfStretch>, live: Option<PerfStretch>) -> Vec<PerfStretch> {
    let mut all = kept;
    all.extend(live);
    all
}

/// The Performance page for the stretches kept and `live`, the one under way.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_page(live: Option<PerfStretch>) -> PerfPage {
    page(perf_log_rows(0), live)
}

/// A stretch's figures and, on the lines under them, why it cost that.
fn stretch_detail(s: &PerfStretch) -> String {
    std::iter::once(stretch_line(s)).chain(why_lines(s)).collect::<Vec<_>>().join("\n")
}

fn page(kept: Vec<PerfStretch>, live: Option<PerfStretch>) -> PerfPage {
    let now = live.as_ref().map(|s| PerfFigures { title: format!("Now: {}", state_name(&s.state)), detail: stretch_detail(s) });
    let stretches = kept.iter().rev().take(40).map(|s| PerfFigures { title: state_name(&s.state).into(), detail: stretch_detail(s) }).collect();
    let all = all(kept, live);
    let totals = totals(&all)
        .iter()
        .map(|t| {
            let name = state_name(&t.state);
            let title = if t.count > 1 { format!("{name} ({})", t.count) } else { name.to_string() };
            PerfFigures { title, detail: t.line() }
        })
        .collect();
    let drawn: i64 = all.iter().map(|s| s.frames).sum();
    let janky: i64 = all.iter().map(|s| s.janky).sum();
    let frames = (drawn > 0).then(|| PerfFigures {
        title: format!("{drawn} frames, {janky} janky ({} %)", fixed(janky as f64 * 100.0 / drawn as f64, 1)),
        detail: format!("The slowest took {} ms", fixed(all.iter().map(|s| s.worst_ms).fold(f64::MIN, f64::max), 0)),
    });
    PerfPage { totals, frames, live: now, stretches }
}

/// The report the Share button sends: plain text, so it reads the same in a chat, a mail or an issue.
/// `calls` and `covers` are the benchmarks' results, empty when they were not run.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_report(live: Option<PerfStretch>, device: PerfDevice, calls: String, covers: String) -> String {
    report(all(perf_log_rows(0), live), &device, &calls, &covers)
}

fn report(all: Vec<PerfStretch>, d: &PerfDevice, calls: &str, covers: &str) -> String {
    let mut out = String::from("Nori perf report\n");
    out.push_str(&format!("Device: {} {} ({}), Android {} (API {})\n", d.manufacturer, d.model, d.device, d.release, d.sdk));
    out.push_str(&format!("Build: {} ({}, {})\n", d.version, d.sha, d.build_type));
    if let (Some(first), Some(last)) = (all.first(), all.last()) {
        out.push_str(&format!("Recorded: {} to {}, {} stretches\n", when(first.start_wall), when(last.start_wall + last.ms), all.len()));
    }
    let counter = if all.iter().any(|s| s.uah.is_some()) { "yes (mAh)" } else { "no (battery % only)" };
    out.push_str(&format!("Battery counter: {counter}\n\nBy state\n"));
    out.push_str(&columns(&totals(&all)));
    out.push_str("\nStretches, newest first\n");
    for s in all.iter().rev().take(60) {
        out.push_str(&format!("{}: {}\n", state_name(&s.state), stretch_line(s)));
        for line in why_lines(s) {
            out.push_str(&format!("    {line}\n"));
        }
    }
    if !calls.is_empty() {
        out.push_str(&format!("\nCall benchmark: {calls}\n"));
    }
    if !covers.is_empty() {
        out.push_str(&format!("\nCover benchmark: {covers}\n"));
    }
    out
}

/// The states side by side in padded columns, for a monospaced reader; "-" where a figure does not apply.
fn columns(totals: &[Totals]) -> String {
    let head = ["state", "time", "CPU %", "wakeups/s", "KB/min", "GCs", "PSS MB", "mAh", "mAh/h", "%/h", "frames", "janky %"];
    let dash = || "-".to_string();
    let mut rows: Vec<Vec<String>> = vec![head.iter().map(|h| h.to_string()).collect()];
    for t in totals {
        rows.push(vec![
            state_name(&t.state).to_string(),
            duration(t.ms),
            fixed(cpu_pct(t.cpu_ms, t.ms), 2),
            fixed(per_s(t.wakeups, t.ms), 1),
            fixed(kb_per_min(t.alloc_bytes, t.ms), 0),
            t.gcs.to_string(),
            (t.pss_kb / 1024).to_string(),
            if t.battery() && t.uah_ms > 0 { fixed(t.mah(), 1) } else { dash() },
            t.mah_per_h().filter(|_| t.battery()).map_or_else(dash, |h| fixed(h, 1)),
            if t.battery() { fixed(t.pct_per_h(), 2) } else { dash() },
            if t.frames > 0 { t.frames.to_string() } else { dash() },
            t.jank_pct().map_or_else(dash, |j| fixed(j, 1)),
        ]);
    }
    let widths: Vec<usize> = (0..head.len()).map(|i| rows.iter().map(|r| r[i].chars().count()).max().unwrap_or(0)).collect();
    let mut out = String::new();
    for r in &rows {
        let cells: Vec<String> =
            r.iter().enumerate().map(|(i, c)| if i == 0 { format!("{c:<w$}", w = widths[i]) } else { format!("{c:>w$}", w = widths[i]) }).collect();
        out.push_str(cells.join("  ").trim_end());
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stretch(state: &str, ms: i64) -> PerfStretch {
        PerfStretch {
            state: state.into(),
            start_wall: 0,
            ms,
            cpu_ms: ms / 100,
            wakeups: ms / 100,
            alloc_bytes: 60 * 1024 * ms / 60_000,
            gcs: 2,
            pss_kb: 150 * 1024,
            uah: None,
            pct: 3,
            gauge_ma: None,
            temp_min: 301,
            temp_max: 314,
            frames: 0,
            janky: 0,
            worst_ms: 0.0,
            cfg: "engine exoplayer, on the CPU".into(),
            threads: Vec::new(),
            out: None,
            rx: None,
            tx: None,
        }
    }

    fn thread(tid: i32, name: &str, cpu_ms: i64, switches: i64) -> PerfThread {
        PerfThread { tid, name: name.into(), cpu_ms, switches }
    }

    fn counters(elapsed_ms: i64, switches: &[(i32, i64)]) -> PerfCounters {
        PerfCounters {
            elapsed_ms,
            wall_ms: 1_000 + elapsed_ms,
            cpu_ms: elapsed_ms / 50,
            threads: switches.iter().map(|&(tid, n)| thread(tid, &format!("t{tid}"), n / 10, n)).collect(),
            alloc_bytes: elapsed_ms * 10,
            gcs: elapsed_ms / 10_000,
            pss_kb: 100_000 + elapsed_ms / 100,
            charge_uah: Some(4_000_000 - elapsed_ms / 10),
            capacity_pct: 90 - (elapsed_ms / 600_000) as i32,
            gauge_ua: Some(-150_000 - elapsed_ms / 1000),
            temp_deci: 300 + (elapsed_ms / 60_000) as i32,
            rx_bytes: Some(elapsed_ms * 100),
            tx_bytes: Some(elapsed_ms),
        }
    }

    fn output() -> PerfOutput {
        PerfOutput {
            engine: "rust".into(),
            rate: 44_100,
            channels: 2,
            encoding: 2,
            asked_bytes: 507_150 * 4,
            size_frames: 22_050,
            capacity_frames: 22_050,
            mode_asked: 2,
            mode: 0,
            offloaded: false,
            device_type: 2,
            device_name: String::new(),
            underruns: 3,
            play_state: 3,
        }
    }

    #[test]
    fn keeps_rows_in_order_and_forgets_old_ones() {
        let c = Connection::open_in_memory().unwrap();
        add(&c, 1_000, "a").unwrap();
        add(&c, 2_000, "b").unwrap();
        assert_eq!(rows(&c, 0).unwrap(), vec!["a", "b"]);
        assert_eq!(rows(&c, 1_500).unwrap(), vec!["b"]);
        // A row two weeks and a bit later takes the first two with it.
        add(&c, 2_001 + KEEP_MS, "c").unwrap();
        assert_eq!(rows(&c, 0).unwrap(), vec!["c"]);
    }

    #[test]
    fn reads_nothing_from_a_database_without_the_table() {
        let c = Connection::open_in_memory().unwrap();
        assert!(rows(&c, 0).unwrap().is_empty());
    }

    #[test]
    fn a_row_reads_back_as_the_platform_wrote_it_before() {
        // The shape the Kotlin recorder wrote, keys and all: the rows already on phones read as they are.
        let old = r#"{"s":"off-playing","t0":1700000000000,"ms":600000,"cpu":1200,"wk":3000,"al":2048,"gc":1,"pss":150000,"uah":42000,"pct":1,"gma":152.5,"tmin":301,"tmax":305,"fr":0,"jk":0,"worst":41.5,"cfg":"engine exoplayer, on the CPU"}"#;
        let s = &parsed(vec![old.into(), "not a stretch".into()])[..];
        assert_eq!(s.len(), 1, "a row that does not read is passed over");
        assert_eq!((s[0].state.as_str(), s[0].uah, s[0].gauge_ma, s[0].worst_ms), ("off-playing", Some(42_000), Some(152.5), 41.5));
        let back: serde_json::Value = serde_json::from_str(&serde_json::to_string(&s[0]).unwrap()).unwrap();
        assert_eq!(back, serde_json::from_str::<serde_json::Value>(old).unwrap(), "and is written the same way");
        let none = PerfStretch { uah: None, gauge_ma: None, ..s[0].clone() };
        assert!(!serde_json::to_string(&none).unwrap().contains("uah"), "a figure the phone has not is left out");
        let why = PerfStretch {
            threads: vec![PerfThreadUse { name: "nori-track".into(), cpu_ms: 3, wakeups: 90, born: true }],
            out: Some(output()),
            rx: Some(1),
            tx: Some(0),
            ..s[0].clone()
        };
        assert_eq!(parsed(vec![serde_json::to_string(&why).unwrap()]), [why], "the threads, the output and the network are kept too");
    }

    #[test]
    fn a_stretch_is_filed_by_charging_then_the_screen_then_the_music_then_the_app() {
        assert_eq!(perf_state(true, false, true, false, false), "charging");
        assert_eq!(perf_state(false, false, true, true, true), "off-playing");
        assert_eq!(perf_state(false, false, false, true, false), "off-paused");
        assert_eq!(perf_state(false, true, false, true, true), "on-paused");
        assert_eq!(perf_state(false, true, true, false, true), "on-playing-away");
        assert_eq!(perf_state(false, true, true, true, true), "on-playing-player");
        assert_eq!(perf_state(false, true, true, true, false), "on-playing-app");
        assert_eq!(state_name("on-playing-away"), "Screen on, playing, another app");
        assert_eq!(state_name("new"), "new", "a state this build does not know keeps its key");
    }

    #[test]
    fn the_settings_line_names_the_path_that_plays() {
        let p = crate::settings::StoredPrefs {
            eq_enabled: true,
            auto_mix: false,
            crossfade_sec: 6,
            offload: true,
            hi_res: false,
            bit_perfect: false,
            playback_engine: 1,
            ..Default::default()
        };
        assert_eq!(config(None, &p), "engine rust, eq on, automix off, crossfade 6 s, offload on, hi-res off, bit-perfect off");
        assert!(config(Some("exoplayer".into()), &p).starts_with("engine exoplayer, "), "the running service's own path wins");
    }

    #[test]
    fn two_readings_make_a_stretch() {
        let a = counters(10_000, &[(1, 100), (2, 50), (3, 7)]);
        let b = counters(610_000, &[(1, 400), (2, 60), (4, 9)]);
        let drawn = PerfFrames { frames: 120, janky: 3, worst_ns: 41_500_000 };
        let s = perf_stretch("off-playing".into(), "engine rust".into(), a.clone(), b.clone(), drawn, true, Some(output()), false).unwrap();
        assert_eq!((s.ms, s.start_wall, s.cpu_ms), (600_000, 11_000, 12_000));
        assert_eq!(s.wakeups, 310, "only the threads alive at both ends");
        assert_eq!((s.uah, s.pct, s.pss_kb), (Some(60_000), 1, 106_100));
        // The gauge's two readings averaged in whole µA before becoming mA, as the recorder did.
        assert_eq!(s.gauge_ma, Some(((150_010 + 150_610) / 2) as f64 / 1000.0));
        assert_eq!((s.temp_min, s.temp_max, s.frames, s.janky, s.worst_ms), (300, 310, 120, 3, 41.5));
        assert_eq!(s.cfg, "engine rust, offloaded");
        assert_eq!((s.rx, s.tx), (Some(60_000_000), Some(600_000)));
        assert_eq!(s.out, Some(output()));
        let blink = counters(12_000, &[]);
        assert_eq!(perf_stretch("x".into(), String::new(), a.clone(), blink.clone(), drawn, false, None, false), None, "a blink is not kept");
        assert!(perf_stretch("x".into(), String::new(), a, blink, drawn, false, None, true).is_some(), "but the one under way is shown");
    }

    #[test]
    fn a_stretch_keeps_the_threads_that_woke_most() {
        let mut a = counters(0, &[]);
        a.threads = vec![thread(1, "main", 500, 1_000), thread(2, "nori-track", 10, 100), thread(3, "Thread-3", 0, 5), thread(9, "gone", 0, 0)];
        let mut b = counters(60_000, &[]);
        b.threads = vec![
            thread(1, "main", 520, 1_060),
            thread(2, "nori-track", 40, 1_300),
            thread(3, "binder:1_3", 3, 40),
            thread(4, "nori-load", 90, 600),
            thread(5, "idle", 0, 0),
            thread(6, "a", 1, 1),
            thread(7, "b", 1, 1),
            thread(8, "c", 2, 1),
        ];
        let s = perf_stretch("off-playing".into(), String::new(), a, b, PerfFrames { frames: 0, janky: 0, worst_ns: 0 }, false, None, false).unwrap();
        assert_eq!(s.wakeups, 60 + 1_200 + 35, "the total is still the threads alive at both ends");
        let names: Vec<(&str, i64, i64, bool)> = s.threads.iter().map(|t| (t.name.as_str(), t.wakeups, t.cpu_ms, t.born)).collect();
        assert_eq!(
            names,
            [("nori-track", 1_200, 30, false), ("nori-load", 600, 90, true), ("main", 60, 20, false), ("binder:1_3", 40, 3, true), ("c", 1, 2, true), ("a", 1, 1, true)],
            "the most woken first, a new thread with all it did, a thread id taken again as a new thread, none that did nothing"
        );
        assert_eq!(
            threads_line(&s).unwrap(),
            "threads by wakeups: nori-track 20.0/s 30 ms, nori-load 10.0/s 90 ms (new), main 1.0/s 20 ms, binder:1_3 0.7/s 3 ms (new), c 0.0/s 2 ms (new), a 0.0/s 1 ms (new)"
        );
    }

    #[test]
    fn the_output_says_what_was_asked_and_what_was_given() {
        assert_eq!(
            output_line(&output()),
            "output: rust, 44100 Hz stereo 16-bit, buffer 500 ms of 11500 ms asked, mode power saving asked, none given, to Phone speaker, 3 underruns, playing"
        );
        let exo = PerfOutput {
            engine: "exoplayer".into(),
            encoding: 9,
            asked_bytes: 300 * 1024,
            size_frames: 441_000,
            capacity_frames: 882_000,
            mode_asked: 0,
            mode: 0,
            offloaded: true,
            device_type: 8,
            device_name: "Buds".into(),
            underruns: 0,
            play_state: 2,
            ..output()
        };
        assert_eq!(
            output_line(&exo),
            "output: exoplayer, 44100 Hz stereo MP3, buffer 10000 ms (300 KB asked), up to 20000 ms, mode none asked, none given, offloaded, to Bluetooth: Buds, 0 underruns, paused"
        );
    }

    #[test]
    fn stretches_add_up_by_state_in_the_page_order() {
        let mut playing = stretch("off-playing", 3_600_000);
        playing.uah = Some(40_000);
        let t = totals(&[stretch("on-paused", 60_000), playing.clone(), stretch("new-state", 5_000), playing, stretch("charging", 90_000)]);
        let states: Vec<&str> = t.iter().map(|t| t.state.as_str()).collect();
        assert_eq!(states, ["off-playing", "on-paused", "charging", "new-state"]);
        assert_eq!(t[0].line(), "2 h 00 min, CPU 1.00 %, 10.0 wakeups/s, 60 KB/min allocated, 4 GCs, PSS 150 MB, 80.0 mAh (40.0 mAh/h)");
        assert_eq!(t[1].line(), "1 min 00 s, CPU 1.00 %, 10.0 wakeups/s, 60 KB/min allocated, 2 GCs, PSS 150 MB, 3 % (180.00 %/h)");
        assert_eq!(t[2].line(), "1 min 30 s, CPU 1.00 %, 10.0 wakeups/s, 60 KB/min allocated, 2 GCs, PSS 150 MB", "charging has no battery figures");
    }

    #[test]
    fn a_stretch_reads_on_one_line() {
        let mut s = stretch("on-playing-player", 187_000);
        s.frames = 900;
        s.janky = 12;
        s.worst_ms = 48.4;
        s.gauge_ma = Some(212.6);
        assert!(stretch_line(&s).ends_with(
            "  3 min 07 s, CPU 1.00 %, 10.0 wakeups/s, 60 KB/min, 2 GCs, PSS 150 MB, 3 %, gauge 213 mA, 30.1-31.4 °C, 900 frames, 12 janky, worst 48 ms [engine exoplayer, on the CPU]"
        ));
        s.uah = Some(9_350);
        assert!(stretch_line(&s).contains(", 9.4 mAh (180.0 mAh/h)"));
        assert_eq!(duration(42_999), "42 s");
        assert_eq!(duration(3_900_000), "1 h 05 min");
        assert_eq!(fixed(0.125, 2), "0.13", "Java's rounding, not the float's");
    }

    #[test]
    fn the_page_counts_the_frames_and_lists_the_newest_first() {
        let mut a = stretch("on-playing-app", 60_000);
        a.frames = 1_000;
        a.janky = 25;
        a.worst_ms = 33.3;
        let b = stretch("on-playing-app", 120_000);
        let mut live = stretch("off-playing", 10_000);
        live.worst_ms = 70.0;
        live.frames = 0;
        let p = page(vec![a, b], Some(live));
        assert_eq!(p.totals.iter().map(|t| t.title.as_str()).collect::<Vec<_>>(), ["Screen off, playing", "Screen on, playing, other page (2)"]);
        let f = p.frames.unwrap();
        assert_eq!((f.title.as_str(), f.detail.as_str()), ("1000 frames, 25 janky (2.5 %)", "The slowest took 70 ms"));
        assert_eq!(p.live.unwrap().title, "Now: Screen off, playing");
        assert!(p.stretches[0].detail.contains("  2 min 00 s,"), "newest first");
        assert_eq!(page(Vec::new(), None), PerfPage { totals: Vec::new(), frames: None, live: None, stretches: Vec::new() });
        let mut why = stretch("off-playing", 60_000);
        why.threads = vec![PerfThreadUse { name: "nori-track".into(), cpu_ms: 12, wakeups: 1_440, born: false }];
        why.out = Some(output());
        let lines: Vec<String> = page(vec![why], None).stretches[0].detail.lines().map(String::from).collect();
        assert_eq!(lines.len(), 3, "the figures, then the threads and the output under them: {lines:?}");
        assert_eq!(lines[1], "threads by wakeups: nori-track 24.0/s 12 ms");
        assert!(lines[2].starts_with("output: rust, 44100 Hz"));
    }

    #[test]
    fn the_report_is_plain_text_in_columns() {
        let mut playing = stretch("off-playing", 3_600_000);
        playing.uah = Some(40_000);
        let d = PerfDevice {
            manufacturer: "Google".into(),
            model: "Pixel 8".into(),
            device: "shiba".into(),
            release: "16".into(),
            sdk: 36,
            version: "0.3.4".into(),
            sha: "abc1234".into(),
            build_type: "perf".into(),
        };
        let mut charging = stretch("charging", 90_000);
        charging.out = Some(output());
        charging.rx = Some(3 * 1024 * 1024 + 300 * 1024);
        charging.tx = Some(20 * 1024);
        let r = report(vec![playing, charging], &d, "", "12 ns");
        let lines: Vec<&str> = r.lines().collect();
        assert_eq!(lines[0], "Nori perf report");
        assert_eq!(lines[1], "Device: Google Pixel 8 (shiba), Android 16 (API 36)");
        assert_eq!(lines[2], "Build: 0.3.4 (abc1234, perf)");
        assert!(lines[3].starts_with("Recorded: ") && lines[3].ends_with(", 2 stretches"));
        assert_eq!(lines[4], "Battery counter: yes (mAh)");
        assert_eq!(lines[6], "By state");
        assert_eq!(lines[7], "state                      time  CPU %  wakeups/s  KB/min  GCs  PSS MB   mAh  mAh/h   %/h  frames  janky %");
        assert_eq!(lines[8], "Screen off, playing  1 h 00 min   1.00       10.0      60    2     150  40.0   40.0  3.00       -        -");
        assert_eq!(lines[9], "Charging             1 min 30 s   1.00       10.0      60    2     150     -      -     -       -        -");
        assert_eq!(lines[11], "Stretches, newest first");
        assert!(lines[12].starts_with("Charging: ") && lines[12].contains(", network 3.3 MB in, 20 KB out"), "{}", lines[12]);
        assert!(lines[13].starts_with("    output: rust, "), "the output under its stretch: {}", lines[13]);
        assert!(lines[14].starts_with("Screen off, playing: "));
        assert_eq!(&lines[15..], ["", "Cover benchmark: 12 ns"], "a benchmark not run is left out");
    }

    #[test]
    fn a_start_is_written_by_the_phone_clock() {
        let offset = crate::library::local_offset_s(0) * 1000;
        assert_eq!(when(-offset), "01-01 00:00");
        assert_eq!(when(-offset + 86_400_000 * 31 + 3_600_000 * 13 + 60_000 * 7), "02-01 13:07");
    }
}
