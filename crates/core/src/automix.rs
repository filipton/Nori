//! AutoMix is nori-automix's; here are the analysis store's calls on the core's own database, and the
//! calls that finish a measurement into it.

use nori_automix::beats::{EndGrid, MixEnd};
use nori_automix::analysis::Analyzer;
use nori_automix::store::{get, get_voice, missing, neural_missing, put, put_measured, put_voice, AnalysisStream};

use crate::{Core, Result, TrackAnalysis};

pub use nori_automix::*;

#[cfg(test)]
mod tests;

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    pub fn analysis_get(&self, song_id: String) -> Result<Option<TrackAnalysis>> {
        Ok(get(&self.db.lock(), &song_id)?)
    }

    /// Forgets every analysis, so tracks are measured again as they play. For when the measurements look wrong.
    pub fn analysis_clear(&self) -> Result<u32> {
        let c = self.db.lock();
        let n = c.execute("DELETE FROM track_analysis WHERE server=sid()", [])? as u32;
        c.execute("DELETE FROM vocal_curve WHERE server=sid()", [])?;
        Ok(n)
    }

    /// How many tracks have an analysis, for the settings screen.
    pub fn analysis_count(&self) -> Result<u32> {
        Ok(self.db.lock().query_row("SELECT count(*) FROM track_analysis WHERE server=sid()", [], |r| r.get(0))?)
    }
}

/// Asked only in Rust, so not exported to Kotlin.
impl Core {
    /// Which of `song_ids` still need analysing (no row, or an older analysis version); ids that can
    /// never be measured (a provider's song, a radio stream) are left out.
    pub fn analysis_missing(&self, song_ids: Vec<String>) -> Result<Vec<String>> {
        let song_ids: Vec<String> = song_ids.into_iter().filter(|id| crate::queue::analysable(id)).collect();
        Ok(missing(&self.db.lock(), &song_ids)?)
    }
}

/// Finishing a measurement into the store, for nori-engine's measurer.
impl Core {
    /// Finishes `stream`, fed one whole song from its first sample, into the store: `expected_ms` is the song's
    /// length as the server has it (0 unknown), and a measurement that heard much less or more is dropped.
    pub fn analysis_finish_whole(&self, song_id: &str, stream: AnalysisStream, expected_ms: i64) -> Result<Option<TrackAnalysis>> {
        let a = stream.into_analyzer();
        let heard_ms = (a.samples() as f64 * 1000.0 / a.rate()) as i64;
        if !nori_player::transitions::whole_song(heard_ms, expected_ms) {
            return Ok(None);
        }
        self.analysis_finish(song_id, a)
    }

    /// Finishes an analyser fed one whole track from its first sample, stores and returns the result. `None` when
    /// it holds less than 30 s.
    pub fn analysis_finish(&self, song_id: &str, mut a: Analyzer) -> Result<Option<TrackAnalysis>> {
        if a.samples() < (a.rate() * 30.0) as u64 {
            return Ok(None);
        }
        let f = a.take_features();
        let a = finish(song_id, &f).track;
        let stored = {
            let c = self.db.lock();
            let stored = put_measured(&c, a)?;
            put_voice(&c, song_id, &f.voice_curve())?;
            stored
        };
        nori_automix::planner::analyses_changed();
        Ok(Some(stored))
    }

    /// The song's vocal activity curve, measured with its analysis: what synced lyrics are checked against.
    pub fn analysis_voice(&self, song_id: &str) -> Result<Option<nori_player::automix::vocal::VocalCurve>> {
        Ok(get_voice(&self.db.lock(), song_id)?)
    }
}

/// Beat This!'s side of the store, for nori-engine's measurer; nothing here runs the model.
impl Core {
    /// Which of `song_ids` have a current analysis with an end the beat model has not looked at yet, in the order
    /// given. Songs with no current analysis are left out: they need measuring first (`analysis_missing`).
    pub fn analysis_neural_missing(&self, song_ids: Vec<String>) -> Result<Vec<String>> {
        let song_ids: Vec<String> = song_ids.into_iter().filter(|id| crate::queue::analysable(id)).collect();
        Ok(neural_missing(&self.db.lock(), &song_ids)?)
    }

    /// What the model found at `end` of `song_id` (`None`: nothing it was sure of, or too little music there):
    /// the grid replaces the stored one at that end when it is confident, and the end is marked as looked at
    /// either way. Whether the grid was adopted; false with no analysis to add to.
    pub fn analysis_neural_store(&self, song_id: &str, end: MixEnd, grid: Option<EndGrid>) -> Result<bool> {
        let c = self.db.lock();
        let Some(mut row) = get(&c, song_id)? else { return Ok(false) };
        let adopted = nori_automix::beats::merge(&mut row, end, grid);
        put(&c, &row)?;
        if adopted {
            nori_automix::planner::analyses_changed();
        }
        Ok(adopted)
    }
}
