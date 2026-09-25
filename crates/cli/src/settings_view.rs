//! Settings, drawn from the core's settings schema (`nori_settings::settings_schema`): its groups, its
//! sections and rows, their words, options, ranges and whether they are live. Nothing here lists a
//! setting by hand: a row Android gains shows up here as it is. A row sends back the setting's name
//! with the value picked (`setting_set`), or moves a level in place (`edit_level`), as Android does.
//! The client's own few settings (the mouse, covers, the volume) come first, as rows of the same kinds.

use nori_core::fmt;
use nori_core::settings::{EqLevel, SoundBand, StoredPrefs, EQ_RANGES};
use nori_core::settings_schema::{self, SettingRow, SettingsFacts, SettingsGroup, SettingsPage, SettingsSection};

use crate::app::{Cmd, Overlay, Screen, Sel, SoundToolCmd};

/// The client's own group, first in the list.
pub const OWN: &str = "terminal";

/// What opening a row does.
pub enum Opened {
    Cmds(Vec<Cmd>),
    Overlay(Overlay),
    Screen(Screen),
    /// One of the client's own switches, by name.
    Own(&'static str),
    Login,
}

/// A line of a page: a section's title, or one of its rows.
pub enum Line<'a> {
    Title(&'a str),
    Row(&'a SettingRow),
}

#[derive(Default)]
pub struct SettingsView {
    /// 0 the groups, 1 the page's rows.
    pub pane: usize,
    pub group: Sel,
    pub row: Sel,
    groups: Vec<SettingsGroup>,
    page: Option<SettingsPage>,
    page_of: Option<String>,
    pub facts: SettingsFacts,
    pub facts_asked: bool,
    /// What the client's own page says: the image protocol, whether the mouse and covers are on, the volume.
    pub own: Own,
}

#[derive(Default, Clone)]
pub struct Own {
    pub mouse: bool,
    pub images: bool,
    pub volume: f32,
    pub protocol: String,
    pub data: String,
}

impl SettingsView {
    pub fn groups(&mut self) -> &[SettingsGroup] {
        if self.groups.is_empty() {
            self.groups.push(SettingsGroup { id: OWN.into(), title: "Terminal".into(), summary: "Mouse, covers, volume".into() });
            self.groups.extend(settings_schema::settings_groups());
        }
        &self.groups
    }

    pub fn invalidate(&mut self) {
        self.page = None;
    }

    pub fn set_facts(&mut self, f: SettingsFacts) {
        self.facts = f;
        self.invalidate();
    }

    pub fn group_id(&mut self) -> String {
        let at = self.group.at;
        self.groups().get(at).map_or_else(String::new, |g| g.id.clone())
    }

    /// The page of the group selected, worked out again only when something it shows changed.
    pub fn page(&mut self, prefs: &StoredPrefs) -> &SettingsPage {
        let id = self.group_id();
        if self.page.is_none() || self.page_of.as_deref() != Some(&id) {
            let page = match id.as_str() {
                OWN => own_page(&self.own),
                "about" => about_page(),
                _ => settings_schema::page(&id, prefs, &self.facts).unwrap_or(SettingsPage { title: id.clone(), sections: Vec::new() }),
            };
            self.page = Some(page);
            self.page_of = Some(id);
        }
        self.page.as_ref().expect("made above")
    }

    /// The page's lines, titles and rows, as drawn.
    pub fn lines<'a>(page: &'a SettingsPage) -> Vec<Line<'a>> {
        let mut out = Vec::new();
        for s in &page.sections {
            if !s.title.is_empty() {
                out.push(Line::Title(&s.title));
            }
            out.extend(s.rows.iter().map(Line::Row));
        }
        out
    }

    /// The list the keys move in: the groups, or the page's lines.
    pub fn list(&mut self) -> (&mut Sel, usize) {
        if self.pane == 0 {
            let n = self.groups().len();
            return (&mut self.group, n);
        }
        let n = self.page.as_ref().map_or(0, |p| Self::lines(p).len());
        (&mut self.row, n)
    }

    /// The row selected on the page, if the page is showing one.
    fn selected(&self) -> Option<&SettingRow> {
        let page = self.page.as_ref()?;
        match Self::lines(page).into_iter().nth(self.row.at)? {
            Line::Row(r) => Some(r),
            Line::Title(_) => None,
        }
    }

