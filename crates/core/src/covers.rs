//! Artwork worth fetching before it is asked for. Covers are only kept once something has drawn them, so
//! a song downloaded from a menu would otherwise arrive on the device with no picture, and a skip would
//! show an empty sleeve for as long as the server takes to render the next cover. The app fetches what
//! this says at the sizes it says, with the same requests the rows and the player make, so a warmed
//! cover is a cache hit.

use crate::Core;

/// The list rendition: a row's thumbnail and a grid's card share it. A Subsonic server renders each size
/// it is asked for on demand and keeps it per size, so every extra size is another slow first fetch for
/// every album - measured at over a second each on a real server.
const ROW: u32 = 320;
/// The player's rendition, shared with the notification and the lock screen.
const FULL: u32 = 800;

/// The two sizes the app draws covers at: the list rendition and the player's.
const SIZES: [u32; 2] = [ROW, FULL];

/// Whether cover id `id` is an octo-fiesta provider item's: `ext-<provider>-...` (songs, albums,
/// artists) or a provider's playlist, `pl-<provider>-<id>`. Navidrome names its own playlists' covers
/// `pl-<id>_<when it changed>` too; those are the server's, kept and warmed like an album's (they were
/// taken for a provider's, never kept, and offline a playlist's cover never came). Looks only at the id,
/// up to the next parameter, and allocates nothing.
pub fn is_provider_id(id: &str) -> bool {
    let id = id.split('&').next().unwrap_or(id);
    if id.starts_with("ext-") {
        return true;
    }
    let Some(rest) = id.strip_prefix("pl-") else { return false };
    if rest.contains('_') {
        return false;
    }
    match rest.split_once('-') {
        Some((provider, _)) => !provider.is_empty() && provider.bytes().all(|b| b.is_ascii_lowercase()),
        None => false,
    }
}

/// How the app sizes, names and keeps artwork. Read once; a list asks for thousands of covers and builds
/// their addresses itself from the signed prefix (`Core::url_prefix`) rather than crossing for each.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct CoverRules {
    pub row: u32,
    pub card: u32,
    pub full: u32,
    /// What follows the signed prefix of `getCoverArt`: `&id=` and the encoded id, then `&size=`.
    pub id_param: String,
    pub size_param: String,
    /// The share of the app's memory decoded covers may hold, and the disk cache's size.
    pub memory_share: f64,
    pub disk_bytes: u64,
    /// The part of that memory still kept while no screen of the app is in sight (the screen off, another
    /// app): the most recently drawn covers, the page that comes back first. Music may play on for hours
    /// with nothing drawn.
    pub hidden_share: f64,
}

#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn cover_rules() -> CoverRules {
    CoverRules {
        row: ROW,
        card: ROW,
        full: FULL,
        id_param: "&id=".into(),
        size_param: "&size=".into(),
        memory_share: 0.15,
        disk_bytes: 256 * 1024 * 1024,
        hidden_share: 0.25,
    }
}

/// One cover to fetch: the cover id at one size.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct CoverWant {
    pub id: String,
    pub size: u32,
}

/// Provider artwork is left alone: asking octo-fiesta for it is asking a provider, for a song the user
/// may never keep.
fn warmable(art: &str) -> bool {
    !is_provider_id(art)
}

/// The covers of `arts` (cover ids, in order) to fetch, each at both sizes: provider artwork left out,
/// each cover once, at most `cap` covers.
pub fn cover_wants(arts: Vec<String>, cap: u32) -> Vec<CoverWant> {
    let mut seen: Vec<&str> = Vec::new();
    let mut out = Vec::new();
    for art in arts.iter().filter(|a| warmable(a)) {
        if seen.len() == cap as usize {
            break;
        }
        if seen.contains(&art.as_str()) {
            continue;
        }
        seen.push(art);
        out.extend(SIZES.iter().map(|&size| CoverWant { id: art.clone(), size }));
    }
    out
}

/// The most covers one download fetches ahead.
const DOWNLOAD_COVERS: u32 = 500;

