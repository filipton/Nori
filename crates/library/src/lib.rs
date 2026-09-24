//! The music library as the app shows it, over the app's database: play history and the taste model
//! (history.rs), mixes and the "For you" row (mixes.rs, mixes/board.rs), smart playlists (smart.rs),
//! M3U in and out (m3u.rs), the browsing screens and search (browse.rs, search.rs), this session's stars
//! laid over what the server sent (stars.rs), how each page is laid out (pages.rs, rows.rs), what a song's
//! menu offers (menus.rs), the car's browse tree (car.rs) and the repository's small decisions
//! (library.rs). The calls that go through the core's database handle or the client are the core's.

pub mod browse;
pub mod car;
pub mod history;
pub mod library;
pub mod m3u;
pub mod menus;
pub mod mixes;
pub mod pages;
pub mod rows;
pub mod search;
pub mod smart;
pub mod stars;
