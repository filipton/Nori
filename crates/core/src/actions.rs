//! What playing something means, where it takes more than handing a list to the player: what a tap on
//! a song does, shuffling, starting a radio or an instant mix from one song, playing a whole artist,
//! and making a playlist out of an M3U file. The requests these need are made here too, so a screen
//! asks once and gets the list to play.

use crate::autofill::seed_now;
use crate::cache_policy::Read;
use crate::client::{Client, NetResult};
use crate::{m3u, mixes, words, Album, Core, Result, Song};

/// Songs the server is asked for when it finds nothing similar to a radio's song, and how many similar
/// ones it is asked for in the first place.
const RADIO: i32 = 50;
/// Songs in an instant mix drawn from the index.
const INSTANT_MIX: u32 = 50;
/// Songs "shuffle all" plays.
const SHUFFLE_ALL: i32 = 200;

/// What a plain tap on a song in a list does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum TapPlan {
    /// Something is selected: the tap adds the song to the selection or takes it out.
    Select,
    /// The whole list, from the song tapped.
    PlayList,
    /// Only the song tapped.
    PlayOne,
    Queue,
    PlayNext,
}

/// `TapAction` in the settings, by ordinal (see settings.rs).
fn tap(selecting: bool, tap_action: i32) -> TapPlan {
    match tap_action {
        _ if selecting => TapPlan::Select,
        1 => TapPlan::PlayOne,
        2 => TapPlan::Queue,
        3 => TapPlan::PlayNext,
        _ => TapPlan::PlayList,
    }
}

/// What a tap on a song does, as the settings say; while songs are selected, a tap selects.
#[uniffi::export]
pub fn tap_plan(selecting: bool) -> TapPlan {
    tap(selecting, crate::settings_store::with_prefs(|p| p.tap_action).unwrap_or(0))
}

/// How to shuffle a list.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum ShufflePlan {
    /// Nothing to play.
    Empty,
    /// The player's own random order.
    PlayerShuffle,
    /// This order, played as it is: artists and albums spread apart.
    Order { songs: Vec<Song> },
}

/// Spreads artists and albums apart unless the user prefers a plain random order (weighted shuffle off
/// in the settings).
#[uniffi::export]
pub fn shuffle_plan(songs: Vec<Song>) -> ShufflePlan {
    let weighted = crate::settings_store::with_prefs(|p| p.weighted_shuffle).unwrap_or(true);
    plan_shuffle(songs, weighted, seed_now())
}

/// Two songs cannot be spread, so they get the plain shuffle too.
fn plan_shuffle(songs: Vec<Song>, weighted: bool, seed: u64) -> ShufflePlan {
    if songs.is_empty() {
        ShufflePlan::Empty
    } else if weighted && songs.len() > 2 {
        ShufflePlan::Order { songs: mixes::weighted_shuffle(songs, seed) }
    } else {
        ShufflePlan::PlayerShuffle
    }
}

/// A radio started from `seed`: the song itself, then what the server finds similar to it. None when it
/// finds nothing but the song itself; the radio then goes on with random songs of the same genre
/// ([radio_fallback]).
fn radio_queue(seed: Song, similar: Vec<Song>) -> Option<Vec<Song>> {
    let rest: Vec<Song> = similar.into_iter().filter(|s| s.id != seed.id).collect();
    if rest.is_empty() {
        return None;
    }
    Some(std::iter::once(seed).chain(rest).collect())
}

/// The radio when nothing similar was found: the song, then the server's random songs as they came.
fn radio_fallback(seed: Song, random: Vec<Song>) -> Vec<Song> {
    std::iter::once(seed).chain(random).collect()
}

#[uniffi::export]
impl Client {
    /// An endless-ish mix seeded from one song: [radio_queue], else [radio_fallback].
    pub async fn radio(&self, seed: Song) -> NetResult<Vec<Song>> {
        let similar = self.songs(Read::SimilarSongs { id: seed.id.clone(), count: RADIO }).await?;
        if let Some(queue) = radio_queue(seed.clone(), similar) {
            return Ok(queue);
        }
        let random = self.songs(Read::RandomSongs { size: RADIO, genre: seed.genre.clone() }).await?;
        Ok(radio_fallback(seed, random))
    }

