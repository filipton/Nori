//! Smart playlists: a rule tree evaluated against the local index and the play statistics.
//!
//! uniffi records cannot nest themselves, so a definition crosses the FFI as a JSON string:
//!
//! ```json
//! {
//!   "match": { "all": true, "rules": [
//!     { "field": "genre", "op": "is", "value": "Rock" },
//!     { "all": false, "rules": [
//!       { "field": "year", "op": "between", "value": [1970, 1979] },
//!       { "field": "starred", "op": "isTrue" } ] } ] },
//!   "sort": { "field": "playCount", "descending": true },
//!   "limit": 50,
//!   "limitMs": 3600000
//! }
//! ```
//!
//! - `match` (optional, default: every song) is a group. A group is `{ "all": bool, "rules": [node..] }`:
//!   `all` true (the default) means every node must match, false means any. A node is a rule or another
//!   group, up to 8 levels deep. An empty group matches everything when `all`, nothing otherwise.
//! - A rule is `{ "field", "op", "value" }`. What `value` must be depends on the field's type:
//!
//!   | type   | fields | operators and their value |
//!   |--------|--------|---------------------------|
//!   | text   | `title` `album` `artist` `genre` `suffix` | `is` `isNot` `contains` `notContains` `startsWith` `endsWith`: a string, compared without regard to case |
//!   | number | `year` `duration` (s) `track` `discNumber` `bitRate` (kbps) `sampleRate` (Hz) `bitDepth` `size` (bytes) `userRating` (0-5) `playCount` `skipCount` (this device) `serverPlayCount` | `is` `isNot` `greater` `less`: an integer; `between`: `[low, high]`, inclusive |
//!   | date   | `lastPlayed` (this device) `added` (server) | `withinDays` `notWithinDays`: a number of days; `greater` (after) `less` (before): `"YYYY-MM-DD"` or `"YYYY-MM-DDTHH:MM:SS"`, UTC; `between`: two of those, a bare end date includes its whole day. A song never played, or with no added date, is "not within" and "before" everything |
//!   | flag   | `starred` `isDownloaded` `excludedFromMixes` | `isTrue` `isFalse`: no value |
//!
//! - `sort` (optional, default: index order): `field` is any text, number or date field, `starred`, or
//!   `"random"`; `descending` (default false); `seed` (default 0) makes `random` a stable order, so
//!   paging works and a new seed is a reshuffle.
//! - `limit` (optional) caps the number of songs, `limitMs` (optional) the total duration: the playlist
//!   ends before the first song that would exceed it. 0 or absent means no cap.
//!
//! Unknown keys, fields and operators are errors, with the path of the offending node in the message.
//!
//! Evaluation. The tree becomes one SQL condition over `items` joined with `song_stats`, and the
//! statement returns rowids only, in order, so neither the sorter nor Rust ever holds 100k JSON
//! documents; the JSON of the requested page is then fetched by rowid. Two things SQL cannot do are
//! finished in Rust while streaming those rowids: text rules with a non-ASCII value (SQLite folds
//! case for ASCII only; such a rule is relaxed to "true" in SQL and every candidate re-checked here),
//! and the `limitMs` running total.

use std::collections::HashSet;

