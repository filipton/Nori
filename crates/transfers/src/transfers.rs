//! Downloads as they run: how far each song is, how fast the bytes arrive, the batch the notification
//! counts ("12 of 49"), which of the notification's messages applies and how the batch went. The
//! platform moves the bytes (media3 on Android), reports to this, and words what this says: the facts
//! come out as numbers and kinds. The per-chunk report is a slot number and three numbers - nothing is
//! looked up by name or allocated while bytes flow - and the once-a-second notification is only rebuilt
//! when its facts actually change.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};

use nori_model::{alog, Song};
use parking_lot::Mutex;

/// Finished songs the downloads screen keeps listing this session.
pub const RECENT: usize = 50;
/// A progress figure is passed on at most this often, and only when it moved a whole percent.
const GATE_MS: i64 = 250;
const GATE_STEP: f32 = 0.01;
/// Against an estimated size, progress is held short of full: an estimate can be low, and a ring sitting
/// at 100 % while bytes still arrive looks stuck.
const ESTIMATE_CEILING: f32 = 0.97;
/// What an unlisted song is guessed to weigh when nothing in the batch says otherwise.
const UNKNOWN_SONG_BYTES: i64 = 8_000_000;

// media3's `Download.STATE_*`, which the platform reports downloads in.
pub const QUEUED: i32 = 0;
pub const STOPPED: i32 = 1;
pub const DOWNLOADING: i32 = 2;
pub const COMPLETED: i32 = 3;
pub const FAILED: i32 = 4;
pub const RESTARTING: i32 = 7;

/// What [`followed`] and [`removed`] tell the platform to do.
pub const NEW_BATCH: i32 = 1;
pub const DRAINED: i32 = 2;
pub const MARKS: i32 = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Downloading = 1,
    Failed = 2,
    Done = 3,
}

/// What the screens say about a song being downloaded, read once from the downloads table.
#[derive(Debug, Clone, Default)]
pub struct Info {
    title: String,
    album: String,
    artist: String,
    /// What the song should weigh, bytes (0 unknown).
    pub estimate: i64,
}

#[derive(Debug, Clone)]
struct Slot {
    id: String,
    estimate: i64,
    length: i64,
    bytes: i64,
    started_at: i64,
    gate_value: f32,
    gate_at: i64,
    speed_bytes: i64,
    speed_at: i64,
    rate: f64,
    live: bool,
}

/// One run of the queue: everything queued since it was last empty. Its total holds still while songs
/// finish ("12 of 49", never "1 of 47").
#[derive(Debug, Default)]
struct Batch {
    open: HashSet<String>,
    failed_ids: HashSet<String>,
    labels: HashMap<String, String>,
    total: i32,
    done: i32,
    failed: i32,
}

impl Batch {
    fn finished(&self) -> i32 {
        self.done + self.failed
    }

    /// A song entered the queue; true when this starts a new batch.
    fn queued(&mut self, id: &str, label: &str) -> bool {
        if self.open.contains(id) {
            return false;
        }
        let fresh = self.open.is_empty();
        if fresh {
            self.failed_ids.clear();
            self.labels.clear();
            (self.total, self.done, self.failed) = (0, 0, 0);
        }
        // Tried again: the same song, not one more.
        if self.failed_ids.remove(id) {
            self.failed -= 1;
        } else {
            self.total += 1;
        }
        self.open.insert(id.to_string());
        if !label.trim().is_empty() {
            self.labels.insert(id.to_string(), label.to_string());
        }
        fresh
    }

    fn completed(&mut self, id: &str) {
        if self.open.remove(id) {
            self.done += 1;
        }
    }

    fn failed(&mut self, id: &str) {
        if self.open.remove(id) {
            self.failed += 1;
            self.failed_ids.insert(id.to_string());
        }
    }

    /// Stopped before it finished, or a failure given up on: it no longer counts at all.
    fn removed(&mut self, id: &str) {
        if self.open.remove(id) {
            self.total -= 1;
        } else if self.failed_ids.remove(id) {
            self.failed -= 1;
            self.total -= 1;
        }
        self.labels.remove(id);
    }

    /// The one name every song of the batch shares (an album), when they all share one.
    fn label(&self) -> Option<&str> {
        if (self.labels.len() as i32) < self.total {
            return None;
        }
        let mut names = self.labels.values();
        let first = names.next()?;
        names.all(|n| n == first).then_some(first.as_str())
    }
}

/// What the running batch's notification is made from ([`notice`]).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Notice {
    pub kind: NoticeKind,
    /// Which song of the batch is in flight, from 1, and how many the batch has.
    pub position: i32,
    pub total: i32,
    /// The bar, in thousandths.
    pub permille: i32,
    /// Bytes a second, and seconds left (-1 unknown).
    pub speed_bps: i64,
    pub eta_s: i64,
    /// The song in flight's title; empty when none is known.
    pub current: String,
    /// The album the batch is, when it has more than one song and all are from it; empty otherwise.
    pub label: String,
}

