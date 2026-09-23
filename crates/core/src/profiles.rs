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
use rusqlite::OptionalExtension;

use crate::settings::{self, sound_from, sound_json, SoundError, SoundSettings, StoredPrefs};
use crate::{alog, autoeq, Arrival, AutoEqEntry, Core, CoreError, CurveStep, SoundProfile};

/// The sound playing now, kept from before a bound device took over (`app_kv`).
const LOOSE: &str = "looseSound";
/// The devices the user said should never be offered a curve, as a JSON list (`app_kv`).
const QUIET: &str = "quietOutputs";

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
enum LooseChange {
    Keep,
    /// Keep this sound: it is what plays now, and nobody bound it to a device.
    Store { json: String },
    /// It has been used, or is no longer wanted.
    Clear,
}

/// What a step decided, before the core settles its own part of it (the quiet mark and the kept sound).
#[derive(Debug, Clone, PartialEq)]
struct Step {
    quiet: Option<bool>,
    loose: LooseChange,
    effect: DeviceEffect,
}

impl Step {
    fn none() -> Self {
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
    fn none() -> Self {
        DeviceEffect { refresh: false, apply: None, arrive: false, created: false }
    }
}

/// What the settings say about device sound right now.
struct Now {
    sound: SoundSettings,
    per_output: bool,
    auto_apply: bool,
}

impl Now {
    fn of(p: &StoredPrefs) -> Self {
        Now { sound: p.sound(), per_output: p.profile_per_output, auto_apply: p.auto_eq_auto }
    }

