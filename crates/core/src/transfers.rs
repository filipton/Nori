//! Downloads as they run: how far each song is, how fast the bytes arrive, the batch the notification
//! counts ("12 of 49"), what the notification and the downloads screen say, and how the batch went.
//! The platform moves the bytes (media3 on Android) and reports to this; this decides and words
//! everything. The per-chunk report is a slot number and three numbers - nothing is looked up by name
//! or allocated while bytes flow - and the once-a-second notification is only rebuilt when its words
//! or its bar actually change.

use std::collections::{HashMap, HashSet};

use jni::objects::{JClass, JString};
use jni::sys::{jfloat, jint, jlong, jstring};
use jni::JNIEnv;
use parking_lot::Mutex;

use crate::{alog, Core};

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

// media3's `Download.STATE_*`.
const QUEUED: jint = 0;
const STOPPED: jint = 1;
const DOWNLOADING: jint = 2;
const COMPLETED: jint = 3;
const FAILED: jint = 4;
const RESTARTING: jint = 7;

/// What [`followed`] and [`removed`] tell the platform to do.
const NEW_BATCH: jint = 1;
const DRAINED: jint = 2;
const MARKS: jint = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Downloading = 1,
    Failed = 2,
    Done = 3,
}

#[derive(Debug, Clone, Default)]
struct Info {
    title: String,
    album: String,
    /// What the song should weigh, bytes (0 unknown).
    estimate: i64,
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

#[derive(Debug, Clone, PartialEq, Default)]
struct Notice {
    title: String,
    text: String,
    permille: i32,
}

#[derive(Debug, Default)]
struct Tracker {
    slots: Vec<Slot>,
    batch: Batch,
    marks: HashMap<String, (Phase, i64)>,
    info: HashMap<String, Info>,
    download_kbps: i32,
    speed_bps: i64,
    remaining_bytes: i64,
    eta_s: i64,
    notice: Notice,
    scratch_title: String,
    scratch_text: String,
}

static TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);

fn with<R>(f: impl FnOnce(&mut Tracker) -> R) -> R {
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

impl Tracker {
    fn info(&mut self, id: &str) -> &Info {
        if !self.info.contains_key(id) {
            let found = crate::active().and_then(|core| {
                let c = core.db.lock();
                let json: String = c.query_row("SELECT json FROM downloads WHERE server=sid() AND id=?1", [id], |r| r.get(0)).ok()?;
                serde_json::from_str::<crate::Song>(&json).ok()
            });
            let info = found.map_or_else(Info::default, |s| Info {
                estimate: expected_bytes(s.size as i64, s.duration as i64, self.download_kbps),
                title: s.title.replace('\n', " "),
                album: s.album.replace('\n', " "),
            });
            self.info.insert(id.to_string(), info);
        }
        &self.info[id]
    }

    fn slot_of(&self, id: &str) -> Option<usize> {
        self.slots.iter().position(|s| s.live && s.id == id)
    }

    fn close(&mut self, id: &str) {
        if let Some(i) = self.slot_of(id) {
            self.slots[i].live = false;
        }
    }

    fn mark(&mut self, id: &str, phase: Option<Phase>, now: i64) -> bool {
        match phase {
            Some(p) if self.marks.get(id).map(|m| m.0) == Some(p) && p == Phase::Downloading => false,
            Some(p) => {
                self.marks.insert(id.to_string(), (p, now));
                if p == Phase::Done {
                    self.recent_only();
                }
                true
            }
            None => self.marks.remove(id).is_some(),
        }
    }

    /// Finished marks beyond the latest [`RECENT`] go; the song's own "downloaded" state carries on.
    fn recent_only(&mut self) {
        let mut done: Vec<(i64, String)> = self.marks.iter().filter(|(_, m)| m.0 == Phase::Done).map(|(id, m)| (m.1, id.clone())).collect();
        if done.len() <= RECENT {
            return;
        }
        done.sort();
        for (_, id) in done.iter().take(done.len() - RECENT) {
            self.marks.remove(id);
        }
    }

    fn running(&self) -> usize {
        self.marks.values().filter(|m| m.0 == Phase::Downloading).count()
    }
}

// ---- JNI: dev.nori.music.downloads.DownloadsJni ----------------------------------------------------------

fn id_of(env: &mut JNIEnv, s: &JString) -> Option<String> {
    env.get_string(s).ok().map(Into::into)
}

/// Takes the download quality from the settings (0: the original file), for what songs should weigh;
/// songs weighed at another quality are weighed again. Asked whenever songs are queued, and when an
/// earlier process's queue is picked up.
fn follow_quality() {
    let kbps = crate::settings_store::with_prefs(|p| p.download.bit_rate).unwrap_or(0);
    with(|t| {
        if t.download_kbps != kbps {
            t.download_kbps = kbps;
            t.info.clear();
        }
    });
}

/// media3 reported `id` in `state`. Returns [`NEW_BATCH`] (a batch starts: the last one's result goes),
/// [`DRAINED`] (the last song settled: say how it went) and [`MARKS`] (the phases changed).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_followed(mut env: JNIEnv, _: JClass, id: JString, state: jint, now: jlong) -> jint {
    let Some(id) = id_of(&mut env, &id) else { return 0 };
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
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_removed(mut env: JNIEnv, _: JClass, id: JString) -> jint {
    let Some(id) = id_of(&mut env, &id) else { return 0 };
    with(|t| {
        let was_open = t.batch.open.contains(&id);
        t.batch.removed(&id);
        t.close(&id);
        let mut flags = if t.marks.remove(&id).is_some() { MARKS } else { 0 };
        if was_open && t.batch.open.is_empty() {
            flags |= DRAINED;
        }
        flags
    })
}

/// Forgets `id`'s mark and figures (it is being asked for again, or cancelled).
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_unmark(mut env: JNIEnv, _: JClass, id: JString) -> jint {
    let Some(id) = id_of(&mut env, &id) else { return 0 };
    with(|t| {
        t.close(&id);
        if t.marks.remove(&id).is_some() {
            MARKS
        } else {
            0
        }
    })
}

/// Where a ring starts before any bytes arrive: 0, or negative when the size cannot be told.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_startFraction(mut env: JNIEnv, _: JClass, id: JString) -> jfloat {
    let Some(id) = id_of(&mut env, &id) else { return -1.0 };
    with(|t| if t.info(&id).estimate > 0 { 0.0 } else { -1.0 })
}

