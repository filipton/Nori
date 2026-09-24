//! AutoMix is nori-automix's; here are the analysis store's calls on the core's own database, and the
//! calls that finish a measurement into it.

use nori_automix::store::{get, missing, put, stream};

use crate::{Core, Result, TrackAnalysis};

pub use nori_automix::*;

#[cfg(test)]
mod tests;

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    pub fn analysis_get(&self, song_id: String) -> Result<Option<TrackAnalysis>> {
        Ok(get(&self.db.lock(), &song_id)?)
    }

    /// Which of `song_ids` still need analysing (no row, or an older analysis version); ids that can
    /// never be measured (a provider's song, a radio stream) are left out.
    pub fn analysis_missing(&self, song_ids: Vec<String>) -> Result<Vec<String>> {
        let song_ids: Vec<String> = song_ids.into_iter().filter(|id| crate::queue::analysable(id)).collect();
        Ok(missing(&self.db.lock(), &song_ids)?)
    }

    /// Forgets every analysis, so tracks are measured again as they play. For when the measurements look wrong.
    pub fn analysis_clear(&self) -> Result<u32> {
        let c = self.db.lock();
        let n = c.execute("DELETE FROM track_analysis WHERE server=sid()", [])? as u32;
        Ok(n)
    }

    /// How many tracks have an analysis, for the settings screen.
    pub fn analysis_count(&self) -> Result<u32> {
        Ok(self.db.lock().query_row("SELECT count(*) FROM track_analysis WHERE server=sid()", [], |r| r.get(0))?)
    }

    pub fn analysis_store(&self, analysis: TrackAnalysis) -> Result<()> {
        Ok(put(&self.db.lock(), &analysis)?)
    }

    /// Analyses a whole decoded track, stores and returns the result. `pcm` is interleaved little-endian PCM as the
    /// decoder hands it out (`encoding` 2 = 16-bit, 4 = float), any rate and channel count. Blocking, about 0.14
    /// CPU-seconds for 4 minutes on a desktop; the database is only locked for the write.
    pub fn analysis_run(&self, song_id: String, pcm: Vec<u8>, sample_rate: i32, channels: i32, encoding: i32) -> Result<TrackAnalysis> {
        let a = analyse_bytes(&song_id, &pcm, sample_rate, channels, encoding).track;
        put(&self.db.lock(), &a)?;
        Ok(a)
    }

    /// [`Core::analysis_finish_stream`], but only for a whole song: `expected_ms` is the song's length as
    /// the server has it (0 unknown), and a measurement that heard much less or more is dropped.
    pub fn analysis_finish_whole(&self, song_id: String, handle: i64, expected_ms: i64) -> Result<Option<TrackAnalysis>> {
        let Some(s) = stream(handle) else { return Ok(None) };
        let heard_ms = {
            let a = s.analyzer();
            (a.samples() as f64 * 1000.0 / a.rate()) as i64
        };
        if !nori_player::transitions::whole_song(heard_ms, expected_ms) {
            s.analyzer().reset();
            return Ok(None);
        }
        self.analysis_finish_stream(song_id, handle)
    }

    /// Finishes a JNI streaming analyser (`AutoMixAnalyzer.create`) that was fed one whole track from its first
    /// sample, stores and returns the result. `None` when it holds less than 30 s. The handle is emptied, not
    /// freed: feed the next track into it or `destroy` it.
    pub fn analysis_finish_stream(&self, song_id: String, handle: i64) -> Result<Option<TrackAnalysis>> {
        let Some(s) = stream(handle) else { return Ok(None) };
        let f = {
            let mut a = s.analyzer();
            if a.samples() < (a.rate() * 30.0) as u64 {
                a.reset();
                return Ok(None);
            }
            a.take_features()
        };
        let a = finish(&song_id, &f).track;
        put(&self.db.lock(), &a)?;
        Ok(Some(a))
    }
}
