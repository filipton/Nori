//! The settings as a model, for a client to build its own settings screen on: every setting there is to
//! show, by the name [`setting_set`] takes, with what it holds and the values it offers (values, never
//! words), the current values in that same form, and the facts about them that follow the core's rules -
//! whether the output is played untouched, whether the sound chain is on, whether the battery saver stands
//! down, the lyrics services in the order they are asked, and how the beat model's download stands.
//!
//! How a settings screen is laid out, which rows it has and what they say are the client's own: Android
//! keeps its words in string resources, the terminal client words things its own way.

use std::collections::HashMap;

use nori_automix::beat_model;
use nori_player::automix::beats;

use crate::lyrics_sources::{self, LyricsService};
use crate::settings::{
    quality_name, SettingChange, StoredPrefs, AUTO_FILL_BASES, AUTO_FILL_KINDS, EQ_RANGES, REPLAY_GAIN_MODES, SWIPE_ACTION_NAMES, TAP_ACTION_NAMES,
    THEME_MODES,
};

/// What a setting holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum SettingKind {
    /// On or off: "true" or "false".
    Switch,
    /// One of [`SettingSpec::options`]; the music folder's options are the server's folders' ids, with ""
    /// for all of them.
    Choice,
    /// A number from [`SettingSpec::min`] to [`SettingSpec::max`], dragged on a slider.
    Level,
    /// Text typed in (a service's key).
    Text,
    /// An accent colour, one of [`SettingSpec::options`] (ARGB numbers).
    Colour,
}

/// One setting: its name (what [`setting_set`] takes and a client sends back), what it holds, the values
/// it offers in the order they are offered, its range, and its value out of the box. Values are in the
/// form [`SettingsState::values`] holds them, so the one chosen is found by comparing strings.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingSpec {
    pub name: String,
    pub kind: SettingKind,
    pub options: Vec<String>,
    pub min: f32,
    pub max: f32,
    pub default: String,
}

enum K {
    Switch,
    Choice(&'static [&'static str]),
    /// An enum setting: its values by name, in ordinal order.
    Named(&'static [&'static str]),
    Level(f32, f32),
    Text,
    Colour,
}

/// The stream qualities offered, the original file first.
const QUALITIES: [&str; 6] = ["0:", "320:mp3", "192:opus", "128:opus", "96:opus", "64:opus"];

