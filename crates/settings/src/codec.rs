//! How each setting is stored, changed by name and read back by name: one [`Codec`] per kind of value,
//! and the [`Row`] each setting becomes. `StoredPrefs` (settings.rs) says, on each field in one
//! `#[setting(...)]` line, its key, codec, default, name, the row a client offers for it and what a change
//! of it asks of the player; `#[derive(Settings)]` (crates/settings-derive) makes its `Default` and the
//! table `ROWS` from them. Everything that walks the settings (`load`, `save`, `set_by_name`, `value_of`,
//! `specs`, `effects`) walks that table.

use std::collections::HashMap;

use crate::settings::{PrefValue, SavedQuality, Span, StoredPrefs};

/// What was stored, as it is: a value of the wrong kind is as good as missing.
pub(crate) struct Raw<'a>(pub(crate) &'a HashMap<String, PrefValue>);

impl Raw<'_> {
    fn get<T: Scalar>(&self, k: &str) -> Option<T> {
        self.0.get(k).and_then(T::read)
    }
}

/// Everything to write.
pub(crate) type Put = HashMap<String, PrefValue>;

/// A value stored as one [`PrefValue`] of its own kind, and read from a change by name: `None` for a
/// value that does not read (a number that is not one).
pub(crate) trait Scalar: Sized + Clone + PartialOrd + ToString {
    fn read(v: &PrefValue) -> Option<Self>;
    fn write(&self) -> PrefValue;
    fn parse(value: &str) -> Option<Self>;
}

macro_rules! scalar {
    ($($t:ty => $kind:ident, $parse:expr),*) => {$(
        impl Scalar for $t {
            fn read(v: &PrefValue) -> Option<Self> {
                if let PrefValue::$kind { v } = v { Some(v.clone()) } else { None }
            }
            fn write(&self) -> PrefValue {
                PrefValue::$kind { v: self.clone() }
            }
            fn parse(value: &str) -> Option<Self> {
                $parse(value)
            }
        }
    )*};
}
// A switch reads "true" (any case) or "1" as on, anything else as off; text is trimmed.
scalar!(
    bool => Flag, |v: &str| Some(v.eq_ignore_ascii_case("true") || v == "1"),
    i32 => Number, |v: &str| v.trim().parse().ok(),
    i64 => Big, |v: &str| v.trim().parse().ok(),
    f32 => Decimal, |v: &str| v.trim().parse().ok(),
    String => Text, |v: &str| Some(v.trim().to_string())
);

/// One kind of stored value. `set` is a change by name: `None` refuses the value (the change fails, so a
/// typo in a script fails loudly), and a number that does not read keeps `now`. `show` is the value in
/// the form a client's options are in.
pub(crate) trait Codec<T> {
    fn load(&self, r: &Raw, key: &str, default: T) -> T;
    fn save(&self, v: &T, key: &str, out: &mut Put);
    fn set(&self, _value: &str, _now: &T) -> Option<T> {
        None
    }
    fn show(&self, _v: &T) -> String {
        String::new()
    }
}

fn put(out: &mut Put, k: &str, v: PrefValue) {
    out.insert(k.to_string(), v);
}

/// A switch reads "true" (any case) or "1" as on, anything else as off.
pub(crate) fn on(value: &str) -> bool {
    bool::parse(value) == Some(true)
}

/// Comma-separated names, each trimmed, the empty ones left out.
pub(crate) fn names(value: &str) -> Vec<String> {
    value.split(',').map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).collect()
}

/// A switch, a number or a text, as it was stored. A change by name is held in `range`; with `held`, what
/// is loaded is too, otherwise it is taken as it was stored.
pub(crate) struct Plain<T> {
    range: Option<(T, T)>,
    held: bool,
}

pub(crate) const FLAG: Plain<bool> = Plain { range: None, held: false };
pub(crate) const TEXT: Plain<String> = Plain { range: None, held: false };
pub(crate) const INT: Plain<i32> = Plain { range: None, held: false };
pub(crate) const LONG: Plain<i64> = Plain { range: None, held: false };
pub(crate) const FLOAT: Plain<f32> = Plain { range: None, held: false };

/// A number whose changes by name are held in `lo..=hi`.
pub(crate) const fn within<T>(lo: T, hi: T) -> Plain<T> {
    Plain { range: Some((lo, hi)), held: false }
}

