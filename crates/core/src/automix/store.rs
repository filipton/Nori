//! The `track_analysis` table, the `Core` methods around it, and the JNI streaming analyser that feeds it from the
//! playback path.

use jni::objects::{JByteBuffer, JClass};
use jni::sys::{jint, jlong};
use jni::JNIEnv;
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};

use super::analysis::Analyzer;
use super::ANALYSIS_VERSION;
use crate::{Core, Result, TrackAnalysis};

const COLUMNS: &str = "song_id, analysis_version, duration_ms, bpm, bpm_confidence, beat_offset_ms, stability, downbeat_phase, \
     downbeat_confidence, lufs, key, key_confidence, silence_start_ms, silence_end_ms, mixramp_start_ms, mixramp_end_ms, \
     intro_end_ms, outro_start_ms, analysed_ms";

pub fn put(c: &Connection, a: &TrackAnalysis) -> rusqlite::Result<()> {
    c.prepare_cached(&format!(
        "INSERT OR REPLACE INTO track_analysis({COLUMNS}) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19)"
    ))?
    .execute(params![
        a.song_id,
        a.analysis_version,
        a.duration_ms,
        a.bpm,
        a.bpm_confidence,
        a.beat_offset_ms,
        a.stability,
        a.downbeat_phase,
        a.downbeat_confidence,
        a.lufs,
        a.key,
        a.key_confidence,
        a.silence_start_ms,
        a.silence_end_ms,
        a.mixramp_start_ms,
        a.mixramp_end_ms,
        a.intro_end_ms,
        a.outro_start_ms,
        a.analysed_ms
    ])
    .map(|_| ())
}

pub fn get(c: &Connection, song_id: &str) -> rusqlite::Result<Option<TrackAnalysis>> {
    c.prepare_cached(&format!("SELECT {COLUMNS} FROM track_analysis WHERE song_id=?1"))?
        .query_row([song_id], |r| {
            Ok(TrackAnalysis {
                song_id: r.get(0)?,
                analysis_version: r.get(1)?,
                duration_ms: r.get(2)?,
                bpm: r.get(3)?,
                bpm_confidence: r.get(4)?,
                beat_offset_ms: r.get(5)?,
                stability: r.get(6)?,
                downbeat_phase: r.get(7)?,
                downbeat_confidence: r.get(8)?,
                lufs: r.get(9)?,
                key: r.get(10)?,
                key_confidence: r.get(11)?,
                silence_start_ms: r.get(12)?,
                silence_end_ms: r.get(13)?,
                mixramp_start_ms: r.get(14)?,
                mixramp_end_ms: r.get(15)?,
                intro_end_ms: r.get(16)?,
                outro_start_ms: r.get(17)?,
                analysed_ms: r.get(18)?,
            })
        })
        .optional()
}

/// The ids with no row, or a row from an older analysis version, in the order given.
pub fn missing(c: &Connection, ids: &[String]) -> rusqlite::Result<Vec<String>> {
    let mut st = c.prepare_cached("SELECT analysis_version FROM track_analysis WHERE song_id=?1")?;
    let mut out = Vec::new();
    for id in ids {
        let v: Option<i32> = st.query_row([id], |r| r.get(0)).optional()?;
        if v.is_none_or(|v| v < ANALYSIS_VERSION) {
            out.push(id.clone());
        }
    }
    Ok(out)
}

/// The streaming analyser behind a JNI handle.
struct Stream {
    a: Mutex<Analyzer>,
    channels: usize,
}

fn stream<'a>(h: i64) -> Option<&'a Stream> {
    (h != 0).then(|| unsafe { &*(h as *const Stream) })
}

#[uniffi::export]
impl Core {
    pub fn analysis_get(&self, song_id: String) -> Result<Option<TrackAnalysis>> {
        Ok(get(&self.db.lock(), &song_id)?)
    }

    /// Which of `song_ids` still need analysing (no row, or an older analysis version).
    pub fn analysis_missing(&self, song_ids: Vec<String>) -> Result<Vec<String>> {
        Ok(missing(&self.db.lock(), &song_ids)?)
    }

    /// Forgets every analysis, so tracks are measured again as they play. For when the measurements look wrong.
    pub fn analysis_clear(&self) -> Result<u32> {
        let c = self.db.lock();
        let n = c.execute("DELETE FROM track_analysis", [])? as u32;
        Ok(n)
    }

    /// How many tracks have an analysis, for the settings screen.
    pub fn analysis_count(&self) -> Result<u32> {
        Ok(self.db.lock().query_row("SELECT count(*) FROM track_analysis", [], |r| r.get(0))?)
    }