    /// Whether ← and → change the row selected (rather than seek).
    pub fn adjustable(&self) -> bool {
        matches!(self.selected(), Some(SettingRow::Choice { .. } | SettingRow::Toggle { .. } | SettingRow::Slider { .. } | SettingRow::Palette { .. } | SettingRow::Ranked { .. }))
    }

    /// Whether a single click does what the row does (a switch, a button), rather than only selecting it.
    pub fn clicks_open(&self) -> bool {
        matches!(self.selected(), Some(SettingRow::Toggle { .. } | SettingRow::Button { .. } | SettingRow::Link { .. } | SettingRow::Action { .. } | SettingRow::Server { .. } | SettingRow::Ranked { .. }))
    }

    /// Enter on the selection.
    pub fn open(&mut self, prefs: &StoredPrefs, _mouse: bool, _images: bool) -> Option<Opened> {
        if self.pane == 0 {
            self.pane = 1;
            self.row = Sel::default();
            self.page(prefs);
            self.skip_titles(true);
            return Some(Opened::Cmds(Vec::new()));
        }
        let row = self.selected()?.clone();
        Some(match row {
            SettingRow::Toggle { name, on, enabled, .. } => match own_name(&name) {
                Some(key) => Opened::Own(key),
                None if enabled => Opened::Cmds(vec![Cmd::Setting(name, (!on).to_string())]),
                None => return None,
            },
            SettingRow::Choice { name, title, options, shown, enabled, .. } if enabled => {
                let at = options.iter().position(|o| o.label == shown).unwrap_or(0);
                let options = options.into_iter().map(|o| (o.label, o.value)).collect();
                Opened::Overlay(Overlay::Picker { title, options, sel: Sel { at, top: 0 }, name })
            }
            SettingRow::Link { action, .. } | SettingRow::Action { action, enabled: true, .. } => match action.as_str() {
                "equalizer" => Opened::Screen(Screen::Equalizer),
                "downloads" => Opened::Screen(Screen::Downloads),
                _ => Opened::Cmds(vec![Cmd::Action(action)]),
            },
            SettingRow::Server { id, active, .. } => {
                if active {
                    return None;
                }
                Opened::Cmds(vec![Cmd::SwitchServer(id)])
            }
            SettingRow::Button { action, .. } if action == "add-server" => Opened::Login,
            SettingRow::Button { action, .. } => Opened::Cmds(vec![Cmd::Action(action)]),
            SettingRow::Ranked { name, on, .. } => Opened::Cmds(vec![Cmd::Setting(name, (!on).to_string())]),
            SettingRow::Text { name, title, value, secret, .. } => Opened::Overlay(Overlay::Input { title, text: value, secret, name }),
            SettingRow::Palette { name, colours, chosen } => {
                let at = colours.iter().position(|c| *c == chosen).map_or(0, |i| (i + 1) % colours.len());
                Opened::Cmds(vec![Cmd::Setting(name, colours[at].to_string())])
            }
            _ => return None,
        })
    }

    /// ← or → on the selection: the previous or next option, off or on, a step of a slider.
    pub fn step(&mut self, _prefs: &StoredPrefs, up: bool) -> Vec<Cmd> {
        let Some(row) = self.selected().cloned() else { return Vec::new() };
        match row {
            SettingRow::Choice { name, options, shown, enabled: true, .. } => {
                let at = options.iter().position(|o| o.label == shown);
                let to = match at {
                    Some(i) if up => (i + 1).min(options.len() - 1),
                    Some(i) => i.saturating_sub(1),
                    None => 0,
                };
                if Some(to) == at {
                    return Vec::new();
                }
                vec![Cmd::Setting(name, options[to].value.clone())]
            }
            SettingRow::Toggle { name, on, enabled, .. } => {
                if on == up {
                    return Vec::new();
                }
                match own_name(&name) {
                    Some("mouse") => vec![Cmd::Mouse(up)],
                    Some("images") => vec![Cmd::Images(up)],
                    Some(_) => Vec::new(),
                    None if enabled => vec![Cmd::Setting(name, up.to_string())],
                    None => Vec::new(),
                }
            }
            SettingRow::Slider { name, value, min, max, level, .. } => {
                let step = slider_step(min, max);
                let v = (value + if up { step } else { -step }).clamp(min, max);
                match (name.as_str(), level) {
                    ("!volume", _) => vec![Cmd::Volume(v)],
                    (_, Some(l)) => vec![Cmd::Level(l, v)],
                    (_, None) => vec![Cmd::Setting(name, v.to_string())],
                }
            }
            SettingRow::Palette { name, colours, chosen } => {
                let at = colours.iter().position(|c| *c == chosen).unwrap_or(0) as isize + if up { 1 } else { -1 };
                let at = at.rem_euclid(colours.len() as isize) as usize;
                vec![Cmd::Setting(name, colours[at].to_string())]
            }
            SettingRow::Ranked { id, on: true, .. } => vec![Cmd::Setting("lyricsMove".into(), format!("{id}:{}", if up { 1 } else { -1 }))],
            _ => Vec::new(),
        }
    }