/// A number held in `lo..=hi` both when it is loaded and when it is changed.
pub(crate) const fn clamped<T>(lo: T, hi: T) -> Plain<T> {
    Plain { range: Some((lo, hi)), held: true }
}

impl<T: Scalar> Plain<T> {
    fn hold(&self, v: T) -> T {
        match &self.range {
            Some((lo, _)) if v < *lo => lo.clone(),
            Some((_, hi)) if v > *hi => hi.clone(),
            _ => v,
        }
    }
}

impl<T: Scalar> Codec<T> for Plain<T> {
    fn load(&self, r: &Raw, k: &str, d: T) -> T {
        let v = r.get(k).unwrap_or(d);
        if self.held { self.hold(v) } else { v }
    }
    fn save(&self, v: &T, k: &str, out: &mut Put) {
        put(out, k, v.write());
    }
    fn set(&self, value: &str, now: &T) -> Option<T> {
        Some(T::parse(value).map_or_else(|| now.clone(), |v| self.hold(v)))
    }
    fn show(&self, v: &T) -> String {
        v.to_string()
    }
}

/// An enum setting (`#[derive(Choice)]`, crates/settings-derive): every value in order, and each one's
/// name, the same as the Kotlin bindings give it.
pub(crate) trait Choice: Copy + PartialEq + 'static {
    const ALL: &'static [Self];
    const NAMES: &'static [&'static str];

    /// Its place in [`Self::ALL`]: what it is stored as.
    fn ordinal(self) -> i32 {
        Self::ALL.iter().position(|c| *c == self).unwrap_or(0) as i32
    }
    fn nth(i: i32) -> Option<Self> {
        usize::try_from(i).ok().and_then(|i| Self::ALL.get(i)).copied()
    }
    fn named(name: &str) -> Option<Self> {
        Self::NAMES.iter().position(|n| *n == name).map(|i| Self::ALL[i])
    }
    fn name(self) -> &'static str {
        Self::NAMES[self.ordinal() as usize]
    }
}

/// An enum stored as its ordinal and changed by its name (any case) or its ordinal. One stored out of
/// range (a value from a newer version) is the default, or with `nearest` the nearest.
pub(crate) struct Pick {
    nearest: bool,
}

pub(crate) const PICK: Pick = Pick { nearest: false };
pub(crate) const PICK_NEAREST: Pick = Pick { nearest: true };

impl<T: Choice> Codec<T> for Pick {
    fn load(&self, r: &Raw, k: &str, d: T) -> T {
        match r.get::<i32>(k) {
            Some(v) if self.nearest => T::ALL[v.clamp(0, T::ALL.len() as i32 - 1) as usize],
            Some(v) => T::nth(v).unwrap_or(d),
            None => d,
        }
    }
    fn save(&self, v: &T, k: &str, out: &mut Put) {
        put(out, k, v.ordinal().write());
    }
    fn set(&self, value: &str, _: &T) -> Option<T> {
        let by_name = T::NAMES.iter().position(|n| n.eq_ignore_ascii_case(value)).map(|i| T::ALL[i]);
        by_name.or_else(|| T::nth(value.trim().parse().ok()?))
    }
    fn show(&self, v: &T) -> String {
        v.name().to_string()
    }
}

/// A list of an enum's values, stored by name, comma-separated; a name that is not one is dropped.
pub(crate) struct Picks;

impl<T: Choice> Codec<Vec<T>> for Picks {
    fn load(&self, r: &Raw, k: &str, d: Vec<T>) -> Vec<T> {
        r.get::<String>(k).map_or(d, |s| s.split(',').filter_map(T::named).collect())
    }
    fn save(&self, v: &Vec<T>, k: &str, out: &mut Put) {
        put(out, k, PrefValue::Text { v: v.iter().map(|c| c.name()).collect::<Vec<_>>().join(",") });
    }
}

/// Stream quality, under `<key>BitRate` and `<key>Format`; by name "0:" is the original file, "320:mp3" a
/// bitrate and a format.
pub(crate) struct Quality;

