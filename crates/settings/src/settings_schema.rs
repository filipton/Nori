//! The settings screen as data: its groups, what its search can find, and each group's page - every
//! row with its title, its words, its options and whether it is shown and live - worked out from the
//! settings and what the platform knows about the output. A platform draws the rows and sends back
//! the row's setting name with the value picked ([`setting_set`], the same names the debug test bridge
//! uses), so a second front end on this core shows the same settings, worded the same, under the same
//! rules. The page is asked for once per change of what it depends on, never per frame.

use std::sync::{Arc, OnceLock};

use nori_automix::beat_model;
use nori_library::pages::TextIndex;
use nori_model::MusicFolder;
use nori_player::automix::beats;

use crate::lyrics_sources::{self, LyricsService};
use crate::settings::{kotlin_float, label, quality_name, AUTO_FILL_BASIS_LABELS, AUTO_FILL_KIND_LABELS, SavedQuality, SettingChange, StoredPrefs};

/// A settings group: its own page, so the root of Settings is a few rows instead of eighty. The icon
/// is the platform's, by `id`.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingsGroup {
    pub id: String,
    pub title: String,
    pub summary: String,
}

/// Connect first, then what plays, then how it sounds, then how it looks.
const GROUPS: [(&str, &str, &str); 8] = [
    ("servers", "Servers", "Accounts and music folder"),
    ("playing", "Playback", "Crossfade, AutoMix, queue"),
    ("sound", "Sound", "Equalizer, volume, output"),
    ("look", "Appearance", "Theme, colours, size"),
    ("lyrics", "Lyrics", "Display and sources"),
    ("library", "Library", "Gestures, search, history"),
    ("data", "Downloads and storage", "Quality, downloads, space"),
    ("about", "About", "Version, build, licences"),
];

/// Pages reached from inside another page rather than from the list: About's licences.
const SUBPAGES: [(&str, &str, &str); 1] = [("licences", "Licences", "What this app is made of")];

fn group_title(id: &str) -> Option<&'static str> {
    GROUPS.iter().chain(SUBPAGES.iter()).find(|g| g.0 == id).map(|g| g.1)
}

/// What the search can find: which page a row lives on, its title, and the words under it. A row is
/// matched by its own title, so the entry here and the row on the page cannot drift apart in wording -
/// only in existence, which a missing result makes obvious. The hints are the rows' own descriptions,
/// plus the jargon someone might type (ReplayGain, AMOLED).
const INDEX: [(&str, &str, &str); 93] = [
    ("playing", "Crossfade", "One song fades into the next"),
    ("playing", "AutoMix", "Blends songs like a DJ, matching the beat"),
    ("playing", "Longest mix", ""),
    ("playing", "Match the beat", "Nudges the next song's speed so the beats line up"),
    ("playing", "Biggest speed change", ""),
    ("playing", "Keep the pitch", "Off lets the pitch follow the speed"),
    ("playing", "Swap the bass", "The new song's bass replaces the old one's"),
    ("playing", "Muffle the ending", "The outgoing song fades out muffled"),
    ("playing", "Echo out clashes", "Overlapping vocals end in an echo instead"),
    ("playing", "Better beat detection", "A neural beat tracker for AutoMix, downloaded once (Beat This!)"),
    ("playing", "Download over mobile data", "For the beat model; otherwise it waits for Wi-Fi"),
    ("playing", "Measured songs", "Tempo and beats, measured on this phone"),
    ("playing", "Keep albums gapless", "No mixing between songs of the same album"),
    ("playing", "Fade on play and pause", "A short fade when you play, pause, seek or skip"),
    ("playing", "Speed", ""),
    ("playing", "Pitch", ""),
    ("playing", "Skip silence", "Cuts quiet gaps in and between songs"),
    ("playing", "Previous goes back a song", "Instead of restarting the current one"),
    ("playing", "Skip explicit songs", "Songs your server marks explicit"),
    ("playing", "Keep playing when the queue ends", "Adds more music automatically"),
    ("playing", "Carry on with", "Songs, or a whole album at a time"),
    ("playing", "Chosen by", "Similar music, the same artist, genre or era"),
    ("playing", "Skip songs that won't play", "Up to three in a row"),
    ("playing", "Play downloads when offline", "If the server drops, keep playing from downloads"),
    ("sound", "Equalizer and crossfeed", ""),
    ("sound", "AutoEQ for headphones", "Applies a known correction curve when headphones connect"),
    ("sound", "Remember sound per device", "Each device keeps its own equalizer settings"),
    ("sound", "System audio effects", ""),
    ("sound", "Even out volume", "ReplayGain. Quiet and loud songs play at the same level"),
    ("sound", "Volume for songs without tags", ""),
    ("sound", "High quality output", "Plays 24-bit files in full. Turns the equalizer off"),
    ("sound", "Bit perfect USB DAC", "Sends the file to a USB DAC unchanged. Android 14 and later"),
    ("sound", "Save battery while playing", "Offload. The audio chip decodes instead of the processor"),
    ("look", "Theme", "Light, dark or the same as the phone"),
    ("look", "Black background", "AMOLED. True black in dark mode, saves power on OLED"),
    ("look", "Player in the cover's colours", "Off makes the player black too"),
    ("look", "Wallpaper colours", "Material You. Accent colour from your wallpaper"),
    ("look", "Colours from the cover", "Pages take their colours from the artwork"),
    ("look", "Blur the bottom of the cover", "The player's artwork softens into the page"),
    ("look", "Moving covers", "Animated album art in the player, when Apple Music has it. Sends the artist and album name"),
    ("look", "Moving covers on mobile data", "Each one is a few megabytes"),
    ("look", "Confirm favourites", "A short message when you favourite or unfavourite something"),
    ("look", "Text and button size", ""),
    ("look", "Less movement", "Shorter, simpler animations"),
    ("look", "Animate anyway", "Keeps animations on even when Android's are off"),
    ("lyrics", "Fill in words as they're sung", "For lyrics timed word by word"),
    ("lyrics", "Text size", ""),
    ("lyrics", "Show translations", "When your server has them"),
    ("lyrics", "Keep the screen on", "While lyrics are shown and music plays"),
    ("lyrics", "Find missing lyrics online", "Asks the lyrics services below. Sends the artist, song and album name"),
    ("lyrics", "Prefer word-by-word lyrics", "Keeps looking past lyrics timed line by line"),
    ("lyrics", "BiniLyrics", "Lyrics service. Apple Music's lyrics, syllable by syllable"),
    ("lyrics", "BetterLyrics", "Lyrics service. Apple Music's lyrics, syllable by syllable"),
    ("lyrics", "PaxSenix", "Lyrics service. Apple Music's lyrics, syllable by syllable"),
    ("lyrics", "LyricsPlus", "Lyrics service. YouLy+, syllable by syllable"),
    ("lyrics", "BetterLyrics Portato", "Lyrics service. QQ Music, word by word"),
    ("lyrics", "PaxSenix: Musixmatch", "Lyrics service. Word by word, with a key"),
    ("lyrics", "SimpMusic", "Lyrics service. Timed by listeners, matched on YouTube"),
    ("lyrics", "Unison", "Lyrics service. Open, written and timed by listeners, word by word"),
    ("lyrics", "NetEase Cloud Music", "Lyrics service. Word by word, strong on Chinese music"),
    ("lyrics", "KuGou", "Lyrics service. Word by word, Chinese, Japanese and Korean music"),
    ("lyrics", "LRCLIB", "Lyrics service. Open, run by volunteers"),
    ("lyrics", "PaxSenix: Spotify", "Lyrics service. Spotify's lyrics, with a key"),
    ("lyrics", "YouTube captions", "Lyrics service. Timed line by line"),
    ("lyrics", "Megalobiz", "Lyrics service. Timed line by line"),
    ("lyrics", "YouTube Music", "Lyrics service. Not timed"),
    ("lyrics", "Genius", "Lyrics service. Not timed"),
    ("lyrics", "PaxSenix key", "For its Spotify and Musixmatch lyrics"),
    ("lyrics", "BetterLyrics key", "Finds lyrics it has not stored yet"),
    ("library", "Tapping a song", ""),
    ("library", "Swipe right", ""),
    ("library", "Swipe left", ""),
    ("library", "Offline search", "Song names kept on the phone so search works without a connection"),
    ("library", "Search delay", "How long to wait after you stop typing"),
    ("library", "Keep listening history", "Stored on this phone. Powers mixes and stats"),
    ("library", "Tell the server what you play", "Scrobbling. Sends your plays to your server"),
    ("library", "Count a play after", ""),
    ("library", "Look things up online", "Update checks, missing lyrics and moving covers. Sends the artist, song and album name"),
    ("data", "Quality on Wi-Fi", ""),
    ("data", "Quality on mobile data", ""),
    ("data", "Quality for downloads", ""),
    ("data", "Downloads at once", "How many songs download at the same time"),
    ("data", "Download the whole library", ""),
    ("data", "Load ahead on Wi-Fi", "Songs fetched before you get to them"),
    ("data", "Load ahead on mobile data", ""),
    ("data", "Load covers ahead", ""),
    ("data", "Space for streamed music", "How much streamed music to keep on the phone"),
    ("data", "Stored on this phone", "Streamed music, covers, downloads and the library"),
    ("data", "Streamed music", "Clear the streamed music. Downloads stay"),
    ("data", "Covers", "Clear the covers. They are fetched again when needed"),
    ("about", "Licences", "Open source libraries, fonts and data, and their terms"),
    ("servers", "Music folder", ""),
    ("servers", "Bitrate limit on the second address", ""),
];

