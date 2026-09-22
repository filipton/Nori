//! Which sound an output device gets when music moves to it. A device can have a sound of its own (a
//! saved profile bound to it), be marked quiet (never offered a curve), or have nothing chosen; with
//! nothing chosen, headphones whose name matches an AutoEQ curve get it offered, or applied straight
//! away when the user asked for that. The platform looks things up and applies the answer.

/// What is known about the device as it arrives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Arrival {
    /// A profile is bound to this device.
    pub bound: bool,
    /// The user wants each device to keep its own sound.
    pub per_output: bool,
    /// The phone's own speaker: never offered a curve.
    pub speaker: bool,
    /// The user said this device should never be offered a curve.
    pub quiet: bool,
    /// Apply a matching curve without asking.
    pub auto_apply: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CurveStep {
    /// Leave it: no curve is looked for.
    None,
    /// Look for a curve; offer it if one matches.
    Offer,
    /// Look for a curve; apply it if one matches (an offer if fetching it fails).
    Apply,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ArrivalPlan {
    /// Load the device's bound profile.
    pub load_bound: bool,
    /// Bring back the sound from before a bound device took over, if one was kept.
    pub restore: bool,
    pub curve: CurveStep,
}

pub fn on_arrival(a: Arrival) -> ArrivalPlan {
    if a.bound {
        return ArrivalPlan { load_bound: a.per_output, restore: false, curve: CurveStep::None };
    }
    let curve = if a.speaker || a.quiet {
        CurveStep::None
    } else if a.auto_apply && a.per_output {
        CurveStep::Apply
    } else {
        CurveStep::Offer
    };
    ArrivalPlan { load_bound: false, restore: a.per_output, curve }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn arrival() -> Arrival {
        Arrival { bound: false, per_output: true, speaker: false, quiet: false, auto_apply: false }
    }

    #[test]
    fn a_bound_device_gets_its_own_sound_and_nothing_else() {
        assert_eq!(on_arrival(Arrival { bound: true, ..arrival() }), ArrivalPlan { load_bound: true, restore: false, curve: CurveStep::None });
        assert!(!on_arrival(Arrival { bound: true, per_output: false, ..arrival() }).load_bound, "per-device sound off");
    }

    #[test]
    fn an_unbound_device_gets_the_sound_from_before_and_maybe_a_curve() {
        assert_eq!(on_arrival(arrival()), ArrivalPlan { load_bound: false, restore: true, curve: CurveStep::Offer });
        assert_eq!(on_arrival(Arrival { auto_apply: true, ..arrival() }).curve, CurveStep::Apply);
        assert_eq!(on_arrival(Arrival { auto_apply: true, per_output: false, ..arrival() }).curve, CurveStep::Offer, "applying needs per-device sound");
    }

    #[test]
    fn the_speaker_and_quiet_devices_are_never_offered_a_curve() {
        assert_eq!(on_arrival(Arrival { speaker: true, ..arrival() }).curve, CurveStep::None);
        assert_eq!(on_arrival(Arrival { quiet: true, auto_apply: true, ..arrival() }).curve, CurveStep::None);
    }
}
