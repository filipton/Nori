//! Cover art for a client without an image loader of its own (a desktop or terminal client): covers
//! fetched through the core's [`Transport`](norimusic::transport::Transport) at the addresses the core
//! builds ([`cover_url_into`]), kept on disk under a size limit ([`DiskCache`]) and decoded in memory
//! ([`MemoryCache`]), and decoded straight to the size drawn ([`Decoder`]): JPEG, PNG and WebP, in pure
//! Rust, into RGBA the caller's buffer holds. [`Loader`] puts them together behind requests a GUI makes
//! per view, shared and cancellable, on a few worker threads.
//!
//! The Android app keeps Coil for the rest and decodes its covers with this [`Decoder`], sized from
//! [`header`] (docs/clients.md, "Pictures").

pub mod decode;
pub mod disk;
pub mod loader;
pub mod memory;
pub mod scale;

pub use decode::{format, header, Decoder, Error as DecodeError, Format, Header};
pub use disk::{DiskCache, Key};
pub use loader::{Config, Error, Loader, Ticket};
pub use memory::{Image, MemoryCache, Sized};
pub use norimusic::covers::{cover_url_into, is_provider_cover};
pub use scale::{Alpha, Target};