/// A row's key: its title, lowercased, with every run of anything but a-z and 0-9 made one '-'. Search
/// lands on a row by it.
pub fn setting_key(title: &str) -> String {
    let mut key = String::with_capacity(title.len());
    for c in title.to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            key.push(c);
        } else if !key.ends_with('-') {
            key.push('-');
        }
    }
    key.trim_matches('-').to_string()
}

/// One search result: the page it opens, the row it points at, and the line under its title (the
/// page's name, then the row's words).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingsHit {
    pub group: String,
    pub key: String,
    pub title: String,
    pub detail: String,
}

/// Rows only a build with the beat model's runtime has.
const BEAT_MODEL_ROWS: [&str; 2] = ["Better beat detection", "Download over mobile data"];

fn search(query: &str) -> Vec<SettingsHit> {
    static TEXT: OnceLock<Arc<TextIndex>> = OnceLock::new();
    let text = TEXT.get_or_init(|| TextIndex::new(INDEX.iter().map(|(_, t, h)| vec![t.to_string(), h.to_string()]).collect()));
    // Titles first, then anything whose explanation mentions it: "oled" finds AMOLED black.
    text.ranked(query.trim().to_string())
        .into_iter()
        .filter(|i| beats::AVAILABLE || !BEAT_MODEL_ROWS.contains(&INDEX[*i as usize].1))
        .map(|i| {
            let (group, title, hint) = INDEX[i as usize];
            let page = group_title(group).unwrap_or_default();
            SettingsHit {
                group: group.to_string(),
                key: setting_key(title),
                title: title.to_string(),
                detail: if hint.is_empty() { page.to_string() } else { format!("{page} · {hint}") },
            }
        })
        .collect()
}

// ---- a page ----

/// One choice in a list of options: what it says, and the value [`setting_set`] takes for it.
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingOption {
    pub label: String,
    pub value: String,
}

/// One row of a settings page. `key` is what search lands on; `name` is the setting it changes, for
/// [`setting_set`]; `enabled: false` is a setting the app is going to ignore right now - still there,
/// plainly not live.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Enum))]
pub enum SettingRow {
    Toggle { key: String, name: String, title: String, detail: String, on: bool, enabled: bool },
    /// `shown` is the chosen option's label (or the value itself when no option is it).
    Choice { key: String, name: String, title: String, options: Vec<SettingOption>, shown: String, enabled: bool },
    /// A line of explanation among the rows.
    Note { text: String },
    /// A row that opens something: another screen (`action`), with a status at its end. `dimmed`: what it
    /// opens is not reaching the sound right now.
    Link { key: String, title: String, status: String, dimmed: bool, action: String, divider: bool },
    /// A line of text and a button at its end: a count, an action. `error`: the line is a failure.
    Action { key: String, title: String, detail: String, button: String, enabled: bool, error: bool, action: String },
    /// A title and a line under it, nothing to press.
    Info { key: String, title: String, detail: String },
    /// `level`: the slider is dragged through `settings_store::edit_level` (a door that edits the kept
    /// settings in place on every step) instead of [`setting_set`] by `name`.
    Slider { name: String, label: String, value: f32, min: f32, max: f32, centred: bool, level: Option<crate::settings::EqLevel> },
    /// Colour swatches, ARGB; the chosen one is drawn larger.
    Palette { name: String, colours: Vec<i64>, chosen: i64 },
    /// A saved server: its name, a line about it, and whether it is the one in use.
    Server { id: String, label: String, detail: String, active: bool },
    Button { title: String, action: String },
    /// One of a ranked list of sources (the lyrics services): switched by [`setting_set`] with `name`,
    /// and, while `on`, held and dragged to move it past its neighbours, each place one `lyricsMove` of
    /// `id` by -1 or 1.
    Ranked { key: String, name: String, id: String, title: String, detail: String, on: bool },
    /// Text typed in (a service's key): `value` is what is stored, shown hidden when `secret`, and a new
    /// one goes back through [`setting_set`] with `name`.
    Text { key: String, name: String, title: String, detail: String, value: String, secret: bool },
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingsSection {
    pub title: String,
    pub rows: Vec<SettingRow>,
}

#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingsPage {
    pub title: String,
    /// Empty for a page the platform draws itself (About, Licences).
    pub sections: Vec<SettingsSection>,
}

/// The USB DAC as the platform sees it.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct DacFacts {
    pub device: Option<String>,
    pub bit_perfect: bool,
    pub sample_rate: u32,
    pub bits: u32,
    pub supported: bool,
    pub modes: Vec<String>,
    pub blocked_by: Option<String>,
    pub playing: Option<String>,
    pub track: Option<String>,
}

/// The offline index: what is on the phone, whether it is being filled, and why that failed.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SyncFacts {
    pub running: bool,
    pub songs: u32,
    pub albums: u32,
    pub artists: u32,
    pub error: Option<String>,
}

/// What lives on the phone, in bytes; `busy` while something is being cleared.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct StorageFacts {
    pub stream_bytes: i64,
    pub cover_bytes: i64,
    pub download_bytes: i64,
    pub download_songs: u32,
    pub index_bytes: i64,
    pub busy: bool,
}

/// What a page depends on besides the settings.
#[derive(Debug, Clone, Default, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct SettingsFacts {
    pub dac: DacFacts,
    /// The platform can take its accent from the wallpaper.
    pub wallpaper_colours: bool,
    /// The platform can blur the bottom of the player's cover.
    pub cover_blur: bool,
    /// Songs AutoMix has measured.
    pub analysed: u32,
    pub sync: SyncFacts,
    pub storage: StorageFacts,
    /// The active server's music folders.
    pub folders: Vec<MusicFolder>,
}

/// Whether the samples go out untouched: bit-perfect output and high quality output both hand exactly
/// the file's samples to the DAC, so nothing may be mixed into them - every transition, skipping
/// silence and the equalizer are out of the path while either is on.
pub fn untouched(hi_res: bool, dac_bit_perfect: bool) -> bool {
    hi_res || dac_bit_perfect
}

fn toggle(name: &str, title: &str, detail: &str, on: bool, enabled: bool) -> SettingRow {
    SettingRow::Toggle { key: setting_key(title), name: name.into(), title: title.into(), detail: detail.into(), on, enabled }
}

/// A list of options. `current` is the option that is chosen, if any; `fallback` is shown when none is.
fn choice(name: &str, title: &str, options: Vec<SettingOption>, current: Option<usize>, fallback: String, enabled: bool) -> SettingRow {
    let shown = current.map_or(fallback, |i| options[i].label.clone());
    SettingRow::Choice { key: setting_key(title), name: name.into(), title: title.into(), options, shown, enabled }
}

fn opts<T: ToString>(o: &[(T, &str)]) -> Vec<SettingOption> {
    o.iter().map(|(v, l)| SettingOption { label: l.to_string(), value: v.to_string() }).collect()
}

fn ints(name: &str, title: &str, value: i32, o: &[(i32, &str)], enabled: bool) -> SettingRow {
    choice(name, title, opts(o), o.iter().position(|x| x.0 == value), value.to_string(), enabled)
}

fn floats(name: &str, title: &str, value: f32, o: &[(f32, &str)], enabled: bool) -> SettingRow {
    choice(name, title, opts(o), o.iter().position(|x| x.0 == value), kotlin_float(value), enabled)
}

/// An enum setting, its options in ordinal order.
fn ordinal(name: &str, title: &str, value: i32, labels: &[&str]) -> SettingRow {
    let o: Vec<(i32, &str)> = labels.iter().enumerate().map(|(i, l)| (i as i32, *l)).collect();
    ints(name, title, value, &o, true)
}

/// The stream qualities offered, the original file first.
const QUALITIES: [(i32, &str, &str); 6] =
    [(0, "", "Original"), (320, "mp3", "MP3 320"), (192, "opus", "Opus 192"), (128, "opus", "Opus 128"), (96, "opus", "Opus 96"), (64, "opus", "Opus 64")];

fn quality(name: &str, title: &str, q: &SavedQuality) -> SettingRow {
    let o: Vec<SettingOption> = QUALITIES
        .iter()
        .map(|(r, f, l)| SettingOption { label: l.to_string(), value: quality_name(&SavedQuality { bit_rate: *r, format: f.to_string() }) })
        .collect();
    let at = QUALITIES.iter().position(|(r, f, _)| *r == q.bit_rate && *f == q.format);
    choice(name, title, o, at, format!("{}{}", q.bit_rate, q.format), true)
}

fn action(title: &str, detail: String, button: &str, enabled: bool, act: &str) -> SettingRow {
    SettingRow::Action { key: setting_key(title), title: title.into(), detail, button: button.into(), enabled, error: false, action: act.into() }
}

fn section(title: &str, rows: Vec<SettingRow>) -> SettingsSection {
    SettingsSection { title: title.into(), rows }
}

