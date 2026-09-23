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

/// The profile "Flat": the equalizer off on a device, everything else as it was when it was made.
pub const FLAT: &str = "Flat";

/// Loading a device's own sound. The first time one replaces a sound nobody bound to a device, that
/// sound is kept, so it comes back when the music goes to such a device again (the DAC unplugged, back
/// to the speaker). True: keep the sound playing now before loading the device's.
pub fn keep_loose(per_output: bool, loose_kept: bool) -> bool {
    per_output && !loose_kept
}

/// What a device in the list gets.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChoiceKind {
    /// Nothing chosen: a matching AutoEQ curve is offered (or applied, with the setting on).
    Automatic,
    /// Nothing chosen and nothing offered.
    Quiet,
    /// The equalizer off on this device, everything else as it is now.
    Flat,
    /// A saved profile.
    Profile,
}

/// One output device in the equalizer's device list: `name` is what it calls itself, `kind` where it is
/// plugged in, `sound` what it gets (a profile's name, "Flat", "Automatic", "Leave as is").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceRow {
    pub output: String,
    pub name: String,
    pub kind: Option<String>,
    pub current: bool,
    pub sound: String,
    pub choice: ChoiceKind,
    /// The profile bound to it, for `ChoiceKind::Profile`.
    pub profile: Option<String>,
}

/// Every output seen, the one playing now included, each with the sound it gets: the speaker first,
/// then by name. `profiles` are (name, the outputs bound to it).
pub fn rows(known: &[String], current: &str, profiles: &[(&str, &[String])], quiet: &[String]) -> Vec<DeviceRow> {
    let mut outputs: Vec<&str> = Vec::with_capacity(known.len() + 1);
    for o in known.iter().map(String::as_str).chain(std::iter::once(current)) {
        if !outputs.contains(&o) {
            outputs.push(o);
        }
    }
    let mut rows: Vec<DeviceRow> = outputs
        .into_iter()
        .map(|o| {
            let bound = profiles.iter().find(|(_, outs)| outs.iter().any(|x| x == o)).map(|(n, _)| *n);
            let choice = match bound {
                Some(FLAT) => ChoiceKind::Flat,
                Some(_) => ChoiceKind::Profile,
                None if quiet.iter().any(|q| q == o) => ChoiceKind::Quiet,
                None => ChoiceKind::Automatic,
            };
            let sound = match choice {
                ChoiceKind::Automatic => "Automatic",
                ChoiceKind::Quiet => "Leave as is",
                ChoiceKind::Flat => "Flat",
                ChoiceKind::Profile => bound.unwrap_or_default(),
            };
            let (kind, name) = match o.split_once(": ") {
                Some((k, n)) => ((!k.is_empty()).then(|| k.to_string()), n),
                None => (None, o),
            };
            DeviceRow {
                output: o.to_string(),
                name: name.to_string(),
                kind,
                current: o == current,
                sound: sound.to_string(),
                choice,
                profile: (choice == ChoiceKind::Profile).then(|| sound.to_string()),
            }
        })
        .collect();
    rows.sort_by_cached_key(|r| (r.output != crate::outputs::SPEAKER, r.name.to_lowercase()));
    rows
}

/// A line for the snackbar about the device that just connected, with the one thing it offers to do.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoticeText {
    pub message: String,
    pub action: String,
}

/// What to say about the device that just connected; only while it is still the one playing. `offer`:
/// a curve `name` is offered; otherwise the curve `name` was applied without asking.
pub fn notice(offer: bool, name: &str, output: &str, current: &str) -> Option<NoticeText> {
    if output != current {
        return None;
    }
    Some(if offer {
        NoticeText { message: format!("{name} connected. Use its AutoEQ curve?"), action: "Apply".to_string() }
    } else {
        NoticeText { message: format!("Using AutoEQ for {name}"), action: "Undo".to_string() }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::outputs::SPEAKER;

    #[test]
    fn device_rows_say_what_each_device_gets() {
        let s = |v: &[&str]| v.iter().map(|x| x.to_string()).collect::<Vec<_>>();
        let known = s(&["USB: K3", SPEAKER, "Bluetooth: buds", "Wired headphones"]);
        let flat = s(&["Wired headphones"]);
        let warm = s(&["USB: K3", "Bluetooth: Other"]);
        let profiles: [(&str, &[String]); 2] = [(FLAT, &flat), ("Warm", &warm)];
        let rows = rows(&known, "Bluetooth: Other", &profiles, &s(&["Bluetooth: buds"]));
        let got: Vec<(&str, &str, Option<&str>, bool, &str, ChoiceKind)> =
            rows.iter().map(|r| (r.output.as_str(), r.name.as_str(), r.kind.as_deref(), r.current, r.sound.as_str(), r.choice)).collect();
        assert_eq!(
            got,
            [
                (SPEAKER, SPEAKER, None, false, "Automatic", ChoiceKind::Automatic),
                ("Bluetooth: buds", "buds", Some("Bluetooth"), false, "Leave as is", ChoiceKind::Quiet),
                ("USB: K3", "K3", Some("USB"), false, "Warm", ChoiceKind::Profile),
                ("Bluetooth: Other", "Other", Some("Bluetooth"), true, "Warm", ChoiceKind::Profile),
                ("Wired headphones", "Wired headphones", None, false, "Flat", ChoiceKind::Flat),
            ]
        );
        assert_eq!(rows[2].profile.as_deref(), Some("Warm"));
        assert_eq!(rows[4].profile, None);
    }

    #[test]
    fn the_current_device_is_listed_once() {
        let known = vec![SPEAKER.to_string()];
        assert_eq!(rows(&known, SPEAKER, &[], &[]).len(), 1);
    }

    #[test]
    fn a_notice_is_only_about_the_device_playing_now() {
        assert_eq!(notice(true, "Sony WH-1000XM5", "Bluetooth: X", "Bluetooth: X").unwrap().message, "Sony WH-1000XM5 connected. Use its AutoEQ curve?");
        assert_eq!(notice(true, "A", "o", "o").unwrap().action, "Apply");
        assert_eq!(notice(false, "A", "o", "o"), Some(NoticeText { message: "Using AutoEQ for A".into(), action: "Undo".into() }));
        assert_eq!(notice(true, "A", "o", SPEAKER), None);
    }

    #[test]
    fn the_loose_sound_is_kept_once() {
        assert!(keep_loose(true, false));
        assert!(!keep_loose(true, true));
        assert!(!keep_loose(false, false));
    }

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
