//! Android's side of `nori_player::outputs` and `nori_player::dac`: its device types and PCM encodings
//! mapped onto the player's, and the doors the Kotlin glue calls when a device is attached or removed
//! and when the output format changes. Event rate only, never per buffer.

use nori_player::dac::{self, DacDecision, DacMode, DacStep};
use nori_player::outputs::{self, OutputKind, Seen};

#[uniffi::remote(Record)]
pub struct Seen {
    pub current: String,
    pub known: Option<Vec<String>>,
    pub usb: bool,
}

#[uniffi::remote(Enum)]
pub enum DacStep {
    Release,
    Keep,
    Prefer { index: u32 },
}

#[uniffi::remote(Record)]
pub struct DacDecision {
    pub step: DacStep,
    pub device: String,
    pub supported: bool,
    pub modes: Vec<String>,
    pub playing: Option<String>,
    pub bits: u32,
    pub blocked_by: Option<String>,
    pub refused: String,
}

/// `AudioDeviceInfo.TYPE_*`.
fn kind(t: i32) -> OutputKind {
    match t {
        11 | 22 => OutputKind::Usb,          // USB_DEVICE, USB_HEADSET
        12 => OutputKind::UsbAccessory,      // USB_ACCESSORY
        3 | 4 => OutputKind::Wired,          // WIRED_HEADSET, WIRED_HEADPHONES
        8 | 26 | 27 => OutputKind::Bluetooth, // BLUETOOTH_A2DP, BLE_HEADSET, BLE_SPEAKER
        13 | 9 | 19 => OutputKind::Line,     // DOCK, HDMI, AUX_LINE
        2 => OutputKind::Speaker,            // BUILTIN_SPEAKER
        _ => OutputKind::Other,
    }
}

/// The glyph the player's output button shows for where the sound is going.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum OutputGlyph {
    Headphones,
    Bluetooth,
    /// The phone's speaker and anything else: Apple's AirPlay mark, filled in when it is elsewhere.
    Cast,
}

/// The output button as it stands.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct OutputLook {
    pub glyph: OutputGlyph,
    /// The sound is going somewhere other than the phone's speaker: the glyph takes the accent.
    pub elsewhere: bool,
    /// What the button says to a screen reader.
    pub description: String,
}

/// The output button for `output`, one of the names outputs are remembered by ("USB: …",
/// "Bluetooth: …", "Wired headphones", the speaker, or a device's own name).
#[uniffi::export]
pub fn output_look(output: String) -> OutputLook {
    let glyph = if output.starts_with("USB") || output.starts_with("Wired") {
        OutputGlyph::Headphones
    } else if output.starts_with("Bluetooth") {
        OutputGlyph::Bluetooth
    } else {
        OutputGlyph::Cast
    };
    OutputLook { glyph, elsewhere: output != outputs::SPEAKER, description: format!("Output: {output}") }
}

/// `AudioFormat.ENCODING_*` in bits per sample; float counts as 32.
fn bits(encoding: i32) -> u32 {
    match encoding {
        2 => 16,       // PCM_16BIT
        21 => 24,      // PCM_24BIT_PACKED
        22 | 4 => 32,  // PCM_32BIT, PCM_FLOAT
        _ => 0,
    }
}

const PCM_FLOAT: i32 = 4;

fn mode(rate: u32, encoding: i32) -> DacMode {
    DacMode { rate, bits: bits(encoding), float: encoding == PCM_FLOAT }
}

fn modes(rates: &[u32], encodings: &[i32]) -> Vec<DacMode> {
    rates.iter().zip(encodings).map(|(r, e)| mode(*r, *e)).collect()
}

/// The output devices Android lists now, as parallel lists of `AudioDeviceInfo` types and product
/// names, against the list of every output seen so far.
#[uniffi::export]
pub fn outputs_refresh(types: Vec<i32>, names: Vec<String>, known: Vec<String>, fake_usb: Option<String>) -> Seen {
    let attached: Vec<(OutputKind, &str)> = types.iter().zip(&names).map(|(t, n)| (kind(*t), n.as_str())).collect();
    outputs::refresh(&attached, &known, fake_usb.as_deref())
}

/// The name the phone's own speaker goes by (`nori_player::outputs::SPEAKER`).
#[uniffi::export]
pub fn outputs_speaker() -> String {
    nori_player::outputs::SPEAKER.into()
}

/// The name of the sound profile that changes nothing (`nori_player::device::FLAT`).
#[uniffi::export]
pub fn device_flat() -> String {
    nori_player::device::FLAT.into()
}

/// The list of outputs as it comes back from storage.
#[uniffi::export]
pub fn outputs_known(stored: Vec<String>) -> Vec<String> {
    outputs::initial_known(&stored)
}

/// The list without `output`, or `None` when it stays as it is.
#[uniffi::export]
pub fn outputs_forget(known: Vec<String>, current: String, output: String) -> Option<Vec<String>> {
    outputs::forget(&known, &current, &output)
}