use rusqlite::{params, types::Value as Sql, Connection, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::{db, mixes, model::*, Core, CoreError, Result};

const DAY_MS: i64 = 86_400_000;
const MAX_DEPTH: usize = 8;

// ---- the definition ---------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq)]
enum Kind {
    Text,
    Num,
    /// Epoch milliseconds from `song_stats`; 0 is never.
    DateMs,
    /// ISO-8601 string from the song JSON; compares as text. Missing is ''.
    DateIso,
    Flag,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Field {
    Title,
    Album,
    Artist,
    Genre,
    Suffix,
    Year,
    Duration,
    Track,
    DiscNumber,
    BitRate,
    SampleRate,
    BitDepth,
    Size,
    UserRating,
    PlayCount,
    SkipCount,
    ServerPlayCount,
    LastPlayed,
    Added,
    Starred,
    IsDownloaded,
    ExcludedFromMixes,
}

use Field::*;

const FIELDS: [Field; 22] = [
    Title, Album, Artist, Genre, Suffix, Year, Duration, Track, DiscNumber, BitRate, SampleRate, BitDepth, Size, UserRating, PlayCount, SkipCount, ServerPlayCount, LastPlayed, Added, Starred,
    IsDownloaded, ExcludedFromMixes,
];

impl Field {
    /// (name in the JSON, type, SQL expression). Text expressions are nullable; the rest are not.
    fn def(self) -> (&'static str, Kind, &'static str) {
        match self {
            Title => ("title", Kind::Text, "json_extract(i.json,'$.title')"),
            Album => ("album", Kind::Text, "json_extract(i.json,'$.album')"),
            Artist => ("artist", Kind::Text, "json_extract(i.json,'$.artist')"),
            Genre => ("genre", Kind::Text, "json_extract(i.json,'$.genre')"),
            Suffix => ("suffix", Kind::Text, "json_extract(i.json,'$.suffix')"),
            Year => ("year", Kind::Num, "json_extract(i.json,'$.year')"),
            Duration => ("duration", Kind::Num, "json_extract(i.json,'$.duration')"),
            Track => ("track", Kind::Num, "json_extract(i.json,'$.track')"),
            DiscNumber => ("discNumber", Kind::Num, "json_extract(i.json,'$.discNumber')"),
            BitRate => ("bitRate", Kind::Num, "json_extract(i.json,'$.bitRate')"),
            SampleRate => ("sampleRate", Kind::Num, "json_extract(i.json,'$.samplingRate')"),
            BitDepth => ("bitDepth", Kind::Num, "json_extract(i.json,'$.bitDepth')"),
            Size => ("size", Kind::Num, "json_extract(i.json,'$.size')"),
            UserRating => ("userRating", Kind::Num, "json_extract(i.json,'$.userRating')"),
            PlayCount => ("playCount", Kind::Num, "coalesce(s.plays,0)"),
            SkipCount => ("skipCount", Kind::Num, "coalesce(s.skips,0)"),
            ServerPlayCount => ("serverPlayCount", Kind::Num, "coalesce(json_extract(i.json,'$.playCount'),0)"),
            LastPlayed => ("lastPlayed", Kind::DateMs, "coalesce(s.last_played_ms,0)"),
            Added => ("added", Kind::DateIso, "coalesce(json_extract(i.json,'$.created'),'')"),
            Starred => ("starred", Kind::Flag, "json_extract(i.json,'$.starred')"),
            IsDownloaded => ("isDownloaded", Kind::Flag, ""),
            ExcludedFromMixes => ("excludedFromMixes", Kind::Flag, ""),
        }
    }

    fn name(self) -> &'static str {
        self.def().0
    }

    fn kind(self) -> Kind {
        self.def().1
    }

    /// The `song_stats` column behind the field.
    fn stats_column(self) -> Option<&'static str> {
        match self {
            PlayCount => Some("s.plays"),
            SkipCount => Some("s.skips"),
            LastPlayed => Some("s.last_played_ms"),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Op {
    Is,
    IsNot,
    Contains,
    NotContains,
    StartsWith,
    EndsWith,
    Greater,
    Less,
    Between,
    WithinDays,
    NotWithinDays,
    IsTrue,
    IsFalse,
}

const OPS: [(&str, Op); 13] = [
    ("is", Op::Is),
    ("isNot", Op::IsNot),
    ("contains", Op::Contains),
    ("notContains", Op::NotContains),
    ("startsWith", Op::StartsWith),
    ("endsWith", Op::EndsWith),
    ("greater", Op::Greater),
    ("less", Op::Less),
    ("between", Op::Between),
    ("withinDays", Op::WithinDays),
    ("notWithinDays", Op::NotWithinDays),
    ("isTrue", Op::IsTrue),
    ("isFalse", Op::IsFalse),
];

fn ops_of(kind: Kind) -> &'static [Op] {
    match kind {
        Kind::Text => &[Op::Is, Op::IsNot, Op::Contains, Op::NotContains, Op::StartsWith, Op::EndsWith],
        Kind::Num => &[Op::Is, Op::IsNot, Op::Greater, Op::Less, Op::Between],
        Kind::DateMs | Kind::DateIso => &[Op::WithinDays, Op::NotWithinDays, Op::Greater, Op::Less, Op::Between],
        Kind::Flag => &[Op::IsTrue, Op::IsFalse],
    }
}

fn op_name(op: Op) -> &'static str {
    OPS.iter().find(|(_, o)| *o == op).map(|(n, _)| *n).unwrap_or("")
}

#[derive(Debug, Clone, PartialEq)]
enum Val {
    None,
    Text(String),
    Num(i64),
    Range(i64, i64),
    /// A point in time, both as the text `added` compares with and the milliseconds `lastPlayed` compares with.
    Date(String, i64),
    DateRange((String, i64), (String, i64)),
}

#[derive(Debug, Clone, PartialEq)]
struct Rule {
    field: Field,
    op: Op,
    value: Val,
}

impl Rule {
    /// True when a song without a `song_stats` row can never pass ("played more than 0 times", "played
    /// this month"). The rule then reads the bare column, where NULL fails it, and a playlist that
    /// requires such a rule is evaluated from the small stats table instead of from the whole index.
    fn needs_stats(&self) -> bool {
        self.field.stats_column().is_some()
            && match (self.op, &self.value) {
                (Op::WithinDays, _) => true,
                (Op::Is, Val::Num(v)) => *v > 0,
                (Op::Greater, Val::Num(v)) => *v >= 0,
                (Op::Between, Val::Range(low, _)) => *low > 0,
                (Op::Greater, Val::Date(_, ms)) => *ms >= 0,
                (Op::Between, Val::DateRange(from, _)) => from.1 > 0,
                _ => false,
            }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Node {
    Group { all: bool, rules: Vec<Node> },
    Rule(Rule),
}

impl Node {
    fn needs_stats(&self) -> bool {
        match self {
            Node::Rule(r) => r.needs_stats(),
            Node::Group { all: true, rules } => rules.iter().any(Node::needs_stats),
            Node::Group { all: false, rules } => !rules.is_empty() && rules.iter().all(Node::needs_stats),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Sort {
    Index,
    Random(u64),
    By(Field, bool),
}

#[derive(Debug, Clone, PartialEq)]
struct Def {
    root: Node,
    sort: Sort,
    limit: Option<u32>,
    limit_ms: Option<i64>,
}

// ---- parsing, with errors a person can act on -------------------------------

type Parsed<T> = std::result::Result<T, String>;

fn only_keys(o: &Map<String, Value>, allowed: &[&str], path: &str) -> Parsed<()> {
    match o.keys().find(|k| !allowed.contains(&k.as_str())) {
        Some(k) => Err(format!("{path}: unknown key \"{k}\" (expected {})", allowed.join(", "))),
        None => Ok(()),
    }
}

fn integer(v: &Value, path: &str) -> Parsed<i64> {
    v.as_i64()
        .or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64))
        .or_else(|| v.as_str().and_then(|s| s.trim().parse().ok()))
        .ok_or_else(|| format!("{path}: expected a whole number, found {v}"))
}

fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400);
    let doy = (153 * ((m + 9) % 12) + 2) / 5 + d - 1;
    era * 146_097 + yoe * 365 + yoe / 4 - yoe / 100 + doy - 719_468
}

/// "YYYY-MM-DDTHH:MM:SS", the prefix every ISO-8601 UTC timestamp shares, so it compares against them as text.
fn iso_from_ms(ms: i64) -> String {
    let (days, rest) = (ms.div_euclid(DAY_MS), ms.rem_euclid(DAY_MS) / 1000);
    let z = days + 719_468;
    let (era, doe) = (z.div_euclid(146_097), z.rem_euclid(146_097));
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let (d, m) = (doy - (153 * mp + 2) / 5 + 1, if mp < 10 { mp + 3 } else { mp - 9 });
    let y = yoe + era * 400 + (m <= 2) as i64;
    format!("{y:04}-{m:02}-{d:02}T{:02}:{:02}:{:02}", rest / 3600, rest / 60 % 60, rest % 60)
}

/// `end` makes a bare date mean the last instant of its day.
fn date(v: &Value, end: bool, path: &str) -> Parsed<(String, i64)> {
    let bad = || format!("{path}: expected a date like \"2024-05-31\" or \"2024-05-31T18:00:00\", found {v}");
    let s = v.as_str().map(str::trim).ok_or_else(bad)?;
    let num = |r: std::ops::Range<usize>| s.get(r).and_then(|p| p.parse::<i64>().ok());
    let b = s.as_bytes();
    let (Some(y), Some(m), Some(d)) = (num(0..4), num(5..7), num(8..10)) else { return Err(bad()) };
    if b.len() < 10 || b[4] != b'-' || b[7] != b'-' || !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return Err(bad());
    }
    let day = days_from_civil(y, m, d) * DAY_MS;
    if b.len() == 10 {
        // Second 60 sorts after every timestamp of the day whatever its precision ("...59Z", "...59.999Z").
        return Ok(if end { (format!("{s}T23:59:60"), day + DAY_MS - 1) } else { (s.to_string(), day) });
    }
    let (Some(h), Some(mi), Some(sec)) = (num(11..13), num(14..16), num(17..19)) else { return Err(bad()) };
    if b[10] != b'T' || h > 23 || mi > 59 || sec > 60 {
        return Err(bad());
    }
    Ok((s[..19].to_string(), day + ((h * 60 + mi) * 60 + sec) * 1000))
}

fn rule(o: &Map<String, Value>, path: &str) -> Parsed<Rule> {
    only_keys(o, &["field", "op", "value"], path)?;
    let name = o.get("field").and_then(Value::as_str).ok_or_else(|| format!("{path}.field: expected a field name"))?;
    let field = *FIELDS
        .iter()
        .find(|f| f.name() == name)
        .ok_or_else(|| format!("{path}.field: unknown field \"{name}\" (known: {})", FIELDS.iter().map(|f| f.name()).collect::<Vec<_>>().join(", ")))?;
    let op_text = o.get("op").and_then(Value::as_str).ok_or_else(|| format!("{path}.op: expected an operator"))?;
    let allowed = ops_of(field.kind());
    let op = OPS.iter().find(|(n, _)| *n == op_text).map(|(_, o)| *o).filter(|o| allowed.contains(o)).ok_or_else(|| {
        let known = if OPS.iter().any(|(n, _)| *n == op_text) { "does not apply to" } else { "is not an operator for" };
        format!("{path}.op: \"{op_text}\" {known} \"{name}\" (use one of: {})", allowed.iter().map(|o| op_name(*o)).collect::<Vec<_>>().join(", "))
    })?;

    let vpath = format!("{path}.value");
    let v = o.get("value").filter(|v| !v.is_null());
    let need = |what: &str| v.ok_or_else(|| format!("{vpath}: \"{op_text}\" needs {what}"));
    let pair = |what: &str| -> Parsed<(&Value, &Value)> {
        match need(what)?.as_array().map(Vec::as_slice) {
            Some([a, b]) => Ok((a, b)),
            _ => Err(format!("{vpath}: \"between\" needs {what}")),
        }
    };
    let is_date = matches!(field.kind(), Kind::DateMs | Kind::DateIso);
    let value = match op {
        Op::IsTrue | Op::IsFalse => {
            if v.is_some() {
                return Err(format!("{vpath}: \"{op_text}\" takes no value"));
            }
            Val::None
        }
        Op::WithinDays | Op::NotWithinDays => {
            let n = integer(need("a number of days")?, &vpath)?;
            if !(0..=100_000).contains(&n) {
                return Err(format!("{vpath}: days must be between 0 and 100000"));
            }
            Val::Num(n)
        }
        Op::Between if is_date => {
            let (a, b) = pair("two dates: [from, to]")?;
            let (a, b) = (date(a, false, &format!("{vpath}[0]"))?, date(b, true, &format!("{vpath}[1]"))?);
            if a.1 > b.1 {
                return Err(format!("{vpath}: the range is backwards"));
            }
            Val::DateRange(a, b)
        }
        Op::Between => {
            let (a, b) = pair("two numbers: [low, high]")?;
            let (a, b) = (integer(a, &format!("{vpath}[0]"))?, integer(b, &format!("{vpath}[1]"))?);
            if a > b {
                return Err(format!("{vpath}: the range is backwards"));
            }
            Val::Range(a, b)
        }
        // "after" a bare date means after that day is over
        Op::Greater | Op::Less if is_date => {
            let (text, ms) = date(need("a date")?, op == Op::Greater, &vpath)?;
            Val::Date(text, ms)
        }
        _ if field.kind() == Kind::Num => Val::Num(integer(need("a number")?, &vpath)?),
        _ => Val::Text(need("a string")?.as_str().ok_or_else(|| format!("{vpath}: expected a string for \"{name}\""))?.to_string()),
    };
    Ok(Rule { field, op, value })
}

fn node(v: &Value, path: &str, depth: usize) -> Parsed<Node> {
    let o = v.as_object().ok_or_else(|| format!("{path}: expected an object"))?;
    if o.contains_key("field") {
        return rule(o, path).map(Node::Rule);
    }
    if !o.contains_key("rules") {
        return Err(format!("{path}: expected a rule {{field, op, value}} or a group {{all, rules}}"));
    }
    if depth >= MAX_DEPTH {
        return Err(format!("{path}: groups are nested more than {MAX_DEPTH} deep"));
    }
    only_keys(o, &["all", "rules"], path)?;
    let all = match o.get("all") {
        None => true,
        Some(a) => a.as_bool().ok_or_else(|| format!("{path}.all: expected true (all rules) or false (any rule)"))?,
    };
    let list = o["rules"].as_array().ok_or_else(|| format!("{path}.rules: expected a list"))?;
    let rules = list.iter().enumerate().map(|(i, r)| node(r, &format!("{path}.rules[{i}]"), depth + 1)).collect::<Parsed<_>>()?;
    Ok(Node::Group { all, rules })
}

fn definition(text: &str) -> Parsed<Def> {
    let v: Value = serde_json::from_str(text).map_err(|e| format!("not JSON: {e}"))?;
    let o = v.as_object().ok_or("the definition must be a JSON object")?;
    only_keys(o, &["match", "sort", "limit", "limitMs"], "definition")?;
    let root = match o.get("match").filter(|m| !m.is_null()) {
        None => Node::Group { all: true, rules: vec![] },
        Some(m) if m.get("field").is_some() => return Err("match: expected a group {all, rules}, found a single rule".into()),
        Some(m) => node(m, "match", 1)?,
    };
    let sort = match o.get("sort").filter(|s| !s.is_null()) {
        None => Sort::Index,
        Some(s) => {
            let so = s.as_object().ok_or("sort: expected an object")?;
            only_keys(so, &["field", "descending", "seed"], "sort")?;
            let descending = match so.get("descending") {
                None => false,
                Some(d) => d.as_bool().ok_or("sort.descending: expected true or false")?,
            };
            match so.get("field").and_then(Value::as_str) {
                Some("random") => Sort::Random(match so.get("seed") {
                    None => 0,
                    Some(s) => s.as_u64().or_else(|| s.as_i64().map(|i| i as u64)).ok_or("sort.seed: expected a whole number")?,
                }),
                Some(name) => match FIELDS.iter().find(|f| f.name() == name && (f.kind() != Kind::Flag || **f == Starred)) {
                    Some(f) => Sort::By(*f, descending),
                    None => return Err(format!("sort.field: cannot sort by \"{name}\"")),
                },
                None => return Err("sort.field: expected a field name or \"random\"".into()),
            }
        }
    };
    let cap = |key: &str| -> Parsed<Option<i64>> {
        match o.get(key).filter(|l| !l.is_null()) {
            None => Ok(None),
            Some(l) => match integer(l, key)? {
                n if n < 0 => Err(format!("{key}: must not be negative")),
                0 => Ok(None),
                n => Ok(Some(n)),
            },
        }
    };
    Ok(Def { root, sort, limit: cap("limit")?.map(|n| n.min(u32::MAX as i64) as u32), limit_ms: cap("limitMs")? })
}

fn parse(text: &str) -> Result<Def> {
    definition(text).map_err(|reason| CoreError::Parse { reason: format!("smart playlist: {reason}") })
}

// ---- to SQL ----------------------------------------------------------------

struct Compiler<'a> {
    args: Vec<Sql>,
    downloaded: &'a [String],
    downloaded_arg: Option<usize>,
    now_ms: i64,
    /// A rule was relaxed to "true": SQL yields a superset (there is no NOT node, so relaxing a leaf can
    /// only add rows) and `matches` has the last word.
    needs_rust: bool,
}

