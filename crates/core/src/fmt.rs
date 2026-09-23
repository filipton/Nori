//! What the app writes on screen about numbers: times, decibels, frequencies, sizes, captions. Each is
//! worked out here once (a page, a song, a band) so the UI only shows strings, and each prints exactly
//! what the Kotlin it replaces printed: fractions go through `nori_text`, which rounds as `String.format`
//! did and writes the phone's own decimal separator. Where the Kotlin wrote a `Double` into a template
//! instead ("44.1 kHz" in a song's details), it was never localised and `{:?}` writes it the same way.

use std::fmt::Write;

use nori_text::{decimal, fixed};

use crate::model::{ListeningStats, Song};

/// The platform's number style, once at start and again whenever the locale changes: the decimal and
/// grouping separators its default locale formats with (`DecimalFormatSymbols` on Android). Every
/// fraction the app writes, here and in the player, then reads as the phone's own did - "12,4 MB" on a
/// Polish phone. Never called, it is a point (see `nori_text`).
#[uniffi::export]
pub fn fmt_set_locale(decimal_separator: String, grouping_separator: String) {
    nori_text::set_style(&decimal_separator, &grouping_separator);
}

/// "3:07", or "1:02:03" from an hour.
#[uniffi::export]
pub fn duration(seconds: i64) -> String {
    let s = seconds;
    if s >= 3600 {
        format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60)
    } else {
        format!("{}:{:02}", s / 60, s % 60)
    }
}

/// The time left in a song, under the seek bar: "-3:07".
#[uniffi::export]
pub fn duration_left(seconds: i64) -> String {
    format!("-{}", duration(seconds))
}

/// A decibel figure with its sign, one decimal: "+3.5", "-1.0", and "+0.0" for nothing at all. `-0.0`
/// is a real float - the automatic pre-amp is minus the largest boost, and minus nothing is negative
/// zero - and printed as it is it read "-0.0 dB".
#[uniffi::export]
pub fn signed_db(db: f32) -> String {
    fixed(if db == 0.0 { 0.0 } else { db as f64 }, 1, true)
}

/// How far the lyrics are nudged: "+0.5 s", "-1.3 s".
#[uniffi::export]
pub fn nudge_seconds(ms: i64) -> String {
    format!("{} s", fixed((ms as f32 / 1000.0) as f64, 1, true))
}

/// A band's frequency as its label says it: "63", "1k", "2.500k", "12.50k", "16k". The trailing zeros
/// were cut with a point in the pattern, so with a decimal comma they stay ("1,000k"), as they did.
fn hz_text_in(f: f32, sep: char) -> String {
    if f >= 1000.0 {
        let k = nori_text::general_in((f / 1000.0) as f64, 4, sep) + "k";
        k.replace(".000k", "k").replace(".00k", "k")
    } else {
        nori_text::fixed_in(f as f64, 0, false, sep)
    }
}

fn hz_text(f: f32) -> String {
    hz_text_in(f, decimal())
}

/// A band's label: its frequency and a mark for what kind of band it is - one channel only (L, R), a
/// shelf (↙ low, ↗ high) or a band with no gain (∿), in that order of precedence.
#[uniffi::export]
pub fn eq_band_label(freq: f32, left: bool, right: bool, low_shelf: bool, high_shelf: bool, uses_gain: bool) -> String {
    let mark = if left {
        " L"
    } else if right {
        " R"
    } else if low_shelf {
        " ↙"
    } else if high_shelf {
        " ↗"
    } else if !uses_gain {
        " ∿"
    } else {
        ""
    };
    hz_text(freq) + mark
}

/// A band's frequency alone: the band dialog's title says "{this} Hz".
#[uniffi::export]
pub fn eq_hz(freq: f32) -> String {
    hz_text(freq)
}

/// The frequency slider is logarithmic: its position is the exponent, 20 Hz at 0 to 20 kHz at 1.
#[uniffi::export]
pub fn eq_freq_to_slider(freq: f32) -> f32 {
    (((freq / 20.0) as f64).log10() as f32) / 3.0
}

#[uniffi::export]
pub fn eq_slider_to_freq(x: f32) -> f32 {
    20.0 * (10f64.powf(x as f64 * 3.0) as f32)
}