    fn read() -> Self {
        crate::settings_store::with_prefs(Now::of).unwrap_or_else(|| Now::of(&StoredPrefs::default()))
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
fn loaded(sound: SoundSettings, current: &SoundSettings, per_output: bool, loose_kept: bool) -> (Option<SoundSettings>, LooseChange) {
    let loose = if keep_loose(per_output, loose_kept) { LooseChange::Store { json: sound_json(current) } } else { LooseChange::Keep };
    (Some(sound), loose)
}

/// What an output's own name says about the headphones behind it: "Bluetooth: LE_WH-1000XM5" is
/// "LE_WH-1000XM5". Empty for the speaker and anything else without a name of its own.
fn device_name(output: &str) -> &str {
    output.split_once(": ").map_or("", |(_, n)| n)
}

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    /// The output that music now goes to.
    pub fn device_arrive(&self, output: String) -> DeviceArrival {
        self.arrive_as(output, &Now::read())
    }

    /// An AutoEQ curve for `output`: `preset` is the fetched text of the curve `name`. It goes on top of
    /// the sound as it is now, is saved as a profile named after the curve (keeping the devices it
    /// already had) and bound to `output` alone; `live` loads it too. An error when the preset has no
    /// filters in it.
    pub fn device_adopt(&self, output: String, name: String, preset: String, live: bool) -> Result<DeviceEffect, SoundError> {
        let step = self.adopt_as(&output, &name, &preset, live, &Now::read())?;
        Ok(self.settle(&output, step))
    }

    /// Undoes a curve applied without asking: nothing bound, the profile gone again if this made it and
    /// nothing else uses it, the sound from `before` back, and this device is not offered a curve again.
    pub fn device_undo(&self, output: String, curve: String, created: bool, before: SoundSettings) -> DeviceEffect {
        let unbind = || -> Result<(), CoreError> {
            self.profile_bind(output.clone(), None)?;
            if created && self.profiles()?.iter().any(|p| p.name == curve && p.outputs.is_empty()) {
                self.profile_delete(curve.clone())?;
            }
            Ok(())
        };
        let _ = unbind();
        let effect = DeviceEffect { refresh: true, apply: Some(before), ..DeviceEffect::none() };
        self.settle(&output, Step { quiet: Some(true), loose: LooseChange::Clear, effect })
    }

    /// What the user picked for a device in the device list (a curve goes through `device_adopt`):
    /// nothing, never ask, flat, or the profile `profile`. `live`: the device is the one playing, so its
    /// sound is loaded straight away.
    pub fn device_assign(&self, output: String, choice: ChoiceKind, profile: String, live: bool) -> Result<DeviceEffect, SoundError> {
        let step = self.assign_as(&output, choice, profile, live, &Now::read())?;
        Ok(self.settle(&output, step))
    }

    /// A device the list no longer needs to show: its binding and its "never ask" go with it.
    pub fn device_forget(&self, output: String) -> DeviceEffect {
        let _ = self.profile_bind(output.clone(), None);
        let effect = DeviceEffect { refresh: true, ..DeviceEffect::none() };
        self.settle(&output, Step { quiet: Some(false), loose: LooseChange::Keep, effect })
    }

    /// The devices never to be offered a curve, for the device list.
    pub fn device_quiet(&self) -> Vec<String> {
        self.quiet_list()
    }

    /// Saves `sound` under `name` (trimmed). Saving over a profile keeps the devices it is chosen for.
    pub fn profile_save_sound(&self, name: String, sound: SoundSettings) -> Result<(), SoundError> {
        let name = name.trim().to_string();
        let kept = self.profiles()?.into_iter().find(|p| p.name == name).map(|p| p.outputs).unwrap_or_default();
        self.profile_save(SoundProfile { name, json: sound_json(&sound), outputs: kept })?;
        Ok(())
    }

    /// The AutoEQ curves this output's own name points at, best first. Empty for the speaker, a nameless
    /// DAC, or no index.
    pub fn autoeq_for_output(&self, output: String, limit: u32) -> Vec<AutoEqEntry> {
        let name = device_name(&output);
        if name.is_empty() {
            return Vec::new();
        }
        self.autoeq_for_device(name.to_string(), limit).unwrap_or_default()
    }

    /// The AutoEQ browser's search: nothing until two characters are typed, then the first 40 hits.
    pub fn autoeq_find(&self, query: String) -> Vec<AutoEqEntry> {
        if autoeq_too_short(&query) {
            return Vec::new();
        }
        self.autoeq_search(query, 40).unwrap_or_default()
    }

    /// The AutoEQ browser's search as the screens show it: whether the query is too short to search,
    /// and the hits with the lines under them.
    pub fn autoeq_browse(&self, query: String) -> AutoEqFound {
        let too_short = autoeq_too_short(&query);
        AutoEqFound { too_short, hits: autoeq_hits(self.autoeq_find(query)) }
    }
}

impl Core {
    fn arrive_as(&self, output: String, now: &Now) -> DeviceArrival {
        let bound = self.profile_for_output(output.clone()).ok().flatten();
        alog::info(&format!("device sound: {output} -> {}", bound.as_ref().map_or("nothing chosen", |p| p.name.as_str())));
        let loose = self.loose();
        let quiet = self.quiet_list().contains(&output);
        let plan = device::on_arrival(Arrival { bound: bound.is_some(), per_output: now.per_output, speaker: output == SPEAKER, quiet, auto_apply: now.auto_apply });
        let mut step = Step::none();
        if plan.load_bound {
            if let Some(sound) = bound.and_then(|b| sound_from(&b.json)) {
                (step.effect.apply, step.loose) = loaded(sound, &now.sound, now.per_output, loose.is_some());
            }
        }
        if plan.restore {
            if let Some(json) = loose {
                step.loose = LooseChange::Clear;
                step.effect.apply = sound_from(&json);
            }
        }
        let entry = if plan.curve == CurveStep::None { None } else { self.autoeq_for_output(output.clone(), 5).into_iter().next() };
        let preset_url = entry.as_ref().map(autoeq::preset_url);
        DeviceArrival { effect: self.settle(&output, step), curve: plan.curve, entry, preset_url }
    }

