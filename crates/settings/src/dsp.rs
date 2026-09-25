//! The sound settings' answers the platform asks for: the pre-amp in effect, the built-in curves and which
//! parts of the chain may run. The chain itself (`nori_player::dsp`) runs inside the player, which is
//! handed the settings as they change.

pub use nori_player::dsp::*;

/// The pre-amp in effect: the one set, or the automatic one for these bands; none with the equalizer off.
pub fn effective_preamp_db(s: &crate::settings::StoredPrefs) -> f32 {
    if !s.eq_enabled {
        return 0.0;
    }
    s.eq_preamp_db.unwrap_or_else(|| nori_player::dsp::auto_preamp_db(s.eq_bands.iter().map(|b| (b.kind, b.gain_db))))
}

/// The built-in curves, as data, so the UI (and the settings store) never holds a frequency of its own.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_presets() -> Vec<nori_model::NamedPreset> {
    nori_player::dsp::eq_presets()
}

/// Which parts of the chain may run, from the settings and the output; see `nori_player::policy`.
pub fn audio_policy(prefs: nori_model::AudioPrefs, output: nori_model::OutputState) -> nori_model::AudioPolicy {
    nori_player::policy::audio_policy(&prefs, &output)
}
