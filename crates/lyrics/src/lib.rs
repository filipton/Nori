//! Lyrics in one shape, whatever the server or a third party had (lyrics.rs), read from every format the
//! lyrics services answer in (formats.rs, json.rs, html.rs), the services themselves (services.rs, with
//! LRCLIB's own in lrclib.rs), asked together and ranked when the server has no timed lyrics (race.rs),
//! and the lyrics page's clock asked every frame (look.rs). The requests go out through the core's
//! Transport and the answers are kept in the core's response cache, both handed in by the core.

pub mod formats;
pub mod html;
pub mod json;
pub mod look;
pub mod lrclib;
pub mod lyrics;
pub mod race;
pub mod services;