    fn adopt_as(&self, output: &str, name: &str, preset: &str, live: bool, now: &Now) -> Result<Step, SoundError> {
        let sound = settings::import(now.sound.clone(), preset, "that preset had no filters in it")?;
        let created = self.save_bound(name, &sound, output)?;
        let (apply, loose) = if live { loaded(sound, &now.sound, now.per_output, self.loose().is_some()) } else { (None, LooseChange::Keep) };
        if live {
            alog::info(&format!("device sound: applied AutoEQ {name} to {output}"));
        }
        Ok(Step { quiet: Some(false), loose, effect: DeviceEffect { refresh: true, apply, arrive: false, created } })
    }

    fn assign_as(&self, output: &str, choice: ChoiceKind, profile: String, live: bool, now: &Now) -> Result<Step, SoundError> {
        let name = match choice {
            ChoiceKind::Automatic | ChoiceKind::Quiet => {
                let _ = self.profile_bind(output.to_string(), None);
                let effect = DeviceEffect { refresh: true, arrive: live, ..DeviceEffect::none() };
                return Ok(Step { quiet: Some(choice == ChoiceKind::Quiet), loose: LooseChange::Keep, effect });
            }
            ChoiceKind::Flat => {
                // "Flat" is made the first time it is chosen: the equalizer off, everything else as it is now.
                if !self.profiles()?.iter().any(|p| p.name == FLAT) {
                    let flat = SoundSettings { eq_enabled: false, ..now.sound.clone() };
                    self.profile_save(SoundProfile { name: FLAT.to_string(), json: sound_json(&flat), outputs: Vec::new() })?;
                }
                FLAT.to_string()
            }
            ChoiceKind::Profile => profile,
        };
        self.profile_bind(output.to_string(), Some(name.clone()))?;
        let mut step = Step { quiet: Some(false), loose: LooseChange::Keep, effect: DeviceEffect { refresh: true, ..DeviceEffect::none() } };
        if live {
            if let Some(sound) = self.profiles()?.into_iter().find(|p| p.name == name).and_then(|p| sound_from(&p.json)) {
                (step.effect.apply, step.loose) = loaded(sound, &now.sound, now.per_output, self.loose().is_some());
            }
        }
        Ok(step)
    }

    /// Keeps the step's own part (the quiet mark, the kept sound) and hands back the platform's.
    fn settle(&self, output: &str, step: Step) -> DeviceEffect {
        if let Some(on) = step.quiet {
            self.set_quiet(output, on);
        }
        self.set_loose(step.loose);
        step.effect
    }

    fn kv(&self, key: &str) -> Option<String> {
        let c = self.db.lock();
        c.query_row("SELECT value FROM app_kv WHERE key=?1", [key], |r| r.get(0)).optional().ok().flatten()
    }

    fn set_kv(&self, key: &str, value: Option<&str>) {
        let c = self.db.lock();
        let done = match value {
            Some(v) => c.execute("INSERT OR REPLACE INTO app_kv(key, value) VALUES(?1, ?2)", [key, v]),
            None => c.execute("DELETE FROM app_kv WHERE key=?1", [key]),
        };
        if let Err(e) = done {
            alog::info(&format!("device sound: could not keep {key}: {e}"));
        }
    }

    fn loose(&self) -> Option<String> {
        self.kv(LOOSE)
    }

    fn set_loose(&self, change: LooseChange) {
        match change {
            LooseChange::Keep => {}
            LooseChange::Store { json } => self.set_kv(LOOSE, Some(&json)),
            LooseChange::Clear => self.set_kv(LOOSE, None),
        }
    }

    fn quiet_list(&self) -> Vec<String> {
        self.kv(QUIET).and_then(|j| serde_json::from_str(&j).ok()).unwrap_or_default()
    }

