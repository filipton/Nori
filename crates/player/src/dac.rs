//! Bit-perfect output to a USB DAC: which of the DAC's modes plays a song untouched, or why none can.
//! The platform lists the modes and applies the choice.

/// One of the DAC's modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DacMode {
    pub rate: u32,
    pub bits: u32,
    /// Float samples: 32 bits, but not the same mode as 32-bit integers.
    pub float: bool,
}

/// Which mode to use (`use_index` into the modes, -1 for none), and why none, when that is worth saying.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DacChoice {
    pub use_index: i32,
    pub blocked_by: Option<String>,
}

/// "44.1 kHz", in the phone's number style (`nori_text`): "44,1 kHz" on a Polish phone, as it was.
pub fn khz(rate: u32) -> String {
    nori_text::fixed(rate as f64 / 1000.0, 1, false) + " kHz"
}

/// The mode that plays a song of `rate` and `bits` exactly. Off, a DAC with no modes, or nothing
/// playing yet: no choice and nothing to explain.
pub fn choose(enabled: bool, modes: &[DacMode], playing: DacMode) -> DacChoice {
    let DacMode { rate, .. } = playing;
    if !enabled || modes.is_empty() || rate == 0 {
        return DacChoice { use_index: -1, blocked_by: None };
    }
    if let Some(i) = modes.iter().position(|m| *m == playing) {
        return DacChoice { use_index: i as i32, blocked_by: None };
    }
    let same_rate: Vec<u32> = modes.iter().filter(|m| m.rate == rate).map(|m| m.bits).collect();
    let why = if same_rate.is_empty() {
        format!("this DAC has no bit-perfect mode at {}", khz(rate))
    } else {
        // The rate is there, but only in a sample format the output path cannot write: Android's own
        // path writes 16-bit or float and nothing else. The way in is a USB exclusive driver.
        let wants: Vec<String> = same_rate.iter().map(|b| format!("{b} bit")).collect();
        format!("this DAC wants {} at {}, which needs USB exclusive output (not built yet)", wants.join(" or "), khz(rate))
    };
    DacChoice { use_index: -1, blocked_by: Some(why) }
}

/// "44.1 kHz / 24 bit": one of the DAC's modes, or what is playing, as the user reads it.
pub fn label(rate: u32, bits: u32) -> String {
    format!("{} / {bits} bit", khz(rate))
}

/// What the AudioTrack was actually opened with, and whether it was offloaded: the honest answer.
pub fn track_line(rate: u32, bits: u32, offloaded: bool) -> String {
    format!("{}{}", label(rate, bits), if offloaded { ", offloaded to the audio chip" } else { "" })
}

/// What the platform does with the DAC after a decision.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DacStep {
    /// Clear the preferred mode: none is usable.
    Release,
    /// The mode already applied is the one wanted; leave everything as it is.
    Keep,
    /// Clear what is applied and ask for this mode (an index into the modes).
    Prefer { index: u32 },
}

/// The whole decision about a DAC, and what the user is shown about it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DacDecision {
    pub step: DacStep,
    /// The name shown for the DAC.
    pub device: String,
    /// The device offers at least one bit-perfect mode.
    pub supported: bool,
    /// Every bit-perfect mode the device offers, as "44.1 kHz / 24 bit".
    pub modes: Vec<String>,
    /// The format currently being played, for the diagnostic line.
    pub playing: Option<String>,
    /// The bits of what is playing.
    pub bits: u32,
    /// Why nothing is bit-perfect, when that is worth saying.
    pub blocked_by: Option<String>,
    /// What to say when the platform refuses the mode it was asked for.
    pub refused: String,
}