#[cfg_attr(feature = "ffi", uniffi::export)]
impl Core {
    /// The covers of songs being downloaded (`arts`, their cover ids, in order), as addresses to fetch
    /// onto the disk now and not decode (a cover loader's `warm`): covers are only kept once something
    /// has drawn them, so a song downloaded from a menu, its cover never on screen, would arrive with no
    /// picture and show a blank plate for the rest of its life offline. Both sizes the app draws, each
    /// cover once, never a provider's, at most 500 covers.
    pub fn download_cover_urls(&self, arts: Vec<String>) -> Vec<String> {
        let prefix = self.url_prefix("getCoverArt".into());
        cover_wants(arts, DOWNLOAD_COVERS)
            .into_iter()
            .map(|w| {
                let mut out = String::new();
                cover_url_into(&mut out, &prefix, &w.id, w.size as i32);
                out
            })
            .collect()
    }
}

/// Asked only in Rust, so not exported to Kotlin.
impl Core {
    /// The address of cover `id` at `size` px, as every client asks for it ([`cover_url_into`]).
    pub fn cover_address(&self, id: String, size: u32) -> String {
        let mut out = String::new();
        cover_url_into(&mut out, &self.url_prefix("getCoverArt".into()), &id, size as i32);
        out
    }
}

/// The queue positions whose covers to fetch while `index` plays, nearest first: both neighbours first,
/// as a skip would reach them (`previous` and `next` are what a skip lands on, shuffle included; -1 at an
/// end), and then outwards in both directions a step at a time, `ahead` steps each way. Backwards as well
/// as forwards: going back through a queue is as ordinary as going on, and with only the one song behind
/// warmed, the second swipe back always waited on the server. `ahead` 0 still warms the song behind.
/// Only positions inside a queue of `len` songs, never the one playing.
pub fn cover_neighbours(index: i32, previous: i32, next: i32, ahead: i32, len: u32) -> Vec<u32> {
    let ahead = ahead.max(0);
    let mut around = vec![previous, next];
    for d in 2..=ahead {
        around.push(index + d);
        around.push(index - d);
    }
    around.truncate(if ahead == 0 { 1 } else { ahead as usize * 2 });
    let mut out: Vec<u32> = Vec::with_capacity(around.len());
    for p in around {
        if p != index && p >= 0 && (p as u32) < len && !out.contains(&(p as u32)) {
            out.push(p as u32);
        }
    }
    out
}

/// What to fetch and colour ahead around the song playing, from the core's own queue.
#[derive(Debug, Clone, PartialEq)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct CoversAround {
    /// The playing song's cover, then the covers a skip either way lands on: their colours are worked out
    /// before they are reached. Provider artwork included (the caller draws those, and only stores none).
    pub near: Vec<String>,
    /// The covers to fetch ahead ([`cover_neighbours`] `ahead` steps each way), each at both sizes, as
    /// [`cover_wants`] gives them.
    pub wants: Vec<CoverWant>,
}

/// [`cover_neighbours`] and [`cover_wants`] over the queue the core keeps, in one call: the page passes
/// where it is and the skips' targets (queue positions, -1 for none), not the songs. A skip used to cost
/// three calls, one of them carrying every cover id ahead.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn covers_around(index: i32, previous: i32, next: i32, ahead: i32) -> CoversAround {
    let ids: Vec<String> = crate::playlist::with(|p| p.ids().to_vec());
    let arts = crate::queue::cover_arts(&ids);
    around(&arts, index, previous, next, ahead)
}

fn around(arts: &[Option<String>], index: i32, previous: i32, next: i32, ahead: i32) -> CoversAround {
    let len = arts.len() as u32;
    let at = |i: u32| arts.get(i as usize).cloned().flatten();
    let current = u32::try_from(index).ok().and_then(at);
    let near = current.into_iter().chain(cover_neighbours(index, previous, next, 1, len).into_iter().filter_map(at)).collect();
    let wants = cover_wants(cover_neighbours(index, previous, next, ahead, len).into_iter().filter_map(at).collect(), u32::MAX);
    CoversAround { near, wants }
}