    /// A mix around one song from the index and the listening history, or the radio when the index has
    /// nothing to go with it: the song is not indexed, or nothing indexed is near it and the mix would be
    /// the song alone.
    pub async fn instant_mix(&self, seed: Song) -> NetResult<Vec<Song>> {
        let mix = self.core.mix_instant(seed.id.clone(), INSTANT_MIX, seed_now())?;
        if mix.len() > 1 {
            return Ok(mix);
        }
        self.radio(seed).await
    }

    /// Every album of an artist, in order, as one list of songs. A provider's albums are left out:
    /// asking for them would make octo-fiesta download them. An album that cannot be read is skipped.
    pub async fn artist_songs(&self, albums: Vec<Album>) -> Vec<Song> {
        let mut out = Vec::new();
        for a in albums.into_iter().filter(|a| !a.is_external) {
            out.extend(self.songs(Read::AlbumSongs { id: a.id }).await.unwrap_or_default());
        }
        out
    }

    /// "Shuffle all": the server's random songs.
    pub async fn shuffle_all(&self) -> NetResult<Vec<Song>> {
        self.songs(Read::RandomSongs { size: SHUFFLE_ALL, genre: None }).await
    }
}

/// What the debug test bridge names: `album:<id>`, `song:<id>`, `search:<text>` (the first song found),
/// `downloaded:<n>` (the n-th finished download, from 0), or nothing it knows.
#[derive(Debug, Clone, PartialEq, uniffi::Enum)]
pub enum TestRef {
    Album { id: String },
    Song { id: String },
    Search { text: String },
    Downloaded { index: u32 },
    Nothing,
}

#[uniffi::export]
pub fn test_ref(text: String) -> TestRef {
    let Some((kind, arg)) = text.split_once(':') else { return TestRef::Nothing };
    let arg = arg.to_string();
    match kind {
        "album" => TestRef::Album { id: arg },
        "song" => TestRef::Song { id: arg },
        "search" => TestRef::Search { text: arg },
        "downloaded" => TestRef::Downloaded { index: arg.parse().unwrap_or(0) },
        _ => TestRef::Nothing,
    }
}

/// An M3U file matched against the index: the songs for the new playlist, in the file's order, and the
/// message to show once it exists (or, with no songs, instead of creating it).
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct M3uImport {
    pub song_ids: Vec<String>,
    pub message: String,
}

