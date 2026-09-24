//! Which sound each output device gets, as whole steps: a device arriving, a curve adopted for it, an
//! undo, a choice from the device list. The decision is nori-player's (`nori_player::device`); this
//! reads the settings, looks up the profiles, saves and binds them, keeps the two small things the
//! steps need between them (the sound from before a device took over, and the devices never to be
//! offered a curve), and says what the platform should do next as one `DeviceEffect`: which sound to
//! load, whether to read the profiles again, whether to run the device's arrival again. The platform
//! fetches an AutoEQ preset when asked (that is transport) and applies the effect.

use nori_player::device::{self, keep_loose, FLAT};
use nori_player::outputs::SPEAKER;

// Public, like model.rs's, since the uniffi scaffolding in crates/android names them by a public path.
pub use nori_player::device::{ChoiceKind, DeviceRow, NoticeText};

use nori_settings::settings::{sound_json, SoundSettings, StoredPrefs};
use nori_model::AutoEqEntry;
use nori_model::CurveStep;
use nori_model::SoundProfile;

/// The sound playing now, kept from before a bound device took over (`app_kv`).
pub const LOOSE: &str = "looseSound";
/// The devices the user said should never be offered a curve, as a JSON list (`app_kv`).
pub const QUIET: &str = "quietOutputs";

#[cfg(feature = "ffi")]
#[uniffi::remote(Enum)]
pub enum ChoiceKind {
    Automatic,
    Quiet,
    Flat,
    Profile,
}

#[cfg(feature = "ffi")]
#[uniffi::remote(Record)]
pub struct DeviceRow {
    pub output: String,
    pub name: String,
    pub kind: Option<String>,
    pub current: bool,
    pub sound: String,
    pub choice: ChoiceKind,
    pub profile: Option<String>,
}

#[cfg(feature = "ffi")]
#[uniffi::remote(Record)]
pub struct NoticeText {
    pub message: String,
    pub action: String,
}

/// What happens to the sound kept from before a device took over (see `nori_player::device::keep_loose`).
#[derive(Debug, Clone, PartialEq)]
pub enum LooseChange {
    Keep,
    /// Keep this sound: it is what plays now, and nobody bound it to a device.
    Store { json: String },
    /// It has been used, or is no longer wanted.
    Clear,
}

/// What a step decided, before the core settles its own part of it (the quiet mark and the kept sound).
#[derive(Debug, Clone, PartialEq)]
pub struct Step {
    pub quiet: Option<bool>,
    pub loose: LooseChange,
    pub effect: DeviceEffect,
}

impl Step {
    pub fn none() -> Self {
        Step { quiet: None, loose: LooseChange::Keep, effect: DeviceEffect::none() }
    }
}

/// What the platform does after a step, in this order: read the profiles (and the quiet devices) again,
/// load the sound, and run the device's arrival again.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DeviceEffect {
    pub refresh: bool,
    pub apply: Option<SoundSettings>,
    pub arrive: bool,
    /// The step made a new profile (an AutoEQ curve not saved before): undo deletes it again.
    pub created: bool,
}

impl DeviceEffect {
    pub fn none() -> Self {
        DeviceEffect { refresh: false, apply: None, arrive: false, created: false }
    }
}

/// What the settings say about device sound right now.
pub struct Now {
    pub sound: SoundSettings,
    pub per_output: bool,
    pub auto_apply: bool,
}

impl Now {
    fn of(p: &StoredPrefs) -> Self {
        Now { sound: p.sound(), per_output: p.profile_per_output, auto_apply: p.auto_eq_auto }
    }

    pub fn read() -> Self {
        nori_settings::settings_store::with_prefs(Now::of).unwrap_or_else(|| Now::of(&StoredPrefs::default()))
    }
}

/// A device arriving: the effect of its bound profile or of the sound from before, and whether a curve
/// is offered or applied (`entry`, with the url of its preset, when one matches).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DeviceArrival {
    pub effect: DeviceEffect,
    pub curve: CurveStep,
    pub entry: Option<AutoEqEntry>,
    pub preset_url: Option<String>,
}

/// Loads a device's own sound, keeping the sound playing now when it is the first one replaced.
pub fn loaded(sound: SoundSettings, current: &SoundSettings, per_output: bool, loose_kept: bool) -> (Option<SoundSettings>, LooseChange) {
    let loose = if keep_loose(per_output, loose_kept) { LooseChange::Store { json: sound_json(current) } } else { LooseChange::Keep };
    (Some(sound), loose)
}

/// What an output's own name says about the headphones behind it: "Bluetooth: LE_WH-1000XM5" is
/// "LE_WH-1000XM5". Empty for the speaker and anything else without a name of its own.
pub fn device_name(output: &str) -> &str {
    output.split_once(": ").map_or("", |(_, n)| n)
}

/// Every output seen, the one playing now included, each with the sound it gets.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn device_rows(known: Vec<String>, current: String, profiles: Vec<SoundProfile>, quiet: Vec<String>) -> Vec<DeviceRow> {
    let bound: Vec<(&str, &[String])> = profiles.iter().map(|p| (p.name.as_str(), p.outputs.as_slice())).collect();
    device::rows(&known, &current, &bound, &quiet)
}