/// The server addresses that are one server's (a profile's second address, `alt`, and its first,
/// `primary`), so a cover fetched at one is the cover kept for the other: offline the app falls back to
/// the second address, and covers kept while at home must still be found. Set with the profile.
static ALIKE: std::sync::RwLock<Vec<(String, String)>> = std::sync::RwLock::new(Vec::new());

/// A server address with no scheme and no trailing slash or `/rest`: what tells a server's covers from
/// another's in their keys.
fn host_of(base: &str) -> &str {
    let base = base.trim().trim_end_matches('/');
    let base = base.strip_suffix("/rest").unwrap_or(base);
    base.split_once("://").map_or(base, |(_, rest)| rest)
}

/// Says that `alt` is another address of the server at `primary` (the client's two addresses), for the
/// covers' keys ([`cover_key_parts`]). Blank `alt`: nothing.
pub fn cover_address_alike(primary: &str, alt: &str) {
    let (p, a) = (host_of(primary), host_of(alt));
    if a.is_empty() || p.is_empty() || a == p {
        return;
    }
    let mut alike = ALIKE.write().unwrap_or_else(|e| e.into_inner());
    alike.retain(|(x, _)| x != a);
    alike.push((a.to_string(), p.to_string()));
}

/// The query parameters that sign a request rather than name what it asks for: the user, token, salt,
/// password or API key, and the API version, client name and format. None of them is part of a cover's
/// key.
const SIGNATURE: [&str; 8] = ["u", "t", "s", "p", "apiKey", "v", "c", "f"];

/// What a cover's key is made of, in order, handed to `part` piece by piece (nothing allocated but the
/// lookup of a second address): the server (its first address, whichever of its addresses `url` is at,
/// with no scheme), the endpoint, and every parameter but the signature ([`SIGNATURE`]) - for a cover,
/// its id and size. So a cover is kept under the same key whatever token, salt or address fetched it,
/// and a new password or the other address still finds it on the disk.
pub fn cover_key_parts(url: &str, mut part: impl FnMut(&[u8])) {
    let (head, query) = url.split_once('?').unwrap_or((url, ""));
    let (base, path) = match head.find("/rest/") {
        Some(i) => (&head[..i], &head[i..]),
        None => (head, ""),
    };
    let host = host_of(base);
    {
        let alike = ALIKE.read().unwrap_or_else(|e| e.into_inner());
        part(alike.iter().find(|(a, _)| a == host).map_or(host, |(_, p)| p.as_str()).as_bytes());
    }
    part(path.as_bytes());
    for pair in query.split('&').filter(|p| !p.is_empty()) {
        let name = pair.split_once('=').map_or(pair, |(k, _)| k);
        if !SIGNATURE.contains(&name) {
            part(b"&");
            part(pair.as_bytes());
        }
    }
}

/// Whether the cover address `url` is an octo-fiesta provider item's ([`is_provider_id`]):
/// such a cover is never stored, since the provider redraws it under the same id once the item is in the
/// library. Asked for every cover a list draws, so it only looks, and allocates nothing.
///
/// Android asks it through a `@FastNative` door (`CoverPixels.isProvider`), which measured faster than
/// the same test written in Kotlin and allocates nothing.
pub fn is_provider_cover(url: &str) -> bool {
    url.match_indices("&id=").any(|(at, mark)| is_provider_id(&url[at + mark.len()..]))
}

/// The platform's GET, for a cover loader that runs beside the core's client rather than inside it
/// (nori-covers, which Android's covers go through): the same transport, so covers ride the API's
/// connection. Covers are fetched at addresses the client has already signed, so the loader needs no
/// client of its own, and one transport serves every server profile.
static COVER_TRANSPORT: std::sync::Mutex<Option<std::sync::Arc<dyn crate::transport::Transport>>> = std::sync::Mutex::new(None);
static COVER_TRANSPORT_SET: std::sync::Condvar = std::sync::Condvar::new();

/// Hands the core the transport its cover loader fetches through. Android does it where it builds the
/// transport for the client, on the thread that warms the app up.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn set_cover_transport(transport: std::sync::Arc<dyn crate::transport::Transport>) {
    *COVER_TRANSPORT.lock().unwrap_or_else(|e| e.into_inner()) = Some(transport);
    COVER_TRANSPORT_SET.notify_all();
}