/// Every download the platform has reported, the batch they make and what the notification says.
#[derive(Debug, Default)]
pub struct Tracker {
    slots: Vec<Slot>,
    batch: Batch,
    pub marks: HashMap<String, (Phase, i64)>,
    /// The songs whose mark changed since the platform last asked (see [`download_marks_changed`]).
    pub changed: HashSet<String>,
    info: HashMap<String, Info>,
    download_kbps: i32,
    speed_bps: i64,
    remaining_bytes: i64,
    eta_s: i64,
    notice: Notice,
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

/// The one download tracker, lent to `f`.
pub fn with<R>(f: impl FnOnce(&mut Tracker) -> R) -> R {
    f(TRACKER.lock().get_or_insert_with(Tracker::default))
}

/// What a song should weigh once downloaded: its length at the transcoded bitrate, or the file itself at
/// the original quality. A transcoding server rarely sends a length, so this is what the bar fills against.
pub fn expected_bytes(size_bytes: i64, duration_s: i64, bitrate_kbps: i32) -> i64 {
    if bitrate_kbps > 0 && duration_s > 0 {
        duration_s * bitrate_kbps as i64 * 125
    } else {
        size_bytes
    }
}

/// How far along: against the stated length when there is one, else against the estimate (held short of
/// full), else unknown (negative).
pub fn fraction(length: i64, bytes: i64, estimate: i64) -> f32 {
    if length > 0 {
        (bytes as f64 / length as f64).clamp(0.0, 1.0) as f32
    } else if estimate > 0 {
        ((bytes as f64 / estimate as f64) as f32).clamp(0.0, ESTIMATE_CEILING)
    } else {
        -1.0
    }
}

/// Song `id` as the downloads table keeps it.
fn download_song(c: &rusqlite::Connection, id: &str) -> Option<Song> {
    let json: String = c.query_row("SELECT json FROM downloads WHERE server=sid() AND id=?1", [id], |r| r.get(0)).ok()?;
    serde_json::from_str::<Song>(&json).ok()
}

impl Tracker {
    pub fn info(&mut self, id: &str) -> &Info {
        if !self.info.contains_key(id) {
            let found = nori_db::active().and_then(|db| download_song(&db.lock(), id));
            self.keep_info(id, found);
        }
        &self.info[id]
    }

    /// [`Self::info`] without waiting for the database: none while another thread holds it (a sync
    /// writing a page), and nothing remembered then, so the next ask reads it. For the downloads
    /// screen's line, asked through a door that must not block.
    fn info_now(&mut self, id: &str) -> Option<&Info> {
        if !self.info.contains_key(id) {
            let db = nori_db::active()?;
            let c = db.try_lock()?;
            let found = download_song(&c, id);
            drop(c);
            self.keep_info(id, found);
        }
        self.info.get(id)
    }

    fn keep_info(&mut self, id: &str, found: Option<Song>) {
        {
            let info = found.map_or_else(Info::default, |s| Info {
                estimate: expected_bytes(s.size as i64, s.duration as i64, self.download_kbps),
                title: s.title.replace('\n', " "),
                album: s.album.replace('\n', " "),
                artist: s.artist.replace('\n', " "),
            });
            self.info.insert(id.to_string(), info);
        }
    }

    fn slot_of(&self, id: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.live && s.id == id)
    }

    pub fn close(&mut self, id: &str) {
        if let Some(i) = self.slot_of(id) {
            self.slots[i].live = false;
        }
    }

    fn mark(&mut self, id: &str, phase: Option<Phase>, now: i64) -> bool {
        match phase {
            Some(p) if self.marks.get(id).map(|m| m.0) == Some(p) && p == Phase::Downloading => false,
            Some(p) => {
                self.marks.insert(id.to_string(), (p, now));
                self.changed.insert(id.to_string());
                if p == Phase::Done {
                    self.recent_only();
                }
                true
            }
            None => self.unmark(id),
        }
    }

    /// Takes `id`'s mark away; true when it had one.
    pub fn unmark(&mut self, id: &str) -> bool {
        let had = self.marks.remove(id).is_some();
        if had {
            self.changed.insert(id.to_string());
        }
        had
    }

    /// Finished marks beyond the latest [`RECENT`] go; the song's own "downloaded" state carries on.
    fn recent_only(&mut self) {
        let mut done: Vec<(i64, String)> = self.marks.iter().filter(|(_, m)| m.0 == Phase::Done).map(|(id, m)| (m.1, id.clone())).collect();
        if done.len() <= RECENT {
            return;
        }
        done.sort();
        for (_, id) in done.iter().take(done.len() - RECENT) {
            self.unmark(id);
        }
    }

    fn running(&self) -> usize {
        self.marks.values().filter(|m| m.0 == Phase::Downloading).count()
    }
}

// ---- what the platform reports as downloads run ------------------------------------------------------------

/// Takes the download quality from the settings (0: the original file), for what songs should weigh;
/// songs weighed at another quality are weighed again. Asked whenever songs are queued, and when an
/// earlier process's queue is picked up.
pub fn follow_quality() {
    let kbps = nori_settings::settings_store::with_prefs(|p| p.download.bit_rate).unwrap_or(0);
    with(|t| {
        if t.download_kbps != kbps {
            t.download_kbps = kbps;
            t.info.clear();
        }
    });
}

