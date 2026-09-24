//! Lyrics in one shape, whatever the server or a third party had (lyrics.rs), looked up on LRCLIB when the
//! server has none (lrclib.rs), and the lyrics page's clock asked every frame (look.rs). The lookups go
//! out through the core's client, which is the core's.

pub mod look;
pub mod lrclib;
pub mod lyrics;
