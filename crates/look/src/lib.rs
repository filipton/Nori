//! How Nori looks, independent of any platform: the colours a page takes from its cover (the page, the
//! text on it, an accent, the colour the sleeve melts from and a blurred wash of the cover to lay under
//! it) and the tones a theme builds from one colour. A platform hands in pixels and gets colours back,
//! so every app built on this dresses the same record the same way.
//!
//! Colours are `u32` ARGB throughout, as Android and most image libraries hold them.

pub mod color;
pub mod cover;
pub mod palette;
mod random;
pub mod theme;