/// Every setting a settings screen can offer, grouped as they relate, not as any screen lays them out.
const SPECS: &[(&str, K)] = &[
    // Between songs.
    ("crossfadeSec", K::Choice(&["0", "2", "4", "6", "8", "12"])),
    ("autoMix", K::Switch),
    ("autoMixMaxS", K::Choice(&["6", "8", "12", "16", "24"])),
    ("autoMixBeatMatch", K::Switch),
    ("autoMixMaxTempoPct", K::Choice(&["2", "4", "6", "8"])),
    ("autoMixKeepPitch", K::Switch),
    ("autoMixBassSwap", K::Switch),
    ("autoMixFilters", K::Switch),
    ("autoMixEchoOut", K::Switch),
    ("autoMixBetterBeats", K::Switch),
    ("autoMixBeatsMobileData", K::Switch),
    ("crossfadeKeepAlbums", K::Switch),
    ("fadeMs", K::Choice(&["0", "150", "300", "500", "1000"])),
    // Controls.
    ("previousAlwaysSkips", K::Switch),
    ("speed", K::Choice(&["0.75", "1", "1.25", "1.5", "2"])),
    ("pitch", K::Choice(&["0.9", "0.95", "1", "1.05", "1.1"])),
    ("skipSilence", K::Switch),
    // The queue.
    ("skipExplicit", K::Switch),
    ("autoFill", K::Switch),
    ("autoFillKind", K::Named(&AUTO_FILL_KINDS)),
    ("autoFillBasis", K::Named(&AUTO_FILL_BASES)),
    ("skipOnError", K::Switch),
    ("bridgeOffline", K::Switch),
    // Sound.
    ("eq", K::Switch),
    ("mono", K::Switch),
    ("limiter", K::Switch),
    ("eqPreampDb", K::Level(EQ_RANGES.preamp.min, EQ_RANGES.preamp.max)),
    ("crossfeedDb", K::Level(EQ_RANGES.crossfeed.min, EQ_RANGES.crossfeed.max)),
    ("limiterThresholdDb", K::Level(EQ_RANGES.limiter.min, EQ_RANGES.limiter.max)),
    ("autoEqAuto", K::Switch),
    ("autoEqDownload", K::Switch),
    ("profilePerOutput", K::Switch),
    ("replayGain", K::Named(&REPLAY_GAIN_MODES)),
    ("preampDb", K::Level(EQ_RANGES.replay_gain_preamp.min, EQ_RANGES.replay_gain_preamp.max)),
    ("untaggedGainDb", K::Choice(&["0", "-3", "-6", "-9", "-12"])),
    ("hiRes", K::Switch),
    ("bitPerfect", K::Switch),
    ("offload", K::Switch),
    // Appearance.
    ("theme", K::Named(&THEME_MODES)),
    ("amoled", K::Switch),
    ("playerColours", K::Switch),
    ("dynamicColor", K::Switch),
    ("accent", K::Colour),
    ("coverColors", K::Switch),
    ("softSleeve", K::Switch),
    ("motionArtwork", K::Switch),
    // "On mobile data" is "Wi-Fi only" turned round; either may be offered.
    ("motionArtworkMobile", K::Switch),
    ("motionArtworkWifiOnly", K::Switch),
    ("favouriteNotice", K::Switch),
    ("uiScale", K::Choice(&["0", "0.9", "1", "1.1"])),
    ("reduceMotion", K::Switch),
    ("ignoreSystemMotion", K::Switch),
    // Lyrics.
    ("lyricsSweep", K::Switch),
    ("lyricsSize", K::Choice(&["0", "1", "2"])),
    ("lyricsTranslation", K::Switch),
    ("lyricsKeepScreenOn", K::Switch),
    ("lyricsOnline", K::Switch),
    ("lyricsPreferWords", K::Switch),
    ("paxSenixKey", K::Text),
    ("betterLyricsKey", K::Text),
    // The library.
    ("tapAction", K::Named(&TAP_ACTION_NAMES)),
    ("swipeRight", K::Named(&SWIPE_ACTION_NAMES)),
    ("swipeLeft", K::Named(&SWIPE_ACTION_NAMES)),
    ("liveSearchDelayMs", K::Choice(&["150", "250", "350", "500", "800"])),
    ("tasteModel", K::Switch),
    ("scrobble", K::Switch),
    ("scrobblePercent", K::Choice(&["25", "50", "75", "90", "100"])),
    ("thirdPartyLookups", K::Switch),
    // Downloads and storage.
    ("wifi", K::Choice(&QUALITIES)),
    ("mobile", K::Choice(&QUALITIES)),
    ("download", K::Choice(&QUALITIES)),
    ("parallelDownloads", K::Choice(&["1", "2", "3", "4", "5", "6", "7", "8", "9", "10"])),
    ("precacheWifi", K::Choice(&["1", "2", "3", "5", "10"])),
    ("precacheMobile", K::Choice(&["1", "2", "3", "5"])),
    ("coversAhead", K::Choice(&["0", "1", "2", "3", "5", "8", "10"])),
    ("cacheMb", K::Choice(&["256", "1024", "4096", "16384"])),
    // The server in use: which music folder it browses, and the cap on its second address.
    ("musicFolder", K::Choice(&[])),
    ("altMaxBitRate", K::Choice(&["0", "320", "192", "128", "96"])),
];

fn spec(name: &str, k: &K) -> SettingSpec {
    let (kind, options, (min, max)): (SettingKind, Vec<String>, (f32, f32)) = match k {
        K::Switch => (SettingKind::Switch, vec!["false".into(), "true".into()], (0.0, 0.0)),
        K::Choice(o) | K::Named(o) => (SettingKind::Choice, o.iter().map(|s| s.to_string()).collect(), (0.0, 0.0)),
        K::Level(lo, hi) => (SettingKind::Level, Vec::new(), (*lo, *hi)),
        K::Text => (SettingKind::Text, Vec::new(), (0.0, 0.0)),
        K::Colour => (SettingKind::Colour, nori_look::theme::ACCENTS.iter().map(|c| (*c as i64).to_string()).collect(), (0.0, 0.0)),
    };
    let default = value_of(&StoredPrefs::default(), name).unwrap_or_default();
    SettingSpec { name: name.into(), kind, options, min, max, default }
}