/// The decision about an attached DAC; see `nori_player::dac::decide`. Modes are parallel lists of
/// sample rates and `AudioFormat` encodings; `applied_*` are the modes of the port the preferred mode
/// is held for, when it is the same port.
#[uniffi::export]
#[allow(clippy::too_many_arguments)]
pub fn dac_decide(
    enabled: bool, platform_ok: bool, name: String, rates: Vec<u32>, encodings: Vec<i32>, playing_rate: u32, playing_encoding: i32,
    applied_rates: Option<Vec<u32>>, applied_encodings: Option<Vec<i32>>, was_bit_perfect: bool,
) -> DacDecision {
    let applied = applied_rates.zip(applied_encodings).map(|(r, e)| modes(&r, &e));
    dac::decide(enabled, platform_ok, &name, &modes(&rates, &encodings), mode(playing_rate, playing_encoding), applied.as_deref(), was_bit_perfect)
}

/// What the AudioTrack was opened with, for the user to check.
#[uniffi::export]
pub fn dac_track_line(rate: u32, encoding: i32, offloaded: bool) -> String {
    dac::track_line(rate, bits(encoding), offloaded)
}

/// A DAC that is not there, as the test bridge describes it.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct MockDac {
    pub name: String,
    pub rates: Vec<u32>,
    /// `AudioFormat` encodings, one per rate.
    pub encodings: Vec<i32>,
}

/// `name@44100/16,96000/24`: the rates and bit depths the pretend DAC offers bit-perfect ("16", "24",
/// "32" or "float"; 16 when left out). A mode that does not read is left out.
#[uniffi::export]
pub fn dac_mock(spec: String) -> MockDac {
    let name = match spec.split_once('@') {
        Some((n, _)) if !n.is_empty() => n.to_string(),
        _ => "Mock DAC".to_string(),
    };
    let (mut rates, mut encodings) = (Vec::new(), Vec::new());
    for m in spec.split_once('@').map_or("", |(_, m)| m).split(',') {
        let (rate, depth) = m.split_once('/').unwrap_or((m, "16"));
        let Ok(rate) = rate.trim().parse::<u32>() else { continue };
        let encoding = match depth.trim() {
            "16" => 2,
            "24" => 21,
            "32" => 22,
            "float" => PCM_FLOAT,
            _ => continue,
        };
        rates.push(rate);
        encodings.push(encoding);
    }
    MockDac { name, rates, encodings }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_output_button_names_where_the_sound_goes() {
        let l = |o: &str| {
            let l = output_look(o.into());
            (l.glyph, l.elsewhere)
        };
        assert_eq!(l("USB: K3"), (OutputGlyph::Headphones, true));
        assert_eq!(l("Wired headphones"), (OutputGlyph::Headphones, true));
        assert_eq!(l("Bluetooth: buds"), (OutputGlyph::Bluetooth, true));
        assert_eq!(l(outputs::SPEAKER), (OutputGlyph::Cast, false));
        assert_eq!(l("HDMI"), (OutputGlyph::Cast, true));
        assert_eq!(output_look("Bluetooth: buds".into()).description, "Output: Bluetooth: buds");
    }

    #[test]
    fn android_devices_map_onto_the_player_kinds() {
        let seen = outputs_refresh(vec![18, 2, 8, 22], vec!["".into(), "".into(), "Buds".into(), " K3 ".into()], vec![], None);
        assert_eq!(seen.current, "USB: K3");
        assert!(seen.usb);
        assert_eq!(seen.known.unwrap(), ["Bluetooth: Buds", "Phone speaker", "USB: K3"]);
        assert!(outputs_refresh(vec![12, 2], vec!["".into(), "".into()], vec![], None).usb, "a USB accessory");
        assert_eq!(outputs_refresh(vec![4], vec!["".into()], vec![], None).current, "Wired headphones");
    }

    #[test]
    fn encodings_are_bits() {
        assert_eq!(dac_track_line(44_100, 2, false), "44.1 kHz / 16 bit");
        assert_eq!(dac_track_line(96_000, 4, true), "96.0 kHz / 32 bit, offloaded to the audio chip");
        let d = dac_decide(true, true, "K3".into(), vec![44_100, 96_000], vec![2, 4], 96_000, 4, None, None, false);
        assert_eq!(d.step, DacStep::Prefer { index: 1 });
        let d = dac_decide(true, true, "K3".into(), vec![96_000], vec![22], 96_000, 4, None, None, false);
        assert_eq!(d.step, DacStep::Release, "float is not 32-bit integer");
        let d = dac_decide(true, true, "K3".into(), vec![96_000], vec![22], 96_000, 22, Some(vec![96_000]), Some(vec![22]), true);
        assert_eq!(d.step, DacStep::Keep);
    }

    #[test]
    fn mock_dac_specs() {
        assert_eq!(dac_mock("K3@44100/16,96000/24, 48000/float".into()), MockDac { name: "K3".into(), rates: vec![44_100, 96_000, 48_000], encodings: vec![2, 21, 4] });
        assert_eq!(dac_mock("@44100".into()), MockDac { name: "Mock DAC".into(), rates: vec![44_100], encodings: vec![2] });
        assert_eq!(dac_mock("K3".into()), MockDac { name: "Mock DAC".into(), rates: vec![], encodings: vec![] });
        assert_eq!(dac_mock("K3@x/16,44100/8".into()).rates, Vec::<u32>::new());
    }
}
