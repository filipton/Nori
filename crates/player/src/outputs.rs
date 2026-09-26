//! Which output the music is going to, as a stable key a sound profile can be bound to: the speaker,
//! wired headphones, each Bluetooth device by name, each USB DAC by name. The platform lists what is
//! attached (what kind each device is, and what it calls itself); this names each one, picks the one
//! media goes to, and keeps the list of every output ever seen, so a device can be given its own sound
//! while it is unplugged.

/// The phone's own speaker's key. Always in the list of outputs, and never forgotten.
///
/// The keys are identifiers, stored with the settings (the known outputs, the profiles bound to them) and
/// kept as they always were; a client shows an output by its [`parts`], in its own words.
pub const SPEAKER: &str = "Phone speaker";
const WIRED: &str = "Wired headphones";
const USB: &str = "USB: ";
const BLUETOOTH: &str = "Bluetooth: ";
/// What a nameless USB device, a nameless Bluetooth one and a nameless other output are keyed by.
const NAMELESS_USB: &str = "DAC";
const NAMELESS_BLUETOOTH: &str = "device";
const OTHER: &str = "Other output";

/// Where an output is plugged in, as a client names it ("USB", "Bluetooth"; the speaker and wired
/// headphones by that alone).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum OutputPort {
    Speaker,
    Wired,
    Usb,
    Bluetooth,
    /// A dock, HDMI, a line out or anything else, known by its own name when it has one.
    Other,
}

/// An output key read back: where it is plugged in, and the name the device gives itself (none when it
/// gave none, or for the speaker and wired headphones).
pub fn parts(key: &str) -> (OutputPort, Option<&str>) {
    fn named_or<'a>(nameless: &str, name: &'a str) -> Option<&'a str> {
        (name != nameless).then_some(name)
    }
    if key == SPEAKER {
        (OutputPort::Speaker, None)
    } else if key == WIRED {
        (OutputPort::Wired, None)
    } else if let Some(name) = key.strip_prefix(USB) {
        (OutputPort::Usb, named_or(NAMELESS_USB, name))
    } else if let Some(name) = key.strip_prefix(BLUETOOTH) {
        (OutputPort::Bluetooth, named_or(NAMELESS_BLUETOOTH, name))
    } else {
        (OutputPort::Other, named_or(OTHER, key))
    }
}

/// What an attached output is, as far as routing and naming go. The platform maps its own device
/// types onto these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutputKind {
    /// A USB DAC or USB headset.
    Usb,
    /// A USB accessory: USB, so offload has no path to it, but not where media is routed.
    UsbAccessory,
    /// Wired headphones or a wired headset.
    Wired,
    /// A Bluetooth A2DP or LE audio device.
    Bluetooth,
    /// A dock, HDMI or an aux line out.
    Line,
    Speaker,
    /// Everything else the platform lists: telephony, a virtual sink, the earpiece.
    Other,
}

// Lowest wins. Everything the framework lists that is not one of these - telephony, HDMI, a
// virtual sink - ranks *below* the built-in speaker rather than above it. It used to rank above,
// so a phone with a telephony output (which is every phone) reported that as where the music was
// going, and anything keyed on the current output believed it.
pub fn rank(kind: OutputKind) -> u8 {
    match kind {
        OutputKind::Usb => 0,
        OutputKind::Wired => 1,
        OutputKind::Bluetooth => 2,
        OutputKind::Line => 3,
        OutputKind::Speaker => 8,
        OutputKind::UsbAccessory | OutputKind::Other => 9,
    }
}

/// The key a device is remembered by: "USB: <name>", "Bluetooth: <name>", "Wired headphones".
pub fn key(kind: OutputKind, name: &str) -> String {
    let name = name.trim();
    let or = |fallback: &str| if name.is_empty() { fallback.to_string() } else { name.to_string() };
    match kind {
        OutputKind::Speaker => SPEAKER.to_string(),
        OutputKind::Wired => WIRED.to_string(),
        OutputKind::Usb => format!("{USB}{}", or(NAMELESS_USB)),
        OutputKind::Bluetooth => format!("{BLUETOOTH}{}", or(NAMELESS_BLUETOOTH)),
        OutputKind::UsbAccessory | OutputKind::Line | OutputKind::Other => or(OTHER),
    }
}

