//! The sound chain the settings ask for (`nori_player::dsp`), run once per audio buffer by the platform's
//! output: a [`SoundChain`] follows the settings by itself, so nothing crosses from the platform but the
//! samples. It must not allocate, copy or serialise anything on the process path.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

use parking_lot::{Mutex, RwLock};

pub use nori_player::dsp::*;

/// One output's equalizer, set up again from the settings on its next buffer whenever they change. Shared
/// between the audio thread, which processes, and the UI, which reads the limiter's meter: the meter lives
/// outside the lock so the UI can poll it without ever waiting on the playback thread.
pub struct SoundChain {
    eq: Mutex<Equalizer>,
    reduction_db: AtomicU32,
    /// The [`CHAIN_GEN`] this equalizer was last set up for.
    applied: AtomicU64,
}

impl SoundChain {
    pub fn new(rate: u32, channels: usize) -> Self {
        let eq = Mutex::new(Equalizer::new(rate.max(1), channels.max(1)));
        SoundChain { eq, reduction_db: AtomicU32::new(0), applied: AtomicU64::new(0) }
    }

    /// The limiter meter: peak gain reduction in dB in the last buffer, 0 when it is off or idle.
    /// Lock-free, poll freely.
    pub fn gain_reduction_db(&self) -> f32 {
        f32::from_bits(self.reduction_db.load(Ordering::Relaxed))
    }

    /// How many frames the chain holds back (the limiter's look-ahead), as set up for its latest buffer.
    pub fn delay_frames(&self) -> usize {
        self.eq.lock().delay_frames()
    }

    /// A new stream: the filters' memory goes, and the settings are read again.
    pub fn reset(&self) {
        self.eq.lock().reset();
        self.applied.store(0, Ordering::Relaxed);
    }

    /// Filters interleaved 16-bit samples from `input` into `output` (the same length).
    pub fn process_i16(&self, input: &[i16], output: &mut [i16]) {
        let mut eq = self.eq.lock();
        follow_chain(&mut eq, &self.applied);
        eq.process_i16(input, output);
        self.reduction_db.store(eq.gain_reduction_db().to_bits(), Ordering::Relaxed);
    }

    /// Filters interleaved float samples from `input` into `output` (the same length).
    pub fn process_f32(&self, input: &[f32], output: &mut [f32]) {
        let mut eq = self.eq.lock();
        follow_chain(&mut eq, &self.applied);
        eq.process_f32(input, output);
        self.reduction_db.store(eq.gain_reduction_db().to_bits(), Ordering::Relaxed);
    }
}

/// The most bands a chain carries: the parametric editor's own limit, with room to spare.
const MAX_BANDS: usize = 64;

/// The sound chain the settings ask for, kept here so the audio thread picks a change up by itself: the
/// settings store rebuilds it on every change ([`settings_changed`]) and bumps [`CHAIN_GEN`]; each handle
/// sets its equalizer up again on its next buffer when the generation moved. Nothing crosses from the
/// platform, and the audio thread reads this only when something did change.
struct Chain {
    bands: [Band; MAX_BANDS],
    count: usize,
    preamp_db: f64,
    crossfeed_db: f64,
    balance: f64,
    mono: bool,
    threshold_db: f64,
    release_ms: f64,
    lookahead_ms: f64,
}

const NO_BAND: Band = Band { kind: 0, freq: 0.0, gain_db: 0.0, q: 0.0, channel: 0 };
static CHAIN: RwLock<Chain> = RwLock::new(Chain {
    bands: [NO_BAND; MAX_BANDS],
    count: 0,
    preamp_db: 0.0,
    crossfeed_db: 0.0,
    balance: 0.0,
    mono: false,
    threshold_db: -1.0,
    release_ms: 100.0,
    lookahead_ms: 0.0,
});
static CHAIN_GEN: AtomicU64 = AtomicU64::new(1);

/// The limiter's release, and its lookahead when it is on.
const LIMITER_RELEASE_MS: f64 = 120.0;
const LIMITER_LOOKAHEAD_MS: f64 = 5.0;