/// Every setting a client can offer, in a stable order.
pub fn specs() -> Vec<SettingSpec> {
    SPECS.iter().map(|(n, k)| spec(n, k)).collect()
}

/// An enum setting's value by name; the number itself for one out of range.
fn named(names: &[&str], v: i32) -> String {
    usize::try_from(v).ok().and_then(|i| names.get(i)).map_or_else(|| v.to_string(), |n| n.to_string())
}

/// A setting's value now, in the form its options are in (a float as Rust writes it: "1", "0.75").
/// The switches that also need looking things up (lyrics online, moving covers, the AutoEQ list) read as
/// they are in effect: off while looking things up is off.
pub fn value_of(p: &StoredPrefs, name: &str) -> Option<String> {
    let b = |v: bool| v.to_string();
    let server = p.servers.iter().find(|s| s.id == p.active_server_id);
    Some(match name {
        "crossfadeSec" => p.crossfade_sec.to_string(),
        "autoMix" => b(p.auto_mix),
        "autoMixMaxS" => p.auto_mix_max_s.to_string(),
        "autoMixBeatMatch" => b(p.auto_mix_beat_match),
        "autoMixMaxTempoPct" => p.auto_mix_max_tempo_pct.to_string(),
        "autoMixKeepPitch" => b(p.auto_mix_keep_pitch),
        "autoMixBassSwap" => b(p.auto_mix_bass_swap),
        "autoMixFilters" => b(p.auto_mix_filters),
        "autoMixEchoOut" => b(p.auto_mix_echo_out),
        "autoMixBetterBeats" => b(p.auto_mix_better_beats),
        "autoMixBeatsMobileData" => b(p.auto_mix_beats_mobile_data),
        "crossfadeKeepAlbums" => b(p.crossfade_keep_albums),
        "fadeMs" => p.fade_ms.to_string(),
        "previousAlwaysSkips" => b(p.previous_always_skips),
        "speed" => p.speed.to_string(),
        "pitch" => p.pitch.to_string(),
        "skipSilence" => b(p.skip_silence),
        "skipExplicit" => b(p.skip_explicit),
        "autoFill" => b(p.auto_fill),
        "autoFillKind" => named(&AUTO_FILL_KINDS, p.auto_fill_kind),
        "autoFillBasis" => named(&AUTO_FILL_BASES, p.auto_fill_basis),
        "skipOnError" => b(p.skip_on_error),
        "bridgeOffline" => b(p.bridge_offline),
        "eq" => b(p.eq_enabled),
        "mono" => b(p.mono),
        "limiter" => b(p.limiter),
        "eqPreampDb" => p.eq_preamp_db.map_or_else(|| "auto".to_string(), |v| v.to_string()),
        "crossfeedDb" => p.crossfeed_db.to_string(),
        "limiterThresholdDb" => p.limiter_threshold_db.to_string(),
        "autoEqAuto" => b(p.auto_eq_auto),
        "autoEqDownload" => b(p.auto_eq_download && p.third_party_lookups),
        "profilePerOutput" => b(p.profile_per_output),
        "replayGain" => named(&REPLAY_GAIN_MODES, p.replay_gain),
        "preampDb" => p.preamp_db.to_string(),
        "untaggedGainDb" => p.untagged_gain_db.to_string(),
        "hiRes" => b(p.hi_res),
        "bitPerfect" => b(p.bit_perfect),
        "offload" => b(p.offload),
        "theme" => named(&THEME_MODES, p.theme),
        "amoled" => b(p.amoled),
        "playerColours" => b(p.player_colours),
        "dynamicColor" => b(p.dynamic_color),
        "accent" => p.accent.to_string(),
        "coverColors" => b(p.cover_colors),
        "softSleeve" => b(p.soft_sleeve),
        "motionArtwork" => b(p.motion_artwork && p.third_party_lookups),
        "motionArtworkMobile" => b(!p.motion_artwork_wifi_only),
        "motionArtworkWifiOnly" => b(p.motion_artwork_wifi_only),
        "favouriteNotice" => b(p.favourite_notice),
        "uiScale" => p.ui_scale.to_string(),
        "reduceMotion" => b(p.reduce_motion),
        "ignoreSystemMotion" => b(p.ignore_system_motion),
        "lyricsSweep" => b(p.lyrics_sweep),
        "lyricsSize" => p.lyrics_size.to_string(),
        "lyricsTranslation" => b(p.lyrics_translation),
        "lyricsKeepScreenOn" => b(p.lyrics_keep_screen_on),
        "lyricsOnline" => b(p.lyrics_online && p.third_party_lookups),
        "lyricsPreferWords" => b(p.lyrics_prefer_words),
        "paxSenixKey" => p.paxsenix_key.clone(),
        "betterLyricsKey" => p.better_lyrics_key.clone(),
        "tapAction" => named(&TAP_ACTION_NAMES, p.tap_action),
        "swipeRight" => named(&SWIPE_ACTION_NAMES, p.swipe_right),
        "swipeLeft" => named(&SWIPE_ACTION_NAMES, p.swipe_left),
        "liveSearchDelayMs" => p.live_search_delay_ms.to_string(),
        "tasteModel" => b(p.taste_model),
        "scrobble" => b(p.scrobble),
        "scrobblePercent" => p.scrobble_percent.to_string(),
        "thirdPartyLookups" => b(p.third_party_lookups),
        "wifi" => quality_name(&p.wifi),
        "mobile" => quality_name(&p.mobile),
        "download" => quality_name(&p.download),
        "parallelDownloads" => p.parallel_downloads.to_string(),
        "precacheWifi" => p.precache_wifi.to_string(),
        "precacheMobile" => p.precache_mobile.to_string(),
        "coversAhead" => p.covers_ahead.to_string(),
        "cacheMb" => p.cache_mb.to_string(),
        "musicFolder" => server.map(|s| s.music_folder_id.clone()).unwrap_or_default(),
        "altMaxBitRate" => server.map_or(0, |s| s.alt_max_bit_rate).to_string(),
        _ => return None,
    })
}