fn playing(p: &StoredPrefs, f: &SettingsFacts) -> Vec<SettingsSection> {
    let held = untouched(p.hi_res, f.dac.bit_perfect);
    let live = !held;
    let mut between = Vec::new();
    if held {
        let text = if f.dac.bit_perfect { "Off while the USB DAC is in bit perfect mode." } else { "Off while high quality output is on." };
        between.push(SettingRow::Note { text: text.into() });
    }
    // AutoMix plans its own transitions, so the plain crossfade gives way to it.
    if !p.auto_mix {
        let o = [(0, "Off"), (2, "2 s"), (4, "4 s"), (6, "6 s"), (8, "8 s"), (12, "12 s")];
        between.push(ints("crossfadeSec", "Crossfade", p.crossfade_sec, &o, live));
    }
    between.push(toggle("autoMix", "AutoMix", "Blends songs like a DJ, matching the beat.", p.auto_mix, live));
    if p.auto_mix {
        let o = [(6, "6 s"), (8, "8 s"), (12, "12 s"), (16, "16 s"), (24, "24 s")];
        between.push(ints("autoMixMaxS", "Longest mix", p.auto_mix_max_s, &o, live));
        between.push(toggle("autoMixBeatMatch", "Match the beat", "Nudges the next song's speed so the beats line up.", p.auto_mix_beat_match, live));
        if p.auto_mix_beat_match {
            let o = [(2.0, "2 %"), (4.0, "4 %"), (6.0, "6 %"), (8.0, "8 %")];
            between.push(floats("autoMixMaxTempoPct", "Biggest speed change", p.auto_mix_max_tempo_pct, &o, live));
            between.push(toggle("autoMixKeepPitch", "Keep the pitch", "Off lets the pitch follow the speed, up to 2 %.", p.auto_mix_keep_pitch, live));
        }
        between.push(toggle("autoMixBassSwap", "Swap the bass", "The new song's bass replaces the old one's.", p.auto_mix_bass_swap, live));
        between.push(toggle("autoMixFilters", "Muffle the ending", "The outgoing song fades out muffled.", p.auto_mix_filters, live));
        between.push(toggle("autoMixEchoOut", "Echo out clashes", "Overlapping vocals end in an echo instead.", p.auto_mix_echo_out, live));
        // Only in a build that carries the beat model's runtime.
        if beats::AVAILABLE {
            let on = p.auto_mix_better_beats;
            between.push(toggle("autoMixBetterBeats", "Better beat detection", &beat_model::detail(on), on, live));
            if on && beat_model::ready().is_none() {
                between.push(toggle("autoMixBeatsMobileData", "Download over mobile data", "Otherwise it waits for Wi-Fi.", p.auto_mix_beats_mobile_data, live));
            }
        }
        between.push(action("Measured songs", format!("{} songs measured for tempo and beats.", f.analysed), "Measure again", f.analysed > 0, "measure-again"));
    }
    between.push(toggle("crossfadeKeepAlbums", "Keep albums gapless", "No mixing between songs of the same album.", p.crossfade_keep_albums, live));
    let o = [(0, "Off"), (150, "150 ms"), (300, "300 ms"), (500, "500 ms"), (1000, "1 s")];
    between.push(ints("fadeMs", "Fade on play and pause", p.fade_ms, &o, true));

    let silence = if held { "Off while the audio is played untouched." } else { "Cuts quiet gaps in and between songs." };
    let controls = vec![
        toggle("previousAlwaysSkips", "Previous goes back a song", "Instead of restarting the current one.", p.previous_always_skips, true),
        floats("speed", "Speed", p.speed, &[(0.75, "0.75×"), (1.0, "Normal"), (1.25, "1.25×"), (1.5, "1.5×"), (2.0, "2×")], true),
        floats("pitch", "Pitch", p.pitch, &[(0.9, "−10 %"), (0.95, "−5 %"), (1.0, "Normal"), (1.05, "+5 %"), (1.1, "+10 %")], true),
        toggle("skipSilence", "Skip silence", silence, p.skip_silence, live),
    ];

    let mut queue = vec![
        toggle("skipExplicit", "Skip explicit songs", "Songs your server marks explicit.", p.skip_explicit, true),
        toggle("autoFill", "Keep playing when the queue ends", "Adds more music automatically.", p.auto_fill, true),
    ];
    // What arrives and what it is chosen by are two separate questions, so they are two rows: somebody
    // who listens to records wants the next record, whatever it is picked by.
    if p.auto_fill {
        queue.push(ordinal("autoFillKind", "Carry on with", p.auto_fill_kind, &AUTO_FILL_KIND_LABELS));
        queue.push(ordinal("autoFillBasis", "Chosen by", p.auto_fill_basis, &AUTO_FILL_BASIS_LABELS));
    }
    let wrong = vec![
        toggle("skipOnError", "Skip songs that won't play", "Up to three in a row.", p.skip_on_error, true),
        toggle("bridgeOffline", "Play downloads when offline", "If the server drops, keep playing from downloads.", p.bridge_offline, true),
    ];
    vec![section("Between songs", between), section("Controls", controls), section("Queue", queue), section("When something goes wrong", wrong)]
}

fn sound(p: &StoredPrefs, f: &SettingsFacts) -> Vec<SettingsSection> {
    // The chain is taken out of the path by the same rule the transitions are, so the row says so
    // rather than reading "On" over sound it is not touching.
    let bypassed = untouched(p.hi_res, f.dac.bit_perfect);
    let dsp = nori_player::sound::sound_on(p.eq_enabled, p.crossfeed_db, p.balance, p.mono, p.limiter);
    let status = if bypassed { "Off now" } else if dsp { "On" } else { "Off" };
    let eq = vec![
        SettingRow::Link {
            key: setting_key("Equalizer and crossfeed"),
            title: "Equalizer and crossfeed".into(),
            status: status.into(),
            dimmed: bypassed,
            action: "equalizer".into(),
            divider: true,
        },
        toggle("autoEqAuto", "AutoEQ for headphones", "Applies a known correction curve when headphones connect.", p.auto_eq_auto, true),
        toggle("profilePerOutput", "Remember sound per device", "Each device keeps its own equalizer settings.", p.profile_per_output, true),
        SettingRow::Link {
            key: setting_key("System audio effects"),
            title: "System audio effects".into(),
            status: String::new(),
            dimmed: false,
            action: "system-effects".into(),
            divider: false,
        },
    ];
    let mut volume = vec![ordinal("replayGain", "Even out volume", p.replay_gain, &["Off", "Per song", "Per album", "Automatic"])];
    if p.replay_gain != 0 {
        let r = crate::settings::EQ_RANGES.replay_gain_preamp;
        volume.push(SettingRow::Slider {
            name: "preampDb".into(),
            label: format!("Overall level {} dB", nori_words::fmt::signed_db(p.preamp_db)),
            value: p.preamp_db,
            min: r.min,
            max: r.max,
            centred: true,
            level: Some(crate::settings::EqLevel::ReplayGainPreamp),
        });
        let o = [(0.0, "0 dB"), (-3.0, "−3 dB"), (-6.0, "−6 dB"), (-9.0, "−9 dB"), (-12.0, "−12 dB")];
        volume.push(floats("untaggedGainDb", "Volume for songs without tags", p.untagged_gain_db, &o, true));
    }
    let d = &f.dac;
    let mut output = vec![
        toggle("hiRes", "High quality output", "Plays 24-bit files in full. Turns the equalizer off. Starts with the next song.", p.hi_res, true),
        toggle(
            "bitPerfect",
            "Bit perfect USB DAC",
            &nori_words::words::words_bit_perfect(d.device.clone(), d.bit_perfect, d.sample_rate, d.bits, d.blocked_by.clone(), d.supported),
            p.bit_perfect,
            true,
        ),
    ];
    // What is actually going out, rather than what was asked for: the one line that settles "is it
    // even reaching the DAC?" without a cable to a laptop.
    if let Some(text) = nori_words::words::words_dac_detail(d.modes.clone(), d.playing.clone(), d.track.clone()) {
        output.push(SettingRow::Note { text });
    }
    // The settings and output the playback service hands the same rule. A DAC stands in for "anything
    // USB", and a refused offload is the service's own to know.
    let prefs = nori_model::AudioPrefs {
        dsp,
        skip_silence: p.skip_silence,
        offload: p.offload,
        crossfade_s: p.crossfade_sec,
        auto_mix: p.auto_mix,
        speed: p.speed,
        pitch: p.pitch,
    };
    let out = nori_model::OutputState { hi_res: p.hi_res, bit_perfect: d.bit_perfect, usb: d.device.is_some(), offload_refused: false };
    output.push(toggle("offload", "Save battery while playing", &nori_words::words::words_offload(prefs, out), p.offload, true));
    // The Rust player, measured against ExoPlayer before it becomes anything more than a choice here.
    let experimental = vec![
        ordinal("playbackEngine", "Playback engine", p.playback_engine, &["ExoPlayer", "Rust"]),
        SettingRow::Note { text: "Takes effect the next time the app starts. Rust plays through nori's own engine: no audio offload, no offline bridge and no internet radio yet.".into() },
    ];
    vec![section("Equalizer", eq), section("Volume", volume), section("Output", output), section("Experimental", experimental)]
}