impl Codec<SavedQuality> for Quality {
    fn load(&self, r: &Raw, k: &str, d: SavedQuality) -> SavedQuality {
        SavedQuality { bit_rate: r.get(&format!("{k}BitRate")).unwrap_or(d.bit_rate), format: r.get(&format!("{k}Format")).unwrap_or(d.format) }
    }
    fn save(&self, v: &SavedQuality, k: &str, out: &mut Put) {
        put(out, &format!("{k}BitRate"), v.bit_rate.write());
        put(out, &format!("{k}Format"), v.format.write());
    }
    fn set(&self, value: &str, _: &SavedQuality) -> Option<SavedQuality> {
        let (rate, format) = value.split_once(':')?;
        Some(SavedQuality { bit_rate: rate.trim().parse().ok()?, format: format.trim().to_string() })
    }
    fn show(&self, v: &SavedQuality) -> String {
        format!("{}:{}", v.bit_rate, v.format)
    }
}

/// The equalizer's own pre-amp: a decimal stored only when it is set, none (automatic) otherwise. By name
/// a number is held in its span, "auto" is automatic, anything else keeps it.
pub(crate) struct Preamp(pub(crate) Span);

impl Codec<Option<f32>> for Preamp {
    fn load(&self, r: &Raw, k: &str, _: Option<f32>) -> Option<f32> {
        r.get(k)
    }
    fn save(&self, v: &Option<f32>, k: &str, out: &mut Put) {
        if let Some(v) = v {
            put(out, k, v.write());
        }
    }
    fn set(&self, value: &str, now: &Option<f32>) -> Option<Option<f32>> {
        Some(match value.trim().parse::<f32>() {
            Ok(v) => Some(self.0.hold(v)),
            Err(_) if value.trim().eq_ignore_ascii_case("auto") => None,
            Err(_) => *now,
        })
    }
    fn show(&self, v: &Option<f32>) -> String {
        v.map_or_else(|| "auto".to_string(), |v| v.to_string())
    }
}

/// A value kept as one text in a format of its own: `load` reads the stored text (none when there is
/// none) against the default, `save` writes it; `set` and `show` when it is changed and read by name.
pub(crate) struct Custom<T: 'static> {
    pub(crate) load: fn(Option<&str>, T) -> T,
    pub(crate) save: fn(&T) -> String,
    pub(crate) set: Option<fn(&str) -> Option<T>>,
    pub(crate) show: Option<fn(&T) -> String>,
}

impl<T> Codec<T> for Custom<T> {
    fn load(&self, r: &Raw, k: &str, d: T) -> T {
        (self.load)(r.0.get(k).and_then(|v| if let PrefValue::Text { v } = v { Some(v.as_str()) } else { None }), d)
    }
    fn save(&self, v: &T, k: &str, out: &mut Put) {
        put(out, k, PrefValue::Text { v: (self.save)(v) });
    }
    fn set(&self, value: &str, _: &T) -> Option<T> {
        self.set.and_then(|f| f(value))
    }
    fn show(&self, v: &T) -> String {
        self.show.map(|f| f(v)).unwrap_or_default()
    }
}

/// What a client's settings screen offers for a setting (`settings_model::SettingSpec`).
pub(crate) enum K {
    Switch,
    Choice(&'static [&'static str]),
    /// An enum setting: its values by name, in ordinal order.
    Named(&'static [&'static str]),
    Level(f32, f32),
    Text,
    Colour,
}

/// One setting, as the table declares it.
pub(crate) struct Row {
    /// Where it is stored (read by the tests: the table's own keys are the store's).
    #[cfg_attr(not(test), allow(dead_code))]
    pub(crate) key: &'static str,
    /// The name it is changed and read by (`set_by_name`, `value_of`); none for one that is not.
    pub(crate) name: Option<&'static str>,
    /// What a client offers for it; none for one that is not offered.
    pub(crate) spec: Option<K>,
    /// What a change of it asks of the player (`settings_store`'s bits).
    pub(crate) effect: u32,
    /// A switch over something looked up online: it reads off while the lookups are off, and switching
    /// it on switches them on.
    pub(crate) lookups: bool,
    pub(crate) load: fn(&mut StoredPrefs, &Raw),
    pub(crate) save: fn(&StoredPrefs, &mut Put),
    pub(crate) set: fn(&mut StoredPrefs, &str) -> Option<()>,
    pub(crate) show: fn(&StoredPrefs) -> String,
    pub(crate) changed: fn(&StoredPrefs, &StoredPrefs) -> bool,
}