/// One lyrics service, where it stands in the order they are asked, and whether it is switched on. It is
/// switched with the setting `lyricsService:<id>`, dropped at a place with `lyricsPlace` (`<id>:<place>`)
/// and moved a place with `lyricsMove` (`<id>:-1` or `<id>:1`).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct LyricsSource {
    pub id: String,
    pub on: bool,
    /// The finest timing it can answer with: 3 word by word, 2 line by line, 1 not timed.
    pub timing: u8,
    /// It is asked only with a key of its own (PaxSenix's).
    pub needs_key: bool,
}

/// How "Better beat detection"'s model stands.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum BeatModel {
    /// This build has no runtime for it: the setting is not offered.
    Unavailable,
    /// Not on the device yet; it comes the next time AutoMix measures a song.
    Absent,
    WaitingForWifi,
    Downloading,
    Ready,
    Failed { why: nori_automix::beat_model::BeatFailure },
}

/// What a settings screen needs besides the settings' own values, worked out by the core's rules.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingsState {
    /// Every setting's value now ([`value_of`]), by name.
    pub values: HashMap<String, String>,
    /// The samples go out untouched (high quality output, or a USB DAC in bit-perfect mode): every
    /// transition, skipping silence and the sound chain are out of the path.
    pub untouched: bool,
    /// Whether the dac is what makes it untouched (rather than high quality output).
    pub untouched_by_dac: bool,
    /// Any of the equalizer, crossfeed, balance, mono or the limiter is on.
    pub sound_chain_on: bool,
    /// The battery saver is asked for and the player has stood it down (an effect is on).
    pub offload_paused: bool,
    /// Every lyrics service, in the order they are asked.
    pub lyrics_sources: Vec<LyricsSource>,
    pub beat_model: BeatModel,
    /// About how big the beat model's download is, in megabytes.
    pub beat_model_mb: u32,
}

/// What the output is, as the platform sees it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Output {
    /// A USB DAC is taking the file in bit-perfect mode.
    pub dac_bit_perfect: bool,
    /// Something USB is attached.
    pub usb: bool,
}

/// Whether the samples go out untouched: bit-perfect output and high quality output both hand exactly
/// the file's samples to the DAC, so nothing may be mixed into them.
pub fn untouched(hi_res: bool, dac_bit_perfect: bool) -> bool {
    hi_res || dac_bit_perfect
}