fn look(p: &StoredPrefs, f: &SettingsFacts) -> Vec<SettingsSection> {
    let mut theme = vec![
        ordinal("theme", "Theme", p.theme, &["Same as the phone", "Light", "Dark"]),
        toggle("amoled", "Black background", "True black in dark mode. Saves power on OLED screens.", p.amoled, true),
    ];
    if p.amoled {
        theme.push(toggle("playerColours", "Player in the cover's colours", "Off makes the player black too.", p.player_colours, true));
    }
    if f.wallpaper_colours {
        theme.push(toggle("dynamicColor", "Wallpaper colours", "Accent colour from your wallpaper.", p.dynamic_color, true));
    }
    if !p.dynamic_color || !f.wallpaper_colours {
        theme.push(SettingRow::Palette { name: "accent".into(), colours: nori_look::theme::ACCENTS.iter().map(|c| *c as i64).collect(), chosen: p.accent });
    }
    let mut cover = vec![toggle("coverColors", "Colours from the cover", "Pages take their colours from the artwork.", p.cover_colors, true)];
    if f.cover_blur {
        cover.push(toggle("softSleeve", "Blur the bottom of the cover", "The player's artwork softens into the page.", p.soft_sleeve, true));
    }
    // Under the switch for looking things up at all (Library), and turns it on with it, as finding lyrics
    // online does.
    let moving = p.motion_artwork && p.third_party_lookups;
    let about = if moving && p.reduce_motion { "Still while Less movement is on." } else { "Animated album art in the player, when Apple Music has it. Sends the artist and album name." };
    cover.push(toggle("motionArtwork", "Moving covers", about, moving, true));
    if moving {
        cover.push(toggle("motionArtworkMobile", "Moving covers on mobile data", "Each one is a few megabytes.", !p.motion_artwork_wifi_only, true));
    }
    let messages = vec![toggle("favouriteNotice", "Confirm favourites", "A short message when you favourite or unfavourite something.", p.favourite_notice, true)];
    let mut size = vec![
        floats("uiScale", "Text and button size", p.ui_scale, &[(0.0, "Automatic"), (0.9, "Smaller"), (1.0, "Same as the phone"), (1.1, "Larger")], true),
        toggle("reduceMotion", "Less movement", "Shorter, simpler animations.", p.reduce_motion, true),
    ];
    if !p.reduce_motion {
        size.push(toggle("ignoreSystemMotion", "Animate anyway", "Keeps animations on even when Android's are off.", p.ignore_system_motion, true));
    }
    vec![section("Theme", theme), section("Cover art", cover), section("Messages", messages), section("Size and motion", size)]
}

fn lyrics(p: &StoredPrefs) -> Vec<SettingsSection> {
    let display = vec![
        toggle("lyricsSweep", "Fill in words as they're sung", "For lyrics timed word by word.", p.lyrics_sweep, true),
        ints("lyricsSize", "Text size", p.lyrics_size, &[(0, "Small"), (1, "Medium"), (2, "Large")], true),
        toggle("lyricsTranslation", "Show translations", "When your server has them.", p.lyrics_translation, true),
        toggle("lyricsKeepScreenOn", "Keep the screen on", "While lyrics are shown and music plays.", p.lyrics_keep_screen_on, true),
    ];
    // The switch for looking things up at all lives in Library, but somebody looking for lyrics looks
    // here, so the lyrics half of it is offered here too and turns the other one on with it.
    let online = p.lyrics_online && p.third_party_lookups;
    let mut sources = vec![toggle(
        "lyricsOnline",
        "Find missing lyrics online",
        "When your server has no timed lyrics. Sends the artist, song and album name to the services below.",
        online,
        true,
    )];
    if !online {
        return vec![section("Display", display), section("Sources", sources)];
    }
    sources.push(toggle("lyricsPreferWords", "Prefer word-by-word lyrics", "Keeps looking past lyrics timed line by line.", p.lyrics_prefer_words, true));
    let service = |s: LyricsService, on: bool| SettingRow::Ranked {
        key: setting_key(s.title()),
        name: format!("lyricsService:{}", s.name()),
        id: s.name().into(),
        title: s.title().into(),
        detail: s.about().into(),
        on,
    };
    let asked: Vec<LyricsService> = lyrics_sources::switched_on(p);
    let mut ranked: Vec<SettingRow> = asked.iter().map(|s| service(*s, true)).collect();
    ranked.push(SettingRow::Note {
        text: "Asked together, the top ones first; hold one to move it. Lyrics timed word by word are taken first, whoever has them, then lyrics timed by line, and your server's own timed lyrics always come before all of these. The unofficial ones use other apps' lyrics without asking them, so they stay off until you turn them on.".into(),
    });
    let others: Vec<SettingRow> = p.lyrics_order.iter().filter_map(|n| LyricsService::named(n)).filter(|s| !asked.contains(s)).map(|s| service(s, false)).collect();
    let keys = vec![
        SettingRow::Text {
            key: setting_key("PaxSenix key"),
            name: "paxSenixKey".into(),
            title: "PaxSenix key".into(),
            detail: "Your own key, for its Spotify and Musixmatch lyrics.".into(),
            value: p.paxsenix_key.clone(),
            secret: true,
        },
        SettingRow::Text {
            key: setting_key("BetterLyrics key"),
            name: "betterLyricsKey".into(),
            title: "BetterLyrics key".into(),
            detail: "Without one it answers only for songs it has already stored.".into(),
            value: p.better_lyrics_key.clone(),
            secret: true,
        },
    ];
    let mut out = vec![section("Display", display), section("Sources", sources), section("Asked", ranked)];
    if !others.is_empty() {
        out.push(section("Not asked", others));
    }
    out.push(section("Keys", keys));
    out
}

fn library(p: &StoredPrefs, f: &SettingsFacts) -> Vec<SettingsSection> {
    let swipes = ["Nothing", "Add to queue", "Play next", "Favourite", "Download"];
    let gestures = vec![
        ordinal("tapAction", "Tapping a song", p.tap_action, &["Plays the list from there", "Plays only that song", "Adds it to the queue", "Plays it next"]),
        ordinal("swipeRight", "Swipe right", p.swipe_right, &swipes),
        ordinal("swipeLeft", "Swipe left", p.swipe_left, &swipes),
    ];
    let s = &f.sync;
    let search = vec![
        SettingRow::Action {
            key: setting_key("Offline search"),
            title: "Offline search".into(),
            detail: s.error.clone().unwrap_or_else(|| format!("{} songs · {} albums · {} artists on this phone", s.songs, s.albums, s.artists)),
            button: (if s.running { "Updating…" } else { "Update" }).into(),
            enabled: !s.running,
            error: s.error.is_some(),
            action: "sync-library".into(),
        },
        ints("liveSearchDelayMs", "Search delay", p.live_search_delay_ms, &[(150, "150 ms"), (250, "250 ms"), (350, "350 ms"), (500, "500 ms"), (800, "800 ms")], true),
    ];
    let mut history = vec![
        toggle("tasteModel", "Keep listening history", "Stored on this phone. Powers mixes and stats.", p.taste_model, true),
        toggle("scrobble", "Tell the server what you play", "Sends your plays to your server (scrobbling).", p.scrobble, true),
    ];
    if p.scrobble {
        let o = [(25, "25 %"), (50, "50 %"), (75, "75 %"), (90, "90 %"), (100, "the whole song")];
        history.push(ints("scrobblePercent", "Count a play after", p.scrobble_percent, &o, true));
    }
    let online = vec![toggle(
        "thirdPartyLookups",
        "Look things up online",
        "Update checks, missing lyrics and moving covers. Sends the artist, song and album name.",
        p.third_party_lookups,
        true,
    )];
    vec![section("Gestures", gestures), section("Search", search), section("Listening history", history), section("Online", online)]
}

fn data(p: &StoredPrefs, f: &SettingsFacts) -> Vec<SettingsSection> {
    let streaming = vec![quality("wifi", "Quality on Wi-Fi", &p.wifi), quality("mobile", "Quality on mobile data", &p.mobile)];
    let at_once: Vec<(i32, String)> = (1..=10).map(|n| (n, n.to_string())).collect();
    let at_once: Vec<(i32, &str)> = at_once.iter().map(|(n, l)| (*n, l.as_str())).collect();
    let downloads = vec![
        quality("download", "Quality for downloads", &p.download),
        ints("parallelDownloads", "Downloads at once", p.parallel_downloads, &at_once, true),
        action("Download the whole library", "Every song, at the download quality.".into(), "Download", f.sync.songs > 0, "download-library"),
    ];
    let ahead = vec![
        ints("precacheWifi", "Load ahead on Wi-Fi", p.precache_wifi, &[(1, "Next song"), (2, "2 songs"), (3, "3 songs"), (5, "5 songs"), (10, "10 songs")], true),
        ints("precacheMobile", "Load ahead on mobile data", p.precache_mobile, &[(1, "Next song"), (2, "2 songs"), (3, "3 songs"), (5, "5 songs")], true),
        ints("coversAhead", "Load covers ahead", p.covers_ahead, &[(0, "Off"), (1, "1"), (2, "2"), (3, "3"), (5, "5"), (8, "8"), (10, "10")], true),
    ];
    // What lives on the phone, and a way to throw the throwaway parts out. Downloads are the permanent
    // copy and are removed where they are listed; the streamed music and the covers rebuild themselves.
    let s = &f.storage;
    let bytes = nori_words::fmt::format_bytes;
    let clearing = if s.busy { "Clearing…" } else { "Clear" };
    let storage = vec![
        ints("cacheMb", "Space for streamed music", p.cache_mb, &[(256, "256 MB"), (1024, "1 GB"), (4096, "4 GB"), (16384, "16 GB")], true),
        SettingRow::Info {
            key: setting_key("Stored on this phone"),
            title: "Stored on this phone".into(),
            detail: format!(
                "{} streamed · {} covers · {} in {} downloads · {} library",
                bytes(s.stream_bytes),
                bytes(s.cover_bytes),
                bytes(s.download_bytes),
                s.download_songs,
                bytes(s.index_bytes)
            ),
        },
        action("Streamed music", "Oldest goes first. Downloads stay.".into(), clearing, !s.busy && s.stream_bytes > 0, "clear-stream"),
        action("Covers", "Fetched again when needed.".into(), clearing, !s.busy && s.cover_bytes > 0, "clear-covers"),
        action("Downloads", "Remove them from the Downloads page.".into(), "Show", true, "downloads"),
    ];
    vec![section("Streaming quality", streaming), section("Downloads", downloads), section("Loading ahead", ahead), section("Storage", storage)]
}

