//! The `track_analysis` table and the streaming analyser that feeds it from the playback path. The
//! core's calls around the table, on its own database, are the core's (its automix.rs).

use nori_model::TrackAnalysis;
use nori_player::decode::{Decoder, Fault};
use parking_lot::Mutex;
use rusqlite::{params, Connection, OptionalExtension};

use super::analysis::Analyzer;
use super::ANALYSIS_VERSION;

const COLUMNS: &str = "song_id, analysis_version, duration_ms, bpm, bpm_confidence, beat_offset_ms, stability, downbeat_phase, \
     downbeat_confidence, lufs, key, key_confidence, silence_start_ms, silence_end_ms, mixramp_start_ms, mixramp_end_ms, \
     intro_end_ms, outro_start_ms, outro_vocal, intro_vocal, outro_centroid, intro_centroid, analysed_ms, \
     outro_bpm, outro_bpm_confidence, outro_beat_offset_ms, outro_stability, outro_downbeat_phase, \
     intro_bpm, intro_bpm_confidence, intro_beat_offset_ms, intro_stability, intro_downbeat_phase";

pub fn put(c: &Connection, a: &TrackAnalysis) -> rusqlite::Result<()> {
    c.prepare_cached(&format!(
        "INSERT OR REPLACE INTO track_analysis(server, {COLUMNS}) VALUES(sid(), ?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,?11,?12,?13,?14,?15,?16,?17,?18,?19,?20,?21,?22,?23,?24,?25,?26,?27,?28,?29,?30,?31,?32,?33)"
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
        a.outro_vocal,
        a.intro_vocal,
        a.outro_centroid,
        a.intro_centroid,
        a.analysed_ms,
        a.outro_bpm,
        a.outro_bpm_confidence,
        a.outro_beat_offset_ms,
        a.outro_stability,
        a.outro_downbeat_phase,
        a.intro_bpm,
        a.intro_bpm_confidence,
        a.intro_beat_offset_ms,
        a.intro_stability,
        a.intro_downbeat_phase
    ])
    .map(|_| ())
}

pub fn get(c: &Connection, song_id: &str) -> rusqlite::Result<Option<TrackAnalysis>> {
    c.prepare_cached(&format!("SELECT {COLUMNS} FROM track_analysis WHERE server=sid() AND song_id=?1"))?
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
                outro_vocal: r.get(18).unwrap_or(0.0),
                intro_vocal: r.get(19).unwrap_or(0.0),
                outro_centroid: r.get(20).unwrap_or(0.0),
                intro_centroid: r.get(21).unwrap_or(0.0),
                analysed_ms: r.get(22)?,
                outro_bpm: r.get(23).unwrap_or(0.0),
                outro_bpm_confidence: r.get(24).unwrap_or(0.0),
                outro_beat_offset_ms: r.get(25).unwrap_or(0.0),
                outro_stability: r.get(26).unwrap_or(0.0),
                outro_downbeat_phase: r.get(27).unwrap_or(0),
                intro_bpm: r.get(28).unwrap_or(0.0),
                intro_bpm_confidence: r.get(29).unwrap_or(0.0),
                intro_beat_offset_ms: r.get(30).unwrap_or(0.0),
                intro_stability: r.get(31).unwrap_or(0.0),
                intro_downbeat_phase: r.get(32).unwrap_or(0),
            })
        })
        .optional()
}

/// The ids with no row, or a row from an older analysis version, in the order given.
pub fn missing(c: &Connection, ids: &[String]) -> rusqlite::Result<Vec<String>> {
    let mut st = c.prepare_cached("SELECT analysis_version FROM track_analysis WHERE server=sid() AND song_id=?1")?;
    let mut out = Vec::new();
    for id in ids {
        let v: Option<i32> = st.query_row([id], |r| r.get(0)).optional()?;
        if v.is_none_or(|v| v < ANALYSIS_VERSION) {
            out.push(id.clone());
        }
    }
    Ok(out)
}

