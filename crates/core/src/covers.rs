//! Artwork worth fetching before it is asked for. Covers are only kept once something has drawn them, so
//! a song downloaded from a menu would otherwise arrive on the device with no picture, and a skip would
//! show an empty sleeve for as long as the server takes to render the next cover. The app fetches what
//! this says at the sizes it says, with the same requests the rows and the player make, so a warmed
//! cover is a cache hit.

/// The list rendition: a row's thumbnail and a grid's card share it. A Subsonic server renders each size
/// it is asked for on demand and keeps it per size, so every extra size is another slow first fetch for
/// every album - measured at over a second each on a real server.
const ROW: u32 = 320;
/// The player's rendition, shared with the notification and the lock screen.
const FULL: u32 = 800;

/// The two sizes the app draws covers at: the list rendition and the player's.
const SIZES: [u32; 2] = [ROW, FULL];

/// Ids of an octo-fiesta provider's items (songs, albums, artists; playlists).
const PROVIDER_PREFIXES: [&str; 2] = ["ext-", "pl-"];

/// How the app sizes, names and keeps artwork. Read once; a list asks for thousands of covers and builds
/// their addresses itself from the signed prefix (`Core::url_prefix`) rather than crossing for each.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CoverRules {
    pub row: u32,
    pub card: u32,
    pub full: u32,
    /// What follows the signed prefix of `getCoverArt`: `&id=` and the encoded id, then `&size=`.
    pub id_param: String,
    pub size_param: String,
    /// Ids that belong to a provider. octo-fiesta draws a "not downloaded" badge on their covers and
    /// replaces the picture once the item is in the library, under the same id: never stored.
    pub provider_prefixes: Vec<String>,
    /// The share of the app's memory decoded covers may hold, and the disk cache's size.
    pub memory_share: f64,
    pub disk_bytes: u64,
}

#[uniffi::export]
pub fn cover_rules() -> CoverRules {
    CoverRules {
        row: ROW,
        card: ROW,
        full: FULL,
        id_param: "&id=".into(),
        size_param: "&size=".into(),
        provider_prefixes: PROVIDER_PREFIXES.map(String::from).to_vec(),
        memory_share: 0.15,
        disk_bytes: 256 * 1024 * 1024,
    }
}

/// One cover to fetch: the cover id at one size.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct CoverWant {
    pub id: String,
    pub size: u32,
}

/// Provider artwork is left alone: asking octo-fiesta for it is asking a provider, for a song the user
/// may never keep.
fn warmable(art: &str) -> bool {
    !PROVIDER_PREFIXES.iter().any(|p| art.starts_with(p))
}

/// The covers of `arts` (cover ids, in order) to fetch, each at both sizes: provider artwork left out,
/// each cover once, at most `cap` covers.
#[uniffi::export]
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

/// The queue positions whose covers to fetch while `index` plays, nearest first: both neighbours first,
/// as a skip would reach them (`previous` and `next` are what a skip lands on, shuffle included; -1 at an
/// end), and then outwards in both directions a step at a time, `ahead` steps each way. Backwards as well
/// as forwards: going back through a queue is as ordinary as going on, and with only the one song behind
/// warmed, the second swipe back always waited on the server. `ahead` 0 still warms the song behind.
/// Only positions inside a queue of `len` songs, never the one playing.
#[uniffi::export]
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wants_skip_providers_repeat_nothing_and_stop_at_the_cap() {
        let arts = ["a", "ext-1", "b", "a", "pl-2", "c"].map(String::from).to_vec();
        let w = cover_wants(arts.clone(), 500);
        assert_eq!(w.iter().map(|w| (w.id.as_str(), w.size)).collect::<Vec<_>>(), [("a", 320), ("a", 800), ("b", 320), ("b", 800), ("c", 320), ("c", 800)]);
        assert_eq!(cover_wants(arts, 2).len(), 4);
    }

    #[test]
    fn the_rules_the_app_sizes_and_keeps_covers_by() {
        let r = cover_rules();
        assert_eq!((r.row, r.card, r.full), (320, 320, 800));
        assert_eq!((r.id_param.as_str(), r.size_param.as_str()), ("&id=", "&size="));
        assert_eq!(r.provider_prefixes, ["ext-", "pl-"]);
        assert_eq!((r.memory_share, r.disk_bytes), (0.15, 268_435_456));
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
}