/// A download's bytes start moving: the slot its chunks are reported against.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_open(mut env: JNIEnv, _: JClass, id: JString, now: jlong) -> jint {
    let Some(id) = id_of(&mut env, &id) else { return -1 };
    with(|t| {
        let estimate = t.info(&id).estimate;
        let slot = Slot {
            id,
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
                i as jint
            }
            None => {
                t.slots.push(slot);
                t.slots.len() as jint - 1
            }
        }
    })
}

/// A chunk arrived on `slot`: `bytes` so far of `length` (0 unknown). Returns the progress to show, or
/// NaN when it has not moved enough to be worth drawing. Called per chunk; allocates nothing.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_note(_: JNIEnv, _: JClass, slot: jint, length: jlong, bytes: jlong, now: jlong) -> jfloat {
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
fn sections<'a, T: Clone>(pending: &'a [T], done: &'a [T], marks: &HashMap<String, (Phase, i64)>, id: impl Fn(&T) -> &str) -> [Vec<T>; 4] {
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

/// The notification's words and bar for `listed` downloads media3 knows of (`waiting`: no network yet).
/// Returns 0 when nothing changed since the last call (keep the last notification), 1 when it did (read
/// [`notice_title`] and friends), 2 when the batch is over (the "complete" notification). Asked once a
/// second: it works in buffers kept from call to call and allocates nothing unless the words change.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_notice(_: JNIEnv, _: JClass, listed: jint, waiting: jint, now: jlong) -> jint {
    use std::fmt::Write;
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
    let (title, text) = (&mut t.scratch_title, &mut t.scratch_text);
    title.clear();
    text.clear();
    if waiting != 0 {
        title.push_str("Waiting for a network");
    } else if total == 1 {
        match current_title {
            Some(c) => {
                let _ = write!(title, "Downloading “{c}”");
            }
            None => title.push_str("Downloading 1 song"),
        }
    } else {
        let _ = write!(title, "Downloading: {position} of {total}");
    }
    let part = |text: &mut String, f: &dyn Fn(&mut String)| {
        let start = text.len();
        if start > 0 {
            text.push_str(" · ");
        }
        let mark = text.len();
        f(text);
        if text.len() == mark {
            text.truncate(start);
        }
    };
    if let Some(c) = current_title {
        part(text, &|b| b.push_str(c));
    }
    if total != 1 {
        if let Some(l) = t.batch.label() {
            part(text, &|b| {
                let _ = write!(b, "“{l}”");
            });
        }
    }
    let (speed_bps, eta_s) = (t.speed_bps, t.eta_s);
    part(text, &|b| push_speed(b, speed_bps));
    part(text, &|b| push_eta(b, eta_s));
    if t.notice.title == t.scratch_title && t.notice.text == t.scratch_text && t.notice.permille == permille {
        return 0;
    }
    std::mem::swap(&mut t.notice.title, &mut t.scratch_title);
    std::mem::swap(&mut t.notice.text, &mut t.scratch_text);
    t.notice.permille = permille;
    1
}