/// What the attached devices say, after a device was plugged in or removed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    /// Where media goes now.
    pub current: String,
    /// The list of every output seen, when it changed; `None` when it is the same as before.
    pub known: Option<Vec<String>>,
    /// Something USB is attached. Audio offload targets the phone's own DSP: with the stream handed to
    /// the chip, a track routed to USB opens without complaint and then plays nothing, which is the
    /// "silent DAC" this flag exists to prevent. The platform decodes on the CPU while it is true.
    pub usb: bool,
}

/// The outputs the platform lists now, as (kind, the name the device gives itself). `fake_usb`
/// pretends a USB device of that name is attached, so everything that hangs off it can be checked
/// without the hardware.
pub fn refresh(attached: &[(OutputKind, &str)], known: &[String], fake_usb: Option<&str>) -> Seen {
    // Android routes media to the most recently attached of these, in this order of precedence.
    let fake = fake_usb.map(|n| format!("{USB}{n}"));
    let mut best: Option<&(OutputKind, &str)> = None;
    for d in attached {
        // The first of the lowest rank, like minByOrNull.
        if best.is_none_or(|b| rank(d.0) < rank(b.0)) {
            best = Some(d);
        }
    }
    let current = fake.clone().or_else(|| best.map(|d| key(d.0, d.1))).unwrap_or_else(|| SPEAKER.to_string());
    let mut next: Vec<String> = known.to_vec();
    next.extend(attached.iter().filter(|d| rank(d.0) < 8).map(|d| key(d.0, d.1)));
    next.push(SPEAKER.to_string());
    next.extend(fake.clone());
    let next = sorted_distinct(next);
    let usb = fake.is_some() || attached.iter().any(|d| matches!(d.0, OutputKind::Usb | OutputKind::UsbAccessory));
    Seen { current, known: (next != known).then_some(next), usb }
}

/// The list as it is read back from storage: the speaker is always in it.
pub fn initial_known(stored: &[String]) -> Vec<String> {
    let mut all = stored.to_vec();
    all.push(SPEAKER.to_string());
    sorted_distinct(all)
}

/// Drops a device from the list; it comes back by itself the next time it is connected. The speaker
/// and the device playing now stay. `None` when nothing changes.
pub fn forget(known: &[String], current: &str, output: &str) -> Option<Vec<String>> {
    if output == SPEAKER || output == current {
        return None;
    }
    let next: Vec<String> = known.iter().filter(|o| *o != output).cloned().collect();
    (next != known).then_some(next)
}