/// The pre-amp in effect: the one set, or the automatic one for these bands; none with the equalizer off.
pub fn effective_preamp_db(s: &crate::settings::StoredPrefs) -> f32 {
    if !s.eq_enabled {
        return 0.0;
    }
    s.eq_preamp_db.unwrap_or_else(|| nori_player::dsp::auto_preamp_db(s.eq_bands.iter().map(|b| (b.kind, b.gain_db))))
}

/// The settings changed: the chain they ask for, for every handle's next buffer.
pub(crate) fn settings_changed(s: &crate::settings::StoredPrefs) {
    let mut c = CHAIN.write();
    let bands = if s.eq_enabled { &s.eq_bands[..s.eq_bands.len().min(MAX_BANDS)] } else { &[][..] };
    let mut next = Chain {
        bands: [NO_BAND; MAX_BANDS],
        count: bands.len(),
        preamp_db: effective_preamp_db(s) as f64,
        crossfeed_db: s.crossfeed_db as f64,
        balance: s.balance as f64,
        mono: s.mono,
        threshold_db: s.limiter_threshold_db as f64,
        release_ms: LIMITER_RELEASE_MS,
        lookahead_ms: if s.limiter { LIMITER_LOOKAHEAD_MS } else { 0.0 },
    };
    for (i, b) in bands.iter().enumerate() {
        next.bands[i] = Band { kind: b.kind, freq: b.freq as f64, gain_db: b.gain_db as f64, q: b.q as f64, channel: b.channel };
    }
    let same = c.count == next.count
        && c.bands[..c.count].iter().zip(&next.bands[..next.count]).all(|(a, b)| (a.kind, a.freq, a.gain_db, a.q, a.channel) == (b.kind, b.freq, b.gain_db, b.q, b.channel))
        && (c.preamp_db, c.crossfeed_db, c.balance, c.mono, c.threshold_db, c.release_ms, c.lookahead_ms)
            == (next.preamp_db, next.crossfeed_db, next.balance, next.mono, next.threshold_db, next.release_ms, next.lookahead_ms);
    if !same {
        *c = next;
        CHAIN_GEN.fetch_add(1, Ordering::Release);
    }
}

/// Sets `eq` up for the chain the settings ask for, if it changed since `applied`.
fn follow_chain(eq: &mut Equalizer, applied: &AtomicU64) {
    let g = CHAIN_GEN.load(Ordering::Acquire);
    if applied.load(Ordering::Relaxed) == g {
        return;
    }
    let c = CHAIN.read();
    eq.configure(&c.bands[..c.count], c.preamp_db, c.crossfeed_db);
    eq.configure_output(c.balance, c.mono, c.threshold_db, c.release_ms, c.lookahead_ms);
    applied.store(g, Ordering::Relaxed);
}

/// The built-in curves, as data, so the UI (and the settings store) never holds a frequency of its own.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn eq_presets() -> Vec<nori_model::NamedPreset> {
    nori_player::dsp::eq_presets()
}

/// Which parts of the chain may run, from the settings and the output; see `nori_player::policy`.
/// What the output did with the settings last time, so the next change can tell what moved.
struct Applied {
    speed: f32,
    pitch: f32,
    offload: bool,
    processor: bool,
}

static APPLIED: Mutex<Applied> = Mutex::new(Applied { speed: 1.0, pitch: 1.0, offload: false, processor: false });

/// Whether the output wants compressed audio handed to the chip, as last worked out ([`audio_apply`]).
pub(crate) fn offload_wanted() -> bool {
    APPLIED.lock().offload
}

/// Everything the platform applies to its player when the settings or the output change, in one call.
#[derive(Debug, Clone)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct AudioApply {
    pub policy: nori_model::AudioPolicy,
    pub speed: f32,
    pub pitch: f32,
    /// Whether that rebuilds the output now, at the next song boundary or not at all.
    pub rebuild: nori_model::Rebuild,
}