/// The transport [`set_cover_transport`] handed in, waiting up to `wait` for it: a cover asked for as
/// the app starts may reach the network before the transport is built, and waits for it rather than
/// fail.
pub fn cover_transport(wait: std::time::Duration) -> Option<std::sync::Arc<dyn crate::transport::Transport>> {
    let set = COVER_TRANSPORT.lock().unwrap_or_else(|e| e.into_inner());
    let (set, _) = COVER_TRANSPORT_SET.wait_timeout_while(set, wait, |t| t.is_none()).unwrap_or_else(|e| e.into_inner());
    set.clone()
}

/// Writes the address of cover `id` at `size` into `out` (cleared first): the signed `getCoverArt`
/// prefix (`Core::url_prefix`, asked once per server and address), then the id and the size, exactly as
/// the Android app builds it, so every client asks the server for the same renditions and the same
/// cache keys. The id is escaped the way Android's `Uri.encode` escapes it, which leaves `!'()*` alone
/// where `api::encode` escapes them: the two must not be mixed for one cover. A list builds thousands
/// of these, so the caller keeps `out` and nothing is allocated once it has grown.
///
/// Twin of `Library.coverUrl` (core/.../data/Library.kt), which Android keeps (plain string work per
/// row, cheaper than a crossing).
pub fn cover_url_into(out: &mut String, prefix: &str, id: &str, size: i32) {
    use std::fmt::Write;
    out.clear();
    out.push_str(prefix);
    out.push_str("&id=");
    uri_encode(out, id);
    out.push_str("&size=");
    let _ = write!(out, "{size}");
}

