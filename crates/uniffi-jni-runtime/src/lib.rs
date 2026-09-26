/* This Source Code Form is subject to the terms of the Mozilla Public
 * License, v. 2.0. If a copy of the MPL was not distributed with this
 * file, You can obtain one at http://mozilla.org/MPL/2.0/. */

//! uniffi's JNI runtime for Kotlin with the app's two Android changes (marked NORI): upstream's crate at the
//! revision the workspace pins, re-exported whole, with the three pieces the changes live in put in its place.
//! The generated scaffolding reaches this crate under upstream's name (crates/android's Cargo.toml).
//!
//! - caching.rs: upstream's, a class looked up through [`find_class`] instead of `FindClass`;
//! - attach.rs: `attach_current_thread` detaching a thread it attached when that thread ends, and
//!   `attach_for_life` for the app's own doors;
//! - loader.rs: the app's class loader, for classes looked up from threads Rust started.
//!
//! Upstream's own uses of its `CachedClass` (the `InternalException` it throws, `Throwable.getMessage`) run
//! on threads Java called in on, where `FindClass` finds the app's classes; they are left as they are.

pub use uniffi_bindgen_kotlin_jni_runtime::*;

mod attach;
mod caching;
mod loader;

pub use attach::{attach_current_thread, attach_for_life, attached_here};
pub use caching::{CachedClass, CachedMethod, CachedStaticMethod};
pub use loader::{find_class, remember_class_loader};