fn beat_model_now() -> BeatModel {
    if !beats::AVAILABLE {
        return BeatModel::Unavailable;
    }
    match beat_model::state() {
        beat_model::State::Absent => BeatModel::Absent,
        beat_model::State::WaitingForWifi => BeatModel::WaitingForWifi,
        beat_model::State::Downloading => BeatModel::Downloading,
        // Ready only while the file is there.
        beat_model::State::Ready if beat_model::ready().is_some() => BeatModel::Ready,
        beat_model::State::Ready => BeatModel::Absent,
        beat_model::State::Failed(why) => BeatModel::Failed { why },
    }
}

/// [`SettingsState`] for these settings and this output.
pub fn state(p: &StoredPrefs, out: Output) -> SettingsState {
    let dsp = nori_player::sound::sound_on(p.eq_enabled, p.crossfeed_db, p.balance, p.mono, p.limiter);
    let prefs = nori_model::AudioPrefs {
        dsp,
        skip_silence: p.skip_silence,
        offload: p.offload,
        crossfade_s: p.crossfade_sec,
        auto_mix: p.auto_mix,
        speed: p.speed,
        pitch: p.pitch,
    };
    // A DAC stands in for "anything USB", and a refused offload is the service's own to know.
    let output = nori_model::OutputState { hi_res: p.hi_res, bit_perfect: out.dac_bit_perfect, usb: out.usb, offload_refused: false };
    let policy = nori_player::policy::audio_policy(&prefs, &output);
    let on = lyrics_sources::switched_on(p);
    let lyrics_sources = lyrics_sources::complete_order(&p.lyrics_order)
        .iter()
        .filter_map(|n| LyricsService::named(n))
        .map(|s| LyricsSource { id: s.name().into(), on: on.contains(&s), timing: s.best(), needs_key: s.needs().is_some() })
        .collect();
    SettingsState {
        values: specs().into_iter().filter_map(|s| Some((s.name.clone(), value_of(p, &s.name)?))).collect(),
        untouched: untouched(p.hi_res, out.dac_bit_perfect),
        untouched_by_dac: out.dac_bit_perfect,
        sound_chain_on: dsp,
        offload_paused: !out.usb && p.offload && !policy.offload,
        lyrics_sources,
        beat_model: beat_model_now(),
        beat_model_mb: beat_model::SIZE_MB,
    }
}

// ---- the doors ----

/// Every setting a client can offer: its name, kind, options, range and default.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn setting_specs() -> Vec<SettingSpec> {
    specs()
}

/// [`SettingsState`] for the settings as they are kept now; `dac_bit_perfect` and `usb` are the
/// platform's view of the output.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn settings_state(dac_bit_perfect: bool, usb: bool) -> SettingsState {
    let p = crate::settings_store::current().unwrap_or_default();
    state(&p, Output { dac_bit_perfect, usb })
}

/// One setting's value changed (see `settings::set_by_name`), kept where the settings are kept: the
/// settings after it and what the player has to apply again, once, so the platform only takes them in.
/// `None` for a name that is not a setting. The active server's own settings (`server`) are not kept
/// here: the platform puts them through its server update, which connects again.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn setting_set(name: String, value: String) -> Option<SettingChange> {
    crate::settings_store::edit_by_name(&name, &value)
}