/// media3 reported `id` in `state`. Returns [`NEW_BATCH`] (a batch starts: the last one's result goes),
/// [`DRAINED`] (the last song settled: say how it went) and [`MARKS`] (the phases changed).
pub fn followed(id: &str, state: i32, now: i64) -> i32 {
    let id = id.to_string();
    with(|t| {
        let was_open = t.batch.open.contains(&id);
        let mut flags = 0;
        match state {
            QUEUED | DOWNLOADING | RESTARTING | STOPPED => {
                let label = t.info(&id).album.clone();
                if t.batch.queued(&id, &label) {
                    flags |= NEW_BATCH;
                }
            }
            COMPLETED => t.batch.completed(&id),
            FAILED => t.batch.failed(&id),
            _ => {}
        }
        let phase = match state {
            DOWNLOADING => Some(Phase::Downloading),
            COMPLETED => Some(Phase::Done),
            FAILED => Some(Phase::Failed),
            _ => None,
        };
        if matches!(state, COMPLETED | FAILED) {
            t.close(&id);
        }
        if t.mark(&id, phase, now) {
            flags |= MARKS;
            match phase {
                Some(Phase::Downloading) => alog::info(&format!("start {id}: {} downloading at once", t.running())),
                Some(Phase::Failed) => alog::info(&format!("failed {id}")),
                _ => {}
            }
        }
        if was_open && t.batch.open.is_empty() {
            flags |= DRAINED;
        }
        flags
    })
}

/// `id` left the queue for good. Returns flags as [`followed`] does.
pub fn removed(id: &str) -> i32 {
    with(|t| {
        let was_open = t.batch.open.contains(id);
        t.batch.removed(id);
        t.close(id);
        let mut flags = if t.unmark(id) { MARKS } else { 0 };
        if was_open && t.batch.open.is_empty() {
            flags |= DRAINED;
        }
        flags
    })
}

/// Forgets `id`'s mark and figures (it is being asked for again, or cancelled).
pub fn unmark(id: &str) -> i32 {
    with(|t| {
        t.close(id);
        if t.unmark(id) {
            MARKS
        } else {
            0
        }
    })
}

/// Where a ring starts before any bytes arrive: 0, or negative when the size cannot be told.
pub fn start_fraction(id: &str) -> f32 {
    with(|t| if t.info(id).estimate > 0 { 0.0 } else { -1.0 })
}

/// A download's bytes start moving: the slot its chunks are reported against.
pub fn open(id: &str, now: i64) -> i32 {
    with(|t| {
        let estimate = t.info(id).estimate;
        let slot = Slot {
            id: id.to_string(),
            estimate,
            length: 0,
            bytes: 0,
            started_at: now,
            gate_value: f32::NAN,
            gate_at: 0,
            speed_bytes: 0,
            speed_at: now,
            rate: 0.0,
            live: true,
        };
        match t.slots.iter().position(|s| !s.live) {
            Some(i) => {
                t.slots[i] = slot;
                i as i32
            }
            None => {
                t.slots.push(slot);
                t.slots.len() as i32 - 1
            }
        }
    })
}

/// A chunk arrived on `slot`: `bytes` so far of `length` (0 unknown). Returns the progress to show, or
/// NaN when it has not moved enough to be worth drawing. Called per chunk; allocates nothing.
pub fn note(slot: i32, length: i64, bytes: i64, now: i64) -> f32 {
    let mut guard = TRACKER.lock();
    let Some(s) = guard.as_mut().and_then(|t| t.slots.get_mut(slot.max(0) as usize)).filter(|s| s.live) else { return f32::NAN };
    s.bytes = bytes;
    if length > 0 {
        s.length = length;
    }
    // The rate: each gap of at least 0.4 s folded into a running average, so one slow chunk does not
    // make the figure jump.
    let dt = (now - s.speed_at) as f64 / 1000.0;
    if dt >= 0.4 && bytes >= s.speed_bytes {
        let instant = (bytes - s.speed_bytes) as f64 / dt;
        s.rate = if s.rate <= 0.0 { instant } else { s.rate * 0.7 + instant * 0.3 };
        s.speed_bytes = bytes;
        s.speed_at = now;
    }
    let f = fraction(s.length, bytes, s.estimate);
    if gate(s.gate_value, s.gate_at, f, now) {
        s.gate_value = f;
        s.gate_at = now;
        f
    } else {
        f32::NAN
    }
}

/// Whether progress `f` (negative: unknown) at `now` is worth drawing after `last` shown at `last_at`
/// (NaN: nothing shown yet): the first figure, a switch to or from unknown, the finish, otherwise a
/// whole percent no sooner than a quarter second after the last. Bytes arrive in chunks of a few
/// kilobytes; drawing each would redraw a list hundreds of times a second for a ring that moves a pixel.
fn gate(last: f32, last_at: i64, f: f32, now: i64) -> bool {
    if last.is_nan() {
        true
    } else if f < 0.0 {
        last >= 0.0
    } else if last < 0.0 {
        true
    } else if f >= 1.0 {
        last < 1.0
    } else {
        now - last_at >= GATE_MS && (f - last).abs() >= GATE_STEP
    }
}

impl Batch {
    /// "12 of 49": the song being worked on now, counted from one, never past the total.
    #[cfg(test)]
    fn position(&self) -> i32 {
        (self.finished() + 1).min(self.total)
    }

    /// How far the whole batch is, 0..1: finished songs plus the running ones' fractions, which can
    /// never claim more than the songs still open.
    fn fraction(&self, in_flight: f64) -> f32 {
        if self.total <= 0 {
            return 0.0;
        }
        (((self.finished() as f64 + in_flight.clamp(0.0, self.open.len() as f64)) / self.total as f64) as f32).clamp(0.0, 1.0)
    }
}