    fn set_quiet(&self, output: &str, on: bool) {
        let mut list = self.quiet_list();
        let had = list.iter().any(|o| o == output);
        if had == on {
            return;
        }
        if on {
            list.push(output.to_string());
        } else {
            list.retain(|o| o != output);
        }
        self.set_kv(QUIET, Some(&serde_json::to_string(&list).unwrap_or_default()));
    }

    /// Saves `sound` as the profile `name`, keeping the devices it already had, and binds `output` to it
    /// alone. True when the profile is new.
    fn save_bound(&self, name: &str, sound: &SoundSettings, output: &str) -> Result<bool, CoreError> {
        let old = self.profiles()?.into_iter().find(|p| p.name == name);
        let created = old.is_none();
        self.profile_save(SoundProfile { name: name.to_string(), json: sound_json(sound), outputs: old.map(|p| p.outputs).unwrap_or_default() })?;
        self.profile_bind(output.to_string(), Some(name.to_string()))?;
        Ok(created)
    }
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

    fn core() -> std::sync::Arc<Core> {
        Core::new(String::new(), "t".into()).unwrap()
    }

    fn sound() -> SoundSettings {
        sound_from("{}").unwrap()
    }

    fn now(sound: SoundSettings) -> Now {
        Now { sound, per_output: true, auto_apply: false }
    }

    #[test]
    fn the_device_sheet_and_the_curves_are_worded() {
        let names = vec![FLAT.to_string(), "Warm".to_string()];
        let s = sheet("USB: K3", Some("USB"), false, false, &names);
        assert_eq!(s.intro, "What music played through this USB device sounds like. It switches by itself whenever the device connects.");
        assert_eq!(s.automatic, "Offers a matching AutoEQ curve when one is known");
        assert_eq!(s.profiles, ["Warm"]);
        assert!(s.can_forget);
        let s = sheet("x", None, true, true, &[]);
        assert_eq!(s.intro, "What music played through this sounds like. It switches by itself whenever the device connects.");
        assert_eq!(s.automatic, "Uses a matching AutoEQ curve when one is known");
        assert!(!s.can_forget, "not the one playing");
        assert!(!sheet(SPEAKER, None, false, false, &[]).can_forget, "not the speaker");
        assert_eq!((device_playing_now(true), device_playing_now(false)), (" · Playing now".into(), "Playing now".into()));
        let e = |target: &str| AutoEqEntry { name: "HD 600".into(), source: "oratory1990".into(), form: "over-ear".into(), target: target.into(), path: "p".into() };
        let h = autoeq_hits(vec![e("Harman"), e("")]);
        assert_eq!(h[0].caption, "oratory1990 · over-ear · Harman");
        assert_eq!(h[1].caption, "oratory1990 · over-ear");
        assert_eq!(h[0].short, "oratory1990 · over-ear");
        assert!(autoeq_too_short("a") && autoeq_too_short("") && !autoeq_too_short("hd"));
        assert_eq!(autoeq_count_words(8123), AutoEqCount { count: "8123 headphones".into(), search: "Search 8123 headphones".into() });
        let c = core();
        assert!(c.autoeq_browse("h".into()).too_short);
        assert!(!c.autoeq_browse("hd".into()).too_short);
    }

    const PRESET: &str = "Preamp: -6.2 dB\nFilter 1: ON PK Fc 105 Hz Gain -3.5 dB Q 0.70\n";

    #[test]
    fn a_bound_device_loads_its_sound_and_keeps_the_one_before() {
        let c = core();
        let warm = SoundSettings { crossfeed_db: 3.0, ..sound() };
        c.profile_save(SoundProfile { name: "Warm".into(), json: sound_json(&warm), outputs: vec!["USB: K3".into()] }).unwrap();
        let playing = SoundSettings { balance: 0.5, ..sound() };
        let a = c.arrive_as("USB: K3".into(), &now(playing.clone()));
        assert_eq!(a.effect.apply, Some(warm.clone()));
        assert_eq!(c.loose(), Some(sound_json(&playing)), "the sound from before is kept");
        assert_eq!(a.curve, CurveStep::None);
        // A sound already kept stays the one kept.
        c.arrive_as("USB: K3".into(), &now(sound()));
        assert_eq!(c.loose(), Some(sound_json(&playing)));
        // With per-device sound off nothing is loaded.
        let off = Now { per_output: false, ..now(playing) };
        assert_eq!(c.arrive_as("USB: K3".into(), &off).effect.apply, None);
    }