    /// The selection moved off a section's title, onto a row.
    pub fn skip_titles(&mut self, down: bool) {
        let Some(page) = &self.page else { return };
        let lines = Self::lines(page);
        let title = |i: usize| matches!(lines.get(i), Some(Line::Title(_)));
        let mut at = self.row.at;
        while title(at) && at + 1 < lines.len() && down {
            at += 1;
        }
        while title(at) && at > 0 && !down {
            at -= 1;
        }
        // At either end, a title is left the other way.
        while title(at) && at + 1 < lines.len() {
            at += 1;
        }
        self.row.at = at;
    }
}

/// A slider's step: a fortieth of its range, in half decibels for the dB ranges.
pub fn slider_step(min: f32, max: f32) -> f32 {
    let span = max - min;
    if span >= 8.0 {
        0.5
    } else {
        (span / 40.0).max(0.01)
    }
}

fn own_name(name: &str) -> Option<&'static str> {
    match name {
        "!mouse" => Some("mouse"),
        "!images" => Some("images"),
        _ => None,
    }
}

fn toggle(name: &str, title: &str, detail: &str, on: bool) -> SettingRow {
    SettingRow::Toggle { key: settings_schema::setting_key(title), name: name.into(), title: title.into(), detail: detail.into(), on, enabled: true }
}

fn info(title: &str, detail: String) -> SettingRow {
    SettingRow::Info { key: settings_schema::setting_key(title), title: title.into(), detail }
}

/// The client's own page: rows of the schema's kinds, so they are drawn and changed like the rest.
pub fn own_page(o: &Own) -> SettingsPage {
    let rows = vec![
        toggle("!mouse", "Mouse", "Clicks, the wheel and dragging the seek bar. Off, the terminal selects text (m).", o.mouse),
        toggle("!images", "Covers", "Album art in the player and on album pages (I).", o.images),
        info("Pictures drawn with", o.protocol.clone()),
        SettingRow::Slider { name: "!volume".into(), label: format!("Volume {} %", (o.volume * 100.0).round()), value: o.volume, min: 0.0, max: 1.0, centred: false, level: None },
        info("Kept in", o.data.clone()),
    ];
    SettingsPage { title: "Terminal".into(), sections: vec![SettingsSection { title: "This client".into(), rows }] }
}

/// About: what the core is built from, in the schema's own credits.
pub fn about_page() -> SettingsPage {
    let version = SettingsSection {
        title: "nori".into(),
        rows: vec![info("Version", env!("CARGO_PKG_VERSION").into()), info("Terminal client", "ratatui over crossterm; covers through ratatui-image".into())],
    };
    let credits = settings_schema::core_credits().into_iter().map(|c| info(&c.name, format!("{} · {} · {}", c.what, c.copyright, c.licence))).collect();
    let tui = vec![
        info("ratatui, crossterm", "The screen and the keys · The Ratatui developers, Timon Post · MIT".into()),
        info("ratatui-image", "Covers in the terminal: kitty, sixel, iTerm2, half blocks · Benjamin Große · MIT".into()),
        info("image, icy_sixel", "Pictures handed to the terminal · The image-rs developers, Mike Krüger · MIT or Apache-2.0".into()),
    ];
    SettingsPage { title: "About".into(), sections: vec![version, SettingsSection { title: "The core".into(), rows: credits }, SettingsSection { title: "The terminal client".into(), rows: tui }] }
}