impl Compiler<'_> {
    fn arg(&mut self, v: Sql) -> String {
        self.args.push(v);
        format!("?{}", self.args.len())
    }

    fn node(&mut self, n: &Node) -> String {
        match n {
            Node::Rule(r) => self.rule(r),
            Node::Group { all, rules } if rules.is_empty() => if *all { "1" } else { "0" }.to_string(),
            Node::Group { all, rules } => {
                let parts: Vec<String> = rules.iter().map(|r| self.node(r)).collect();
                format!("({})", parts.join(if *all { " AND " } else { " OR " }))
            }
        }
    }

    fn rule(&mut self, r: &Rule) -> String {
        let (_, kind, x) = r.field.def();
        let x = r.field.stats_column().filter(|_| r.needs_stats()).unwrap_or(x);
        match (&r.value, kind) {
            (Val::Text(v), _) => {
                if !v.is_ascii() {
                    self.needs_rust = true;
                    return "1".into();
                }
                let like = |v: &str| v.replace('\\', "\\\\").replace('%', "\\%").replace('_', "\\_");
                match r.op {
                    // The bare expression so that the genre index applies; NULL can only equal the empty string.
                    Op::Is if !v.is_empty() => format!("{x}={} COLLATE NOCASE", self.arg(Sql::Text(v.clone()))),
                    Op::Is => format!("coalesce({x},'')=''"),
                    Op::IsNot => format!("coalesce({x},'')<>{} COLLATE NOCASE", self.arg(Sql::Text(v.clone()))),
                    op => {
                        let pattern = match op {
                            Op::StartsWith => format!("{}%", like(v)),
                            Op::EndsWith => format!("%{}", like(v)),
                            _ => format!("%{}%", like(v)),
                        };
                        let not = if op == Op::NotContains { "NOT " } else { "" };
                        format!("coalesce({x},'') {not}LIKE {} ESCAPE '\\'", self.arg(Sql::Text(pattern)))
                    }
                }
            }
            (Val::Num(days), Kind::DateMs | Kind::DateIso) => {
                let cutoff = self.now_ms - days * DAY_MS;
                let within = r.op == Op::WithinDays;
                // Never played is 0 and must stay "not within" however far back the cutoff goes.
                let p = if kind == Kind::DateMs { self.arg(Sql::Integer(cutoff.max(1))) } else { self.arg(Sql::Text(iso_from_ms(cutoff))) };
                format!("{x}{}{p}", if within { ">=" } else { "<" })
            }
            (Val::Num(v), _) => {
                let op = match r.op {
                    Op::Is => "=",
                    Op::IsNot => "<>",
                    Op::Greater => ">",
                    _ => "<",
                };
                format!("{x}{op}{}", self.arg(Sql::Integer(*v)))
            }
            (Val::Range(a, b), _) => {
                format!("{x} BETWEEN {} AND {}", self.arg(Sql::Integer(*a)), self.arg(Sql::Integer(*b)))
            }
            (Val::Date(text, ms), _) => {
                let p = if kind == Kind::DateMs { self.arg(Sql::Integer(*ms)) } else { self.arg(Sql::Text(text.clone())) };
                format!("{x}{}{p}", if r.op == Op::Greater { ">" } else { "<" })
            }
            (Val::DateRange(a, b), _) => {
                let (a, b) = if kind == Kind::DateMs { (Sql::Integer(a.1), Sql::Integer(b.1)) } else { (Sql::Text(a.0.clone()), Sql::Text(b.0.clone())) };
                format!("{x} BETWEEN {} AND {}", self.arg(a), self.arg(b))
            }
            (Val::None, _) => {
                let yes = r.op == Op::IsTrue;
                match r.field {
                    IsDownloaded => {
                        let n = match self.downloaded_arg {
                            Some(n) => n,
                            None => {
                                self.args.push(Sql::Text(serde_json::to_string(self.downloaded).unwrap_or_else(|_| "[]".into())));
                                *self.downloaded_arg.insert(self.args.len())
                            }
                        };
                        format!("i.id {}IN (SELECT value FROM json_each(?{n}))", if yes { "" } else { "NOT " })
                    }
                    ExcludedFromMixes => format!("i.id {}IN (SELECT song_id FROM mix_excluded)", if yes { "" } else { "NOT " }),
                    // Written exactly like the partial index on starred songs.
                    _ if yes => format!("{x}=1"),
                    _ => format!("coalesce({x},0)<>1"),
                }
            }
        }
    }
}

// ---- the same rules in Rust, for what SQL was told to let through -----------