/// The line under a saved server: who signs in, whether it is in use, and what is special about it.
pub fn server_detail(s: &crate::settings::SavedServer, active: bool) -> String {
    let mut parts: Vec<&str> = vec![if s.user.is_empty() { "API key" } else { &s.user }, if active { "in use" } else { "tap to switch" }];
    if s.wifi_only {
        parts.push("Wi-Fi only");
    }
    if !s.alt_url.trim().is_empty() {
        parts.push("second address");
    }
    parts.join(" · ")
}

fn servers(p: &StoredPrefs, f: &SettingsFacts) -> Vec<SettingsSection> {
    let mut accounts: Vec<SettingRow> = p
        .servers
        .iter()
        .map(|s| {
            let active = s.id == p.active_server_id;
            SettingRow::Server { id: s.id.clone(), label: label(&s.name, &s.url), detail: server_detail(s, active), active }
        })
        .collect();
    accounts.push(SettingRow::Button { title: "Add server".into(), action: "add-server".into() });
    let mut out = vec![section("Accounts", accounts)];
    let server = p.servers.iter().find(|s| s.id == p.active_server_id);
    let folder_row = f.folders.len() > 1;
    let alt_row = server.is_some_and(|s| !s.alt_url.trim().is_empty());
    if folder_row || alt_row {
        let mut rows = Vec::new();
        if folder_row {
            let current = server.map_or("", |s| s.music_folder_id.as_str());
            let mut o = vec![SettingOption { label: "All".into(), value: String::new() }];
            o.extend(f.folders.iter().map(|m| SettingOption { label: m.name.clone(), value: m.id.clone() }));
            let at = o.iter().position(|x| x.value == current);
            rows.push(choice("musicFolder", "Music folder", o, at, current.to_string(), true));
        }
        if alt_row {
            let v = server.map_or(0, |s| s.alt_max_bit_rate);
            let o = [(0, "No limit"), (320, "320 kbps"), (192, "192 kbps"), (128, "128 kbps"), (96, "96 kbps")];
            rows.push(ints("altMaxBitRate", "Bitrate limit on the second address", v, &o, true));
        }
        out.push(section("This server", rows));
    }
    out
}

/// One group's page for these settings and facts; `None` for a group there is not.
pub fn page(id: &str, p: &StoredPrefs, f: &SettingsFacts) -> Option<SettingsPage> {
    let title = group_title(id)?.to_string();
    let sections = match id {
        "playing" => playing(p, f),
        "sound" => sound(p, f),
        "look" => look(p, f),
        "lyrics" => lyrics(p),
        "library" => library(p, f),
        "data" => data(p, f),
        "servers" => servers(p, f),
        _ => Vec::new(),
    };
    Some(SettingsPage { title, sections })
}

/// One thing the core is built from that is not ours: what it is, whose it is, under what terms, and
/// which bundled licence text those terms are (`licences/<file>.txt`; none for something with no
/// licence to reproduce).
#[derive(Debug, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct Credit {
    pub name: String,
    pub what: String,
    pub copyright: String,
    pub licence: String,
    pub file: Option<String>,
}

/// The core's own credits, for the licences page of any app built on it. A line here is added in the
/// same commit that adds the dependency. Where a crate offers MIT or Apache-2.0, the MIT text is shown.
const CORE_CREDITS: [(&str, &str, &str, &str, Option<&str>); 18] = [
    ("uniffi", "Generates the Kotlin bindings to the core and the JNI calls under them", "Mozilla Foundation", "MPL-2.0", Some("MPL-2.0")),
    ("rusqlite", "The library index, full-text search and caches", "Copyright (c) 2014 The rusqlite developers", "MIT", Some("MIT")),
    ("SQLite", "The database itself, bundled into the core", "D. Richard Hipp and the SQLite developers, dedicated to the public domain", "Public domain", None),
    ("RustFFT", "The spectrum analysis behind tempo, beats and key", "Copyright (c) 2015 The RustFFT Developers", "MIT or Apache-2.0", Some("MIT")),
    (
        "Signalsmith Stretch",
        "Time-stretching for beat-matched mixes",
        "Copyright (c) 2022 Geraint Luff / Signalsmith Audio Ltd.; Rust binding Copyright 2024 Colin Marc",
        "MIT",
        Some("MIT"),
    ),
    ("serde and serde_json", "Reading the server's answers", "Copyright (c) David Tolnay and the Serde developers", "MIT or Apache-2.0", Some("MIT")),
    ("jni", "The core's direct calls from the audio path", "Copyright (c) 2016 Prevoty, Inc. and jni-rs contributors", "MIT or Apache-2.0", Some("MIT")),
    ("simd_cesu8", "Java's strings turned into the core's and back", "Copyright (c) Sean C. Roach", "MIT or Apache-2.0", Some("MIT")),
    ("md-5", "Signing requests the way the Subsonic API asks", "Copyright (c) RustCrypto Developers", "MIT or Apache-2.0", Some("MIT")),
    ("parking_lot", "Locks inside the core", "Copyright (c) 2016 The Rust Project Developers (Amanieu d'Antras)", "MIT or Apache-2.0", Some("MIT")),
    ("thiserror", "Errors inside the core", "Copyright (c) David Tolnay", "MIT or Apache-2.0", Some("MIT")),
    (
        "Media3 Sonic and silence skipping, ported",
        "Speed, pitch and shortened silences, ported line for line into the core",
        "Copyright The Android Open Source Project",
        "Apache-2.0",
        Some("Apache-2.0"),
    ),
    (
        "AndroidX Palette, ported",
        "The colour quantiser behind a page's accent, ported line for line into the core",
        "Copyright The Android Open Source Project",
        "Apache-2.0",
        Some("Apache-2.0"),
    ),
    (
        "zune-jpeg, jpeg-decoder, png, image-webp and gif",
        "Decoding cover art: JPEG, PNG, WebP and GIF",
        "Copyright (c) the zune-image, image-rs and jpeg-decoder developers",
        "MIT or Apache-2.0",
        Some("MIT"),
    ),
    ("yaml-rust2", "Reading LRCLIB's word-timed lyrics", "Copyright (c) 2015 Chen Yuheng; Copyright (c) 2023 Ethiraric", "MIT or Apache-2.0", Some("MIT")),
    ("roxmltree", "Reading word-timed lyrics written as TTML", "Copyright (c) 2018 Yevhenii Reizner", "MIT or Apache-2.0", Some("MIT")),
    (
        "miniz_oxide",
        "Unpacking KuGou's word-timed lyrics",
        "Copyright 2013-2014 RAD Game Tools and Valve Software; Copyright 2010-2014 Rich Geldreich and Tenacious Software LLC",
        "MIT, Zlib or Apache-2.0",
        Some("MIT"),
    ),
    ("futures-util", "Asking the lyrics services together", "Copyright (c) 2016 Alex Crichton; Copyright (c) 2017 The Tokio Authors", "MIT or Apache-2.0", Some("MIT")),
];

/// The Android app's own libraries, credited here with every other word it says. A line here is added
/// in the same commit that adds the dependency, and in NOTICE.
const ANDROID_CREDITS: [(&str, &str, &str, &str, Option<&str>); 6] = [
    ("AndroidX Media3", "Playback, the media session and the notification", "Copyright The Android Open Source Project", "Apache-2.0", Some("Apache-2.0")),
    ("Jetpack Compose and Material 3", "The user interface toolkit", "Copyright The Android Open Source Project", "Apache-2.0", Some("Apache-2.0")),
    ("Material Icons", "The icons", "Copyright Google LLC", "Apache-2.0", Some("Apache-2.0")),
    ("AndroidX Navigation, Lifecycle, Activity, Core", "The app's plumbing", "Copyright The Android Open Source Project", "Apache-2.0", Some("Apache-2.0")),
    ("OkHttp", "Every network request", "Copyright Square, Inc.", "Apache-2.0", Some("Apache-2.0")),
    (
        "kotlinx.coroutines",
        "The concurrency the app is written in",
        "Copyright JetBrains s.r.o. and Kotlin Programming Language contributors",
        "Apache-2.0",
        Some("Apache-2.0"),
    ),
];