/// A song measured as it plays: fed from the playback path buffer by buffer, then finished into the store
/// (the core's `analysis_finish_stream`). It crosses to the platform as a handle, since the platform both feeds
/// it and hands it to the store.
pub struct AnalysisStream {
    a: Mutex<Analyzer>,
    channels: usize,
}

impl AnalysisStream {
    /// `expected_ms` (0 if unknown) sizes the buffers so feeding never reallocates.
    pub fn new(rate: u32, channels: usize, expected_ms: u64) -> Self {
        AnalysisStream { a: Mutex::new(Analyzer::new(rate.max(1), expected_ms)), channels: channels.clamp(1, 8) }
    }

    /// The stream as a handle, for [`AnalysisStream::from_handle`] and the store; freed with
    /// [`AnalysisStream::free_handle`].
    pub fn into_handle(self) -> i64 {
        Box::into_raw(Box::new(self)) as i64
    }

    /// The stream behind a handle; none for 0.
    ///
    /// # Safety
    /// `h` is 0 or a handle [`AnalysisStream::into_handle`] made that has not been freed.
    pub unsafe fn from_handle<'a>(h: i64) -> Option<&'a AnalysisStream> {
        // SAFETY: the caller's promise: a live handle is a boxed stream.
        (h != 0).then(|| unsafe { &*(h as *const AnalysisStream) })
    }

    /// Lets a handle go.
    ///
    /// # Safety
    /// `h` is 0 or a live handle [`AnalysisStream::into_handle`] made, and is not used again.
    pub unsafe fn free_handle(h: i64) {
        if h != 0 {
            // SAFETY: the caller's promise: `h` is a boxed stream nobody else will free.
            drop(unsafe { Box::from_raw(h as *mut AnalysisStream) });
        }
    }

    /// The analyser being fed, for finishing it into the store.
    pub fn analyzer(&self) -> parking_lot::MutexGuard<'_, Analyzer> {
        self.a.lock()
    }

    /// Forget everything fed so far (a seek, a new track).
    pub fn reset(&self) {
        self.a.lock().reset();
    }

    /// Frames fed since it was made or reset.
    pub fn frames(&self) -> u64 {
        self.a.lock().samples()
    }

    /// Interleaved 16-bit samples.
    pub fn feed_i16(&self, x: &[i16]) {
        self.a.lock().feed_interleaved(x, self.channels, |v| v as f32 / 32768.0);
    }

    /// Interleaved float samples.
    pub fn feed_f32(&self, x: &[f32]) {
        self.a.lock().feed_interleaved(x, self.channels, |v| v);
    }

    /// Decodes one packet of a track being measured ahead with `d` and folds the samples in, without their
    /// leaving the decoder's own memory. Returns the frames heard; a stream that comes out at another rate
    /// than the analyser was made for is [`Fault::Broken`], to be measured another way from the start.
    pub fn feed_packet(&self, d: &mut Decoder, packet: &[u8]) -> std::result::Result<usize, Fault> {
        let pcm = d.decode_lent(packet)?;
        let mut a = self.a.lock();
        if a.rate() as u32 != pcm.rate {
            return Err(Fault::Broken);
        }
        a.feed_interleaved(pcm.samples, pcm.channels, |v| v);
        Ok(pcm.samples.len() / pcm.channels.max(1))
    }
}

/// A streaming-analyser handle (the kind `AutoMixAnalyzer.create` returns) around an analyser that was
/// fed elsewhere - by the transition engine's tap - so the store can finish it the same way.
pub fn stream_handle(a: Analyzer, channels: usize) -> i64 {
    AnalysisStream { a: Mutex::new(a), channels: channels.clamp(1, 8) }.into_handle()
}

/// Frees a handle from [`stream_handle`] that never reached anyone.
pub fn free_stream_handle(h: i64) {
    // SAFETY: the handle came from `stream_handle` and reached nobody else.
    unsafe { AnalysisStream::free_handle(h) }
}

/// The stream behind a handle the platform handed the store: 0 or a live handle it was given.
pub fn stream<'a>(h: i64) -> Option<&'a AnalysisStream> {
    // SAFETY: the platform hands the store 0 or a live handle it was given.
    unsafe { AnalysisStream::from_handle(h) }
}