/// Android's `Uri.encode(s)`: letters, digits and `_-!.~'()*` stay, every other character goes as its
/// UTF-8 bytes in upper-case `%XX`.
fn uri_encode(out: &mut String, s: &str) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || "_-!.~'()*".contains(c) {
            out.push(c);
        } else {
            let mut utf8 = [0u8; 4];
            for &b in c.encode_utf8(&mut utf8).as_bytes() {
                out.push('%');
                out.push(HEX[(b >> 4) as usize] as char);
                out.push(HEX[(b & 15) as usize] as char);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_download_warms_its_covers_at_the_addresses_the_rows_ask_for() {
        let core = crate::Core::new(String::new(), "t".into()).unwrap();
        core.configure(crate::ServerConfig { url: "http://m".into(), user: "u".into(), password: "p".into(), ..Default::default() }).unwrap();
        let urls = core.download_cover_urls(["al 1", "ext-2", "al 1"].map(String::from).to_vec());
        let prefix = core.url_prefix("getCoverArt".into());
        assert_eq!(urls, [format!("{prefix}&id=al%201&size=320"), format!("{prefix}&id=al%201&size=800")]);
        assert_eq!(core.cover_address("al 1".into(), 320), urls[0]);
    }

    #[test]
    fn wants_skip_providers_repeat_nothing_and_stop_at_the_cap() {
        let arts = ["a", "ext-1", "b", "a", "pl-deezer-2", "c"].map(String::from).to_vec();
        let w = cover_wants(arts.clone(), 500);
        assert_eq!(w.iter().map(|w| (w.id.as_str(), w.size)).collect::<Vec<_>>(), [("a", 320), ("a", 800), ("b", 320), ("b", 800), ("c", 320), ("c", 800)]);
        assert_eq!(cover_wants(arts, 2).len(), 4);
    }

    #[test]
    fn provider_covers_are_told_by_the_id_alone() {
        for (url, provider) in [
            ("https://m.example/rest/getCoverArt.view?u=a&t=b&s=c&id=ext-deezer-song-1&size=320", true),
            ("https://m.example/rest/getCoverArt.view?u=a&id=pl-deezer-12&size=800", true),
            // Navidrome's own playlists: kept like an album's.
            ("https://m.example/rest/getCoverArt.view?u=a&id=pl-6b2d0c1e-5f7a-4e21-9d3c-0a1b2c3d4e5f_65f0a1b2&size=800", false),
            ("https://m.example/rest/getCoverArt.view?u=a&id=pl-abcdefab-5f7a_0&size=800", false),
            ("&id=pl-12", false),
            ("https://m.example/rest/getCoverArt.view?u=a&id=al-3&size=320", false),
            ("https://m.example/rest/getCoverArt.view?id=ext-1", false),
            ("https://m.example/ext-1?xid=ext-2", false),
            ("&id=pl-", false),
            ("&id=pl-qobuz-7&size=320", true),
            ("&id=p", false),
            ("", false),
        ] {
            assert_eq!(is_provider_cover(url), provider, "{url}");
        }
    }

    fn key(url: &str) -> Vec<u8> {
        let mut out = Vec::new();
        cover_key_parts(url, |p| out.extend_from_slice(p));
        out
    }

    #[test]
    fn a_covers_key_is_its_server_id_and_size_never_its_signature() {
        let core = crate::Core::new(String::new(), "t".into()).unwrap();
        core.configure(crate::ServerConfig { url: "https://keys.example".into(), user: "u".into(), password: "one".into(), ..Default::default() }).unwrap();
        let before = core.cover_address("pl-6b2d_65f0".into(), 320);
        core.configure(crate::ServerConfig { url: "https://keys.example".into(), user: "u".into(), password: "two".into(), ..Default::default() }).unwrap();
        let after = core.cover_address("pl-6b2d_65f0".into(), 320);
        assert_ne!(before, after, "a new password signs it otherwise");
        assert_eq!(key(&before), key(&after));
        assert_eq!(key(&before), b"keys.example/rest/getCoverArt&id=pl-6b2d_65f0&size=320");
        assert_ne!(key(&before), key(&core.cover_address("pl-6b2d_65f0".into(), 800)), "each size its own");
        assert_ne!(key(&before), key(&core.cover_address("al-1".into(), 320)));
        // The second address of a server is the first, for its covers; another server stays its own.
        cover_address_alike("http://keys.lan:4533/", "https://keys.example");
        assert_eq!(key("http://keys.lan:4533/rest/getCoverArt?u=a&t=x&s=y&id=al-1&size=320"), key("https://keys.example/rest/getCoverArt?u=b&id=al-1&size=320"));
        assert_ne!(key("https://other.example/rest/getCoverArt?id=al-1&size=320"), key("https://keys.example/rest/getCoverArt?id=al-1&size=320"));
    }

    #[test]
    fn neighbours_go_outwards_from_the_skips() {
        assert_eq!(cover_neighbours(5, 4, 6, 3, 100), [4, 6, 7, 3, 8, 2]);
        // Shuffle: the skips land anywhere; the steps still count from the playing song.
        assert_eq!(cover_neighbours(5, 40, 12, 2, 100), [40, 12, 7, 3]);
        assert_eq!(cover_neighbours(5, 4, 6, 1, 100), [4, 6]);
        assert_eq!(cover_neighbours(5, 4, 6, 0, 100), [4]);
        // At the ends: nothing before the first song or after the last, and no repeats.
        assert_eq!(cover_neighbours(0, -1, 1, 3, 3), [1, 2]);
        assert_eq!(cover_neighbours(1, 0, 0, 2, 2), [0]);
    }

    #[test]
    fn around_the_song_playing_from_the_queue_itself() {
        let arts: Vec<Option<String>> = ["a", "b", "c", "ext-d", "b"].iter().map(|s| Some(s.to_string())).chain([None]).collect();
        let r = around(&arts, 1, 0, 2, 3);
        assert_eq!(r.near, ["b", "a", "c"], "the playing song's cover first, then the skips' targets");
        // Ahead: 0, 2, 3, then 4 (a second copy of b) and 5 (no cover): each cover once, providers left out.
        assert_eq!(r.wants.iter().map(|w| (w.id.as_str(), w.size)).collect::<Vec<_>>(), [("a", 320), ("a", 800), ("c", 320), ("c", 800), ("b", 320), ("b", 800)]);
        assert_eq!(around(&[], -1, -1, -1, 2), CoversAround { near: vec![], wants: vec![] }, "an empty queue");
    }
}