/// The downloads screen's split of `pending` (newest first, as the index keeps them) into downloading,
/// waiting and failed, in the order the queue runs them (oldest first), and this session's finished
/// songs newest first. `done` and `pending` together are where a finished song's details come from, so
/// one that has just completed is not missing from every list while the index catches up.
pub fn sections<'a, T: Clone>(pending: &'a [T], done: &'a [T], marks: &HashMap<String, (Phase, i64)>, id: impl Fn(&T) -> &str) -> [Vec<T>; 4] {
    let (mut active, mut queued, mut failed) = (Vec::new(), Vec::new(), Vec::new());
    for song in pending.iter().rev() {
        match marks.get(id(song)).map(|m| m.0) {
            Some(Phase::Downloading) => active.push(song.clone()),
            Some(Phase::Failed) => failed.push(song.clone()),
            Some(Phase::Done) => {}
            None => queued.push(song.clone()),
        }
    }
    let mut finished: Vec<(i64, &T)> = pending
        .iter()
        .chain(done.iter())
        .filter_map(|song| marks.get(id(song)).filter(|m| m.0 == Phase::Done).map(|m| (m.1, song)))
        .collect();
    finished.sort_by(|a, b| b.0.cmp(&a.0));
    let mut seen = HashSet::new();
    let finished = finished.into_iter().filter(|(_, song)| seen.insert(id(song).to_string())).map(|(_, song)| song.clone()).collect();
    [active, queued, failed, finished]
}

/// The notification's facts and bar for `listed` downloads media3 knows of (`waiting`: no network yet).
/// Returns 0 when nothing changed since the last call (keep the last notification), 1 when something did
/// (read [`notice_facts`]; the platform words them), 2 when the batch is over (the "complete"
/// notification). Asked once a second: it allocates nothing unless the song or the album changes.
pub fn notice(listed: i32, waiting: bool, now: i64) -> i32 {
    let mut guard = TRACKER.lock();
    let t = guard.get_or_insert_with(Tracker::default);
    let total = t.batch.total.max(t.batch.finished() + listed);
    if total == 0 {
        return 2;
    }
    let (mut in_flight, mut speed, mut current) = (0f64, 0f64, None::<usize>);
    for (i, s) in t.slots.iter().enumerate().filter(|(_, s)| s.live) {
        in_flight += fraction(s.length, s.bytes, s.estimate).max(0.0) as f64;
        // Speed from songs heard from in the last 3 s.
        if now - s.speed_at < 3_000 {
            speed += s.rate;
        }
        if current.is_none_or(|c| s.started_at < t.slots[c].started_at) {
            current = Some(i);
        }
    }
    t.speed_bps = speed as i64;
    let permille = (t.batch.fraction(in_flight) * 1000.0) as i32;
    let position = (t.batch.finished() + 1).min(total.max(1));
    // What remains: each open song's size less what it has, with the average for any the platform lists
    // but has not reported yet.
    let (mut remaining, mut known_sum, mut known_n) = (0i64, 0i64, 0i64);
    for id in &t.batch.open {
        let slot = t.slots.iter().find(|s| s.live && s.id == *id);
        let (length, bytes) = slot.map_or((0, 0), |s| (s.length, s.bytes));
        let size = if length > 0 { length } else { t.info.get(id).map_or(0, |i| i.estimate) };
        if size > 0 {
            known_sum += size;
            known_n += 1;
        }
        remaining += (size - bytes).max(0);
    }
    let avg = if known_n == 0 { UNKNOWN_SONG_BYTES } else { known_sum / known_n };
    t.remaining_bytes = remaining + (total - t.batch.finished() - listed).max(0) as i64 * avg;
    t.eta_s = if t.speed_bps > 0 && t.remaining_bytes > 0 { t.remaining_bytes / t.speed_bps } else { -1 };
    let current_title = current.and_then(|i| t.info.get(&t.slots[i].id)).map(|i| i.title.as_str()).filter(|s| !s.is_empty());
    let kind = if waiting {
        NoticeKind::Waiting
    } else if total == 1 {
        if current_title.is_some() { NoticeKind::OneNamed } else { NoticeKind::One }
    } else {
        NoticeKind::Many
    };
    let label = if total != 1 { t.batch.label() } else { None };
    let n = &t.notice;
    if n.kind == kind
        && n.position == position
        && n.total == total
        && n.permille == permille
        && n.speed_bps == t.speed_bps
        && n.eta_s == t.eta_s
        && n.current == current_title.unwrap_or("")
        && n.label == label.unwrap_or("")
    {
        return 0;
    }
    let (current, label) = (current_title.unwrap_or(""), label.unwrap_or(""));
    let n = &mut t.notice;
    // The strings are written into the ones kept, only when they changed.
    if n.current != current {
        n.current.clear();
        n.current.push_str(current);
    }
    if n.label != label {
        n.label.clear();
        n.label.push_str(label);
    }
    (n.kind, n.position, n.total, n.permille, n.speed_bps, n.eta_s) = (kind, position, total, permille, t.speed_bps, t.eta_s);
    1
}