/// "Slope 0.71" for a shelf given by its slope, "Q 1.41" for the rest.
#[uniffi::export]
pub fn eq_shape(slope: bool, q: f32) -> String {
    format!("{} {}", if slope { "Slope" } else { "Q" }, fixed(q as f64, 2, false))
}

/// A balance near the middle is the middle: within 4 % of it the slider snaps to 0.
#[uniffi::export]
pub fn eq_balance_snap(v: f32) -> f32 {
    if v.abs() < 0.04 { 0.0 } else { v }
}

/// "centre", "L 30%", "R 5%".
#[uniffi::export]
pub fn eq_balance(balance: f32) -> String {
    if balance == 0.0 {
        return "centre".into();
    }
    format!("{} {}%", if balance < 0.0 { "L" } else { "R" }, fixed((balance.abs() * 100.0) as f64, 0, false))
}

/// Crossfeed under a decibel is none at all.
#[uniffi::export]
pub fn eq_crossfeed_snap(db: f32) -> f32 {
    if db < 1.0 { 0.0 } else { db }
}

#[uniffi::export]
pub fn eq_crossfeed(db: f32) -> String {
    if db > 0.0 {
        format!("{} dB: each ear also hears a little of the other channel, like loudspeakers. For headphones.", fixed(db as f64, 1, false))
    } else {
        "Off".into()
    }
}

/// "Ceiling -1.0 dB".
#[uniffi::export]
pub fn eq_ceiling(db: f32) -> String {
    format!("Ceiling {} dB", fixed(db as f64, 1, false))
}

/// What the limiter is pulling back right now, or that it is not: "−2.3 dB", "not clipping".
#[uniffi::export]
pub fn eq_reduction(db: f32) -> String {
    if db > 0.05 {
        format!("−{} dB", fixed(db as f64, 1, false))
    } else {
        "not clipping".into()
    }
}

/// "Pre-amp -3.5 dB (automatic)".
#[uniffi::export]
pub fn eq_preamp(db: f32, automatic: bool) -> String {
    format!("Pre-amp {} dB{}", signed_db(db), if automatic { " (automatic)" } else { "" })
}

/// "12.4 MB".
#[uniffi::export]
pub fn megabytes(bytes: u64) -> String {
    format!("{} MB", fixed(bytes as f64 / 1_048_576.0, 1, false))
}

/// "1 song", "12 songs" - or "12 songs" whatever the count when `always_plural`, which is how the album
/// and playlist pages have always put it.
fn songs(n: u32, always_plural: bool) -> String {
    format!("{n} song{}", if n == 1 && !always_plural { "" } else { "s" })
}

/// A list's caption: "12 songs · 48:10".
#[uniffi::export]
pub fn songs_caption(count: u32, seconds: u64, always_plural: bool) -> String {
    format!("{} · {}", songs(count, always_plural), duration(seconds as i64))
}

/// The summed length of `songs`, in seconds.
pub fn total_seconds(songs: &[Song]) -> u64 {
    songs.iter().map(|s| s.duration as u64).sum()
}

/// The format of a record, from its first song: "FLAC 24/96.0", "MP3 320 kbps".
pub fn quality_of(s: &Song) -> Option<String> {
    let suffix = s.suffix.to_lowercase();
    let lossless = matches!(suffix.as_str(), "flac" | "alac" | "wav" | "aiff" | "ape" | "wv" | "dsf" | "dff");
    let name = s.suffix.to_uppercase();
    let detail = if lossless && s.bit_depth > 0 {
        Some(format!("{}/{:?}", s.bit_depth, s.sampling_rate as f64 / 1000.0))
    } else {
        (s.bit_rate > 0).then(|| format!("{} kbps", s.bit_rate))
    };
    let parts: Vec<String> = [(!name.is_empty()).then_some(name), detail].into_iter().flatten().collect();
    (!parts.is_empty()).then(|| parts.join(" "))
}

/// An album page's caption once its songs are in: "2019 · 12 songs · 48:10 · FLAC 16/44.1 · explicit".
#[uniffi::export]
pub fn album_caption(year: u32, songs: Vec<Song>, explicit: bool) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(5);
    if year > 0 {
        parts.push(year.to_string());
    }
    parts.push(format!("{} songs", songs.len()));
    parts.push(duration(total_seconds(&songs) as i64));
    if let Some(q) = songs.first().and_then(quality_of) {
        parts.push(q);
    }
    if explicit {
        parts.push("explicit".into());
    }
    parts.join(" · ")
}