/// Everything about a DAC that is attached. `platform_ok` is false where the platform cannot drive a DAC
/// bit-perfect at all (Android before 14). `applied` is the list of modes of the port the preferred mode
/// is held for, when it is this same port; with `was_bit_perfect` it says whether the wanted mode is
/// already in place. Every step but `Keep` clears the preferred mode first: a mode left pointing at a
/// format the output will not be opened with is how this ends up routed somewhere silent.
pub fn decide(
    enabled: bool, platform_ok: bool, name: &str, modes: &[DacMode], playing: DacMode, applied: Option<&[DacMode]>, was_bit_perfect: bool,
) -> DacDecision {
    let device = if name.trim().is_empty() { "USB DAC".to_string() } else { name.trim().to_string() };
    let shown = (playing.rate != 0).then(|| label(playing.rate, playing.bits));
    let refused = "the system refused the preferred mixer attributes".to_string();
    let base = DacDecision { step: DacStep::Release, device, supported: false, modes: Vec::new(), playing: shown, bits: playing.bits, blocked_by: None, refused };
    if !platform_ok {
        return DacDecision { blocked_by: Some("bit-perfect output needs Android 14 or newer".to_string()), ..base };
    }
    let labels: Vec<String> = modes.iter().map(|m| label(m.rate, m.bits)).collect();
    let choice = choose(enabled, modes, playing);
    let Some(wanted) = usize::try_from(choice.use_index).ok().and_then(|i| modes.get(i)) else {
        return DacDecision { supported: !modes.is_empty(), modes: labels, blocked_by: choice.blocked_by, ..base };
    };
    let step = if was_bit_perfect && applied.is_some_and(|a| a.contains(wanted)) { DacStep::Keep } else { DacStep::Prefer { index: choice.use_index as u32 } };
    DacDecision { step, supported: true, modes: labels, ..base }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_read_like_the_settings_screen() {
        assert_eq!(label(44_100, 16), "44.1 kHz / 16 bit");
        assert_eq!(track_line(96_000, 32, true), "96.0 kHz / 32 bit, offloaded to the audio chip");
        assert_eq!(track_line(48_000, 16, false), "48.0 kHz / 16 bit");
    }

    #[test]
    fn a_decision_names_the_modes_and_what_is_playing() {
        let d = decide(true, true, " K3 ", &MODES, m(48_000, 24), None, false);
        assert_eq!(d.step, DacStep::Prefer { index: 1 });
        assert_eq!(d.device, "K3");
        assert!(d.supported);
        assert_eq!(d.modes, ["44.1 kHz / 16 bit", "48.0 kHz / 24 bit", "96.0 kHz / 32 bit"]);
        assert_eq!(d.playing.as_deref(), Some("48.0 kHz / 24 bit"));
        assert_eq!(d.bits, 24);
        assert_eq!(d.blocked_by, None);
        assert_eq!(decide(true, true, "", &MODES, m(0, 16), None, false).device, "USB DAC");
    }

    #[test]
    fn the_mode_in_place_is_kept() {
        assert_eq!(decide(true, true, "K3", &MODES, m(48_000, 24), Some(&MODES), true).step, DacStep::Keep);
        assert_eq!(decide(true, true, "K3", &MODES, m(48_000, 24), Some(&MODES), false).step, DacStep::Prefer { index: 1 }, "not bit-perfect yet");
        assert_eq!(decide(true, true, "K3", &MODES, m(48_000, 24), Some(&MODES[..1]), true).step, DacStep::Prefer { index: 1 }, "another mode applied");
    }

    #[test]
    fn nothing_usable_releases_and_says_why() {
        let d = decide(true, true, "K3", &MODES, m(88_200, 16), None, false);
        assert_eq!(d.step, DacStep::Release);
        assert!(d.supported);
        assert_eq!(d.modes.len(), 3);
        assert_eq!(d.blocked_by.as_deref(), Some("this DAC has no bit-perfect mode at 88.2 kHz"));
        let old = decide(true, false, "K3", &MODES, m(44_100, 16), None, false);
        assert_eq!(old.step, DacStep::Release);
        assert!(!old.supported && old.modes.is_empty());
        assert_eq!(old.blocked_by.as_deref(), Some("bit-perfect output needs Android 14 or newer"));
        assert_eq!(old.playing.as_deref(), Some("44.1 kHz / 16 bit"));
        let none = decide(true, true, "K3", &[], m(44_100, 16), None, false);
        assert!(!none.supported && none.blocked_by.is_none());
    }

    const fn m(rate: u32, bits: u32) -> DacMode {
        DacMode { rate, bits, float: false }
    }

    const MODES: [DacMode; 3] = [m(44_100, 16), m(48_000, 24), m(96_000, 32)];

    #[test]
    fn the_exact_mode_is_used() {
        assert_eq!(choose(true, &MODES, m(44_100, 16)), DacChoice { use_index: 0, blocked_by: None });
        assert_eq!(choose(true, &MODES, m(96_000, 32)).use_index, 2);
        assert_eq!(choose(true, &MODES, DacMode { float: true, ..m(96_000, 32) }).use_index, -1, "float is not 32-bit integer");
    }

    #[test]
    fn a_missing_mode_says_why() {
        assert_eq!(choose(true, &MODES, m(88_200, 16)).blocked_by.as_deref(), Some("this DAC has no bit-perfect mode at 88.2 kHz"));
        let why = choose(true, &MODES, m(48_000, 16)).blocked_by.unwrap();
        assert!(why.starts_with("this DAC wants 24 bit at 48.0 kHz"), "{why}");
    }

    #[test]
    fn off_or_idle_decides_nothing() {
        assert_eq!(choose(false, &MODES, m(44_100, 16)).use_index, -1);
        assert_eq!(choose(true, &[], m(44_100, 16)), DacChoice { use_index: -1, blocked_by: None });
        assert_eq!(choose(true, &MODES, m(0, 16)).blocked_by, None);
    }
}