fn jstr(env: &mut JNIEnv, s: &str) -> jstring {
    env.new_string(s).map(|s| s.into_raw()).unwrap_or(std::ptr::null_mut())
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_noticeTitle(mut env: JNIEnv, _: JClass) -> jstring {
    let t = with(|t| t.notice.title.clone());
    jstr(&mut env, &t)
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_noticeText(mut env: JNIEnv, _: JClass) -> jstring {
    let t = with(|t| t.notice.text.clone());
    jstr(&mut env, &t)
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_noticePermille(_: JNIEnv, _: JClass) -> jint {
    with(|t| t.notice.permille)
}

/// How the batch went, once it has: "title\ntext" (text may be empty), or empty when there is nothing to
/// say. The caller keeps the notification when [`summary_failed`] says something failed.
#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_summary(mut env: JNIEnv, _: JClass) -> jstring {
    let s = with(|t| {
        let (done, failed) = (t.batch.done, t.batch.failed);
        if done == 0 && failed == 0 {
            return String::new();
        }
        let title = if failed > 0 {
            if failed == 1 {
                "1 song couldn’t be downloaded".to_string()
            } else {
                format!("{failed} songs couldn’t be downloaded")
            }
        } else if let Some(l) = t.batch.label().filter(|_| done > 1) {
            format!("“{l}” downloaded")
        } else if done == 1 {
            "1 song downloaded".to_string()
        } else {
            format!("{done} songs downloaded")
        };
        let text = if failed > 0 && done > 0 {
            format!("{done} downloaded · tap to see what failed")
        } else if failed > 0 {
            "Tap to try again".to_string()
        } else {
            String::new()
        };
        format!("{title}\n{text}")
    });
    jstr(&mut env, &s)
}

#[no_mangle]
pub extern "system" fn Java_dev_nori_music_downloads_DownloadsJni_summaryFailed(_: JNIEnv, _: JClass) -> jint {
    with(|t| t.batch.failed)
}

// ---- uniffi: which songs are queued, and picking up an earlier process's queue -----------------------------

/// What queuing songs did. `fresh` went into the queue now, in the order asked; `again` were in it
/// already but unfinished - failed, or lost to a process that died before the platform heard of them -
/// and are asked for again, so the download button always does something. Finished songs are left as
/// they are.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct DownloadQueued {
    pub fresh: Vec<String>,
    pub again: Vec<String>,
}

/// One download as the platform's own queue remembers it: media3's `Download.STATE_*`, the length (-1
/// unknown) and the bytes it has.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DownloadKnown {
    pub id: String,
    pub state: i32,
    pub length: i64,
    pub bytes: i64,
}

/// A download an earlier process left failed, and how far it got.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct DownloadFailed {
    pub id: String,
    pub progress: f32,
}

/// What an earlier process left unfinished, sorted out (see [`Core::download_recover`]).
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
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

const REMOVING: jint = 5;

