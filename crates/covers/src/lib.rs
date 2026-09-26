//! Cover art for a client: covers fetched through the core's
//! [`Transport`](nori_core::transport::Transport) at the addresses the core builds ([`cover_url_into`]),
//! kept on disk under a size limit ([`DiskCache`]), decoded straight to the size drawn ([`Decoder`]:
//! JPEG, PNG, WebP and a GIF's first frame, in pure Rust, turned the way their EXIF says) and kept
//! decoded in memory ([`MemoryCache`]). [`Loader`] puts them together behind requests a GUI makes per
//! view, shared and cancellable, on a few worker threads.
//!
//! What a cover is decoded into is the client's ([`Paint`]): a desktop or terminal client takes RGBA rows
//! ([`Rgba`]); the Android app has each decoded straight into a Bitmap (crates/android covers.rs) and
//! keeps the Bitmaps itself (docs/clients.md, "Pictures").

pub mod decode;
pub mod disk;
pub mod loader;
pub mod memory;
pub mod scale;

pub use decode::{format, header, Decoder, Error as DecodeError, Format, Header};
pub use disk::{DiskCache, Key};
pub use loader::{Config, Error, Loader, Paint, Rgba, Ticket};
pub use memory::{Image, MemoryCache, Sized};
pub use nori_core::covers::{cover_url_into, is_provider_cover};
pub use scale::{Alpha, Target};