/// The typeface and the data the app fetches from third parties (the core asks AutoEQ and the lyrics
/// services, so any app on it credits them).
const DATA_CREDITS: [(&str, &str, &str, &str, Option<&str>); 16] = [
    ("Inter", "The typeface", "Copyright (c) 2016 The Inter Project Authors (Rasmus Andersson)", "OFL-1.1", Some("OFL-1.1")),
    ("AutoEQ", "Headphone correction curves, fetched when you ask for them", "Copyright (c) 2018 Jaakko Pasanen", "MIT", Some("MIT")),
    ("LRCLIB", "Timed lyrics for songs your server has none for, asked only when switched on", "lrclib.net; lyrics belong to their authors and contributors", "Service", None),
    (
        "Unison",
        "Lyrics written and timed by listeners, asked only when switched on",
        "Lyrics from Unison (https://unison.boidu.dev), under the Open Database License (ODbL-1.0); lyrics belong to their authors",
        "ODbL-1.0",
        None,
    ),
    ("BiniLyrics", "Lyrics timed syllable by syllable, asked only when switched on", "binimum.org, a volunteer's copy of Apple Music's lyrics; lyrics belong to their authors", "Service", None),
    ("BetterLyrics", "Lyrics timed syllable by syllable, and QQ Music's word by word, asked only when switched on", "betterlyrics.org; lyrics belong to their authors", "Service", None),
    (
        "PaxSenix",
        "Apple Music's, Spotify's and Musixmatch's lyrics, asked only when switched on (the last two with your own key)",
        "paxsenix.org, with songs found through Apple's iTunes Search API; lyrics belong to their authors",
        "Service",
        None,
    ),
    ("LyricsPlus", "Lyrics timed syllable by syllable, asked only when switched on", "The YouLy+ project's volunteer servers; lyrics belong to their authors", "Service", None),
    ("NetEase Cloud Music", "Lyrics, often timed word by word, asked only when switched on", "music.163.com; lyrics belong to their authors", "Service", None),
    ("KuGou", "Lyrics, often timed word by word, asked only when switched on", "kugou.com; lyrics belong to their authors", "Service", None),
    ("SimpMusic", "Lyrics timed by listeners, asked only when switched on", "simpmusic.org; lyrics belong to their authors", "Service", None),
    ("YouTube Music", "Captions and lyrics of a song's YouTube upload, asked only when switched on", "Google LLC; lyrics belong to their authors", "Service", None),
    ("Megalobiz", "Lyrics timed line by line by its users, asked only when switched on", "megalobiz.com; lyrics belong to their authors", "Service", None),
    ("Genius", "Untimed lyrics, asked last and only when switched on", "genius.com; lyrics belong to their authors", "Service", None),
    ("iTunes Search API", "Finding a song's Apple Music id for PaxSenix, asked only when switched on", "Apple Inc.", "Service", None),
    ("Apple Music", "Moving album covers, asked only when switched on", "Apple Inc.; the artwork belongs to its artists and labels", "Service", None),
];

// ---- the doors ----

fn credits(list: &[(&str, &str, &str, &str, Option<&str>)]) -> Vec<Credit> {
    list.iter()
        .map(|(n, w, c, l, f)| Credit { name: n.to_string(), what: w.to_string(), copyright: c.to_string(), licence: l.to_string(), file: f.map(str::to_string) })
        .collect()
}

/// What the core is built from, in the order the licences page lists it.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn core_credits() -> Vec<Credit> {
    credits(&CORE_CREDITS)
}

/// What the Android app is built from besides the core.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn android_credits() -> Vec<Credit> {
    credits(&ANDROID_CREDITS)
}

/// The typeface and the third parties' data.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn data_credits() -> Vec<Credit> {
    credits(&DATA_CREDITS)
}

/// The groups the root of Settings lists, in order.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn settings_groups() -> Vec<SettingsGroup> {
    GROUPS.iter().map(|(id, title, summary)| SettingsGroup { id: id.to_string(), title: title.to_string(), summary: summary.to_string() }).collect()
}

/// The rows whose title (first) or words (after) contain `query`; nothing for a blank one.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn settings_search(query: String) -> Vec<SettingsHit> {
    search(&query)
}

/// One group's page for the settings as they are now; `None` for a group there is not.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn settings_page(id: String, facts: SettingsFacts) -> Option<SettingsPage> {
    let p = crate::settings_store::current().unwrap_or_default();
    page(&id, &p, &facts)
}