    #[test]
    fn an_unbound_device_gets_the_sound_from_before_back() {
        let c = core();
        let kept = SoundSettings { mono: true, ..sound() };
        c.set_loose(LooseChange::Store { json: sound_json(&kept) });
        let a = c.arrive_as(SPEAKER.into(), &now(sound()));
        assert_eq!(a.effect.apply, Some(kept));
        assert_eq!(c.loose(), None, "used, so no longer kept");
        assert_eq!(a.curve, CurveStep::None);
        c.set_loose(LooseChange::Store { json: "garbage".into() });
        let a = c.arrive_as(SPEAKER.into(), &now(sound()));
        assert_eq!((a.effect.apply, c.loose()), (None, None), "a kept sound that does not read is dropped");
        let a = c.arrive_as("Bluetooth: Buds".into(), &now(sound()));
        assert_eq!(a.curve, CurveStep::Offer);
        assert_eq!(a.entry, None, "no index, nothing to offer");
        assert_eq!(a.effect, DeviceEffect::none());
    }

    #[test]
    fn a_quiet_device_is_never_offered_a_curve() {
        let c = core();
        c.set_quiet("Bluetooth: Buds", true);
        c.set_quiet("Bluetooth: Buds", true);
        assert_eq!(c.device_quiet(), ["Bluetooth: Buds"]);
        assert_eq!(c.arrive_as("Bluetooth: Buds".into(), &now(sound())).curve, CurveStep::None);
        c.device_forget("Bluetooth: Buds".into());
        assert!(c.device_quiet().is_empty(), "forgetting a device takes its never-ask with it");
        assert_eq!(c.arrive_as("Bluetooth: Buds".into(), &now(sound())).curve, CurveStep::Offer);
    }

    #[test]
    fn adopting_a_curve_saves_binds_and_loads_it() {
        let c = core();
        c.set_quiet("Bluetooth: X", true);
        let playing = SoundSettings { balance: 0.5, ..sound() };
        let step = c.adopt_as("Bluetooth: X", "Sony", PRESET, true, &now(playing.clone())).unwrap();
        let e = c.settle("Bluetooth: X", step);
        assert!(e.created);
        assert!(e.refresh);
        assert!(c.device_quiet().is_empty(), "a device given a curve is no longer quiet");
        let applied = e.apply.unwrap();
        assert!(applied.eq_enabled);
        assert_eq!(applied.eq_preamp_db, Some(-6.2));
        assert_eq!(applied.balance, 0.5, "on top of the sound as it is");
        assert_eq!(c.loose(), Some(sound_json(&playing)));
        assert_eq!(c.profile_for_output("Bluetooth: X".into()).unwrap().unwrap().name, "Sony");
        // The same curve for another device: the profile is not new, and serves both.
        let again = c.adopt_as("USB: Y", "Sony", PRESET, false, &now(playing)).unwrap();
        assert!(!again.effect.created);
        assert_eq!(again.effect.apply, None);
        let p = c.profiles().unwrap().into_iter().find(|p| p.name == "Sony").unwrap();
        assert_eq!(p.outputs, ["Bluetooth: X", "USB: Y"]);
        let err = c.adopt_as("x", "Empty", "nothing", true, &now(sound())).unwrap_err();
        assert_eq!(err.to_string(), "that preset had no filters in it");
    }