/// What the notification says while a batch runs, as [`notice`] last found it: which title applies and
/// the facts the platform words it from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum NoticeKind {
    /// No network yet: "Waiting for a network".
    #[default]
    Waiting,
    /// One song, whose title is known: "Downloading “Title”".
    OneNamed,
    /// One song, not named yet: "Downloading 1 song".
    One,
    /// More: "Downloading: 12 of 49".
    Many,
}

/// The notification's facts as [`notice`] last found them, lent to `f`: the song in flight's title (empty
/// when none) and the batch's album (empty unless the batch has more than one song, all from it), with
/// the numbers. One crossing for all of it.
pub fn notice_facts<R>(f: impl FnOnce(&Notice) -> R) -> R {
    with(|t| f(&t.notice))
}

/// The notification's bar, in thousandths.
pub fn notice_permille() -> i32 {
    with(|t| t.notice.permille)
}

/// How a finished batch went, for its notification's title: which one applies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryTitle {
    /// "`failed` songs couldn’t be downloaded".
    Failed,
    /// "“`label`” downloaded": more than one song, all from one album, none failed.
    Album,
    /// "`done` songs downloaded".
    Downloaded,
}

/// The line under a finished batch's title.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SummaryText {
    None,
    /// Some failed, some did not: "`done` downloaded · tap to see what failed".
    SomeFailed,
    /// All failed: "Tap to try again".
    TryAgain,
}

/// How a batch went.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Summary {
    pub title: SummaryTitle,
    pub text: SummaryText,
    pub done: i32,
    pub failed: i32,
    /// The album the batch was, for [`SummaryTitle::Album`]; empty otherwise.
    pub label: String,
}

/// How the batch went, once it has; none when there is nothing to say. The caller keeps the notification
/// when something failed.
pub fn summary() -> Option<Summary> {
    with(|t| {
        let (done, failed) = (t.batch.done, t.batch.failed);
        if done == 0 && failed == 0 {
            return None;
        }
        let album = t.batch.label().filter(|_| done > 1);
        let title = if failed > 0 {
            SummaryTitle::Failed
        } else if album.is_some() {
            SummaryTitle::Album
        } else {
            SummaryTitle::Downloaded
        };
        let text = if failed > 0 && done > 0 {
            SummaryText::SomeFailed
        } else if failed > 0 {
            SummaryText::TryAgain
        } else {
            SummaryText::None
        };
        let label = if title == SummaryTitle::Album { album.unwrap_or("").to_string() } else { String::new() };
        Some(Summary { title, text, done, failed, label })
    })
}

// ---- uniffi: which songs are queued, and picking up an earlier process's queue -----------------------------

/// What queuing songs did. `fresh` went into the queue now, in the order asked; `again` were in it
/// already but unfinished - failed, or lost to a process that died before the platform heard of them -
/// and are asked for again, so the download button always does something. Finished songs are left as
/// they are.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DownloadQueued {
    pub fresh: Vec<String>,
    pub again: Vec<String>,
}

/// One download as the platform's own queue remembers it: media3's `Download.STATE_*`, the length (-1
/// unknown) and the bytes it has.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DownloadKnown {
    pub id: String,
    pub state: i32,
    pub length: i64,
    pub bytes: i64,
}

/// A download an earlier process left failed, and how far it got.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DownloadFailed {
    pub id: String,
    pub progress: f32,
}

/// What an earlier process left unfinished, sorted out (see [`Core::download_recover`]).
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DownloadRecovery {
    /// Never reached the platform's queue (the add was still in flight): to be asked for again.
    pub lost: Vec<String>,
    /// Failed: marked failed here, so each reads as failed rather than waiting.
    pub failed: Vec<DownloadFailed>,
    /// Finished there but not recorded here (the process went between the two): recorded now. Each
    /// song's streamed copy is the same bytes twice and can go.
    pub finished: Vec<String>,
    /// Queued or interrupted mid-download: the platform's queue has to be started to resume them.
    pub unfinished: bool,
}

/// media3's `Download.STATE_REMOVING`: a download being taken back.
pub const REMOVING: i32 = 5;

/// Which songs the downloads table holds and whether each has finished, kept beside the table so one
/// song can be asked about - by every row a list draws, every track opened - without the database, and
/// the table counted without reading it. Read from the table once, when the core opens; every write to
/// the table updates it while the database is still locked, so the two never disagree.
#[derive(Debug, Default)]
pub struct Held {
    pub ids: HashMap<String, bool>,
    pub done: u32,
}

/// Moves on whenever any core's downloads table changes, so a platform's copy of the counts can tell a
/// change from the same answer asked twice. One counter for every core: a new server's numbers never
/// read as the old one's.
pub static HELD_VERSION: AtomicU64 = AtomicU64::new(1);

impl Held {
    pub fn load(c: &rusqlite::Connection) -> nori_model::Result<Held> {
        let mut st = c.prepare("SELECT id, done FROM downloads WHERE server=sid()")?;
        let ids: HashMap<String, bool> = st.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?.filter_map(|r| r.ok()).collect();
        let done = ids.values().filter(|d| **d).count() as u32;
        HELD_VERSION.fetch_add(1, Ordering::Relaxed);
        Ok(Held { ids, done })
    }

    /// 0 not in the table, 1 queued or failed, 2 finished.
    pub fn state(&self, id: &str) -> i32 {
        self.ids.get(id).map_or(0, |d| if *d { 2 } else { 1 })
    }