/// The words a row shows at its end: a switch, the option chosen, a slider's value.
pub fn row_value(row: &SettingRow) -> String {
    match row {
        SettingRow::Toggle { on, .. } | SettingRow::Ranked { on, .. } => (if *on { "● on" } else { "○ off" }).into(),
        SettingRow::Choice { shown, .. } => format!("‹ {shown} ›"),
        SettingRow::Link { status, .. } => format!("{status} ›"),
        SettingRow::Action { button, .. } => format!("[ {button} ]"),
        SettingRow::Button { title, .. } => format!("[ {title} ]"),
        SettingRow::Server { active, .. } => (if *active { "● in use" } else { "" }).into(),
        SettingRow::Text { value, secret, .. } => {
            if value.is_empty() {
                "none".into()
            } else if *secret {
                "••••••".into()
            } else {
                value.clone()
            }
        }
        _ => String::new(),
    }
}

/// A row's title and the line under it.
pub fn row_words(row: &SettingRow) -> (String, String) {
    match row {
        SettingRow::Toggle { title, detail, .. } | SettingRow::Ranked { title, detail, .. } | SettingRow::Text { title, detail, .. } | SettingRow::Info { title, detail, .. } => {
            (title.clone(), detail.clone())
        }
        SettingRow::Choice { title, .. } => (title.clone(), String::new()),
        SettingRow::Note { text } => (String::new(), text.clone()),
        SettingRow::Link { title, .. } => (title.clone(), String::new()),
        SettingRow::Action { title, detail, .. } => (title.clone(), detail.clone()),
        SettingRow::Slider { label, .. } => (label.clone(), String::new()),
        SettingRow::Palette { .. } => ("Accent colour".into(), String::new()),
        SettingRow::Server { label, detail, .. } => (label.clone(), detail.clone()),
        SettingRow::Button { .. } => (String::new(), String::new()),
    }
}

/// Whether the row is live: a setting the app is ignoring right now is drawn dimmed.
pub fn row_enabled(row: &SettingRow) -> bool {
    match row {
        SettingRow::Toggle { enabled, .. } | SettingRow::Choice { enabled, .. } | SettingRow::Action { enabled, .. } => *enabled,
        SettingRow::Link { dimmed, .. } => !dimmed,
        _ => true,
    }
}

/// A slider's place, 0 to 1.
pub fn slider_share(row: &SettingRow) -> Option<(f32, bool)> {
    match row {
        SettingRow::Slider { value, min, max, centred, .. } => Some((((value - min) / (max - min).max(1e-6)).clamp(0.0, 1.0), *centred)),
        _ => None,
    }
}

// ---- the equalizer ----

/// The equalizer screen's rows, from the settings: the switch, the presets, the pre-amp, every band,
/// then the rest of the chain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EqRow {
    Enabled,
    Presets,
    AutoPreamp,
    Preamp,
    Band(usize),
    AddBand,
    Reset,
    Balance,
    Crossfeed,
    Mono,
    Limiter,
    Ceiling,
}

pub fn eq_rows(p: &StoredPrefs) -> Vec<EqRow> {
    let mut rows = vec![EqRow::Enabled, EqRow::Presets, EqRow::AutoPreamp];
    if p.eq_preamp_db.is_some() {
        rows.push(EqRow::Preamp);
    }
    rows.extend((0..p.eq_bands.len()).map(EqRow::Band));
    rows.extend([EqRow::AddBand, EqRow::Reset, EqRow::Balance, EqRow::Crossfeed, EqRow::Mono, EqRow::Limiter]);
    if p.limiter {
        rows.push(EqRow::Ceiling);
    }
    rows
}

