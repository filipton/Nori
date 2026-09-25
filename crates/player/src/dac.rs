//! Bit-perfect output to a USB DAC: which of the DAC's modes plays a song untouched, or why none can.
//! The platform lists the modes and applies the choice; the client words what is decided here.

/// One of the DAC's modes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DacMode {
    pub rate: u32,
    pub bits: u32,
    /// Float samples: 32 bits, but not the same mode as 32-bit integers.
    pub float: bool,
}

/// Why no mode plays a song bit-perfect, for the client to word. Copy, and nothing in it allocates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DacBlock {
    /// The DAC has no bit-perfect mode at the song's `rate`.
    NoModeAtRate { rate: u32 },
    /// The DAC has `rate`, but only in sample formats the platform's output path cannot write (Android's
    /// writes 16-bit or float and nothing else); the way in is a USB exclusive driver, not built yet.
    /// `depths` is a set of bit depths: [`DEPTH_16`], [`DEPTH_24`], [`DEPTH_32`].
    NeedsExclusive { rate: u32, depths: u8 },
    /// The platform cannot drive a DAC bit-perfect at all (Android before 14).
    PlatformTooOld,
    /// The platform refused the mode it was asked for.
    Refused,
}

/// Bit depths in [`DacBlock::NeedsExclusive`]'s set.
pub const DEPTH_16: u8 = 1;
pub const DEPTH_24: u8 = 2;
pub const DEPTH_32: u8 = 4;

/// A bit depth as a member of the set in [`DacBlock::NeedsExclusive`]; 0 for one that is none of them.
pub fn depth_bit(bits: u32) -> u8 {
    match bits {
        16 => DEPTH_16,
        24 => DEPTH_24,
        32 => DEPTH_32,
        _ => 0,
    }
}

/// Which mode to use (`use_index` into the modes, -1 for none), and why none, when that is worth saying.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DacChoice {
    pub use_index: i32,
    pub blocked_by: Option<DacBlock>,
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
    let why = if modes.iter().any(|m| m.rate == rate) {
        let depths = modes.iter().filter(|m| m.rate == rate).fold(0u8, |set, m| set | depth_bit(m.bits));
        DacBlock::NeedsExclusive { rate, depths }
    } else {
        DacBlock::NoModeAtRate { rate }
    };
    DacChoice { use_index: -1, blocked_by: Some(why) }
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

/// The whole decision about a DAC, and the facts the user is shown about it (the client words them; the
/// platform refusing the mode it was asked for is [`DacBlock::Refused`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DacDecision {
    pub step: DacStep,
    /// The DAC's name as it gives it, trimmed; none when it gives none (the client names it then).
    pub device: Option<String>,
    /// The device offers at least one bit-perfect mode.
    pub supported: bool,
    /// Every bit-perfect mode the device offers.
    pub modes: Vec<DacMode>,
    /// The format currently being played, for the diagnostic line.
    pub playing: Option<DacMode>,
    /// The bits of what is playing.
    pub bits: u32,
    /// Why nothing is bit-perfect, when that is worth saying.
    pub blocked_by: Option<DacBlock>,
}

/// Everything about a DAC that is attached. `platform_ok` is false where the platform cannot drive a DAC
/// bit-perfect at all (Android before 14). `applied` is the list of modes of the port the preferred mode
/// is held for, when it is this same port; with `was_bit_perfect` it says whether the wanted mode is
/// already in place. Every step but `Keep` clears the preferred mode first: a mode left pointing at a
/// format the output will not be opened with is how this ends up routed somewhere silent.
pub fn decide(
    enabled: bool, platform_ok: bool, name: &str, modes: &[DacMode], playing: DacMode, applied: Option<&[DacMode]>, was_bit_perfect: bool,
) -> DacDecision {
    let device = Some(name.trim()).filter(|n| !n.is_empty()).map(str::to_string);
    let shown = (playing.rate != 0).then_some(playing);
    let base = DacDecision { step: DacStep::Release, device, supported: false, modes: Vec::new(), playing: shown, bits: playing.bits, blocked_by: None };
    if !platform_ok {
        return DacDecision { blocked_by: Some(DacBlock::PlatformTooOld), ..base };
    }
    let choice = choose(enabled, modes, playing);
    let Some(wanted) = usize::try_from(choice.use_index).ok().and_then(|i| modes.get(i)) else {
        return DacDecision { supported: !modes.is_empty(), modes: modes.to_vec(), blocked_by: choice.blocked_by, ..base };
    };
    let step = if was_bit_perfect && applied.is_some_and(|a| a.contains(wanted)) { DacStep::Keep } else { DacStep::Prefer { index: choice.use_index as u32 } };
    DacDecision { step, supported: true, modes: modes.to_vec(), ..base }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_decision_gives_the_modes_and_what_is_playing() {
        let d = decide(true, true, " K3 ", &MODES, m(48_000, 24), None, false);
        assert_eq!(d.step, DacStep::Prefer { index: 1 });
        assert_eq!(d.device.as_deref(), Some("K3"));
        assert!(d.supported);
        assert_eq!(d.modes, MODES);
        assert_eq!(d.playing, Some(m(48_000, 24)));
        assert_eq!(d.bits, 24);
        assert_eq!(d.blocked_by, None);
        assert_eq!(decide(true, true, "  ", &MODES, m(0, 16), None, false).device, None, "a nameless DAC is the client's to name");
        assert_eq!(decide(true, true, "K3", &MODES, m(0, 16), None, false).playing, None);
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
        assert_eq!(d.blocked_by, Some(DacBlock::NoModeAtRate { rate: 88_200 }));
        let old = decide(true, false, "K3", &MODES, m(44_100, 16), None, false);
        assert_eq!(old.step, DacStep::Release);
        assert!(!old.supported && old.modes.is_empty());
        assert_eq!(old.blocked_by, Some(DacBlock::PlatformTooOld));
        assert_eq!(old.playing, Some(m(44_100, 16)));
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
        assert_eq!(choose(true, &MODES, m(88_200, 16)).blocked_by, Some(DacBlock::NoModeAtRate { rate: 88_200 }));
        assert_eq!(choose(true, &MODES, m(48_000, 16)).blocked_by, Some(DacBlock::NeedsExclusive { rate: 48_000, depths: DEPTH_24 }));
        let two = [m(48_000, 24), m(48_000, 32)];
        assert_eq!(choose(true, &two, m(48_000, 16)).blocked_by, Some(DacBlock::NeedsExclusive { rate: 48_000, depths: DEPTH_24 | DEPTH_32 }));
    }

    #[test]
    fn off_or_idle_decides_nothing() {
        assert_eq!(choose(false, &MODES, m(44_100, 16)).use_index, -1);
        assert_eq!(choose(true, &[], m(44_100, 16)), DacChoice { use_index: -1, blocked_by: None });
        assert_eq!(choose(true, &MODES, m(0, 16)).blocked_by, None);
    }
}