/// What is known of an album before its songs are: "2019 · 12 songs · 48:10", each part only if known.
#[uniffi::export]
pub fn album_hint_caption(year: u32, song_count: u32, seconds: u32) -> String {
    let mut parts: Vec<String> = Vec::with_capacity(3);
    if year > 0 {
        parts.push(year.to_string());
    }
    if song_count > 0 {
        parts.push(format!("{song_count} songs"));
    }
    if seconds > 0 {
        parts.push(duration(seconds as i64));
    }
    parts.join(" · ")
}

/// An album page's caption from whatever is known of it: [`album_caption`] once its songs are in, the
/// [`album_hint_caption`] of the row that opened it before then. Explicit is the server's own flag.
#[uniffi::export]
pub fn album_page_caption(album: crate::model::Album, songs: Option<Vec<Song>>) -> String {
    match songs {
        Some(songs) => album_caption(album.year, songs, album.explicit_status == "explicit"),
        None => album_hint_caption(album.year, album.song_count, album.duration),
    }
}

/// A caption for `songs` as a whole: "12 songs · 48:10". See [`songs_caption`].
#[uniffi::export]
pub fn list_caption(songs: Vec<Song>, always_plural: bool) -> String {
    songs_caption(songs.len() as u32, total_seconds(&songs), always_plural)
}

/// "ext-deezer-song-123" -> "Deezer": which service an octo-fiesta item comes from.
#[uniffi::export]
pub fn provider_of(id: String) -> Option<String> {
    if !id.starts_with("ext-") && !id.starts_with("pl-") {
        return None;
    }
    let name = id.split('-').nth(1)?;
    let mut c = name.chars();
    let first = c.next()?;
    let cap: String = first.to_uppercase().chain(c).collect();
    Some(if cap == "Squidwtf" { "SquidWTF".into() } else { cap })
}

/// An album card's second line: "Artist · 2019 · ☁ Deezer", each part only if there is one.
#[uniffi::export]
pub fn album_subtitle(artist: String, year: u32, id: String) -> String {
    let mut out = String::new();
    let mut add = |s: &str| {
        if !out.is_empty() {
            out.push_str(" · ");
        }
        out.push_str(s);
    };
    if !artist.is_empty() {
        add(&artist);
    }
    if year > 0 {
        add(&year.to_string());
    }
    if let Some(p) = provider_of(id) {
        add(&format!("☁ {p}"));
    }
    out
}

/// A song row's second line: an explicit mark, then the artist unless the page is already about them.
#[uniffi::export]
pub fn song_line(explicit_status: String, artist: String, page_artist: Option<String>) -> String {
    let show = page_artist.is_none_or(|p| !artist.to_lowercase().eq(&p.to_lowercase()));
    let mut out = String::new();
    if explicit_status == "explicit" {
        out.push_str("🅴 ");
    }
    if show {
        out.push_str(&artist);
    }
    out
}

/// A song's rate, sample rate, depth and (with `channels`) channels as the details say them, each only if known.
fn quality_parts(s: &Song, channels: bool) -> Vec<String> {
    let mut out = Vec::with_capacity(4);
    if s.bit_rate > 0 {
        out.push(format!("{} kbps", s.bit_rate));
    }
    if s.sampling_rate > 0 {
        out.push(format!("{:?} kHz", s.sampling_rate as f64 / 1000.0));
    }
    if s.bit_depth > 0 {
        out.push(format!("{} bit", s.bit_depth));
    }
    if channels && s.channel_count > 0 {
        out.push(format!("{} ch", s.channel_count));
    }
    out
}

/// The line under a song at the top of its menu: "FLAC · 1411 kbps · 44.1 kHz · 16 bit".
#[uniffi::export]
pub fn song_format(song: Song) -> String {
    let mut parts = vec![song.suffix.to_uppercase()];
    parts.extend(quality_parts(&song, false));
    parts.join(" · ")
}

/// One line of a song's details: what it is, and what the server said.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct InfoRow {
    pub label: String,
    pub value: String,
}

