//! Talking to a Subsonic server, below the client: request signing and addresses (api.rs), the one door
//! to the network a platform implements and what its failures mean (transport.rs), what the client asks
//! for as plain values - the profile it acts on, the writes and what they make stale (requests.rs) - and
//! the audio's cache keys and the network the phone is on (stream.rs). The client, which holds the core's
//! database and caches, is the core's.

pub mod api;
pub mod requests;
pub mod stream;
pub mod transport;
