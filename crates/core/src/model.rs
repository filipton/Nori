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

pub(crate) fn id_string<'de, D: Deserializer<'de>>(d: D) -> Result<String, D::Error> {
    id(d)
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
#[serde(default)]
pub struct ArtistRef {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub name: String,
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
    /// Every credited artist (OpenSubsonic); `artist` stays the display string.
    pub artists: Vec<ArtistRef>,
    /// When the server first saw the file.
    pub created: Option<String>,
    /// Server-side play count and last play, across all clients.
    pub play_count: u32,
    pub played: Option<String>,
    pub path: Option<String>,
    /// "explicit", "clean" or empty.
    pub explicit_status: String,
    pub channel_count: u32,
    pub music_brainz_id: Option<String>,
    pub bpm: u32,
    pub comment: Option<String>,
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
    /// OpenSubsonic: "Album", "EP", "Single", "Compilation", "Live", ... Empty on older servers.
    pub release_types: Vec<String>,
    pub is_compilation: bool,
    pub user_rating: u8,
    pub play_count: u32,
    pub created: Option<String>,
    pub explicit_status: String,
    pub music_brainz_id: Option<String>,
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
    /// Names of discs that have one (OpenSubsonic `discTitles`).
    pub disc_titles: Vec<DiscTitle>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct DiscTitle {
    pub disc: u32,
    pub title: String,
}

/// One level of the server's folder tree.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct Directory {
    pub id: String,
    pub name: String,
    pub folders: Vec<Artist>,
    pub songs: Vec<Song>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ArtistDetail {
    pub artist: Artist,
    pub albums: Vec<Album>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct ArtistInfo {
    pub last_fm_url: Option<String>,
    pub music_brainz_id: Option<String>,
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

/// One word (or syllable) of a lyric line and when it is sung. `start`/`end` index the line's text in UTF-16 units.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct LyricWord {
    pub start_ms: i64,
    pub end_ms: i64,
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct LyricLine {
    /// Milliseconds from track start; -1 when the lyrics are unsynced.
    pub start_ms: i64,
    pub end_ms: i64,
    pub text: String,
    /// Empty for unsynced lyrics. See [Lyrics::word_timed] for whether these are real or estimated.
    pub words: Vec<LyricWord>,
    pub translation: Option<String>,
    /// A backing-vocal line, where the server says so.
    pub background: bool,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct Lyrics {
    pub synced: bool,
    /// True when the word timing came from the server; false when it was spread over each line by the core.
    pub word_timed: bool,
    pub lines: Vec<LyricLine>,
}

#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct PlayQueue {
    pub songs: Vec<Song>,
    pub index: u32,
    pub position_ms: u64,
}

/// The order is the wire format: `dsp.rs` reads the ordinal out of the flat band array, so only append.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Enum)]
pub enum EqKind {
    Peaking,
    LowShelf,
    HighShelf,
    LowPass,
    HighPass,
    BandPass,
    Notch,
    AllPass,
    /// Shelves whose `q` is the RBJ slope S (1 is the steepest slope without ripple) rather than a Q.
    LowShelfSlope,
    HighShelfSlope,
}

#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct EqBand {
    pub kind: EqKind,
    pub freq: f32,
    pub gain_db: f32,
    pub q: f32,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct EqPreset {
    pub preamp_db: f32,
    pub bands: Vec<EqBand>,
}

/// One of the built-in curves from `dsp::eq_presets`.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct NamedPreset {
    pub name: String,
    pub preamp_db: f32,
    pub bands: Vec<EqBand>,
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

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, uniffi::Record)]
#[serde(default)]
pub struct MusicFolder {
    #[serde(deserialize_with = "id")]
    pub id: String,
    pub name: String,
}

// ---- play history, listening stats, smart playlists, m3u ----

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct HistoryEntry {
    pub song: Song,
    pub started_ms: i64,
    pub heard_ms: i64,
    pub completed: bool,
    pub skipped: bool,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct SongStat {
    pub song_id: String,
    /// Listens that were not skips.
    pub plays: u32,
    pub skips: u32,
    /// 0 when the song was only ever skipped.
    pub last_played_ms: i64,
    pub heard_ms_total: i64,
    /// The taste score as of now; see `history.rs`. Around 1 per recent full listen.
    pub taste: f64,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct TopSong {
    pub song: Song,
    pub plays: u32,
    pub listened_ms: i64,
}

/// An artist, album or genre in a top list. `id` is empty for genres and for artists the server gave no id.
#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct TopEntry {
    pub id: String,
    pub name: String,
    /// Cover of the most played song of the entry.
    pub cover_art: Option<String>,
    pub plays: u32,
    pub listened_ms: i64,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct ListeningStats {
    pub plays: u32,
    pub skips: u32,
    /// Everything heard, skipped plays included.
    pub listened_ms: i64,
    pub distinct_songs: u32,
    pub distinct_artists: u32,
    pub distinct_albums: u32,
    pub top_songs: Vec<TopSong>,
    pub top_artists: Vec<TopEntry>,
    pub top_albums: Vec<TopEntry>,
    pub top_genres: Vec<TopEntry>,
    /// 24 entries, local hour of day.
    pub plays_per_hour: Vec<u32>,
    /// 7 entries, Monday first.
    pub plays_per_weekday: Vec<u32>,
    pub active_days: u32,
    pub longest_streak_days: u32,
    pub first_play: Option<HistoryEntry>,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct SmartPlaylist {
    pub id: String,
    pub name: String,
    /// The definition; schema in `smart.rs`.
    pub json: String,
}

#[derive(Debug, Clone, Default, PartialEq, uniffi::Record)]
pub struct M3uEntry {
    /// -1 when the playlist does not say.
    pub duration_s: i32,
    pub artist: String,
    pub title: String,
    pub path: String,
}