fn sorted_distinct(mut list: Vec<String>) -> Vec<String> {
    list.sort();
    list.dedup();
    list
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(v: &[&str]) -> Vec<String> {
        v.iter().map(|x| x.to_string()).collect()
    }

    #[test]
    fn devices_are_named_by_kind_and_their_own_name() {
        assert_eq!(key(OutputKind::Usb, "  FiiO K3 "), "USB: FiiO K3");
        assert_eq!(key(OutputKind::Usb, " "), "USB: DAC");
        assert_eq!(key(OutputKind::Bluetooth, "WH-1000XM5"), "Bluetooth: WH-1000XM5");
        assert_eq!(key(OutputKind::Bluetooth, ""), "Bluetooth: device");
        assert_eq!(key(OutputKind::Wired, "anything"), "Wired headphones");
        assert_eq!(key(OutputKind::Speaker, "whatever"), SPEAKER);
        assert_eq!(key(OutputKind::Line, "TV"), "TV");
        assert_eq!(key(OutputKind::Other, ""), "Other output");
    }

    #[test]
    fn a_key_reads_back_as_its_parts() {
        assert_eq!(parts("USB: FiiO K3"), (OutputPort::Usb, Some("FiiO K3")));
        assert_eq!(parts("USB: DAC"), (OutputPort::Usb, None));
        assert_eq!(parts("Bluetooth: device"), (OutputPort::Bluetooth, None));
        assert_eq!(parts("Bluetooth: WH-1000XM5"), (OutputPort::Bluetooth, Some("WH-1000XM5")));
        assert_eq!(parts(SPEAKER), (OutputPort::Speaker, None));
        assert_eq!(parts("Wired headphones"), (OutputPort::Wired, None));
        assert_eq!(parts("TV"), (OutputPort::Other, Some("TV")));
        assert_eq!(parts("Other output"), (OutputPort::Other, None));
        for (k, n) in [(OutputKind::Usb, "K3"), (OutputKind::Usb, ""), (OutputKind::Bluetooth, ""), (OutputKind::Line, "TV"), (OutputKind::Other, "")] {
            let key = key(k, n);
            let (port, name) = parts(&key);
            assert_eq!(name.unwrap_or(""), n, "{k:?} {port:?}");
        }
    }

    #[test]
    fn media_goes_to_the_best_ranked_device() {
        let attached = [(OutputKind::Other, "Telephony"), (OutputKind::Speaker, ""), (OutputKind::Bluetooth, "Buds"), (OutputKind::Wired, "")];
        let seen = refresh(&attached, &[], None);
        assert_eq!(seen.current, "Wired headphones");
        assert!(!seen.usb);
        // Telephony ranks below the speaker, so it is neither current nor remembered.
        assert_eq!(seen.known, Some(s(&["Bluetooth: Buds", SPEAKER, "Wired headphones"])));
        assert_eq!(refresh(&[(OutputKind::Other, "Telephony"), (OutputKind::Speaker, "")], &[], None).current, SPEAKER);
        assert_eq!(refresh(&[], &[], None).current, SPEAKER);
    }

    #[test]
    fn the_known_list_only_changes_when_something_new_is_seen() {
        let known = s(&["Bluetooth: Buds", SPEAKER]);
        assert_eq!(refresh(&[(OutputKind::Bluetooth, "Buds")], &known, None).known, None);
        let seen = refresh(&[(OutputKind::Usb, "K3")], &known, None);
        assert_eq!(seen.current, "USB: K3");
        assert!(seen.usb);
        assert_eq!(seen.known, Some(s(&["Bluetooth: Buds", SPEAKER, "USB: K3"])));
    }

    #[test]
    fn a_usb_accessory_stops_offload_but_is_not_where_music_goes() {
        let seen = refresh(&[(OutputKind::UsbAccessory, "Hub"), (OutputKind::Speaker, "")], &s(&[SPEAKER]), None);
        assert_eq!(seen.current, SPEAKER);
        assert!(seen.usb);
        assert_eq!(seen.known, None);
    }

    #[test]
    fn a_pretend_dac_is_current_and_remembered() {
        let seen = refresh(&[(OutputKind::Speaker, "")], &s(&[SPEAKER]), Some("Mock DAC"));
        assert_eq!(seen.current, "USB: Mock DAC");
        assert!(seen.usb);
        assert_eq!(seen.known, Some(s(&[SPEAKER, "USB: Mock DAC"])));
    }

    #[test]
    fn forgetting_keeps_the_speaker_and_what_is_playing() {
        let known = s(&["Bluetooth: Buds", SPEAKER, "USB: K3"]);
        assert_eq!(forget(&known, "USB: K3", SPEAKER), None);
        assert_eq!(forget(&known, "USB: K3", "USB: K3"), None);
        assert_eq!(forget(&known, "USB: K3", "Bluetooth: Buds"), Some(s(&[SPEAKER, "USB: K3"])));
        assert_eq!(forget(&known, "USB: K3", "Wired headphones"), None);
        assert_eq!(initial_known(&s(&["USB: K3"])), s(&[SPEAKER, "USB: K3"]));
    }
}
