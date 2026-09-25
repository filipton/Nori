//! The sound settings' answers the platform asks for: the pre-amp in effect, the built-in curves, which
//! parts of the chain may run, ReplayGain's volume, what an output device gets and a DAC's mode. The
//! chain itself (`nori_player::dsp`) runs inside the player, which is handed the settings as they change.

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
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn audio_policy(prefs: nori_model::AudioPrefs, output: nori_model::OutputState) -> nori_model::AudioPolicy {
    nori_player::policy::audio_policy(&prefs, &output)
}

/// The volume a track plays at under ReplayGain, 0..1; see `nori_player::policy::replay_gain`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn replay_gain_volume(
    mode: nori_model::GainMode, tags: Option<nori_model::GainTags>, in_album_run: bool, preamp_db: f32, untagged_db: f32, radio: bool, bit_perfect: bool,
) -> f32 {
    nori_player::policy::replay_gain(mode, tags.as_ref(), in_album_run, preamp_db, untagged_db, radio, bit_perfect)
}

/// What an output device gets as music moves to it; see `nori_player::device`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn device_arrival(arrival: nori_model::Arrival) -> nori_model::ArrivalPlan {
    nori_player::device::on_arrival(arrival)
}

/// Which of a DAC's modes plays a song untouched, or why none can; see `nori_player::dac`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn dac_choice(enabled: bool, modes: Vec<nori_model::DacMode>, playing: nori_model::DacMode) -> nori_model::DacChoice {
    nori_player::dac::choose(enabled, &modes, playing)
}