#[uniffi::export]
impl Core {
    /// Tracks that are not in the index are reported, not guessed.
    pub fn m3u_import(&self, name: String, text: String) -> Result<M3uImport> {
        let matched = self.m3u_match(m3u::m3u_parse(text))?;
        let song_ids: Vec<String> = matched.iter().flatten().map(|s| s.id.clone()).collect();
        Ok(M3uImport { message: words::m3u_imported(song_ids.len(), matched.len(), &name), song_ids })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{block, client};
    use crate::client::NetProfile;
    use crate::{db, history::tests::song};

    #[test]
    fn a_tap_does_what_the_settings_say_unless_songs_are_selected() {
        assert_eq!([0, 1, 2, 3, 9].map(|a| tap(false, a)), [TapPlan::PlayList, TapPlan::PlayOne, TapPlan::Queue, TapPlan::PlayNext, TapPlan::PlayList]);
        assert_eq!(tap(true, 2), TapPlan::Select);
    }

    #[test]
    fn shuffle_spreads_only_what_can_be_spread() {
        let l: Vec<Song> = (0..6).map(|i| song(&i.to_string(), "t", &format!("A{}", i % 2), "b", "", 0)).collect();
        assert_eq!(plan_shuffle(vec![], true, 1), ShufflePlan::Empty);
        assert_eq!(plan_shuffle(l[..2].to_vec(), true, 1), ShufflePlan::PlayerShuffle);
        assert_eq!(plan_shuffle(l.clone(), false, 1), ShufflePlan::PlayerShuffle);
        assert_eq!(plan_shuffle(l.clone(), true, 9), ShufflePlan::Order { songs: mixes::weighted_shuffle(l, 9) });
    }

    #[test]
    fn radio_starts_with_its_song_once() {
        let (a, b, c) = (song("a", "t", "x", "y", "", 0), song("b", "t", "x", "y", "", 0), song("c", "t", "x", "y", "", 0));
        assert_eq!(radio_queue(a.clone(), vec![a.clone()]), None);
        assert_eq!(radio_queue(a.clone(), vec![b.clone(), a.clone(), c.clone()]).unwrap(), [a.clone(), b.clone(), c.clone()]);
        assert_eq!(radio_fallback(a.clone(), vec![a.clone(), b.clone()]), [a.clone(), a, b]);
    }

    fn songs_json(key: &str, ids: &[&str]) -> String {
        let s: Vec<String> = ids.iter().map(|i| format!(r#"{{"id":"{i}","title":"{i}","isDir":false}}"#)).collect();
        format!(r#"{{"subsonic-response":{{"status":"ok","{key}":{{"song":[{}]}}}}}}"#, s.join(","))
    }

    #[test]
    fn a_radio_with_nothing_similar_goes_on_with_random_songs() {
        let (c, fake) = client(NetProfile { url: "h".into(), ..Default::default() });
        let seed = Song { id: "r".into(), genre: Some("Jazz".into()), ..Default::default() };
        fake.answer(&songs_json("similarSongs2", &["r"]));
        fake.answer(&songs_json("randomSongs", &["x", "y"]));
        let got = block(c.radio(seed.clone())).unwrap();
        assert_eq!(got.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(), ["r", "x", "y"]);
        let asked = fake.asked();
        assert!(asked[1].contains("getRandomSongs") && asked[1].contains("genre=Jazz"), "{asked:?}");
        // An instant mix the index cannot draw is the radio: the song is indexed now (what the server
        // answers is), but nothing near it is.
        fake.answer(&songs_json("similarSongs2", &["s"]));
        assert_eq!(block(c.instant_mix(seed)).unwrap().len(), 2);
    }

    #[test]
    fn an_artist_plays_its_own_albums_only() {
        let (c, fake) = client(NetProfile { url: "h".into(), ..Default::default() });
        let album = |id: &str, is_external| Album { id: id.into(), is_external, ..Default::default() };
        fake.answer(r#"{"subsonic-response":{"status":"ok","album":{"id":"a1","name":"a1","song":[{"id":"1","title":"1","isDir":false}]}}}"#);
        let got = block(c.artist_songs(vec![album("a1", false), album("ext-2", true)]));
        assert_eq!(got.len(), 1);
        assert_eq!(fake.asked().len(), 1, "a provider's album is never asked for");
    }

    #[test]
    fn test_refs() {
        assert_eq!(test_ref("album:a:1".into()), TestRef::Album { id: "a:1".into() });
        assert_eq!(test_ref("search:dogs".into()), TestRef::Search { text: "dogs".into() });
        assert_eq!(test_ref("downloaded:x".into()), TestRef::Downloaded { index: 0 });
        assert_eq!(test_ref("downloaded:2".into()), TestRef::Downloaded { index: 2 });
        assert_eq!(test_ref("song".into()), TestRef::Nothing);
    }

    #[test]
    fn m3u_import_counts_what_it_found() {
        let core = Core::new(String::new(), "t".into()).unwrap();
        db::index(&mut core.db.lock(), &[], &[], &[song("1", "Dogs", "Pink Floyd", "Animals", "", 1977)]).unwrap();
        let text = "#EXTM3U\n#EXTINF:200,Pink Floyd - Dogs\na.flac\n#EXTINF:100,Nobody - Nothing\nb.flac\n";
        let r = core.m3u_import("Road".into(), text.into()).unwrap();
        assert_eq!(r.song_ids, ["1"]);
        assert_eq!(r.message, "Imported 1 of 2 tracks into Road");
        let none = core.m3u_import("Road".into(), "#EXTM3U\n#EXTINF:1,X - Y\nc.mp3\n".into()).unwrap();
        assert!(none.song_ids.is_empty());
        assert!(none.message.starts_with("None of the 1 entries"));
    }
}