#[derive(Deserialize, Default)]
#[serde(default, rename_all = "camelCase")]
struct Extra {
    created: Option<String>,
    play_count: Option<i64>,
}

struct Row {
    song: Song,
    extra: Extra,
    plays: i64,
    skips: i64,
    last_played_ms: i64,
    excluded: bool,
}

struct Env<'a> {
    downloaded: HashSet<&'a str>,
    now_ms: i64,
}

fn number(f: Field, r: &Row) -> i64 {
    let s = &r.song;
    match f {
        Year => s.year as i64,
        Duration => s.duration as i64,
        Track => s.track as i64,
        DiscNumber => s.disc_number as i64,
        BitRate => s.bit_rate as i64,
        SampleRate => s.sampling_rate as i64,
        BitDepth => s.bit_depth as i64,
        Size => s.size as i64,
        UserRating => s.user_rating as i64,
        PlayCount => r.plays,
        SkipCount => r.skips,
        ServerPlayCount => r.extra.play_count.unwrap_or(0),
        LastPlayed => r.last_played_ms,
        _ => 0,
    }
}

fn string(f: Field, r: &Row) -> &str {
    let s = &r.song;
    match f {
        Title => &s.title,
        Album => &s.album,
        Artist => &s.artist,
        Genre => s.genre.as_deref().unwrap_or(""),
        Suffix => &s.suffix,
        Added => r.extra.created.as_deref().unwrap_or(""),
        _ => "",
    }
}

fn matches(n: &Node, row: &Row, env: &Env) -> bool {
    let r = match n {
        Node::Group { all: true, rules } => return rules.iter().all(|n| matches(n, row, env)),
        Node::Group { all: false, rules } => return rules.iter().any(|n| matches(n, row, env)),
        Node::Rule(r) => r,
    };
    let iso = r.field.kind() == Kind::DateIso;
    match &r.value {
        Val::Text(v) => {
            let (have, want) = (string(r.field, row).to_lowercase(), v.to_lowercase());
            match r.op {
                Op::Is => have == want,
                Op::IsNot => have != want,
                Op::Contains => have.contains(&want),
                Op::NotContains => !have.contains(&want),
                Op::StartsWith => have.starts_with(&want),
                _ => have.ends_with(&want),
            }
        }
        Val::Num(days) if matches!(r.op, Op::WithinDays | Op::NotWithinDays) => {
            let cutoff = env.now_ms - days * DAY_MS;
            let within = if iso { string(r.field, row) >= iso_from_ms(cutoff).as_str() } else { number(r.field, row) >= cutoff.max(1) };
            within == (r.op == Op::WithinDays)
        }
        Val::Num(v) => {
            let have = number(r.field, row);
            match r.op {
                Op::Is => have == *v,
                Op::IsNot => have != *v,
                Op::Greater => have > *v,
                _ => have < *v,
            }
        }
        Val::Range(a, b) => (*a..=*b).contains(&number(r.field, row)),
        Val::Date(text, ms) => {
            let ord = if iso { string(r.field, row).cmp(text.as_str()) } else { number(r.field, row).cmp(ms) };
            ord == if r.op == Op::Greater { std::cmp::Ordering::Greater } else { std::cmp::Ordering::Less }
        }
        Val::DateRange(a, b) => {
            if iso {
                let have = string(r.field, row);
                have >= a.0.as_str() && have <= b.0.as_str()
            } else {
                (a.1..=b.1).contains(&number(r.field, row))
            }
        }
        Val::None => {
            let have = match r.field {
                IsDownloaded => env.downloaded.contains(row.song.id.as_str()),
                ExcludedFromMixes => row.excluded,
                _ => row.song.starred,
            };
            have == (r.op == Op::IsTrue)
        }
    }
}

// ---- evaluation --------------------------------------------------------------

fn load(c: &Connection, rowid: i64) -> rusqlite::Result<Option<Row>> {
    let mut st = c.prepare_cached(
        "SELECT i.json, coalesce(s.plays,0), coalesce(s.skips,0), coalesce(s.last_played_ms,0), EXISTS(SELECT 1 FROM mix_excluded e WHERE e.song_id=i.id)
         FROM items i LEFT JOIN song_stats s ON s.song_id=i.id WHERE i.rowid=?1",
    )?;
    let row = st.query_row([rowid], |r| Ok((r.get::<_, String>(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?))).optional()?;
    Ok(row.and_then(|(json, plays, skips, last_played_ms, excluded)| {
        Some(Row { song: serde_json::from_str(&json).ok()?, extra: serde_json::from_str(&json).unwrap_or_default(), plays, skips, last_played_ms, excluded })
    }))
}

/// CROSS JOIN pins the join order: without ANALYZE data the planner would still start from the index.
fn tables(root: &Node) -> &'static str {
    if root.needs_stats() {
        "FROM song_stats s CROSS JOIN items i ON i.kind=2 AND i.id=s.song_id"
    } else {
        "FROM items i LEFT JOIN song_stats s ON s.song_id=i.id"
    }
}

/// The songs of the playlist at positions `offset..offset+limit`, and how many positions were walked.
/// With `count_all` the walk does not stop at the end of the page, so the second number is the playlist's length.
fn run(c: &Connection, def: &Def, downloaded: &[String], offset: usize, limit: usize, count_all: bool, now_ms: i64) -> rusqlite::Result<(Vec<Song>, usize)> {
    let mut cp = Compiler { args: Vec::new(), downloaded, downloaded_arg: None, now_ms, needs_rust: false };
    let cond = cp.node(&def.root);
    let from = format!("{} WHERE {} AND {cond}", tables(&def.root), mixes::SONGS);
    let cap = def.limit.map(|l| l as usize).unwrap_or(usize::MAX);
    let sql_is_exact = !cp.needs_rust && def.limit_ms.is_none();

    if sql_is_exact && count_all {
        let n: i64 = c.prepare_cached(&format!("SELECT count(*) {from}"))?.query_row(rusqlite::params_from_iter(cp.args), |r| r.get(0))?;
        return Ok((Vec::new(), (n as usize).min(cap)));
    }
    let order = match def.sort {
        Sort::Index => "i.rowid".to_string(),
        Sort::Random(seed) => {
            cp.args.extend(mixes::shuffled_params(seed));
            format!("{}, i.rowid", mixes::shuffled_order("i.rowid", cp.args.len() - 1, cp.args.len()))
        }
        Sort::By(f, descending) => {
            let collate = if f.kind() == Kind::Text { " COLLATE NOCASE" } else { "" };
            format!("{}{collate}{}, i.rowid", f.def().2, if descending { " DESC" } else { "" })
        }
    };
    // A LIMIT lets SQLite keep a top-N heap instead of sorting every match.
    let window = if sql_is_exact {
        let take = limit.min(cap.saturating_sub(offset));
        cp.args.push(Sql::Integer(take as i64));
        cp.args.push(Sql::Integer(offset as i64));
        format!(" LIMIT ?{} OFFSET ?{}", cp.args.len() - 1, cp.args.len())
    } else if !cp.needs_rust && cap != usize::MAX {
        cp.args.push(Sql::Integer(cap as i64));
        format!(" LIMIT ?{}", cp.args.len())
    } else {
        String::new()
    };
    let duration = if def.limit_ms.is_some() { "json_extract(i.json,'$.duration')" } else { "0" };
    let mut st = c.prepare_cached(&format!("SELECT i.rowid, {duration} {from} ORDER BY {order}{window}"))?;
    let mut rows = st.query(rusqlite::params_from_iter(cp.args))?;

    let env = Env { downloaded: if cp.needs_rust { downloaded.iter().map(String::as_str).collect() } else { HashSet::new() }, now_ms };
    let budget = def.limit_ms.unwrap_or(i64::MAX);
    let (mut page, mut position, mut total_ms) = (Vec::new(), if sql_is_exact { offset } else { 0 }, 0i64);
    while let Some(r) = rows.next()? {
        if position >= cap || (!count_all && page.len() >= limit) {
            break;
        }
        let (rowid, duration_s): (i64, Option<i64>) = (r.get(0)?, r.get(1)?);
        let mut row = None;
        if cp.needs_rust {
            match load(c, rowid)? {
                Some(l) if matches(&def.root, &l, &env) => row = Some(l),
                _ => continue,
            }
        }
        total_ms = total_ms.saturating_add(duration_s.unwrap_or(0) * 1000);
        if total_ms > budget {
            break;
        }
        if position >= offset && page.len() < limit {
            let row = match row {
                Some(r) => Some(r),
                None => load(c, rowid)?,
            };
            page.extend(row.map(|r| r.song));
        }
        position += 1;
    }
    Ok((page, position))
}

