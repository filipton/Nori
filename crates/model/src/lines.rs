//! The words a record carries about itself, made when it is read (model.rs): a song row's second line,
//! an album card's subtitle, an artist's album count, a playlist's line. They live under the records so
//! a record can dress itself; the rest of what the app says is nori-words', which uses these as they are.

/// "ext-deezer-song-123" -> "Deezer": which service an octo-fiesta item comes from.
pub fn provider_of(id: &str) -> Option<String> {
    if !id.starts_with("ext-") && !id.starts_with("pl-") {
        return None;
    }
    let name = id.split('-').nth(1)?;
    let mut c = name.chars();
    let first = c.next()?;
    let cap: String = first.to_uppercase().chain(c).collect();
    Some(if cap == "Squidwtf" { "SquidWTF".into() } else { cap })
}

/// An album card's second line: "Artist · 2019 · ☁ Deezer", each part only if there is one. The album
/// carries it as its `subtitle`.
pub fn album_subtitle(artist: &str, year: u32, id: &str) -> String {
    let mut out = String::new();
    let mut add = |s: &str| {
        if !out.is_empty() {
            out.push_str(" · ");
        }
        out.push_str(s);
    };
    if !artist.is_empty() {
        add(artist);
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
/// The song carries it for no page as its `line`; an album's discs carry it for the album's artist.
pub fn song_line(explicit_status: &str, artist: &str, page_artist: Option<&str>) -> String {
    let show = page_artist.is_none_or(|p| !artist.to_lowercase().eq(&p.to_lowercase()));
    let mut out = String::new();
    if explicit_status == "explicit" {
        out.push_str("🅴 ");
    }
    if show {
        out.push_str(artist);
    }
    out
}

/// "1 song", "2 songs": `n` and the word for one or for many.
pub fn counted(n: u32, one: &str, many: &str) -> String {
    format!("{n} {}", if n == 1 { one } else { many })
}

/// How many songs a list holds: "1 song", "12 songs".
pub fn songs(n: u32) -> String {
    counted(n, "song", "songs")
}

/// How many albums an artist has: "1 album", "12 albums".
pub fn albums(n: u32) -> String {
    counted(n, "album", "albums")
}

/// A playlist's line in the library: "12 songs · 48:10".
pub fn playlist_line(songs_in_it: u32, seconds: u32) -> String {
    format!("{} · {}", songs(songs_in_it), nori_text::duration(seconds as i64, false))
}