    pub fn queued(&mut self, id: &str) {
        if !self.ids.contains_key(id) {
            self.ids.insert(id.to_string(), false);
            HELD_VERSION.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn finished(&mut self, id: &str) {
        if let Some(d) = self.ids.get_mut(id).filter(|d| !**d) {
            *d = true;
            self.done += 1;
            HELD_VERSION.fetch_add(1, Ordering::Relaxed);
        }
    }

    pub fn removed(&mut self, id: &str) {
        if let Some(d) = self.ids.remove(id) {
            self.done -= d as u32;
            HELD_VERSION.fetch_add(1, Ordering::Relaxed);
        }
    }
}

/// How many songs are downloaded and how many are still to come, and which version of the table that is.
#[derive(Debug, Clone, Copy, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DownloadCounts {
    pub done: u32,
    pub pending: u32,
    pub version: u64,
}

/// Adds the `rows` (id, song json) that are not in the queue yet, behind everything queued before, and
/// sorts the rest into asked again (unfinished) and left out (finished).
pub fn queue_rows(c: &mut rusqlite::Connection, rows: impl IntoIterator<Item = (String, String)>) -> nori_model::Result<DownloadQueued> {
    use rusqlite::OptionalExtension;
    let tx = c.transaction()?;
    let mut out = DownloadQueued::default();
    {
        // The queue is listed by this stamp. Counting on from the newest keeps a new batch behind the
        // last one, and its own songs in the order asked, whatever the clock does.
        let newest: i64 = tx.query_row("SELECT coalesce(max(ts), 0) FROM downloads WHERE server=sid()", [], |r| r.get(0))?;
        let mut ts = newest.max(nori_db::now_ms());
        let mut done = tx.prepare_cached("SELECT done FROM downloads WHERE server=sid() AND id=?1")?;
        let mut add = tx.prepare_cached("INSERT INTO downloads(server, id, json, ts) VALUES(sid(), ?1, ?2, ?3)")?;
        let mut seen = HashSet::new();
        for (id, json) in rows {
            if !seen.insert(id.clone()) {
                continue;
            }
            match done.query_row([&id], |r| r.get::<_, bool>(0)).optional()? {
                None => {
                    ts += 1;
                    add.execute(rusqlite::params![id, json, ts])?;
                    out.fresh.push(id);
                }
                Some(false) => out.again.push(id),
                Some(true) => {}
            }
        }
    }
    tx.commit()?;
    Ok(out)
}

/// What the platform's queue says about the songs still `pending` here; the failed ones come back
/// separately with their length and bytes.
pub fn recovery(pending: &[String], known: &[DownloadKnown]) -> (DownloadRecovery, Vec<(String, i64, i64)>) {
    let by_id: HashMap<&str, &DownloadKnown> = known.iter().map(|k| (k.id.as_str(), k)).collect();
    let mut r = DownloadRecovery::default();
    let mut failed = Vec::new();
    for id in pending {
        match by_id.get(id.as_str()).map(|k| (k.state, k)) {
            None | Some((REMOVING, _)) => r.lost.push(id.clone()),
            Some((COMPLETED, _)) => r.finished.push(id.clone()),
            Some((FAILED, k)) => failed.push((id.clone(), k.length, k.bytes)),
            Some(_) => r.unfinished = true,
        }
    }
    (r, failed)
}

// ---- uniffi: what the downloads screen shows ----------------------------------------------------------------

/// The downloads screen's lists, in the order the queue will run them.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DownloadSections {
    pub active: Vec<Song>,
    pub queued: Vec<Song>,
    pub failed: Vec<Song>,
    /// This session's finished songs, newest first.
    pub finished: Vec<Song>,
}

/// A download's phase for the screen: 0 waiting (or nothing), 1 downloading, 2 failed, 3 done.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn download_phase(id: String) -> i32 {
    with(|t| t.marks.get(&id).map_or(0, |m| m.0 as i32))
}

/// The ids with a phase, and each one's phase (as [`download_phase`]) and when it began.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DownloadMarks {
    pub ids: Vec<String>,
    pub phases: Vec<i32>,
    pub at: Vec<i64>,
}

/// The marks that changed since the last call, each with its phase now (0: it has none any more). The
/// platform keeps its own copy of the marks and only hears what moved.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn download_marks_changed() -> DownloadMarks {
    with(|t| {
        let mut m = DownloadMarks { ids: Vec::new(), phases: Vec::new(), at: Vec::new() };
        for id in t.changed.drain() {
            let (p, at) = t.marks.get(&id).map_or((0, 0), |(p, at)| (*p as i32, *at));
            m.ids.push(id);
            m.phases.push(p);
            m.at.push(at);
        }
        m
    })
}

/// Where a running song stands, for its row on the downloads screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowFacts {
    /// Whole percent, -1 when the size is not known.
    pub percent: i32,
    /// Bytes a second (0 unknown) and seconds left (-1 unknown).
    pub speed_bps: i64,
    pub eta_s: i64,
}

/// Where `id` stands if it is running; none when it is not.
fn row_facts(slots: &[Slot], id: &str) -> Option<RowFacts> {
    let s = slots.iter().find(|s| s.live && s.id == id)?;
    let f = fraction(s.length, s.bytes, s.estimate);
    let total = if s.length > 0 { s.length } else { s.estimate };
    let speed = s.rate as i64;
    let eta = if speed > 0 && total > s.bytes { (total - s.bytes) / speed } else { -1 };
    Some(RowFacts { percent: if f >= 0.0 { (f * 100.0).round() as i32 } else { -1 }, speed_bps: speed, eta_s: eta })
}