    pub fn analysis_store(&self, analysis: TrackAnalysis) -> Result<()> {
        Ok(put(&self.db.lock(), &analysis)?)
    }

    /// Analyses a whole decoded track, stores and returns the result. `pcm` is interleaved little-endian PCM as the
    /// decoder hands it out (`encoding` 2 = 16-bit, 4 = float), any rate and channel count. Blocking, about 0.14
    /// CPU-seconds for 4 minutes on a desktop; the database is only locked for the write.
    pub fn analysis_run(&self, song_id: String, pcm: Vec<u8>, sample_rate: i32, channels: i32, encoding: i32) -> Result<TrackAnalysis> {
        let a = super::analyse_bytes(&song_id, &pcm, sample_rate, channels, encoding).track;
        put(&self.db.lock(), &a)?;
        Ok(a)
    }

    /// Finishes a JNI streaming analyser (`AutoMixAnalyzer.create`) that was fed one whole track from its first
    /// sample, stores and returns the result. `None` when it holds less than 30 s. The handle is emptied, not
    /// freed: feed the next track into it or `destroy` it.
    pub fn analysis_finish_stream(&self, song_id: String, handle: i64) -> Result<Option<TrackAnalysis>> {
        let Some(s) = stream(handle) else { return Ok(None) };
        let f = {
            let mut a = s.a.lock();
            if a.samples() < (a.rate() * 30.0) as u64 {
                a.reset();
                return Ok(None);
            }
            a.take_features()
        };
        let a = super::finish(&song_id, &f).track;
        put(&self.db.lock(), &a)?;
        Ok(Some(a))
    }
}

#[cfg(test)]
pub fn test_handle(a: Analyzer) -> i64 {
    Box::into_raw(Box::new(Stream { a: Mutex::new(a), channels: 1 })) as i64
}

#[cfg(test)]
pub fn test_with(h: i64, f: impl FnOnce(&mut Analyzer)) {
    f(&mut stream(h).unwrap().a.lock())
}

#[cfg(test)]
pub fn test_destroy(h: jlong) {
    drop(unsafe { Box::from_raw(h as *mut Stream) });
}

// ---- JNI: dev.flint.music.playback.AutoMixAnalyzer --------------------------------------------------------------

const PCM_16: jint = 2;
const PCM_FLOAT: jint = 4;

/// `expected_ms` (0 if unknown) sizes the buffers so feeding never reallocates.
#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_AutoMixAnalyzer_create(_: JNIEnv, _: JClass, rate: jint, channels: jint, expected_ms: jlong) -> jlong {
    let a = Analyzer::new(rate.max(1) as u32, expected_ms.max(0) as u64);
    Box::into_raw(Box::new(Stream { a: Mutex::new(a), channels: channels.clamp(1, 8) as usize })) as jlong
}

#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_AutoMixAnalyzer_destroy(_: JNIEnv, _: JClass, h: jlong) {
    if h != 0 {
        drop(unsafe { Box::from_raw(h as *mut Stream) });
    }
}

/// Forget everything fed so far (a seek, a new track).
#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_AutoMixAnalyzer_reset(_: JNIEnv, _: JClass, h: jlong) {
    if let Some(s) = stream(h) {
        s.a.lock().reset();
    }
}

/// Frames fed since create/reset.
#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_AutoMixAnalyzer_frames(_: JNIEnv, _: JClass, h: jlong) -> jlong {
    stream(h).map_or(0, |s| s.a.lock().samples() as jlong)
}

/// Feeds `bytes` bytes of interleaved PCM from `buffer[pos..]` (a direct buffer; read only). False when it cannot.
#[no_mangle]
pub extern "system" fn Java_dev_flint_music_playback_AutoMixAnalyzer_feed(env: JNIEnv, _: JClass, h: jlong, buffer: JByteBuffer, pos: jint, bytes: jint, encoding: jint) -> bool {
    let Ok(src) = env.get_direct_buffer_address(&buffer) else { return false };
    let Some(s) = stream(h) else { return false };
    if src.is_null() || pos < 0 || bytes <= 0 {
        return false;
    }
    let src = unsafe { src.add(pos as usize) };
    let mut a = s.a.lock();
    match encoding {
        PCM_16 if src as usize % 2 == 0 => {
            let x = unsafe { std::slice::from_raw_parts(src as *const i16, bytes as usize / 2) };
            a.feed_interleaved(x, s.channels, |v| v as f32 / 32768.0);
        }
        PCM_FLOAT if src as usize % 4 == 0 => {
            let x = unsafe { std::slice::from_raw_parts(src as *const f32, bytes as usize / 4) };
            a.feed_interleaved(x, s.channels, |v| v);
        }
        _ => return false,
    }
    true
}