impl EqRow {
    /// Its title and value, in the core's words.
    pub fn words(&self, p: &StoredPrefs) -> (String, String) {
        let on = |b: bool| (if b { "● on" } else { "○ off" }).to_string();
        match *self {
            EqRow::Enabled => ("Equalizer".into(), on(p.eq_enabled)),
            EqRow::Presets => ("Presets".into(), "choose ›".into()),
            EqRow::AutoPreamp => ("Automatic pre-amp".into(), on(p.eq_preamp_db.is_none())),
            EqRow::Preamp => ("Pre-amp".into(), fmt::eq_preamp(p.eq_preamp_db.unwrap_or(0.0), false)),
            EqRow::Band(i) => {
                let b = p.eq_bands.get(i).copied().unwrap_or(SoundBand { kind: 0, freq: 0.0, gain_db: 0.0, q: 1.0, channel: 0 });
                (nori_core::settings::band_label(&b), format!("{} dB", fmt::signed_db(b.gain_db)))
            }
            EqRow::AddBand => ("Add a band".into(), "[ Add ]".into()),
            EqRow::Reset => ("Back to flat".into(), "[ Reset ]".into()),
            EqRow::Balance => ("Balance".into(), fmt::eq_balance(p.balance)),
            EqRow::Crossfeed => ("Crossfeed".into(), if p.crossfeed_db > 0.0 { format!("{} dB", fmt::signed_db(p.crossfeed_db)) } else { "Off".into() }),
            EqRow::Mono => ("Mono".into(), on(p.mono)),
            EqRow::Limiter => ("Limiter".into(), on(p.limiter)),
            EqRow::Ceiling => ("Limiter ceiling".into(), fmt::eq_ceiling(p.limiter_threshold_db)),
        }
    }

    /// ← or →.
    pub fn step(&self, p: &StoredPrefs, up: bool) -> Option<Cmd> {
        let d = if up { 0.5 } else { -0.5 };
        let r = EQ_RANGES;
        match *self {
            EqRow::Enabled => (p.eq_enabled != up).then_some(Cmd::Setting("eq".into(), up.to_string())),
            EqRow::Mono => (p.mono != up).then_some(Cmd::Setting("mono".into(), up.to_string())),
            EqRow::Limiter => (p.limiter != up).then_some(Cmd::Setting("limiter".into(), up.to_string())),
            EqRow::AutoPreamp => (p.eq_preamp_db.is_none() != up).then_some(Cmd::Sound(SoundToolCmd::AutoPreamp(up))),
            EqRow::Preamp => Some(Cmd::Level(EqLevel::Preamp, (p.eq_preamp_db.unwrap_or(0.0) + d).clamp(r.preamp.min, r.preamp.max))),
            EqRow::Band(i) => {
                let b = *p.eq_bands.get(i)?;
                Some(Cmd::Band(i as u32, SoundBand { gain_db: (b.gain_db + d).clamp(r.gain.min, r.gain.max), ..b }))
            }
            EqRow::Balance => Some(Cmd::Level(EqLevel::Balance, (p.balance + d / 10.0).clamp(r.balance.min, r.balance.max))),
            EqRow::Crossfeed => Some(Cmd::Level(EqLevel::Crossfeed, (p.crossfeed_db.max(if up { 0.5 } else { 0.0 }) + d).clamp(r.crossfeed.min, r.crossfeed.max))),
            EqRow::Ceiling => Some(Cmd::Level(EqLevel::Limiter, (p.limiter_threshold_db + d).clamp(r.limiter.min, r.limiter.max))),
            EqRow::Presets | EqRow::AddBand | EqRow::Reset => None,
        }
    }