fn default_definitions() -> Vec<SmartPlaylist> {
    let one = |field: &str, op: &str, value: Value| json!({ "all": true, "rules": [{ "field": field, "op": op, "value": value }] });
    let sorted = |field: &str, descending: bool| json!({ "field": field, "descending": descending });
    let defs = [
        ("default-most-played", "Most played", json!({ "match": one("playCount", "greater", json!(0)), "sort": sorted("playCount", true), "limit": 100 })),
        ("default-recently-played", "Recently played", json!({ "match": one("lastPlayed", "withinDays", json!(30)), "sort": sorted("lastPlayed", true), "limit": 100 })),
        ("default-recently-added", "Recently added", json!({ "match": one("added", "withinDays", json!(90)), "sort": sorted("added", true), "limit": 200 })),
        ("default-never-played", "Never played", json!({ "match": one("playCount", "is", json!(0)), "sort": { "field": "random", "seed": 0 }, "limit": 100 })),
        ("default-top-rated", "Top rated", json!({ "match": one("userRating", "greater", json!(3)), "sort": sorted("userRating", true), "limit": 200 })),
        (
            "default-forgotten-favourites",
            "Forgotten favourites",
            json!({ "match": { "all": true, "rules": [{ "field": "starred", "op": "isTrue" }, { "field": "lastPlayed", "op": "notWithinDays", "value": 180 }] },
                    "sort": sorted("lastPlayed", false), "limit": 100 }),
        ),
        ("default-long-tracks", "Long tracks", json!({ "match": one("duration", "greater", json!(600)), "sort": sorted("duration", true), "limit": 100 })),
    ];
    defs.into_iter().map(|(id, name, json)| SmartPlaylist { id: id.into(), name: name.into(), json: json.to_string() }).collect()
}

/// Checks a definition without running it; the error says where and what (`match.rules[1].op: ...`).
#[uniffi::export]
pub fn smart_validate(json: String) -> Result<()> {
    parse(&json).map(|_| ())
}

/// Definitions the UI can offer as a starting point. They are not stored: save one with `smart_save` to keep
/// or edit it. "Never played" is a random draw; change `sort.seed` to draw again.
#[uniffi::export]
pub fn smart_defaults() -> Vec<SmartPlaylist> {
    default_definitions()
}

#[uniffi::export]
impl Core {
    /// Newest first.
    pub fn smart_list(&self) -> Result<Vec<SmartPlaylist>> {
        let c = self.db.lock();
        let mut st = c.prepare_cached("SELECT id, name, json FROM smart_playlists ORDER BY updated_ms DESC, id")?;
        let rows = st.query_map([], |r| Ok(SmartPlaylist { id: r.get(0)?, name: r.get(1)?, json: r.get(2)? }))?;
        Ok(rows.collect::<rusqlite::Result<_>>()?)
    }

    /// An empty `id` creates the playlist. Returns the id. A definition that does not validate is not stored.
    pub fn smart_save(&self, id: String, name: String, json: String) -> Result<String> {
        parse(&json)?;
        let c = self.db.lock();
        let now = db::now_ms();
        let id = if id.is_empty() {
            let stored: i64 = c.query_row("SELECT count(*) FROM smart_playlists", [], |r| r.get(0))?;
            format!("sp-{:x}", mixes::Rng::new(now as u64 ^ (stored as u64) << 48).next())
        } else {
            id
        };
        c.execute("INSERT OR REPLACE INTO smart_playlists(id, name, json, updated_ms) VALUES(?1, ?2, ?3, ?4)", params![id, name, json, now])?;
        Ok(id)
    }

    pub fn smart_delete(&self, id: String) -> Result<()> {
        self.db.lock().execute("DELETE FROM smart_playlists WHERE id=?1", [id])?;
        Ok(())
    }

    /// One page of the playlist. `downloaded_ids` is only read when a rule uses `isDownloaded`; pass an
    /// empty list otherwise. `offset` and `limit` page inside the playlist's own `limit` / `limitMs`.
    pub fn smart_evaluate(&self, json: String, downloaded_ids: Vec<String>, offset: u32, limit: u32) -> Result<Vec<Song>> {
        let def = parse(&json)?;
        Ok(run(&self.db.lock(), &def, &downloaded_ids, offset as usize, limit as usize, false, db::now_ms())?.0)
    }