/// One row's value changed (see `settings::set_by_name`), kept where the settings are kept: the
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

    fn rows(p: &StoredPrefs, f: &SettingsFacts, id: &str) -> Vec<SettingRow> {
        page(id, p, f).unwrap().sections.into_iter().flat_map(|s| s.rows).collect()
    }

    fn titles(p: &StoredPrefs, f: &SettingsFacts, id: &str) -> Vec<String> {
        rows(p, f, id)
            .into_iter()
            .filter_map(|r| match r {
                SettingRow::Toggle { title, .. } | SettingRow::Choice { title, .. } | SettingRow::Link { title, .. } | SettingRow::Action { title, .. } | SettingRow::Info { title, .. } => {
                    Some(title)
                }
                SettingRow::Note { text } => Some(format!("note: {text}")),
                SettingRow::Slider { label, .. } => Some(format!("slider: {label}")),
                SettingRow::Palette { .. } => Some("palette".into()),
                SettingRow::Server { label, detail, .. } => Some(format!("server: {label} ({detail})")),
                SettingRow::Button { title, .. } => Some(format!("button: {title}")),
                SettingRow::Ranked { title, .. } | SettingRow::Text { title, .. } => Some(title),
            })
            .collect()
    }

    fn find(p: &StoredPrefs, f: &SettingsFacts, id: &str, key: &str) -> SettingRow {
        rows(p, f, id)
            .into_iter()
            .find(|r| match r {
                SettingRow::Toggle { key: k, .. } | SettingRow::Choice { key: k, .. } | SettingRow::Link { key: k, .. } | SettingRow::Action { key: k, .. } => k == key,
                _ => false,
            })
            .unwrap_or_else(|| panic!("no row {key}"))
    }

    fn enabled(r: &SettingRow) -> bool {
        match r {
            SettingRow::Toggle { enabled, .. } | SettingRow::Choice { enabled, .. } | SettingRow::Action { enabled, .. } => *enabled,
            SettingRow::Link { dimmed, .. } => !dimmed,
            _ => true,
        }
    }

    #[test]
    fn the_core_credits_what_it_is_built_from() {
        let c = core_credits();
        assert_eq!(c.len(), 18);
        assert_eq!((c[0].name.as_str(), c[0].licence.as_str(), c[0].file.as_deref()), ("uniffi", "MPL-2.0", Some("MPL-2.0")));
        assert_eq!((c[2].name.as_str(), c[2].file.as_deref()), ("SQLite", None));
        assert_eq!(c[12].name, "AndroidX Palette, ported");
        assert_eq!(c[13].what, "Decoding cover art: JPEG, PNG, WebP and GIF");
        let a = android_credits();
        assert_eq!((a.len(), a[0].name.as_str(), a[4].what.as_str()), (6, "AndroidX Media3", "Every network request"));
        let d = data_credits();
        assert_eq!((d[0].licence.as_str(), d[2].name.as_str(), d[2].file.as_deref()), ("OFL-1.1", "LRCLIB", None), "a service has no licence text");
    }

    #[test]
    fn keys_are_the_titles_slugged() {
        assert_eq!(setting_key("Fill in words as they're sung"), "fill-in-words-as-they-re-sung");
        assert_eq!(setting_key("Quality on Wi-Fi"), "quality-on-wi-fi");
        assert_eq!(setting_key("  AutoEQ for headphones!"), "autoeq-for-headphones");
        assert_eq!(setting_key("Skip songs that won't play"), "skip-songs-that-won-t-play");
    }

    #[test]
    fn the_groups_in_their_order() {
        let g = settings_groups();
        assert_eq!(g.iter().map(|g| g.id.as_str()).collect::<Vec<_>>(), ["servers", "playing", "sound", "look", "lyrics", "library", "data", "about"]);
        assert_eq!((g[6].title.as_str(), g[6].summary.as_str()), ("Downloads and storage", "Quality, downloads, space"));
        assert_eq!(page("licences", &StoredPrefs::default(), &SettingsFacts::default()).unwrap().title, "Licences");
        assert!(page("about", &StoredPrefs::default(), &SettingsFacts::default()).unwrap().sections.is_empty());
        assert_eq!(page("nope", &StoredPrefs::default(), &SettingsFacts::default()), None);
    }

    #[test]
    fn search_finds_titles_first_then_words() {
        let hits = search("oled");
        assert_eq!(hits[0].title, "Black background");
        assert_eq!(hits[0].detail, "Appearance · AMOLED. True black in dark mode, saves power on OLED");
        assert_eq!(hits[0].key, "black-background");
        let gapless = search("gapless");
        assert_eq!(gapless[0].title, "Keep albums gapless");
        let speed = search("Speed");
        assert_eq!(speed[0].title, "Biggest speed change", "in the order of the pages");
        assert_eq!(speed.iter().find(|h| h.title == "Speed").unwrap().detail, "Playback", "no words, only the page");
        assert!(speed.iter().position(|h| h.title == "Biggest speed change") < speed.iter().position(|h| h.title == "Match the beat"));
        assert!(search("  ").is_empty());
        // Every entry points at a row its page has (or a row the platform draws itself).
        let p = StoredPrefs { auto_mix: true, replay_gain: 1, amoled: true, third_party_lookups: true, motion_artwork: true, ..StoredPrefs::default() };
        let active = SavedServer { id: "a".into(), alt_url: "https://b".into(), ..SavedServer::default() };
        let p = StoredPrefs { servers: vec![active], active_server_id: "a".into(), ..p };
        let two = vec![MusicFolder { id: "1".into(), name: "A".into() }, MusicFolder { id: "2".into(), name: "B".into() }];
        let f = SettingsFacts { wallpaper_colours: true, cover_blur: true, folders: two, ..SettingsFacts::default() };
        for (group, title, _) in INDEX {
            if group == "about" || BEAT_MODEL_ROWS.contains(&title) {
                continue;
            }
            // The crossfade gives way to AutoMix, so it is looked for with AutoMix off.
            let plain = StoredPrefs { auto_mix: false, ..p.clone() };
            assert!(titles(&p, &f, group).iter().chain(titles(&plain, &f, group).iter()).any(|t| t == title), "{group}: {title}");
        }
    }

    #[test]
    fn better_beat_detection_is_only_offered_by_a_build_with_the_model() {
        let on = StoredPrefs { auto_mix: true, auto_mix_better_beats: true, ..StoredPrefs::default() };
        let t = titles(&on, &SettingsFacts::default(), "playing");
        assert_eq!(t.iter().any(|t| t == "Better beat detection"), beats::AVAILABLE);
        // Asked for and not on the device yet: whether it may come over mobile data.
        assert_eq!(t.iter().any(|t| t == "Download over mobile data"), beats::AVAILABLE);
        assert_eq!(search("beat detection").iter().any(|h| h.title == "Better beat detection"), beats::AVAILABLE);
        let off = titles(&StoredPrefs { auto_mix_better_beats: false, ..on }, &SettingsFacts::default(), "playing");
        assert!(!off.iter().any(|t| t == "Download over mobile data"));
    }

    #[test]
    fn untouched_output_holds_the_transitions_and_says_why() {
        let f = SettingsFacts::default();
        let p = StoredPrefs { hi_res: true, ..StoredPrefs::default() };
        let t = titles(&p, &f, "playing");
        assert_eq!(t[0], "note: Off while high quality output is on.");
        assert!(!enabled(&find(&p, &f, "playing", "crossfade")));
        assert!(!enabled(&find(&p, &f, "playing", "keep-albums-gapless")));
        assert!(enabled(&find(&p, &f, "playing", "fade-on-play-and-pause")), "the play and pause fade is volume, not mixing");
        match find(&p, &f, "playing", "skip-silence") {
            SettingRow::Toggle { detail, enabled, .. } => assert_eq!((detail.as_str(), enabled), ("Off while the audio is played untouched.", false)),
            r => panic!("{r:?}"),
        }
        let dac = SettingsFacts { dac: DacFacts { bit_perfect: true, ..DacFacts::default() }, ..SettingsFacts::default() };
        assert_eq!(titles(&StoredPrefs::default(), &dac, "playing")[0], "note: Off while the USB DAC is in bit perfect mode.");
        let d = StoredPrefs::default();
        assert!(!titles(&d, &f, "playing")[0].starts_with("note"));
        match find(&d, &f, "playing", "skip-silence") {
            SettingRow::Toggle { detail, enabled, .. } => assert_eq!((detail.as_str(), enabled), ("Cuts quiet gaps in and between songs.", true)),
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn the_equalizer_row_says_what_the_chain_is_doing() {
        let f = SettingsFacts::default();
        let status = |p: &StoredPrefs, f: &SettingsFacts| match find(p, f, "sound", "equalizer-and-crossfeed") {
            SettingRow::Link { status, dimmed, .. } => (status, dimmed),
            r => panic!("{r:?}"),
        };
        assert_eq!(status(&StoredPrefs::default(), &f), ("Off".into(), false));
        assert_eq!(status(&StoredPrefs { mono: true, ..StoredPrefs::default() }, &f), ("On".into(), false));
        assert_eq!(status(&StoredPrefs { mono: true, hi_res: true, ..StoredPrefs::default() }, &f), ("Off now".into(), true));
    }

    #[test]
    fn automix_replaces_the_crossfade_and_unfolds_its_options() {
        let f = SettingsFacts { analysed: 12, ..SettingsFacts::default() };
        let off = titles(&StoredPrefs::default(), &f, "playing");
        assert!(off.contains(&"Crossfade".to_string()) && !off.contains(&"Longest mix".to_string()));
        let on = titles(&StoredPrefs { auto_mix: true, ..StoredPrefs::default() }, &f, "playing");
        assert!(!on.contains(&"Crossfade".to_string()));
        assert_eq!(
            on[..10],
            [
                "AutoMix",
                "Longest mix",
                "Match the beat",
                "Biggest speed change",
                "Keep the pitch",
                "Swap the bass",
                "Muffle the ending",
                "Echo out clashes",
                "Measured songs",
                "Keep albums gapless"
            ]
        );
        let no_beat = titles(&StoredPrefs { auto_mix: true, auto_mix_beat_match: false, ..StoredPrefs::default() }, &f, "playing");
        assert!(!no_beat.contains(&"Biggest speed change".to_string()) && !no_beat.contains(&"Keep the pitch".to_string()));
        match find(&StoredPrefs { auto_mix: true, ..StoredPrefs::default() }, &f, "playing", "measured-songs") {
            SettingRow::Action { detail, enabled, button, .. } => {
                assert_eq!((detail.as_str(), enabled, button.as_str()), ("12 songs measured for tempo and beats.", true, "Measure again"))
            }
            r => panic!("{r:?}"),
        }
        let none = SettingsFacts::default();
        assert!(!enabled(&find(&StoredPrefs { auto_mix: true, ..StoredPrefs::default() }, &none, "playing", "measured-songs")));
    }

    #[test]
    fn choices_show_their_option_and_send_its_value() {
        let f = SettingsFacts::default();
        let p = StoredPrefs::default();
        let choice = |p: &StoredPrefs, id: &str, key: &str| match find(p, &f, id, key) {
            SettingRow::Choice { options, shown, name, .. } => (name, shown, options),
            r => panic!("{r:?}"),
        };
        let (name, shown, o) = choice(&p, "data", "quality-on-mobile-data");
        assert_eq!((name.as_str(), shown.as_str()), ("mobile", "Opus 192"));
        assert_eq!(o.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["Original", "MP3 320", "Opus 192", "Opus 128", "Opus 96", "Opus 64"]);
        assert_eq!(o[1].value, "320:mp3");
        assert_eq!(set_by_name(&p, &name, &o[1].value).unwrap().prefs.mobile, SavedQuality { bit_rate: 320, format: "mp3".into() });
        assert_eq!(choice(&p, "data", "quality-on-wi-fi").1, "Original");
        let (name, shown, o) = choice(&p, "playing", "speed");
        assert_eq!((name.as_str(), shown.as_str()), ("speed", "Normal"));
        assert_eq!(set_by_name(&p, &name, &o[0].value).unwrap().prefs.speed, 0.75);
        assert_eq!(choice(&StoredPrefs { speed: 1.1, ..p.clone() }, "playing", "speed").1, "1.1", "a value no option has shows as itself");
        assert_eq!(choice(&StoredPrefs { crossfade_sec: 5, ..p.clone() }, "playing", "crossfade").1, "5");
        let (_, shown, o) = choice(&p, "look", "theme");
        assert_eq!(shown, "Same as the phone");
        assert_eq!(set_by_name(&p, "theme", &o[2].value).unwrap().prefs.theme, 2);
        assert_eq!(choice(&p, "library", "swipe-left").1, "Favourite");
        assert_eq!(choice(&p, "library", "swipe-right").1, "Add to queue");
        assert_eq!(choice(&p, "library", "tapping-a-song").1, "Plays the list from there");
        assert_eq!(choice(&p, "lyrics", "text-size").1, "Medium");
        assert_eq!(choice(&p, "look", "text-and-button-size").1, "Automatic");
        assert_eq!(choice(&p, "data", "space-for-streamed-music").1, "1 GB");
        assert_eq!(choice(&p, "data", "load-covers-ahead").1, "3");
        assert_eq!(choice(&p, "data", "downloads-at-once").2.len(), 10);
        assert_eq!(choice(&p, "data", "load-ahead-on-mobile-data").1, "Next song");
        assert_eq!(choice(&p, "sound", "even-out-volume").1, "Off");
        assert_eq!(choice(&StoredPrefs { replay_gain: 3, ..p.clone() }, "sound", "even-out-volume").1, "Automatic");
        assert_eq!(choice(&StoredPrefs { replay_gain: 1, ..p.clone() }, "sound", "volume-for-songs-without-tags").1, "−6 dB");
        assert_eq!(choice(&p, "library", "count-a-play-after").1, "50 %");
        assert_eq!(choice(&p, "library", "search-delay").1, "350 ms");
        assert_eq!(choice(&StoredPrefs { pitch: 0.9, ..p.clone() }, "playing", "pitch").1, "−10 %");
    }

    #[test]
    fn rows_that_come_and_go() {
        let f = SettingsFacts::default();
        let d = StoredPrefs::default();
        let has = |p: &StoredPrefs, f: &SettingsFacts, id: &str, t: &str| titles(p, f, id).iter().any(|x| x == t);
        assert!(!has(&d, &f, "sound", "Volume for songs without tags"));
        let rg = StoredPrefs { replay_gain: 2, preamp_db: 1.5, ..d.clone() };
        assert!(has(&rg, &f, "sound", "Volume for songs without tags"));
        assert!(has(&rg, &f, "sound", "slider: Overall level +1.5 dB"));
        assert!(!has(&d, &f, "look", "Player in the cover's colours"));
        assert!(has(&StoredPrefs { amoled: true, ..d.clone() }, &f, "look", "Player in the cover's colours"));
        // Without wallpaper colours (older Android) the swatches are always there; with them, only
        // while they are switched off.
        assert!(has(&d, &f, "look", "palette") && !has(&d, &f, "look", "Wallpaper colours"));
        let wall = SettingsFacts { wallpaper_colours: true, cover_blur: true, ..f.clone() };
        assert!(has(&d, &wall, "look", "Wallpaper colours") && !has(&d, &wall, "look", "palette"));
        assert!(has(&StoredPrefs { dynamic_color: false, ..d.clone() }, &wall, "look", "palette"));
        assert!(has(&d, &wall, "look", "Blur the bottom of the cover") && !has(&d, &f, "look", "Blur the bottom of the cover"));
        assert!(has(&d, &f, "look", "Animate anyway") && !has(&StoredPrefs { reduce_motion: true, ..d.clone() }, &f, "look", "Animate anyway"));
        assert!(has(&d, &f, "library", "Count a play after") && !has(&StoredPrefs { scrobble: false, ..d.clone() }, &f, "library", "Count a play after"));
        assert!(has(&d, &f, "playing", "Chosen by") && !has(&StoredPrefs { auto_fill: false, ..d.clone() }, &f, "playing", "Carry on with"));
        let lrc = |p: &StoredPrefs| match find(p, &f, "lyrics", "find-missing-lyrics-online") {
            SettingRow::Toggle { on, name, .. } => (on, name),
            r => panic!("{r:?}"),
        };
        assert_eq!(lrc(&d), (false, "lyricsOnline".to_string()), "lyrics online needs the lookups switch too");
        assert!(lrc(&StoredPrefs { third_party_lookups: true, ..d.clone() }).0);
        match rows(&d, &f, "look").into_iter().find(|r| matches!(r, SettingRow::Palette { .. })).unwrap() {
            SettingRow::Palette { colours, chosen, .. } => assert_eq!((colours.len(), colours[0], chosen), (8, 0xFF6750A4, 0xFF6750A4)),
            _ => unreachable!(),
        }
    }

    #[test]
    fn the_output_rows_word_the_dac_and_the_battery_saver() {
        let dac = DacFacts { device: Some("Qudelix".into()), supported: true, modes: vec!["44.1 kHz / 24 bit".into()], ..DacFacts::default() };
        let f = SettingsFacts { dac, ..SettingsFacts::default() };
        let t = titles(&StoredPrefs::default(), &f, "sound");
        assert!(t.contains(&"note: Offers 44.1 kHz / 24 bit".to_string()));
        match find(&StoredPrefs::default(), &f, "sound", "bit-perfect-usb-dac") {
            SettingRow::Toggle { detail, .. } => assert_eq!(detail, "Qudelix connected. Starts with the music."),
            r => panic!("{r:?}"),
        }
        match find(&StoredPrefs::default(), &f, "sound", "save-battery-while-playing") {
            SettingRow::Toggle { detail, .. } => assert_eq!(detail, "Not available while a USB DAC is connected."),
            r => panic!("{r:?}"),
        }
        match find(&StoredPrefs { eq_enabled: true, ..StoredPrefs::default() }, &SettingsFacts::default(), "sound", "save-battery-while-playing") {
            SettingRow::Toggle { detail, .. } => assert!(detail.starts_with("Paused now")),
            r => panic!("{r:?}"),
        }
    }

    #[test]
    fn storage_and_the_offline_index() {
        let storage = StorageFacts { stream_bytes: 2048, cover_bytes: 0, download_bytes: 5 * 1_048_576, download_songs: 3, index_bytes: 100, busy: false };
        let sync = SyncFacts { songs: 10, albums: 2, artists: 1, ..SyncFacts::default() };
        let f = SettingsFacts { storage, sync, ..SettingsFacts::default() };
        let p = StoredPrefs::default();
        let r = rows(&p, &f, "data");
        let info = r.iter().find_map(|r| if let SettingRow::Info { detail, .. } = r { Some(detail.clone()) } else { None }).unwrap();
        assert_eq!(info, "2 KB streamed · 0 B covers · 5.0 MB in 3 downloads · 100 B library");
        assert!(enabled(&find(&p, &f, "data", "streamed-music")));
        assert!(!enabled(&find(&p, &f, "data", "covers")), "nothing to clear");
        assert!(enabled(&find(&p, &f, "data", "download-the-whole-library")));
        let busy = SettingsFacts { storage: StorageFacts { busy: true, ..f.storage.clone() }, ..f.clone() };
        match find(&p, &busy, "data", "streamed-music") {
            SettingRow::Action { button, enabled, .. } => assert_eq!((button.as_str(), enabled), ("Clearing…", false)),
            r => panic!("{r:?}"),
        }
        match find(&p, &f, "library", "offline-search") {
            SettingRow::Action { detail, button, error, .. } => {
                assert_eq!((detail.as_str(), button.as_str(), error), ("10 songs · 2 albums · 1 artists on this phone", "Update", false))
            }
            r => panic!("{r:?}"),
        }
        let failed = SettingsFacts { sync: SyncFacts { running: true, error: Some("timeout".into()), ..SyncFacts::default() }, ..SettingsFacts::default() };
        match find(&p, &failed, "library", "offline-search") {
            SettingRow::Action { detail, button, error, enabled, .. } => assert_eq!((detail.as_str(), button.as_str(), error, enabled), ("timeout", "Updating…", true, false)),
            r => panic!("{r:?}"),
        }
        assert!(!enabled(&find(&p, &SettingsFacts::default(), "data", "download-the-whole-library")), "nothing indexed");
    }

    #[test]
    fn the_servers_page() {
        let a = SavedServer { id: "a".into(), name: "Home".into(), user: "me".into(), wifi_only: true, ..SavedServer::default() };
        let b = SavedServer { id: "b".into(), url: "https://music.example.com/x".into(), alt_url: "https://alt".into(), ..SavedServer::default() };
        let p = StoredPrefs { servers: vec![a, b], active_server_id: "b".into(), ..StoredPrefs::default() };
        let f = SettingsFacts::default();
        let t = titles(&p, &f, "servers");
        assert_eq!(
            t,
            [
                "server: Home (me · tap to switch · Wi-Fi only)",
                "server: music.example.com (API key · in use · second address)",
                "button: Add server",
                "Bitrate limit on the second address",
            ]
        );
        let folders = SettingsFacts { folders: vec![MusicFolder { id: "1".into(), name: "Rock".into() }, MusicFolder { id: "2".into(), name: "Jazz".into() }], ..f.clone() };
        match find(&p, &folders, "servers", "music-folder") {
            SettingRow::Choice { shown, options, .. } => {
                assert_eq!(shown, "All");
                assert_eq!(options.iter().map(|o| o.label.as_str()).collect::<Vec<_>>(), ["All", "Rock", "Jazz"]);
            }
            r => panic!("{r:?}"),
        }
        let alone = StoredPrefs { active_server_id: "a".into(), ..p };
        assert_eq!(titles(&alone, &f, "servers").len(), 3, "no second address and one folder: no section for this server");
    }

    #[test]
    fn the_lyrics_services_are_ranked_rows_under_their_switch() {
        let f = SettingsFacts::default();
        let d = StoredPrefs::default();
        let ranked = |p: &StoredPrefs| -> Vec<(String, bool)> {
            rows(p, &f, "lyrics").into_iter().filter_map(|r| match r {
                SettingRow::Ranked { id, on, .. } => Some((id, on)),
                _ => None,
            }).collect()
        };
        assert!(ranked(&d).is_empty(), "nothing to rank while lookups are off");
        let on = StoredPrefs { third_party_lookups: true, ..d.clone() };
        let r = ranked(&on);
        assert_eq!(r.len(), 16);
        assert_eq!(r[..2], [("UNISON".to_string(), true), ("LRCLIB".to_string(), true)], "the ones asked first, in their order");
        assert!(r[2..].iter().all(|(_, on)| !on));
        assert!(titles(&on, &f, "lyrics").contains(&"PaxSenix key".to_string()));
    }

    #[test]
    fn moving_covers_are_off_and_need_the_lookups_switch() {
        let f = SettingsFacts::default();
        let d = StoredPrefs::default();
        let moving = |p: &StoredPrefs| match find(p, &f, "look", "moving-covers") {
            SettingRow::Toggle { on, .. } => on,
            r => panic!("{r:?}"),
        };
        assert!(!moving(&d) && !titles(&d, &f, "look").contains(&"Moving covers on mobile data".to_string()));
        assert!(!moving(&StoredPrefs { motion_artwork: true, ..d.clone() }), "not without looking things up");
        let on = set_by_name(&d, "motionArtwork", "true").unwrap().prefs;
        assert!(on.third_party_lookups && moving(&on), "switching it on switches looking things up on");
        assert!(on.motion_artwork_wifi_only, "Wi-Fi only out of the box");
        assert!(!set_by_name(&on, "motionArtworkMobile", "true").unwrap().prefs.motion_artwork_wifi_only);
    }
}
