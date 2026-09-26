//! What is kept on the device: downloads as they run - their progress, speed, notification and screen,
//! and which songs are downloaded (transfers.rs) - and which streamed songs leave the stream cache first
//! (stream_cache.rs). The platform moves the bytes; the core's calls over its downloads table are the
//! core's.

pub mod stream_cache;
pub mod transfers;