/// Everything the server said about one file, in the order the details sheet shows it, blanks left out.
#[uniffi::export]
pub fn track_info(song: Song) -> Vec<InfoRow> {
    let join = |parts: Vec<Option<String>>, sep: &str| parts.into_iter().flatten().collect::<Vec<_>>().join(sep);
    let artists = song.artists.iter().map(|a| a.name.as_str()).collect::<Vec<_>>().join(", ");
    let gain = song.replay_gain.as_ref().map(|g| {
        join(
            vec![
                g.track_gain.map(|v| format!("track {} dB", fixed(v as f64, 2, true))),
                g.album_gain.map(|v| format!("album {} dB", fixed(v as f64, 2, true))),
                g.track_peak.map(|v| format!("peak {}", fixed(v as f64, 3, false))),
            ],
            " · ",
        )
    });
    let rows: Vec<(&str, Option<String>)> = vec![
        ("Title", Some(song.title.clone())),
        ("Artist", Some(if artists.is_empty() { song.artist.clone() } else { artists })),
        ("Album", Some(song.album.clone())),
        ("Track", Some(join(vec![(song.disc_number > 0).then(|| format!("disc {}", song.disc_number)), (song.track > 0).then(|| format!("track {}", song.track))], ", "))),
        ("Year", (song.year > 0).then(|| song.year.to_string())),
        ("Genre", song.genre.clone()),
        ("Duration", Some(duration(song.duration as i64))),
        ("Format", Some(join(vec![(!song.suffix.is_empty()).then(|| song.suffix.to_uppercase()), (!song.content_type.is_empty()).then(|| song.content_type.clone())], " · "))),
        ("Quality", Some(quality_parts(&song, true).join(" · "))),
        ("Size", (song.size > 0).then(|| megabytes(song.size))),
        ("ReplayGain", gain),
        ("BPM", (song.bpm > 0).then(|| song.bpm.to_string())),
        ("Plays (server)", (song.play_count > 0).then(|| song.play_count.to_string())),
        ("Last played", song.played.as_ref().map(|p| p.chars().take(16).collect::<String>().replace('T', " "))),
        ("Added", song.created.as_ref().map(|c| c.chars().take(10).collect())),
        ("Path", song.path.clone()),
        ("MusicBrainz", song.music_brainz_id.clone()),
        ("Comment", song.comment.clone()),
        ("Id", Some(song.id.clone())),
    ];
    rows.into_iter()
        .filter_map(|(label, v)| v.filter(|v| !v.trim().is_empty()).map(|value| InfoRow { label: label.into(), value }))
        .collect()
}

/// What the listening page says about its numbers, worked out once per period.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct StatsWords {
    /// Under the play count: "plays · 12:34:56 listened".
    pub headline: String,
    /// "Most around 21:00, mostly on Fridays"; none (and no chart) when nothing was played.
    pub habit: Option<String>,
    /// Each hour's bar as a fraction of the busiest hour's.
    pub hours: Vec<f32>,
    /// The top artists' second lines: how long each was listened to.
    pub artist_times: Vec<String>,
}

/// The first index holding the largest value, as Kotlin's `maxByOrNull` picks it.
fn busiest(v: &[u32]) -> Option<usize> {
    v.iter().enumerate().fold(None, |best: Option<(usize, u32)>, (i, &x)| match best {
        Some((_, b)) if b >= x => best,
        _ => Some((i, x)),
    }).map(|(i, _)| i)
}

/// One period the listening stats can cover: `days` back from now, 0 for all time.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StatsPeriod {
    pub days: u32,
    pub label: String,
}

/// The periods, in the order the chips run, and which one opens (a year).
#[uniffi::export]
pub fn stats_periods() -> Vec<StatsPeriod> {
    [(7, "Week"), (30, "Month"), (365, "Year"), (0, "All time")].map(|(days, l)| StatsPeriod { days, label: l.into() }).to_vec()
}

#[uniffi::export]
pub fn stats_default_days() -> u32 {
    365
}

/// One tile of the stats: a number and what it counts.
#[derive(Debug, Clone, PartialEq, Eq, uniffi::Record)]
pub struct StatTileWords {
    pub value: String,
    pub label: String,
}