    /// How many songs the playlist has, its own caps applied.
    pub fn smart_count(&self, json: String, downloaded_ids: Vec<String>) -> Result<u32> {
        let def = parse(&json)?;
        Ok(run(&self.db.lock(), &def, &downloaded_ids, 0, 0, true, db::now_ms())?.1 as u32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::tests::{listen, skip, song, DAY, NOW};

    fn ids(l: &[Song]) -> Vec<&str> {
        l.iter().map(|s| s.id.as_str()).collect()
    }

    fn error(json: &str) -> String {
        match smart_validate(json.into()) {
            Err(CoreError::Parse { reason }) => reason,
            other => panic!("expected a parse error, got {other:?}"),
        }
    }

    /// Both engines at once: SQL alone, and SQL relaxed to "everything" with Rust deciding. They must agree.
    fn eval(core: &Core, json: &str, downloaded: &[&str]) -> Vec<String> {
        let def = parse(json).unwrap();
        let downloaded: Vec<String> = downloaded.iter().map(|s| s.to_string()).collect();
        let c = core.db.lock();
        let (page, n) = run(&c, &def, &downloaded, 0, 10_000, false, NOW).unwrap();
        assert_eq!(n, page.len());
        assert_eq!(run(&c, &def, &downloaded, 0, 0, true, NOW).unwrap().1, n, "count agrees with evaluate");

        let env = Env { downloaded: downloaded.iter().map(String::as_str).collect(), now_ms: NOW };
        let mut by_rust: Vec<String> = Vec::new();
        let mut st = c.prepare("SELECT rowid FROM items WHERE kind=2").unwrap();
        for rowid in st.query_map([], |r| r.get::<_, i64>(0)).unwrap() {
            let row = load(&c, rowid.unwrap()).unwrap().unwrap();
            if matches(&def.root, &row, &env) {
                by_rust.push(row.song.id);
            }
        }
        if def.limit.is_none() && def.limit_ms.is_none() {
            let mut by_sql: Vec<String> = page.iter().map(|s| s.id.clone()).collect();
            by_sql.sort();
            by_rust.sort();
            assert_eq!(by_sql, by_rust, "SQL and Rust disagree on {json}");
        }
        page.into_iter().map(|s| s.id).collect()
    }

    fn library() -> std::sync::Arc<Core> {
        let core = Core::new(String::new()).unwrap();
        let mut songs = vec![
            song("dogs", "Dogs", "Pink Floyd", "Animals", "Progressive Rock", 1977),
            song("pigs", "Pigs (Three Different Ones)", "Pink Floyd", "Animals", "Progressive Rock", 1977),
            song("joga", "Jóga", "Björk", "Homogenic", "Electronic", 1997),
            song("bach", "Hunter", "BJÖRK", "Homogenic", "Electronic", 1997),
            song("so", "So What", "Miles Davis", "Kind of Blue", "Jazz", 1959),
            song("pct", "100% Pure_Love", "Crystal Waters", "Storyteller", "House", 1994),
            Song { id: "bare".into(), title: "Untitled".into(), ..Default::default() },
        ];
        songs[0].duration = 1024;
        songs[0].starred = true;
        songs[1].user_rating = 5;
        songs[2].bit_rate = 320;
        songs[2].suffix = "mp3".into();
        songs[4].user_rating = 4;
        songs[4].starred = true;
        songs[4].sampling_rate = 96_000;
        songs[4].bit_depth = 24;
        songs[4].size = 123_456_789;
        db::index(&mut core.db.lock(), &[], &[], &songs).unwrap();
        // what the server will send once Song carries these keys
        let c = core.db.lock();
        c.execute("UPDATE items SET json=json_set(json,'$.created','2026-08-20T10:00:00.000Z','$.playCount',40) WHERE id='dogs'", []).unwrap();
        c.execute("UPDATE items SET json=json_set(json,'$.created','2024-02-29T23:59:59Z','$.playCount',2) WHERE id='so'", []).unwrap();
        drop(c);
        for d in [1, 2, 3] {
            listen(&core, &songs[0], NOW - d * DAY);
        }
        listen(&core, &songs[4], NOW - 200 * DAY);
        skip(&core, &songs[5], NOW - DAY);
        skip(&core, &songs[5], NOW - 2 * DAY);
        core.mix_excluded_set("pigs".into(), true).unwrap();
        core
    }

    fn one(field: &str, op: &str, value: Value) -> String {
        json!({ "match": { "rules": [{ "field": field, "op": op, "value": value }] } }).to_string()
    }

    #[test]
    fn text_operators() {
        let core = library();
        assert_eq!(eval(&core, &one("artist", "is", json!("pink floyd")), &[]), ["dogs", "pigs"]);
        assert_eq!(eval(&core, &one("artist", "isNot", json!("Pink Floyd")), &[]).len(), 5);
        assert_eq!(eval(&core, &one("genre", "contains", json!("rock")), &[]), ["dogs", "pigs"]);
        assert_eq!(eval(&core, &one("genre", "notContains", json!("rock")), &[]), ["joga", "bach", "so", "pct", "bare"], "no genre is not rock");
        assert_eq!(eval(&core, &one("title", "startsWith", json!("PIGS")), &[]), ["pigs"]);
        assert_eq!(eval(&core, &one("title", "endsWith", json!("ones)")), &[]), ["pigs"]);
        assert_eq!(eval(&core, &one("genre", "is", json!("")), &[]), ["bare"]);
        assert_eq!(eval(&core, &one("genre", "isNot", json!("")), &[]).len(), 6);
        assert_eq!(eval(&core, &one("suffix", "is", json!("MP3")), &[]), ["joga"]);
        // LIKE wildcards in the value are literal
        assert_eq!(eval(&core, &one("title", "contains", json!("100%")), &[]), ["pct"]);
        assert_eq!(eval(&core, &one("title", "contains", json!("e_l")), &[]), ["pct"]);
        assert_eq!(eval(&core, &one("title", "contains", json!("%")), &[]), ["pct"]);
        assert!(eval(&core, &one("title", "contains", json!("'; DROP TABLE items;--")), &[]).is_empty());
    }

    #[test]
    fn unicode_values_fall_back_to_rust_and_fold_case() {
        let core = library();
        assert_eq!(eval(&core, &one("artist", "is", json!("björk")), &[]), ["joga", "bach"]);
        assert_eq!(eval(&core, &one("title", "contains", json!("Ó")), &[]), ["joga"]);
        // mixed with SQL rules, paged, counted
        let def = json!({ "match": { "all": false, "rules": [
            { "field": "artist", "op": "is", "value": "BJÖRK" }, { "field": "year", "op": "less", "value": 1960 } ] },
            "sort": { "field": "title", "descending": true } })
        .to_string();
        assert_eq!(eval(&core, &def, &[]), ["bare", "so", "joga", "bach"], "no year is year 0");
        assert_eq!(ids(&core.smart_evaluate(def.clone(), vec![], 2, 1).unwrap()), ["joga"]);
        assert_eq!(core.smart_count(def, vec![]).unwrap(), 4);
    }

    #[test]
    fn number_operators() {
        let core = library();
        assert_eq!(eval(&core, &one("year", "is", json!(1977)), &[]), ["dogs", "pigs"]);
        assert_eq!(eval(&core, &one("year", "isNot", json!(1977)), &[]).len(), 5);
        assert_eq!(eval(&core, &one("year", "between", json!([1959, 1977])), &[]), ["dogs", "pigs", "so"]);
        assert_eq!(eval(&core, &one("year", "greater", json!("1994")), &[]), ["joga", "bach"], "numeric strings are fine");
        assert_eq!(eval(&core, &one("duration", "greater", json!(600)), &[]), ["dogs"]);
        assert_eq!(eval(&core, &one("bitRate", "is", json!(320)), &[]), ["joga"]);
        assert_eq!(eval(&core, &one("sampleRate", "greater", json!(48_000)), &[]), ["so"]);
        assert_eq!(eval(&core, &one("bitDepth", "is", json!(24)), &[]), ["so"]);
        assert_eq!(eval(&core, &one("size", "greater", json!(100_000_000)), &[]), ["so"]);
        assert_eq!(eval(&core, &one("userRating", "greater", json!(3)), &[]), ["pigs", "so"]);
        assert_eq!(eval(&core, &one("track", "is", json!(0)), &[]).len(), 7);
        assert_eq!(eval(&core, &one("discNumber", "less", json!(1)), &[]).len(), 7);
    }

    #[test]
    fn statistics_fields() {
        let core = library();
        assert_eq!(eval(&core, &one("playCount", "greater", json!(0)), &[]), ["dogs", "so"]);
        assert_eq!(eval(&core, &one("playCount", "is", json!(3)), &[]), ["dogs"]);
        assert_eq!(eval(&core, &one("playCount", "is", json!(0)), &[]), ["pigs", "joga", "bach", "pct", "bare"]);
        assert_eq!(eval(&core, &one("playCount", "between", json!([1, 2])), &[]), ["so"]);
        assert_eq!(eval(&core, &one("playCount", "between", json!([0, 1])), &[]).len(), 6);
        assert_eq!(eval(&core, &one("playCount", "less", json!(1)), &[]).len(), 5);
        assert_eq!(eval(&core, &one("skipCount", "greater", json!(1)), &[]), ["pct"]);
        assert_eq!(eval(&core, &one("serverPlayCount", "greater", json!(1)), &[]), ["dogs", "so"]);
        assert_eq!(eval(&core, &one("serverPlayCount", "is", json!(0)), &[]).len(), 5, "rows from before the key existed");
        assert_eq!(eval(&core, &one("lastPlayed", "withinDays", json!(7)), &[]), ["dogs"]);
        assert_eq!(eval(&core, &one("lastPlayed", "withinDays", json!(365)), &[]), ["dogs", "so"]);
        assert_eq!(eval(&core, &one("lastPlayed", "notWithinDays", json!(7)), &[]).len(), 6, "never played is not within");
        assert_eq!(eval(&core, &one("lastPlayed", "greater", json!("2026-08-01")), &[]), ["dogs"]);
        assert_eq!(eval(&core, &one("lastPlayed", "less", json!("2026-08-01")), &[]).len(), 6);
        assert_eq!(eval(&core, &one("lastPlayed", "between", json!(["2026-01-01", "2026-03-01"])), &[]), ["so"]);
    }

    #[test]
    fn added_reads_the_server_date() {
        let core = library();
        assert_eq!(eval(&core, &one("added", "withinDays", json!(30)), &[]), ["dogs"]);
        assert_eq!(eval(&core, &one("added", "notWithinDays", json!(30)), &[]).len(), 6, "no date is not within");
        assert_eq!(eval(&core, &one("added", "greater", json!("2024-02-29")), &[]), ["dogs"], "after a day means after it ended");
        assert_eq!(eval(&core, &one("added", "greater", json!("2024-02-29T23:00:00")), &[]), ["dogs", "so"]);
        assert_eq!(eval(&core, &one("added", "between", json!(["2024-02-01", "2024-02-29"])), &[]), ["so"], "the end day is included");
        assert_eq!(eval(&core, &one("added", "less", json!("2025-01-01")), &[]).len(), 6);
        let newest = json!({ "sort": { "field": "added", "descending": true }, "limit": 2 }).to_string();
        assert_eq!(eval(&core, &newest, &[]), ["dogs", "so"]);
    }

    #[test]
    fn flags() {
        let core = library();
        let flag = |field: &str, op: &str| json!({ "match": { "rules": [{ "field": field, "op": op }] } }).to_string();
        assert_eq!(eval(&core, &flag("starred", "isTrue"), &[]), ["dogs", "so"]);
        assert_eq!(eval(&core, &flag("starred", "isFalse"), &[]).len(), 5);
        assert_eq!(eval(&core, &flag("excludedFromMixes", "isTrue"), &[]), ["pigs"]);
        assert_eq!(eval(&core, &flag("excludedFromMixes", "isFalse"), &[]).len(), 6);
        assert_eq!(eval(&core, &flag("isDownloaded", "isTrue"), &["joga", "so", "gone"]), ["joga", "so"]);
        assert_eq!(eval(&core, &flag("isDownloaded", "isFalse"), &["joga", "so"]).len(), 5);
        assert!(eval(&core, &flag("isDownloaded", "isTrue"), &[]).is_empty());
        // used twice, bound once
        let both = json!({ "match": { "all": false, "rules": [{ "field": "isDownloaded", "op": "isTrue" }, { "all": true, "rules": [
            { "field": "isDownloaded", "op": "isFalse" }, { "field": "year", "op": "is", "value": 1959 }] }] } })
        .to_string();
        assert_eq!(eval(&core, &both, &["joga", "it's"]), ["joga", "so"]);
    }

    #[test]
    fn nested_groups() {
        let core = library();
        // (rock AND (starred OR rated 5)) OR (jazz AND NOT recently played)
        let def = json!({ "match": { "all": false, "rules": [
            { "all": true, "rules": [
                { "field": "genre", "op": "contains", "value": "rock" },
                { "all": false, "rules": [{ "field": "starred", "op": "isTrue" }, { "field": "userRating", "op": "is", "value": 5 }] } ] },
            { "all": true, "rules": [
                { "field": "genre", "op": "is", "value": "jazz" },
                { "field": "lastPlayed", "op": "notWithinDays", "value": 30 } ] } ] } })
        .to_string();
        assert_eq!(eval(&core, &def, &[]), ["dogs", "pigs", "so"]);
        assert_eq!(eval(&core, r#"{"match":{"all":true,"rules":[]}}"#, &[]).len(), 7);
        assert!(eval(&core, r#"{"match":{"all":false,"rules":[]}}"#, &[]).is_empty());
        assert_eq!(eval(&core, "{}", &[]).len(), 7);
        assert_eq!(eval(&core, r#"{"match":{"rules":[{"all":false,"rules":[]},{"field":"year","op":"is","value":1977}]}}"#, &[]).len(), 0);
    }

    #[test]
    fn sort_limit_and_paging() {
        let core = library();
        let by = |field: &str, descending: bool| json!({ "sort": { "field": field, "descending": descending } }).to_string();
        assert_eq!(eval(&core, &by("title", false), &[])[..3], ["pct", "dogs", "bach"]);
        assert_eq!(eval(&core, &by("year", true), &[])[..2], ["joga", "bach"], "ties keep index order");
        assert_eq!(eval(&core, &by("playCount", true), &[])[..2], ["dogs", "so"]);
        assert_eq!(eval(&core, &by("lastPlayed", true), &[])[..2], ["dogs", "so"]);
        assert_eq!(eval(&core, &by("starred", true), &[])[..2], ["dogs", "so"]);

        let capped = json!({ "sort": { "field": "year" }, "limit": 3 }).to_string();
        assert_eq!(eval(&core, &capped, &[]), ["bare", "so", "dogs"]);
        assert_eq!(core.smart_count(capped.clone(), vec![]).unwrap(), 3);
        assert_eq!(ids(&core.smart_evaluate(capped.clone(), vec![], 2, 50).unwrap()), ["dogs"], "paging stops at the playlist's own limit");
        assert!(core.smart_evaluate(capped.clone(), vec![], 3, 50).unwrap().is_empty());
        assert!(core.smart_evaluate(capped, vec![], 0, 0).unwrap().is_empty());

        let pages: Vec<String> = (0..4).flat_map(|p| core.smart_evaluate(by("title", false), vec![], p * 2, 2).unwrap()).map(|s| s.id).collect();
        assert_eq!(pages, eval(&core, &by("title", false), &[]));
    }

    #[test]
    fn random_sort_is_stable_per_seed() {
        let core = library();
        let shuffled = |seed: u64| eval(&core, &json!({ "sort": { "field": "random", "seed": seed } }).to_string(), &[]);
        assert_eq!(shuffled(1), shuffled(1));
        assert_eq!(shuffled(1).len(), 7);
        assert!((2..12).any(|s| shuffled(s) != shuffled(1)));
        let paged: Vec<String> = (0..7).flat_map(|p| core.smart_evaluate(json!({ "sort": { "field": "random", "seed": 1 } }).to_string(), vec![], p, 1).unwrap()).map(|s| s.id).collect();
        assert_eq!(paged, shuffled(1), "pages of a random order still tile");
    }

    #[test]
    fn duration_budget() {
        let core = library();
        // durations: dogs 1024 s, five of 200 s, bare 0 s
        let def = |ms: i64| json!({ "sort": { "field": "duration", "descending": true }, "limitMs": ms }).to_string();
        assert_eq!(eval(&core, &def(1_500_000), &[]), ["dogs", "pigs", "joga"]);
        assert_eq!(core.smart_count(def(1_500_000), vec![]).unwrap(), 3);
        assert_eq!(ids(&core.smart_evaluate(def(1_500_000), vec![], 1, 5).unwrap()), ["pigs", "joga"]);
        assert!(eval(&core, &def(1000), &[]).is_empty(), "the first song is already over");
        assert_eq!(eval(&core, &def(100_000_000), &[]).len(), 7);
        let both = json!({ "sort": { "field": "duration", "descending": true }, "limitMs": 1_500_000, "limit": 2 }).to_string();
        assert_eq!(eval(&core, &both, &[]), ["dogs", "pigs"]);
        // with the Rust fallback in play as well
        let uni = json!({ "match": { "rules": [{ "field": "artist", "op": "is", "value": "björk" }] }, "limitMs": 250_000 }).to_string();
        assert_eq!(eval(&core, &uni, &[]), ["joga"]);
    }

    #[test]
    fn empty_index() {
        let core = Core::new(String::new()).unwrap();
        for d in smart_defaults() {
            assert!(core.smart_evaluate(d.json.clone(), vec![], 0, 50).unwrap().is_empty());
            assert_eq!(core.smart_count(d.json, vec![]).unwrap(), 0);
        }
        assert!(core.smart_list().unwrap().is_empty());
    }

    #[test]
    fn defaults_validate_and_do_what_they_say() {
        let core = library();
        let defaults = smart_defaults();
        assert_eq!(defaults.len(), 7);
        let run = |id: &str| {
            let d = defaults.iter().find(|d| d.id == id).unwrap();
            smart_validate(d.json.clone()).unwrap();
            eval(&core, &d.json, &[])
        };
        assert_eq!(run("default-most-played"), ["dogs", "so"]);
        assert_eq!(run("default-recently-played"), ["dogs"]);
        assert_eq!(run("default-recently-added"), ["dogs"]);
        assert_eq!(run("default-never-played").len(), 5);
        assert_eq!(run("default-top-rated"), ["pigs", "so"]);
        assert_eq!(run("default-forgotten-favourites"), ["so"]);
        assert_eq!(run("default-long-tracks"), ["dogs"]);
    }

    #[test]
    fn validation_says_where_and_what() {
        smart_validate(r#"{"match":null,"sort":null,"limit":null,"limitMs":0}"#.into()).unwrap();
        assert!(error("nope").contains("not JSON"));
        assert!(error("[]").contains("must be a JSON object"));
        assert!(error(r#"{"macth":{}}"#).contains("unknown key \"macth\""));
        assert!(error(r#"{"match":{"field":"year","op":"is","value":1}}"#).contains("expected a group"));
        assert!(error(r#"{"match":{"rules":[{"op":"is"}]}}"#).starts_with("smart playlist: match.rules[0]: expected a rule"));
        let e = error(r#"{"match":{"rules":[{"field":"year","op":"is","value":1},{"all":false,"rules":[{"field":"yeer","op":"is","value":1}]}]}}"#);
        assert!(e.contains("match.rules[1].rules[0].field: unknown field \"yeer\"") && e.contains("sampleRate"), "{e}");
        let e = error(r#"{"match":{"rules":[{"field":"year","op":"contains","value":"19"}]}}"#);
        assert!(e.contains("\"contains\" does not apply to \"year\"") && e.contains("between"), "{e}");
        assert!(error(r#"{"match":{"rules":[{"field":"year","op":"roughly","value":1}]}}"#).contains("is not an operator"));
        assert!(error(r#"{"match":{"rules":[{"field":"year","op":"is","value":"soon"}]}}"#).contains("match.rules[0].value: expected a whole number"));
        assert!(error(r#"{"match":{"rules":[{"field":"year","op":"is"}]}}"#).contains("needs a number"));
        assert!(error(r#"{"match":{"rules":[{"field":"year","op":"between","value":[1]}]}}"#).contains("two numbers"));
        assert!(error(r#"{"match":{"rules":[{"field":"year","op":"between","value":[2000,1990]}]}}"#).contains("backwards"));
        assert!(error(r#"{"match":{"rules":[{"field":"title","op":"is","value":5}]}}"#).contains("expected a string"));
        assert!(error(r#"{"match":{"rules":[{"field":"starred","op":"isTrue","value":true}]}}"#).contains("takes no value"));
        assert!(error(r#"{"match":{"rules":[{"field":"starred","op":"is","value":true}]}}"#).contains("isTrue, isFalse"));
        assert!(error(r#"{"match":{"rules":[{"field":"added","op":"greater","value":"last week"}]}}"#).contains("expected a date"));
        assert!(error(r#"{"match":{"rules":[{"field":"added","op":"greater","value":"2024-13-01"}]}}"#).contains("expected a date"));
        assert!(error(r#"{"match":{"rules":[{"field":"lastPlayed","op":"withinDays","value":-1}]}}"#).contains("days must be"));
        assert!(error(r#"{"match":{"rules":[{"field":"year","op":"is","value":1,"extra":1}]}}"#).contains("unknown key \"extra\""));
        assert!(error(r#"{"match":{"all":"yes","rules":[]}}"#).contains("match.all"));
        assert!(error(r#"{"match":{"rules":{}}}"#).contains("match.rules: expected a list"));
        assert!(error(r#"{"sort":{"field":"isDownloaded"}}"#).contains("cannot sort by"));
        assert!(error(r#"{"sort":{}}"#).contains("sort.field"));
        assert!(error(r#"{"limit":-1}"#).contains("limit: must not be negative"));
        let mut deep = r#"{"field":"year","op":"is","value":1}"#.to_string();
        for _ in 0..9 {
            deep = format!(r#"{{"rules":[{deep}]}}"#);
        }
        assert!(error(&format!(r#"{{"match":{deep}}}"#)).contains("nested more than 8"));
        // a broken definition is refused everywhere, not only by validate
        let core = Core::new(String::new()).unwrap();
        assert!(matches!(core.smart_evaluate("{".into(), vec![], 0, 1), Err(CoreError::Parse { .. })));
        assert!(matches!(core.smart_count("{".into(), vec![]), Err(CoreError::Parse { .. })));
        assert!(matches!(core.smart_save(String::new(), "x".into(), "{".into()), Err(CoreError::Parse { .. })));
        assert!(core.smart_list().unwrap().is_empty());
    }

    #[test]
    fn dates_convert_both_ways() {
        assert_eq!(iso_from_ms(0), "1970-01-01T00:00:00");
        assert_eq!(iso_from_ms(1_709_251_199_000), "2024-02-29T23:59:59");
        assert_eq!(iso_from_ms(NOW), "2026-08-29T10:40:00");
        assert_eq!(date(&json!("2024-02-29T23:59:59.123Z"), false, "").unwrap(), ("2024-02-29T23:59:59".to_string(), 1_709_251_199_000));
        assert_eq!(date(&json!("1970-01-01"), false, "").unwrap().1, 0);
        assert_eq!(date(&json!("1970-01-01"), true, "").unwrap(), ("1970-01-01T23:59:60".to_string(), DAY - 1));
        for bad in ["2024", "2024/01/01", "2024-00-10", "2024-01-01T25:00:00", "2024-01-01 10:00:00", "ünï-cö-dé"] {
            assert!(date(&json!(bad), false, "").is_err(), "{bad}");
        }
    }

    #[test]
    fn storage_round_trip() {
        let core = Core::new(String::new()).unwrap();
        let def = one("genre", "is", json!("Jazz"));
        let a = core.smart_save(String::new(), "Jazz".into(), def.clone()).unwrap();
        let b = core.smart_save(String::new(), "Ünïcödé ✓".into(), "{}".into()).unwrap();
        assert!(a.starts_with("sp-") && a != b);
        assert_eq!(core.smart_save(a.clone(), "Jazz!".into(), def.clone()).unwrap(), a);
        let l = core.smart_list().unwrap();
        assert_eq!(l.len(), 2);
        assert!(l.contains(&SmartPlaylist { id: a.clone(), name: "Jazz!".into(), json: def }));
        assert!(l.iter().any(|p| p.name == "Ünïcödé ✓"));
        core.smart_delete(a).unwrap();
        core.smart_delete("missing".into()).unwrap();
        assert_eq!(core.smart_list().unwrap().len(), 1);
    }

    #[test]
    fn played_rules_are_driven_from_the_stats_table() {
        let core = Core::new(String::new()).unwrap();
        let c = core.db.lock();
        let plan = |json: &str| -> String {
            let def = parse(json).unwrap();
            let mut cp = Compiler { args: Vec::new(), downloaded: &[], downloaded_arg: None, now_ms: NOW, needs_rust: false };
            let cond = cp.node(&def.root);
            let sql = format!("EXPLAIN QUERY PLAN SELECT i.rowid {} WHERE {} AND {cond}", tables(&def.root), mixes::SONGS);
            let mut st = c.prepare(&sql).unwrap();
            let rows = st.query_map(rusqlite::params_from_iter(cp.args), |r| r.get::<_, String>(3)).unwrap();
            rows.map(|r| r.unwrap()).collect::<Vec<_>>().join("\n")
        };
        let defaults = smart_defaults();
        let of = |id: &str| defaults.iter().find(|d| d.id == id).unwrap().json.clone();
        for id in ["default-most-played", "default-recently-played"] {
            let p = plan(&of(id));
            assert!(p.contains("SCAN s"), "{id}: {p}");
        }
        assert!(plan(&one("genre", "is", json!("rock"))).contains("items_genre"));
        assert!(plan(&of("default-forgotten-favourites")).contains("items_starred"));
    }

    /// The size the index is meant for. Rowids only until the page is known, so this stays in the tens of
    /// milliseconds per call in a release build; here it only has to work and to page correctly.
    #[test]
    fn a_hundred_thousand_songs() {
        let core = Core::new(String::new()).unwrap();
        {
            let mut c = core.db.lock();
            let tx = c.transaction().unwrap();
            {
                let mut st = tx.prepare("INSERT INTO items(kind, id, json) VALUES(2, ?1, ?2)").unwrap();
                for i in 0..100_000u32 {
                    let s = Song {
                        id: format!("s{i}"),
                        title: format!("Title {i}"),
                        artist: format!("Artist {}", i % 3000),
                        genre: Some(["Rock", "Jazz", "Folk", "Ambient"][(i % 4) as usize].into()),
                        year: 1960 + i % 60,
                        duration: 120 + i % 400,
                        starred: i % 1000 == 0,
                        ..Default::default()
                    };
                    st.execute(params![s.id, serde_json::to_string(&s).unwrap()]).unwrap();
                }
            }
            tx.commit().unwrap();
        }
        let def = json!({ "match": { "rules": [
            { "field": "genre", "op": "is", "value": "jazz" }, { "field": "year", "op": "between", "value": [1990, 1999] },
            { "field": "title", "op": "contains", "value": "7" } ] },
            "sort": { "field": "duration", "descending": true }, "limit": 500 })
        .to_string();
        let first = core.smart_evaluate(def.clone(), vec![], 0, 50).unwrap();
        assert_eq!(first.len(), 50);
        assert!(first.windows(2).all(|w| w[0].duration >= w[1].duration));
        assert!(first.iter().all(|s| s.genre.as_deref() == Some("Jazz") && (1990..2000).contains(&s.year) && s.title.contains('7')));
        let last = core.smart_evaluate(def.clone(), vec![], 480, 50).unwrap();
        assert_eq!(last.len(), 20);
        assert_eq!(core.smart_count(def, vec![]).unwrap(), 500);

        let starred = json!({ "match": { "rules": [{ "field": "starred", "op": "isTrue" }] }, "sort": { "field": "random", "seed": 7 } }).to_string();
        assert_eq!(core.smart_count(starred.clone(), vec![]).unwrap(), 100);
        assert_eq!(core.smart_evaluate(starred, vec![], 90, 50).unwrap().len(), 10);

        let budget = json!({ "sort": { "field": "random", "seed": 3 }, "limitMs": 3_600_000 }).to_string();
        let hour = core.smart_evaluate(budget.clone(), vec![], 0, 1000).unwrap();
        let total: u32 = hour.iter().map(|s| s.duration).sum();
        assert!(total <= 3600 && total > 3600 - 520, "{total}");
        assert_eq!(core.smart_count(budget, vec![]).unwrap() as usize, hour.len());

        let unicode = json!({ "match": { "rules": [{ "field": "artist", "op": "is", "value": "ärtist 5" }] }, "limit": 10 }).to_string();
        assert!(core.smart_evaluate(unicode, vec![], 0, 10).unwrap().is_empty());
    }
}