/// Adds the `rows` (id, song json) that are not in the queue yet, behind everything queued before, and
/// sorts the rest into asked again (unfinished) and left out (finished).
fn queue_rows(c: &mut rusqlite::Connection, rows: impl IntoIterator<Item = (String, String)>) -> crate::Result<DownloadQueued> {
    use rusqlite::OptionalExtension;
    let tx = c.transaction()?;
    let mut out = DownloadQueued::default();
    {
        // The queue is listed by this stamp. Counting on from the newest keeps a new batch behind the
        // last one, and its own songs in the order asked, whatever the clock does.
        let newest: i64 = tx.query_row("SELECT coalesce(max(ts), 0) FROM downloads WHERE server=sid()", [], |r| r.get(0))?;
        let mut ts = newest.max(crate::db::now_ms());
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
fn recovery(pending: &[String], known: &[DownloadKnown]) -> (DownloadRecovery, Vec<(String, i64, i64)>) {
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

#[uniffi::export]
impl Core {
    /// Queues `songs` for download; see [`DownloadQueued`].
    pub fn download_queue(&self, songs: Vec<crate::Song>) -> crate::Result<DownloadQueued> {
        follow_quality();
        let rows = songs.into_iter().map(|s| {
            let json = serde_json::to_string(&s).unwrap_or_default();
            (s.id, json)
        });
        queue_rows(&mut self.db.lock(), rows)
    }

    /// Queues every song of the offline index, in index order, as [`Core::download_queue`] does. One
    /// call however big the library: the songs go from the index into the queue without leaving the core.
    pub fn download_queue_library(&self) -> crate::Result<DownloadQueued> {
        follow_quality();
        let mut c = self.db.lock();
        let rows: Vec<(String, String)> = {
            let mut st = c.prepare("SELECT id, json FROM items WHERE server=sid() AND kind=?1 ORDER BY rowid")?;
            let rows = st.query_map([crate::db::SONG], |r| Ok((r.get(0)?, r.get(1)?)))?;
            rows.filter_map(|r| r.ok()).collect()
        };
        queue_rows(&mut c, rows)
    }

    /// Brings the downloads table and the platform's queue (`known`: what it holds of the songs pending
    /// here) back into agreement after the process died: finished songs are recorded, failed ones marked,
    /// and what is left to do is said.
    pub fn download_recover(&self, known: Vec<DownloadKnown>) -> crate::Result<DownloadRecovery> {
        follow_quality();
        let pending: Vec<String> = self.downloads(false)?.into_iter().map(|s| s.id).collect();
        let (mut r, failed) = recovery(&pending, &known);
        for id in &r.finished {
            self.download_done(id.clone())?;
        }
        r.failed = with(|t| {
            failed
                .into_iter()
                .map(|(id, length, bytes)| {
                    let estimate = t.info(&id).estimate;
                    t.marks.entry(id.clone()).or_insert((Phase::Failed, 0));
                    DownloadFailed { progress: fraction(length, bytes, estimate), id }
                })
                .collect()
        });
        Ok(r)
    }
}

// ---- uniffi: what the downloads screen shows ----------------------------------------------------------------

/// The downloads screen's lists, in the order the queue will run them.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DownloadSections {
    pub active: Vec<crate::Song>,
    pub queued: Vec<crate::Song>,
    pub failed: Vec<crate::Song>,
    /// This session's finished songs, newest first.
    pub finished: Vec<crate::Song>,
}

/// A download's phase for the screen: 0 waiting (or nothing), 1 downloading, 2 failed, 3 done.
#[uniffi::export]
pub fn download_phase(id: String) -> i32 {
    with(|t| t.marks.get(&id).map_or(0, |m| m.0 as i32))
}

/// The ids with a phase, and each one's phase (as [`download_phase`]) and when it began.
#[derive(Debug, Clone, uniffi::Record)]
pub struct DownloadMarks {
    pub ids: Vec<String>,
    pub phases: Vec<i32>,
    pub at: Vec<i64>,
}

#[uniffi::export]
pub fn download_marks() -> DownloadMarks {
    with(|t| {
        let mut m = DownloadMarks { ids: Vec::new(), phases: Vec::new(), at: Vec::new() };
        for (id, (p, at)) in &t.marks {
            m.ids.push(id.clone());
            m.phases.push(*p as i32);
            m.at.push(*at);
        }
        m
    })
}

/// "2 downloading · 14 waiting · 1 failed · 3.2 MB/s · 12:34 left", or "Nothing downloading".
#[uniffi::export]
pub fn download_summary(active: i32, queued: i32, failed: i32) -> String {
    use std::fmt::Write;
    let (speed, eta) = with(|t| (t.speed_bps, t.eta_s));
    let mut out = String::new();
    let mut add = |f: &dyn Fn(&mut String)| {
        let start = out.len();
        if start > 0 {
            out.push_str(" · ");
        }
        let mark = out.len();
        f(&mut out);
        if out.len() == mark {
            out.truncate(start);
        }
    };
    if active > 0 {
        add(&|o| {
            let _ = write!(o, "{active} downloading");
        });
    }
    if queued > 0 {
        add(&|o| {
            let _ = write!(o, "{queued} waiting");
        });
    }
    if failed > 0 {
        add(&|o| {
            let _ = write!(o, "{failed} failed");
        });
    }
    if active > 0 {
        add(&|o| push_speed(o, speed));
        add(&|o| push_eta(o, eta));
    }
    if out.is_empty() {
        out.push_str("Nothing downloading");
    }
    out
}

/// A running song's second line: its artist, then where it stands ("45% · 2.1 MB/s · 1:20 left").
#[uniffi::export]
pub fn download_row(id: String, artist: String) -> String {
    use std::fmt::Write;
    with(|t| {
        let mut out = artist;
        let Some(i) = t.slot_of(&id) else { return out };
        let s = &t.slots[i];
        let f = fraction(s.length, s.bytes, s.estimate);
        let total = if s.length > 0 { s.length } else { s.estimate };
        let speed = s.rate as i64;
        let eta = if speed > 0 && total > s.bytes { (total - s.bytes) / speed } else { -1 };
        // Each figure only when there is one to give.
        let add = |out: &mut String, f: &dyn Fn(&mut String)| {
            let start = out.len();
            out.push_str(" · ");
            let mark = out.len();
            f(out);
            if out.len() == mark {
                out.truncate(start);
            }
        };
        if f >= 0.0 {
            add(&mut out, &|o| {
                let _ = write!(o, "{}%", (f * 100.0).round() as i32);
            });
        }
        add(&mut out, &|o| push_speed(o, speed));
        add(&mut out, &|o| push_eta(o, eta));
        out
    })
}

/// The download statistics for checks: bytes a second right now, and seconds left (-1 unknown).
#[uniffi::export]
pub fn download_speed_eta() -> Vec<i64> {
    with(|t| vec![t.speed_bps, t.eta_s])
}

#[uniffi::export]
impl Core {
    /// The downloads screen's lists: pending songs split by what they are doing, oldest first (the order
    /// they run), and this session's finished songs newest first.
    pub fn download_sections(&self) -> crate::Result<DownloadSections> {
        let pending = self.downloads(false)?;
        let done = self.downloads(true)?;
        with(|t| {
            let [active, queued, failed, finished] = sections(&pending, &done, &t.marks, |s: &crate::Song| s.id.as_str());
            Ok(DownloadSections { active, queued, failed, finished })
        })
    }
}

// ---- words ----------------------------------------------------------------------------------------------------

/// "850 KB/s", "3.2 MB/s"; empty when nothing is measurable.
#[uniffi::export]
pub fn format_speed(bps: i64) -> String {
    let mut s = String::new();
    push_speed(&mut s, bps);
    s
}

/// [`format_speed`] onto the end of `out`, in the phone's number style (`nori_text`).
pub fn push_speed(out: &mut String, bps: i64) {
    use std::fmt::Write;
    let (v, places, unit) = match bps {
        b if b <= 0 => return,
        b if b < 1_000 => {
            let _ = write!(out, "{b} B/s");
            return;
        }
        b if b < 1_000_000 => (b as f64 / 1_000.0, 0, " KB/s"),
        b if b < 10_000_000 => (b as f64 / 1_000_000.0, 1, " MB/s"),
        b => (b as f64 / 1_000_000.0, 0, " MB/s"),
    };
    nori_text::push_fixed(out, v, places, false);
    out.push_str(unit);
}

/// "45 s left", "12:34 left", "2:05:00 left"; empty when it cannot be said (negative).
#[uniffi::export]
pub fn format_eta(sec: i64) -> String {
    let mut s = String::new();
    push_eta(&mut s, sec);
    s
}

/// [`format_eta`] onto the end of `out`.
pub fn push_eta(out: &mut String, sec: i64) {
    use std::fmt::Write;
    let (h, m, s) = (sec / 3600, (sec % 3600) / 60, sec % 60);
    let _ = match sec {
        x if x < 0 => Ok(()),
        x if x < 60 => write!(out, "{x} s left"),
        _ if h > 0 => write!(out, "{h}:{m:02}:{s:02} left"),
        _ => write!(out, "{m}:{s:02} left"),
    };
}

/// "850 B", "38 MB", "2.1 GB", in the phone's number style (`nori_text`).
#[uniffi::export]
pub fn format_bytes(bytes: i64) -> String {
    let (v, places, unit) = match bytes {
        b if b < 1024 => return format!("{b} B"),
        b if b < 1_048_576 => (b as f64 / 1024.0, 0, " KB"),
        b if b < 10_485_760 => (b as f64 / 1_048_576.0, 1, " MB"),
        b if b < 1_073_741_824 => (b as f64 / 1_048_576.0, 0, " MB"),
        b => (b as f64 / 1_073_741_824.0, 1, " GB"),
    };
    let mut out = nori_text::fixed(v, places, false);
    out.push_str(unit);
    out
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

    fn song(id: &str) -> crate::Song {
        crate::Song { id: id.into(), ..Default::default() }
    }

    #[test]
    fn queuing_adds_what_is_new_and_asks_again_for_what_is_stuck() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let q = core.download_queue(vec![song("a"), song("b"), song("a")]).unwrap();
        assert_eq!((q.fresh, q.again), (vec!["a".to_string(), "b".into()], vec![]));
        core.download_done("a".into()).unwrap();
        let q = core.download_queue(vec![song("a"), song("b"), song("c")]).unwrap();
        assert_eq!(q.fresh, ["c"], "a finished song is left alone");
        assert_eq!(q.again, ["b"], "an unfinished one is asked for again");
        // Listed newest first; the queue runs, and the screen shows it, the other way round.
        let pending: Vec<String> = core.downloads(false).unwrap().into_iter().rev().map(|s| s.id).collect();
        assert_eq!(pending, ["b", "c"]);
    }

    #[test]
    fn the_whole_library_is_queued_in_index_order() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        let songs: Vec<crate::Song> = ["x", "y", "z"].map(song).to_vec();
        crate::db::index(&mut core.db.lock(), &[], &[], &songs).unwrap();
        core.download_queue(vec![songs[1].clone()]).unwrap();
        let q = core.download_queue_library().unwrap();
        assert_eq!((q.fresh, q.again), (vec!["x".to_string(), "z".into()], vec!["y".to_string()]));
        assert_eq!(core.downloads(false).unwrap().len(), 3);
    }

    #[test]
    fn an_earlier_process_queue_is_sorted_out() {
        let known = |id: &str, state| DownloadKnown { id: id.into(), state, length: 100, bytes: 50 };
        let pending = ["lost", "removing", "done", "failed", "queued"].map(String::from);
        let all = [known("removing", REMOVING), known("done", COMPLETED), known("failed", FAILED), known("queued", QUEUED), known("other", COMPLETED)];
        let (r, failed) = recovery(&pending, &all);
        assert_eq!(r.lost, ["lost", "removing"]);
        assert_eq!(r.finished, ["done"]);
        assert_eq!(failed, [("failed".to_string(), 100, 50)]);
        assert!(r.unfinished);
        assert!(!recovery(&pending[..1], &[]).0.unfinished);

        let core = Core::new(String::new(), "t".into()).unwrap();
        core.download_queue(vec![song("rc-a"), song("rc-b")]).unwrap();
        let r = core.download_recover(vec![known("rc-a", COMPLETED), known("rc-b", FAILED)]).unwrap();
        assert_eq!(r.finished, ["rc-a"]);
        assert_eq!(r.failed, [DownloadFailed { id: "rc-b".into(), progress: 0.5 }]);
        assert_eq!(core.downloads(true).unwrap().len(), 1, "recorded as finished");
        assert_eq!(download_phase("rc-b".into()), Phase::Failed as i32);
    }


    #[test]
    fn words() {
        assert_eq!((format_speed(0), format_speed(850_000), format_speed(3_200_000)), (String::new(), "850 KB/s".into(), "3.2 MB/s".into()));
        assert_eq!((format_eta(45), format_eta(754), format_eta(7500), format_eta(-1)), ("45 s left".into(), "12:34 left".into(), "2:05:00 left".into(), String::new()));
        assert_eq!((format_bytes(850), format_bytes(38 * 1_048_576), format_bytes(2_254_857_830)), ("850 B".into(), "38 MB".into(), "2.1 GB".into()));
    }
}