/// The six tiles under the headline, in two rows of three.
#[uniffi::export]
pub fn stats_tiles(stats: ListeningStats) -> Vec<StatTileWords> {
    [
        (stats.distinct_songs, "songs"),
        (stats.distinct_artists, "artists"),
        (stats.distinct_albums, "albums"),
        (stats.skips, "skips"),
        (stats.active_days, "active days"),
        (stats.longest_streak_days, "day streak"),
    ]
    .map(|(v, l)| StatTileWords { value: v.to_string(), label: l.into() })
    .to_vec()
}

/// The hours marked under the listening-day chart.
#[uniffi::export]
pub fn stats_hour_ticks() -> Vec<String> {
    ["00", "06", "12", "18", "23"].map(String::from).to_vec()
}

#[uniffi::export]
pub fn stats_words(stats: ListeningStats) -> StatsWords {
    const DAYS: [&str; 7] = ["Mondays", "Tuesdays", "Wednesdays", "Thursdays", "Fridays", "Saturdays", "Sundays"];
    let when = busiest(&stats.plays_per_hour).filter(|&h| stats.plays_per_hour[h] > 0).map(|h| {
        let mut out = format!("Most around {h}:00");
        if let Some(d) = busiest(&stats.plays_per_weekday).and_then(|d| DAYS.get(d)) {
            let _ = write!(out, ", mostly on {d}");
        }
        out
    });
    let peak = stats.plays_per_hour.iter().copied().max().unwrap_or(0).max(1) as f32;
    StatsWords {
        headline: format!("plays · {} listened", duration(stats.listened_ms / 1000)),
        habit: when,
        hours: stats.plays_per_hour.iter().map(|&p| p as f32 / peak).collect(),
        artist_times: stats.top_artists.iter().map(|a| duration(a.listened_ms / 1000)).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Printed by Kotlin's hz(): the float's bits and the label.
    const HZ: &[(u32, &str)] = include!("fmt_hz.in");

    #[test]
    fn stats_periods_and_tiles() {
        assert_eq!(stats_periods().iter().map(|p| (p.days, p.label.as_str())).collect::<Vec<_>>(), [(7, "Week"), (30, "Month"), (365, "Year"), (0, "All time")]);
        assert_eq!(stats_default_days(), 365);
        let t = stats_tiles(ListeningStats { distinct_songs: 4, longest_streak_days: 2, ..Default::default() });
        assert_eq!((t[0].value.as_str(), t[0].label.as_str()), ("4", "songs"));
        assert_eq!((t[5].value.as_str(), t[5].label.as_str()), ("2", "day streak"));
        assert_eq!(stats_hour_ticks(), ["00", "06", "12", "18", "23"]);
    }

    #[test]
    fn frequencies_read_as_they_did() {
        for &(bits, want) in HZ {
            assert_eq!(hz_text(f32::from_bits(bits)), want, "{}", f32::from_bits(bits));
        }
        assert_eq!(eq_band_label(1000.0, false, false, true, false, true), "1k ↙");
        assert_eq!(eq_band_label(62.5, true, false, false, false, false), "63 L");
        assert_eq!((hz_text(12_500.0), hz_text(2_500.0), hz_text(16_000.0)), ("12.50k".into(), "2.500k".into(), "16k".into()));
        // Kotlin's hz() on a Polish phone: the comma is written, and the point-only trim misses it.
        assert_eq!((hz_text_in(1_000.0, ','), hz_text_in(12_500.0, ','), hz_text_in(62.5, ',')), ("1,000k".into(), "12,50k".into(), "63".into()));
    }

    #[test]
    fn times_decibels_and_captions() {
        assert_eq!((duration(0), duration(187), duration(3723)), ("0:00".into(), "3:07".into(), "1:02:03".into()));
        assert_eq!((duration_left(187), duration_left(0)), ("-3:07".into(), "-0:00".into()));
        assert_eq!((signed_db(-0.0), signed_db(3.25), signed_db(-1.0)), ("+0.0".into(), "+3.3".into(), "-1.0".into()));
        assert_eq!(nudge_seconds(-250), "-0.3 s");
        assert_eq!((eq_balance(0.0), eq_balance(-0.3), eq_balance(0.05)), ("centre".into(), "L 30%".into(), "R 5%".into()));
        assert_eq!((eq_balance_snap(0.03), eq_crossfeed_snap(0.9), eq_crossfeed_snap(2.0)), (0.0, 0.0, 2.0));
        assert_eq!(eq_shape(true, 0.707), "Slope 0.71");
        assert_eq!(eq_reduction(0.01), "not clipping");
        assert_eq!(megabytes(10 * 1_048_576 + 104_858), "10.1 MB");
        assert_eq!(songs_caption(1, 200, false), "1 song · 3:20");
        assert_eq!(songs_caption(1, 200, true), "1 songs · 3:20");
        assert_eq!(provider_of("ext-squidwtf-song-1".into()).as_deref(), Some("SquidWTF"));
        assert_eq!(provider_of("ext-deezer-song-1".into()).as_deref(), Some("Deezer"));
        assert_eq!(provider_of("al-1".into()), None);
        assert_eq!(album_subtitle("Radiohead".into(), 1997, "pl-deezer-1".into()), "Radiohead · 1997 · ☁ Deezer");
        assert_eq!(album_subtitle("".into(), 0, "1".into()), "");
        assert_eq!(song_line("explicit".into(), "Radiohead".into(), Some("radiohead".into())), "🅴 ");
        assert_eq!(song_line("".into(), "Radiohead".into(), None), "Radiohead");
        let s = Song { suffix: "flac".into(), bit_depth: 24, sampling_rate: 96000, duration: 100, ..Default::default() };
        assert_eq!(quality_of(&s).as_deref(), Some("FLAC 24/96.0"));
        assert_eq!(album_caption(2019, vec![s.clone(), s], true), "2019 · 2 songs · 3:20 · FLAC 24/96.0 · explicit");
        assert_eq!(album_hint_caption(0, 12, 0), "12 songs");
        let album = crate::model::Album { year: 2019, song_count: 2, duration: 200, explicit_status: "explicit".into(), ..Default::default() };
        assert_eq!(album_page_caption(album.clone(), None), "2019 · 2 songs · 3:20");
        let t = Song { suffix: "flac".into(), bit_depth: 24, sampling_rate: 96000, duration: 100, ..Default::default() };
        assert_eq!(album_page_caption(album, Some(vec![t.clone(), t])), "2019 · 2 songs · 3:20 · FLAC 24/96.0 · explicit");
    }

    #[test]
    fn track_info_leaves_blanks_out() {
        let s = Song { id: "1".into(), title: "T".into(), artist: "A".into(), sampling_rate: 44100, size: 1_048_576, ..Default::default() };
        let rows = track_info(s);
        let labels: Vec<&str> = rows.iter().map(|r| r.label.as_str()).collect();
        assert_eq!(labels, ["Title", "Artist", "Duration", "Quality", "Size", "Id"]);
        assert_eq!(rows[3].value, "44.1 kHz");
        assert_eq!(rows[4].value, "1.0 MB");
        let s = Song { suffix: "flac".into(), bit_rate: 1411, sampling_rate: 44100, bit_depth: 16, channel_count: 2, ..Default::default() };
        assert_eq!(song_format(s), "FLAC · 1411 kbps · 44.1 kHz · 16 bit");
    }

    #[test]
    fn the_listening_page_reads_its_numbers() {
        use crate::model::TopEntry;
        let mut hours = vec![0u32; 24];
        hours[9] = 5;
        hours[21] = 10;
        hours[22] = 10;
        let stats = ListeningStats {
            listened_ms: 3_723_000,
            plays_per_hour: hours,
            plays_per_weekday: vec![1, 0, 0, 0, 4, 4, 0],
            top_artists: vec![TopEntry { listened_ms: 187_900, ..Default::default() }],
            ..Default::default()
        };
        let w = stats_words(stats);
        assert_eq!(w.headline, "plays · 1:02:03 listened");
        // The first of equal peaks, as `maxByOrNull` picks.
        assert_eq!(w.habit.as_deref(), Some("Most around 21:00, mostly on Fridays"));
        assert_eq!((w.hours[9], w.hours[21], w.hours[0]), (0.5, 1.0, 0.0));
        assert_eq!(w.artist_times, ["3:07"]);
        // Nothing played: no chart and nothing to say about it, and no dividing by nothing.
        let none = stats_words(ListeningStats { plays_per_hour: vec![0; 24], plays_per_weekday: vec![0; 7], ..Default::default() });
        assert_eq!((none.habit, none.hours[3]), (None, 0.0));
    }
}