// ---- the downloads screen's facts -----------------------------------------------------------------------------

/// A song's row on the downloads screen: its artist (the downloads table's, read once per song; empty when
/// unknown) and, while it runs, where it stands. Asked whenever its ring moves; the artist is lent to `f`.
pub fn row<R>(id: &str, f: impl FnOnce(&str, Option<RowFacts>) -> R) -> R {
    let mut guard = TRACKER.lock();
    let t = guard.get_or_insert_with(Tracker::default);
    let facts = row_facts(&t.slots, id);
    let artist = t.info_now(id).map(|i| i.artist.as_str()).unwrap_or("");
    f(artist, facts)
}
// ---- whether a song is downloaded -----------------------------------------------------------------------------

/// The downloads table's ids of the core the app is using now; the core holds them, this only keeps them
/// while it does.
static ACTIVE_HELD: Mutex<Weak<Mutex<Held>>> = Mutex::new(Weak::new());

/// `held` are the downloads of the core the app uses from now on: the newest one made.
pub fn set_active_held(held: &Arc<Mutex<Held>>) {
    *ACTIVE_HELD.lock() = Arc::downgrade(held);
}

/// Whether `id` is in the active core's downloads table: 0 no, 1 queued or failed, 2 finished. Asked by
/// every row a list draws and every track opened; answered from memory.
pub fn held(id: &str) -> i32 {
    ACTIVE_HELD.lock().upgrade().map_or(0, |held| held.lock().state(id))
}

/// The download statistics for checks: bytes a second right now, and seconds left (-1 unknown).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn download_speed_eta() -> Vec<i64> {
    let (speed, eta) = speed_eta();
    vec![speed, eta]
}