/// Whether the interface is dark for the theme setting (0 system, 1 light, 2 dark) and the system's own.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn theme_is_dark(theme: i32, system_dark: bool) -> bool {
    nori_look::theme::is_dark(theme, system_dark)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::settings::{set_by_name, SavedServer};

    #[test]
    fn every_setting_offered_is_one_the_core_takes_and_reads_back() {
        let p = StoredPrefs {
            servers: vec![SavedServer { id: "a".into(), ..SavedServer::default() }],
            active_server_id: "a".into(),
            ..StoredPrefs::default()
        };
        let all = specs();
        assert!(all.len() > 70, "{}", all.len());
        let mut names: Vec<&str> = all.iter().map(|s| s.name.as_str()).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), all.len(), "each once");
        for s in &all {
            assert!(value_of(&p, &s.name).is_some(), "{} has no value", s.name);
            // Each value offered is taken by name, and reads back as itself.
            for o in &s.options {
                let after = set_by_name(&p, &s.name, o).unwrap_or_else(|| panic!("{} does not take {o}", s.name)).prefs;
                assert_eq!(value_of(&after, &s.name).as_deref(), Some(o.as_str()), "{} = {o}", s.name);
            }
            // Out of the box, a choice's value is one it offers.
            if s.kind == SettingKind::Choice && !s.options.is_empty() {
                assert!(s.options.contains(&s.default), "{}: {} is not offered", s.name, s.default);
            }
            if s.kind == SettingKind::Level {
                assert!(s.min < s.max, "{}", s.name);
                let mid = ((s.min + s.max) / 2.0).to_string();
                assert!(set_by_name(&p, &s.name, &mid).is_some(), "{}", s.name);
            }
        }
        assert!(set_by_name(&p, "lyricsService:LRCLIB", "false").is_some());
    }

    #[test]
    fn values_read_as_the_options_do() {
        let d = StoredPrefs::default();
        assert_eq!(value_of(&d, "speed").as_deref(), Some("1"));
        assert_eq!(value_of(&d, "mobile").as_deref(), Some("192:opus"));
        assert_eq!(value_of(&d, "wifi").as_deref(), Some("0:"));
        assert_eq!(value_of(&d, "theme").as_deref(), Some("SYSTEM"));
        assert_eq!(value_of(&d, "swipeLeft").as_deref(), Some("FAVOURITE"));
        assert_eq!(value_of(&d, "untaggedGainDb").as_deref(), Some("-6"));
        assert_eq!(value_of(&StoredPrefs { pitch: 0.9, ..d.clone() }, "pitch").as_deref(), Some("0.9"));
        assert_eq!(value_of(&d, "nope"), None);
        // In effect: off while looking things up is off.
        let off = StoredPrefs { third_party_lookups: false, ..d.clone() };
        assert_eq!(value_of(&d, "lyricsOnline").as_deref(), Some("true"));
        assert_eq!(value_of(&off, "lyricsOnline").as_deref(), Some("false"));
        assert_eq!(value_of(&off, "autoEqDownload").as_deref(), Some("false"));
        assert_eq!(value_of(&StoredPrefs { motion_artwork: true, ..off }, "motionArtwork").as_deref(), Some("false"));
    }

    #[test]
    fn the_state_says_what_the_rules_make_of_the_settings() {
        let d = StoredPrefs::default();
        let s = state(&d, Output::default());
        assert!(!s.untouched && !s.sound_chain_on && !s.offload_paused);
        assert_eq!(s.values["crossfadeSec"], d.crossfade_sec.to_string());
        let hi = state(&StoredPrefs { hi_res: true, ..d.clone() }, Output::default());
        assert!(hi.untouched && !hi.untouched_by_dac);
        let dac = state(&d, Output { dac_bit_perfect: true, usb: true });
        assert!(dac.untouched && dac.untouched_by_dac);
        assert!(state(&StoredPrefs { mono: true, ..d.clone() }, Output::default()).sound_chain_on);
        // The battery saver stands down while an effect is on, but not over USB, where it is not offered.
        let eq = StoredPrefs { eq_enabled: true, offload: true, ..d.clone() };
        assert!(state(&eq, Output::default()).offload_paused);
        assert!(!state(&eq, Output { dac_bit_perfect: false, usb: true }).offload_paused);
        // Every lyrics service, in the order they are asked, each on or off where it stands.
        assert_eq!(s.lyrics_sources.iter().map(|l| l.id.clone()).collect::<Vec<_>>(), d.lyrics_order);
        let on: Vec<&str> = s.lyrics_sources.iter().filter(|l| l.on).map(|l| l.id.as_str()).collect();
        assert_eq!(on, ["PAXSENIX", "BINILYRICS", "UNISON", "BETTER_LYRICS", "KUGOU", "NETEASE", "LYRICS_PLUS", "SIMPMUSIC", "LRCLIB"]);
        let off = set_by_name(&d, "lyricsService:BINILYRICS", "false").unwrap().prefs;
        let s2 = state(&off, Output::default());
        assert_eq!(s2.lyrics_sources[1], LyricsSource { id: "BINILYRICS".into(), on: false, timing: 3, needs_key: false }, "switched off where it stands");
        assert!(s.lyrics_sources.iter().any(|l| l.needs_key));
        assert_eq!(s.beat_model == BeatModel::Unavailable, !beats::AVAILABLE);
        assert_eq!(s.beat_model_mb, beat_model::SIZE_MB);
    }
}