/// The settings (the core's own) against the output as it is now (`offloaded`: the chip is decoding the
/// song playing): which parts of the chain may run (`nori_player::policy`) and whether the output must be
/// rebuilt for it (`nori_player::transport::rebuild`). The sound chain itself follows the settings by
/// itself ([`settings_changed`]).
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn audio_apply(output: nori_model::OutputState, offloaded: bool) -> AudioApply {
    let s = crate::settings_store::current().unwrap_or_default();
    let prefs = nori_model::AudioPrefs {
        dsp: nori_player::sound::sound_on(s.eq_enabled, s.crossfeed_db, s.balance, s.mono, s.limiter),
        skip_silence: s.skip_silence,
        offload: s.offload,
        crossfade_s: s.crossfade_sec,
        auto_mix: s.auto_mix,
        speed: s.speed,
        pitch: s.pitch,
    };
    let policy = nori_player::policy::audio_policy(&prefs, &output);
    let mut a = APPLIED.lock();
    let change = nori_model::ChainChange {
        offloaded,
        offload: policy.offload,
        offload_changed: a.offload != policy.offload,
        usb: output.usb,
        offload_refused: output.offload_refused,
        tempo_changed: a.speed != s.speed || a.pitch != s.pitch,
        processor_changed: a.processor != policy.processor_in_chain,
    };
    *a = Applied { speed: s.speed, pitch: s.pitch, offload: policy.offload, processor: policy.processor_in_chain };
    AudioApply { policy, speed: s.speed, pitch: s.pitch, rebuild: nori_player::transport::rebuild(change) }
}

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

/// The dip around a switch made while music plays; `None`: switch at once. See `nori_player::transport`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn switch_dip(fade_ms: i32, switch: nori_model::Switch, playing: bool) -> Option<nori_model::Dip> {
    nori_player::transport::switch_dip(fade_ms, switch, playing)
}

/// Pressing play: fade in over this long; `None` to start at full volume.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn play_fade(fade_ms: i32, playing: bool) -> Option<i32> {
    nori_player::transport::play_fade(fade_ms, playing)
}

/// Pressing pause: fade out over this long first; `None` to pause at once.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn pause_fade(fade_ms: i32, playing: bool) -> Option<i32> {
    nori_player::transport::pause_fade(fade_ms, playing)
}

/// Whether a settings change must rebuild the output, and when.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn sink_rebuild(change: nori_model::ChainChange) -> nori_model::Rebuild {
    nori_player::transport::rebuild(change)
}

/// What an output device gets as music moves to it; see `nori_player::device`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn device_arrival(arrival: nori_model::Arrival) -> nori_model::ArrivalPlan {
    nori_player::device::on_arrival(arrival)
}

/// How deep the output buffer is made so it can be fed in bursts; see `nori_player::burst`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn burst_buffer_us() -> i64 {
    nori_player::burst::BUFFER_US
}

/// Which of a DAC's modes plays a song untouched, or why none can; see `nori_player::dac`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn dac_choice(enabled: bool, modes: Vec<nori_model::DacMode>, playing: nori_model::DacMode) -> nori_model::DacChoice {
    nori_player::dac::choose(enabled, &modes, playing)
}

#[cfg(test)]
mod chain_tests {
    use super::*;

    #[test]
    fn a_settings_change_reaches_the_equalizer_on_its_next_buffer() {
        let mut s = crate::settings::StoredPrefs::default();
        s.eq_enabled = true;
        s.eq_bands = vec![crate::settings::SoundBand { kind: 0, freq: 1000.0, gain_db: 6.0, q: 1.0, channel: 0 }];
        s.eq_preamp_db = Some(-3.0);
        settings_changed(&s);
        let g = CHAIN_GEN.load(Ordering::Acquire);
        settings_changed(&s);
        assert_eq!(CHAIN_GEN.load(Ordering::Acquire), g, "the same chain again is no change");
        {
            let c = CHAIN.read();
            assert_eq!((c.count, c.preamp_db, c.lookahead_ms), (1, -3.0, 0.0));
        }
        let mut eq = Equalizer::new(48_000, 2);
        let applied = AtomicU64::new(0);
        follow_chain(&mut eq, &applied);
        assert_eq!(applied.load(Ordering::Relaxed), g);
        s.eq_enabled = false;
        settings_changed(&s);
        assert_eq!(CHAIN.read().count, 0, "the equalizer off leaves no bands, and no pre-amp");
        assert_eq!(CHAIN.read().preamp_db, 0.0);
    }
}