/// The batch's bytes a second right now, and seconds left (-1 unknown): the downloads screen's summary
/// line asks once a second while it is open.
pub fn speed_eta() -> (i64, i64) {
    with(|t| (t.speed_bps, t.eta_s))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_total_holds_still_while_songs_finish() {
        let mut b = Batch::default();
        for i in 0..49 {
            b.queued(&format!("s{i}"), "");
        }
        assert_eq!((b.position(), b.total), (1, 49));
        b.completed("s0");
        b.completed("s1");
        assert_eq!((b.position(), b.total), (3, 49));
        for i in 0..49 {
            b.completed(&format!("s{i}"));
        }
        assert_eq!((b.done, b.position(), b.total), (49, 49, 49));
        assert!(b.open.is_empty());
    }

    #[test]
    fn a_song_queued_twice_counts_once_and_a_drained_batch_starts_over() {
        let mut b = Batch::default();
        assert!(b.queued("a", ""), "the first song starts a batch");
        assert!(!b.queued("a", ""));
        b.queued("b", "");
        assert_eq!(b.total, 2);
        let mut b = Batch::default();
        b.queued("a", "");
        b.completed("a");
        assert!(b.open.is_empty() && b.done == 1);
        assert!(b.queued("b", ""));
        assert_eq!((b.total, b.done), (1, 0));
    }

    #[test]
    fn a_retry_inside_the_batch_is_the_same_song() {
        let mut b = Batch::default();
        b.queued("a", "");
        b.queued("b", "");
        b.failed("a");
        assert_eq!(b.failed, 1);
        b.queued("a", "");
        assert_eq!((b.failed, b.total), (0, 2));
        b.completed("a");
        b.completed("b");
        assert_eq!(b.done, 2);
    }

    #[test]
    fn cancelled_songs_leave_the_count() {
        let mut b = Batch::default();
        for id in ["a", "b", "c"] {
            b.queued(id, "");
        }
        b.failed("b");
        b.removed("c");
        b.removed("b");
        assert_eq!((b.total, b.failed), (1, 0));
        b.removed("x");
        assert_eq!(b.total, 1, "never part of it");
    }

    #[test]
    fn progress_counts_finished_songs_and_the_running_ones() {
        let mut b = Batch::default();
        for i in 0..4 {
            b.queued(&format!("s{i}"), "");
        }
        assert_eq!(b.fraction(0.0), 0.0);
        b.completed("s0");
        assert!((b.fraction(0.5) - 0.375).abs() < 1e-6);
        assert!((b.fraction(99.0) - 1.0).abs() < 1e-6, "running songs never claim more than the open ones");
    }

    #[test]
    fn a_batch_is_named_after_an_album_only_when_every_song_is_from_it() {
        let mut b = Batch::default();
        b.queued("a", "Blue");
        b.queued("b", "Blue");
        assert_eq!(b.label(), Some("Blue"));
        b.queued("c", "Red");
        assert_eq!(b.label(), None);
        b.removed("c");
        assert_eq!(b.label(), Some("Blue"));
        b.queued("d", "");
        assert_eq!(b.label(), None, "a song without an album name");
    }

    #[test]
    fn the_gate_lets_through_at_most_four_updates_a_second_of_whole_percents() {
        let (mut last, mut at) = (f32::NAN, 0i64);
        let mut offer = |f: f32, now: i64| {
            let pass = gate(last, at, f, now);
            if pass {
                (last, at) = (f, now);
            }
            pass
        };
        assert!(offer(0.0, 1_000), "the first figure always shows");
        assert!(!offer(0.05, 1_100), "too soon");
        assert!(offer(0.05, 1_250));
        assert!(!offer(0.055, 1_600), "under a percent");
        assert!(offer(0.07, 1_600));
        let passed = (0..2_000).filter(|&t| offer(0.07 + t as f32 * 0.0004, 2_000 + t as i64)).count();
        assert!(passed <= 8, "{passed} updates in two seconds");
    }

    #[test]
    fn the_gate_always_shows_the_finish_and_the_switch_to_unknown() {
        assert!(gate(0.995, 0, 1.0, 10), "the finish is not held back by the interval");
        assert!(!gate(1.0, 10, 1.0, 1_000), "and only once");
        assert!(gate(f32::NAN, 0, -1.0, 0));
        assert!(!gate(-1.0, 0, -1.0, 5_000), "still unknown: nothing to redraw");
        assert!(gate(-1.0, 5_000, 0.2, 5_001), "the size arriving shows at once");
        assert!(gate(0.2, 5_001, -1.0, 5_002));
    }

    #[test]
    fn progress_uses_the_stated_length_then_the_estimate() {
        assert_eq!(fraction(200, 100, 999), 0.5);
        assert_eq!(fraction(-1, 100, 400), 0.25);
        assert!(fraction(-1, 900, 400) < 1.0, "an estimate that is too low never reads as finished");
        assert_eq!(fraction(-1, 100, 0), -1.0);
        assert_eq!(expected_bytes(9_000_000, 240, 0), 9_000_000);
        assert_eq!(expected_bytes(9_000_000, 240, 192), 240 * 192 * 125);
        assert_eq!(expected_bytes(9_000_000, 0, 192), 9_000_000);
    }

    fn marks(list: &[(&str, Phase, i64)]) -> HashMap<String, (Phase, i64)> {
        list.iter().map(|&(id, p, at)| (id.to_string(), (p, at))).collect()
    }

    #[test]
    fn sections_run_in_queue_order() {
        let pending = ["e", "d", "c", "b", "a"].map(String::from);
        let done = ["x", "y", "old"].map(String::from);
        let m = marks(&[("a", Phase::Downloading, 0), ("c", Phase::Downloading, 0), ("b", Phase::Failed, 0), ("x", Phase::Done, 5), ("y", Phase::Done, 9)]);
        let [active, queued, failed, finished] = sections(&pending, &done, &m, |s: &String| s.as_str());
        assert_eq!(active, ["a", "c"]);
        assert_eq!(queued, ["d", "e"], "a failure does not hold up the songs behind it");
        assert_eq!(failed, ["b"]);
        assert_eq!(finished, ["y", "x"], "newest first, and only this session's");
    }

    #[test]
    fn a_song_just_finished_is_listed_before_the_index_catches_up() {
        let pending = ["b", "a"].map(String::from);
        let [_, queued, _, finished] = sections(&pending, &[], &marks(&[("a", Phase::Done, 0)]), |s: &String| s.as_str());
        assert_eq!((queued, finished), (vec!["b".to_string()], vec!["a".to_string()]));
    }

    #[test]
    fn only_the_marks_that_moved_come_over() {
        with(|t| {
            t.mark("mk-a", Some(Phase::Downloading), 1);
            t.mark("mk-b", Some(Phase::Failed), 2);
        });
        let m = download_marks_changed();
        let mine = |m: &DownloadMarks, id: &str| m.ids.iter().position(|i| i == id).map(|i| m.phases[i]);
        assert_eq!((mine(&m, "mk-a"), mine(&m, "mk-b")), (Some(1), Some(2)));
        with(|t| {
            t.unmark("mk-a");
        });
        let m = download_marks_changed();
        assert_eq!((mine(&m, "mk-a"), mine(&m, "mk-b")), (Some(0), None), "gone, and the other one did not move");
        with(|t| {
            t.unmark("mk-b");
        });
    }

    #[test]
    fn a_rows_facts_are_given_only_while_it_runs() {
        let slot = Slot { id: "r".into(), estimate: 0, length: 1000, bytes: 450, started_at: 0, gate_value: 0.0, gate_at: 0, speed_bytes: 0, speed_at: 0, rate: 0.0, live: true };
        assert_eq!(row_facts(std::slice::from_ref(&slot), "r"), Some(RowFacts { percent: 45, speed_bps: 0, eta_s: -1 }));
        assert_eq!(row_facts(std::slice::from_ref(&slot), "other"), None);
    }

    #[test]
    fn a_rows_artist_and_the_notification_come_from_the_tracker() {
        with(|t| {
            t.info.insert("rw-1".into(), Info { artist: "Nils".into(), ..Info::default() });
        });
        assert_eq!(row("rw-1", |a, f| (a.to_string(), f)), ("Nils".into(), None), "known, and not running: the artist alone");
        assert_eq!(row("rw-unknown", |a, f| (a.to_string(), f)), (String::new(), None), "no core to read it from: nothing, and nothing kept");
        with(|t| {
            t.notice.kind = NoticeKind::Many;
            t.notice.label = "Album".into();
        });
        assert_eq!(notice_facts(|n| (n.kind, n.label.clone())), (NoticeKind::Many, "Album".into()));
    }
}