/// The snackbar line about the device that just connected; see `nori_player::device::notice`.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn device_notice(offer: bool, name: String, output: String, current: String) -> Option<NoticeText> {
    device::notice(offer, &name, &output, &current)
}

/// What the test bridge asks a device to get.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum SpecKind {
    Automatic,
    Quiet,
    Flat,
    Profile,
    /// The first AutoEQ curve found for `arg`.
    Curve,
}

#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DeviceSpec {
    pub output: String,
    pub kind: SpecKind,
    pub arg: String,
}

/// The test bridge's `set deviceSound "<output>=flat|auto|quiet|profile:<name>|curve:<search>"`; anything
/// else after the last '=' is automatic.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn device_spec(value: String) -> DeviceSpec {
    let (output, spec) = value.rsplit_once('=').unwrap_or((&value, &value));
    let (kind, arg) = if spec == "flat" {
        (SpecKind::Flat, "")
    } else if spec == "quiet" {
        (SpecKind::Quiet, "")
    } else if let Some(name) = spec.strip_prefix("profile:") {
        (SpecKind::Profile, name)
    } else if let Some(search) = spec.strip_prefix("curve:") {
        (SpecKind::Curve, search)
    } else {
        (SpecKind::Automatic, "")
    };
    DeviceSpec { output: output.to_string(), kind, arg: arg.to_string() }
}

// ---- the device list and the AutoEQ browser, worded ----

/// Fewer than two characters (UTF-16 units, as the platform counts them) is not searched.
pub fn autoeq_too_short(query: &str) -> bool {
    query.trim().encode_utf16().count() < 2
}

/// One AutoEQ curve with the lines under it: `caption` in the browser (who measured it, the form and
/// the target, whichever are known), `short` in a device's sheet (who measured it and the form).
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct AutoEqHit {
    pub entry: AutoEqEntry,
    pub caption: String,
    pub short: String,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct AutoEqFound {
    pub too_short: bool,
    pub hits: Vec<AutoEqHit>,
}

fn hit(entry: AutoEqEntry) -> AutoEqHit {
    let caption = [&entry.source, &entry.form, &entry.target].iter().filter(|s| !s.is_empty()).map(|s| s.as_str()).collect::<Vec<_>>().join(" · ");
    let short = format!("{} · {}", entry.source, entry.form);
    AutoEqHit { entry, caption, short }
}

/// The curves with their lines.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn autoeq_hits(entries: Vec<AutoEqEntry>) -> Vec<AutoEqHit> {
    entries.into_iter().map(hit).collect()
}

/// The size of the downloaded AutoEQ list: "8123 headphones", and the search field's "Search 8123 headphones".
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct AutoEqCount {
    pub count: String,
    pub search: String,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn autoeq_count_words(count: u32) -> AutoEqCount {
    AutoEqCount { count: format!("{count} headphones"), search: format!("Search {count} headphones") }
}

/// What a device's sheet says and offers.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DeviceSheet {
    /// The words under its name.
    pub intro: String,
    /// The line under "Automatic": whether a known curve is used or offered.
    pub automatic: String,
    /// The saved profiles it can be given; "Flat" is its own row, so it is not among them.
    pub profiles: Vec<String>,
    /// Neither the one playing now nor the phone's speaker, which are always there.
    pub can_forget: bool,
}

pub fn sheet(output: &str, kind: Option<&str>, current: bool, auto_eq_auto: bool, profiles: &[String]) -> DeviceSheet {
    let this = match kind {
        None => "this".to_string(),
        Some(k) => format!("this {k} device"),
    };
    DeviceSheet {
        intro: format!("What music played through {this} sounds like. It switches by itself whenever the device connects."),
        automatic: (if auto_eq_auto { "Uses a matching AutoEQ curve when one is known" } else { "Offers a matching AutoEQ curve when one is known" }).into(),
        profiles: profiles.iter().filter(|p| *p != FLAT).cloned().collect(),
        can_forget: !current && output != SPEAKER,
    }
}

/// The sheet of the device `output` (plugged in as `kind`, the one playing now or not), given the
/// saved profiles' names.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn device_sheet(output: String, kind: Option<String>, current: bool, auto_eq_auto: bool, profiles: Vec<String>) -> DeviceSheet {
    sheet(&output, kind.as_deref(), current, auto_eq_auto, &profiles)
}

/// The mark on the device playing now, after its kind when it has one: " · Playing now", "Playing now".
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn device_playing_now(after_kind: bool) -> String {
    (if after_kind { " · Playing now" } else { "Playing now" }).into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_bridge_specs() {
        let s = |v: &str| {
            let d = device_spec(v.into());
            (d.output, d.kind, d.arg)
        };
        assert_eq!(s("USB: K3=flat"), ("USB: K3".into(), SpecKind::Flat, String::new()));
        assert_eq!(s("a=b=quiet"), ("a=b".into(), SpecKind::Quiet, String::new()));
        assert_eq!(s("USB: K3=profile:Warm"), ("USB: K3".into(), SpecKind::Profile, "Warm".into()));
        assert_eq!(s("x=curve:HD 600"), ("x".into(), SpecKind::Curve, "HD 600".into()));
        assert_eq!(s("x=auto"), ("x".into(), SpecKind::Automatic, String::new()));
        assert_eq!(s("flat"), ("flat".into(), SpecKind::Flat, String::new()));
    }
}