    #[test]
    fn undo_puts_everything_back() {
        let c = core();
        let step = c.adopt_as("Bluetooth: X", "Sony", PRESET, true, &now(sound())).unwrap();
        c.settle("Bluetooth: X", step);
        let before = SoundSettings { mono: true, ..sound() };
        let e = c.device_undo("Bluetooth: X".into(), "Sony".into(), true, before.clone());
        assert_eq!(e, DeviceEffect { refresh: true, apply: Some(before), arrive: false, created: false });
        assert!(c.profiles().unwrap().is_empty(), "the profile it made is gone");
        assert_eq!(c.device_quiet(), ["Bluetooth: X"], "and the device is not offered one again");
        assert_eq!(c.loose(), None);
        // A profile that was there before stays.
        let step = c.adopt_as("Bluetooth: X", "Sony", PRESET, true, &now(sound())).unwrap();
        c.settle("Bluetooth: X", step);
        c.device_undo("Bluetooth: X".into(), "Sony".into(), false, sound());
        assert_eq!(c.profiles().unwrap().len(), 1);
    }

    #[test]
    fn choices_from_the_device_list() {
        let c = core();
        c.set_loose(LooseChange::Store { json: "{}".into() });
        let playing = SoundSettings { eq_enabled: true, crossfeed_db: 2.0, ..sound() };
        let s = c.assign_as("USB: K3", ChoiceKind::Flat, String::new(), true, &now(playing.clone())).unwrap();
        let flat = s.effect.apply.clone().unwrap();
        assert!(!flat.eq_enabled);
        assert_eq!(flat.crossfeed_db, 2.0);
        assert_eq!(s.loose, LooseChange::Keep, "a sound was already kept");
        assert_eq!(s.quiet, Some(false));
        assert_eq!(c.profile_for_output("USB: K3".into()).unwrap().unwrap().name, FLAT);
        // "Flat" is made once; choosing it again does not change it.
        c.assign_as("Wired headphones", ChoiceKind::Flat, String::new(), false, &now(sound())).unwrap();
        assert_eq!(sound_from(&c.profiles().unwrap()[0].json).unwrap().crossfeed_db, 2.0);

        let q = c.assign_as("USB: K3", ChoiceKind::Quiet, String::new(), true, &now(sound())).unwrap();
        assert_eq!((q.quiet, q.effect.refresh, q.effect.arrive, q.effect.apply), (Some(true), true, true, None));
        assert!(c.profile_for_output("USB: K3".into()).unwrap().is_none());
        let a = c.assign_as("USB: K3", ChoiceKind::Automatic, String::new(), false, &now(sound())).unwrap();
        assert_eq!((a.quiet, a.effect.arrive), (Some(false), false));

        c.profile_save_sound(" Warm ".into(), playing.clone()).unwrap();
        let p = c.assign_as("USB: K3", ChoiceKind::Profile, "Warm".into(), false, &now(sound())).unwrap();
        assert_eq!(p.effect.apply, None, "not playing, nothing loaded");
        assert_eq!(c.profile_for_output("USB: K3".into()).unwrap().unwrap().name, "Warm");
        // Saving over it keeps the device.
        c.profile_save_sound("Warm".into(), sound()).unwrap();
        assert_eq!(c.profile_for_output("USB: K3".into()).unwrap().unwrap().json, sound_json(&sound()));
        c.device_forget("USB: K3".into());
        assert!(c.profile_for_output("USB: K3".into()).unwrap().is_none());
    }

    #[test]
    fn curves_are_only_looked_for_by_a_devices_own_name() {
        let c = core();
        assert!(c.autoeq_for_output(SPEAKER.into(), 5).is_empty());
        assert!(c.autoeq_for_output("USB: ".into(), 5).is_empty());
        assert_eq!(device_name("Bluetooth: LE_WH-1000XM5"), "LE_WH-1000XM5");
        assert!(c.autoeq_find("a".into()).is_empty());
    }

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
