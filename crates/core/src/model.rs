//! Wire + FFI models. The same structs deserialize Subsonic JSON, cross the
//! FFI as uniffi records and are stored as JSON in the local index.

use serde::{Deserialize, Deserializer, Serialize};

/// `starred` is a timestamp on the wire and a bool once stored.
fn flag<'de, D: Deserializer<'de>>(d: D) -> Result<bool, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum F {
        B(bool),
        S(String),
    }
    Ok(match Option::<F>::deserialize(d)? {
        Some(F::B(b)) => b,
        Some(F::S(s)) => !s.is_empty(),
        None => false,
    })
}

/// Some servers send numeric ids; ids are strings everywhere in the app.
fn opt_id<'de, D: Deserializer<'de>>(d: D) -> Result<Option<String>, D::Error> {
    #[derive(Deserialize)]
    #[serde(untagged)]
    enum I {
        S(String),
        N(i64),
    }
    Ok(Option::<I>::deserialize(d)?.map(|i| match i {
        I::S(s) => s,
        I::N(n) => n.to_string(),
    }))
}

fn id<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    Ok(opt_id(d)?.unwrap_or_default())
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default, rename_all = "camelCase")]
pub struct ReplayGain {
    pub track_gain: Option<f32>,
    pub album_gain: Option<f32>,
    pub track_peak: Option<f32>,
    pub album_peak: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default, rename_all = "camelCase")]
pub struct Song {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub title: String,
    pub album: String,
    pub artist: String,
    #[serde(deserialize_with = "opt_id")]
    pub album_id: Option<String>,
    #[serde(deserialize_with = "opt_id")]
    pub artist_id: Option<String>,
    #[serde(deserialize_with = "opt_id")]
    pub cover_art: Option<String>,
    pub duration: u32,
    pub track: u32,
    pub disc_number: u32,
    pub year: u32,
    pub genre: Option<String>,
    pub suffix: String,
    pub content_type: String,
    pub bit_rate: u32,
    pub size: u64,
    pub sampling_rate: u32,
    pub bit_depth: u32,
    pub user_rating: u8,
    #[serde(deserialize_with = "flag")]
    pub starred: bool,
    /// Set by octo-fiesta for provider items that are not in the library yet.
    pub is_external: bool,
    pub replay_gain: Option<ReplayGain>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default, rename_all = "camelCase")]
pub struct Album {
    #[serde(deserialize_with = "id")]
    pub id: String,
    #[serde(alias = "title", alias = "album")]
    pub name: String,
    pub artist: String,
    #[serde(deserialize_with = "opt_id")]
    pub artist_id: Option<String>,
    #[serde(deserialize_with = "opt_id")]
    pub cover_art: Option<String>,
    pub song_count: u32,
    pub duration: u32,
    pub year: u32,
    pub genre: Option<String>,
    #[serde(deserialize_with = "flag")]
    pub starred: bool,
    /// Set by octo-fiesta for provider items that are not in the library yet.
    pub is_external: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default, rename_all = "camelCase")]
pub struct Artist {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub name: String,
    #[serde(deserialize_with = "opt_id")]
    pub cover_art: Option<String>,
    pub artist_image_url: Option<String>,
    pub album_count: u32,
    #[serde(deserialize_with = "flag")]
    pub starred: bool,
    /// Set by octo-fiesta for provider items that are not in the library yet.
    pub is_external: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default, rename_all = "camelCase")]
pub struct Playlist {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub name: String,
    pub comment: Option<String>,
    pub owner: Option<String>,
    pub public: bool,
    pub song_count: u32,
    pub duration: u32,
    #[serde(deserialize_with = "opt_id")]
    pub cover_art: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default, rename_all = "camelCase")]
pub struct RadioStation {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub name: String,
    pub stream_url: String,
    pub home_page_url: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct Genre {
    #[serde(rename = "value")]
    pub name: String,
    #[serde(rename = "songCount")]
    pub song_count: u32,
    #[serde(rename = "albumCount")]
    pub album_count: u32,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct SearchResult {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub songs: Vec<Song>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct AlbumDetail {
    pub album: Album,
    pub songs: Vec<Song>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ArtistDetail {
    pub artist: Artist,
    pub albums: Vec<Album>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ArtistInfo {
    pub biography: Option<String>,
    pub image_url: Option<String>,
    pub similar: Vec<Artist>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct PlaylistDetail {
    pub playlist: Playlist,
    pub songs: Vec<Song>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct Starred {
    pub artists: Vec<Artist>,
    pub albums: Vec<Album>,
    pub songs: Vec<Song>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct LyricLine {
    /// Milliseconds from track start; -1 when the lyrics are unsynced.
    pub start_ms: i64,
    pub text: String,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct Lyrics {
    pub synced: bool,
    pub lines: Vec<LyricLine>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct PlayQueue {
    pub songs: Vec<Song>,
    pub index: u32,
    pub position_ms: u64,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct PendingScrobble {
    pub row_id: i64,
    pub song_id: String,
    pub time_ms: i64,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ServerInfo {
    pub version: String,
    pub server_type: String,
    pub server_version: String,
    pub open_subsonic: bool,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct IngestStats {
    pub artists: u32,
    pub albums: u32,
    pub songs: u32,
}