    /// Enter.
    pub fn open(&self, p: &StoredPrefs) -> Option<Cmd> {
        match *self {
            EqRow::Enabled => Some(Cmd::Setting("eq".into(), (!p.eq_enabled).to_string())),
            EqRow::Mono => Some(Cmd::Setting("mono".into(), (!p.mono).to_string())),
            EqRow::Limiter => Some(Cmd::Setting("limiter".into(), (!p.limiter).to_string())),
            EqRow::AutoPreamp => Some(Cmd::Sound(SoundToolCmd::AutoPreamp(p.eq_preamp_db.is_some()))),
            EqRow::AddBand => Some(Cmd::Sound(SoundToolCmd::AddBand)),
            EqRow::Reset => Some(Cmd::Sound(SoundToolCmd::ResetBands)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_row(prefs: &StoredPrefs) -> Vec<(String, SettingRow)> {
        let facts = SettingsFacts::default();
        let mut out = Vec::new();
        for g in settings_schema::settings_groups() {
            if let Some(p) = settings_schema::page(&g.id, prefs, &facts) {
                for s in p.sections {
                    out.extend(s.rows.into_iter().map(|r| (g.id.clone(), r)));
                }
            }
        }
        out
    }

    #[test]
    fn every_group_of_the_schema_is_listed_after_the_clients_own() {
        let mut v = SettingsView::default();
        let ids: Vec<String> = v.groups().iter().map(|g| g.id.clone()).collect();
        let schema: Vec<String> = settings_schema::settings_groups().into_iter().map(|g| g.id).collect();
        assert_eq!(ids[0], OWN);
        assert_eq!(ids[1..], schema[..]);
    }

    #[test]
    fn every_setting_the_schema_has_opens_or_steps_to_a_setting_by_its_own_name() {
        // With everything that reveals more rows switched on, every row the schema can show is here.
        let prefs = StoredPrefs { auto_mix: true, auto_mix_beat_match: true, auto_fill: true, replay_gain: 1, scrobble: true, lyrics_online: true, third_party_lookups: true, ..StoredPrefs::default() };
        let rows = every_row(&prefs);
        assert!(rows.len() > 40, "the schema's rows: {}", rows.len());
        for (group, row) in &rows {
            let mut v = SettingsView { pane: 1, ..Default::default() };
            v.page = Some(SettingsPage { title: group.clone(), sections: vec![SettingsSection { title: String::new(), rows: vec![row.clone()] }] });
            v.page_of = None;
            match row {
                SettingRow::Toggle { name, enabled: true, .. } => {
                    let Some(Opened::Cmds(c)) = v.open(&prefs, true, true) else { panic!("{name} does not switch") };
                    assert!(matches!(&c[0], Cmd::Setting(n, _) if n == name), "{name}");
                }
                SettingRow::Choice { name, enabled: true, options, .. } => {
                    let Some(Opened::Overlay(Overlay::Picker { name: picked, options: shown, .. })) = v.open(&prefs, true, true) else { panic!("{name} offers no choice") };
                    assert_eq!(&picked, name);
                    assert_eq!(shown.len(), options.len());
                    // And ← → step through the same options by the same name.
                    let steps = [v.step(&prefs, true), v.step(&prefs, false)].concat();
                    assert!(steps.iter().all(|c| matches!(c, Cmd::Setting(n, _) if n == name)), "{name}");
                }
                SettingRow::Slider { name, level, .. } => {
                    let c = v.step(&prefs, true);
                    assert!(matches!(&c[..], [Cmd::Level(l, _)] if Some(*l) == *level) || matches!(&c[..], [Cmd::Setting(n, _)] if n == name), "{name}");
                }
                _ => {}
            }
        }
        // Every setting name the rows send is one the core knows.
        for (_, row) in &rows {
            if let SettingRow::Toggle { name, .. } | SettingRow::Choice { name, .. } = row {
                let value = match row {
                    SettingRow::Choice { options, .. } => options[0].value.clone(),
                    _ => "true".into(),
                };
                assert!(nori_core::settings::set_by_name(&prefs, name, &value).is_some(), "{name} is not a setting");
            }
        }
    }

    #[test]
    fn the_equalizer_lists_every_band_and_steps_them_in_range() {
        let prefs = StoredPrefs { eq_bands: nori_core::settings::graphic(), ..StoredPrefs::default() };
        let rows = eq_rows(&prefs);
        assert_eq!(rows.iter().filter(|r| matches!(r, EqRow::Band(_))).count(), prefs.eq_bands.len());
        let Some(Cmd::Band(0, b)) = EqRow::Band(0).step(&prefs, true) else { panic!() };
        assert_eq!(b.gain_db, prefs.eq_bands[0].gain_db + 0.5);
        let loud = StoredPrefs { eq_bands: prefs.eq_bands.iter().map(|b| SoundBand { gain_db: 12.0, ..*b }).collect(), ..prefs.clone() };
        let Some(Cmd::Band(_, b)) = EqRow::Band(0).step(&loud, true) else { panic!() };
        assert_eq!(b.gain_db, 12.0, "held in the core's range");
    }
}
