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

/// "44.1 kHz".
pub fn khz(rate: u32) -> String {
    format!("{:.1} kHz", rate as f64 / 1000.0)
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

#[cfg(test)]
mod tests {
    use super::*;

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
