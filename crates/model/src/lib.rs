//! The shapes every part of the core shares: the library's records (model.rs), which deserialize the
//! server's JSON, cross the FFI and are stored in the index, with the few lines of words they carry about
//! themselves (lines.rs); the error every call into the core may end in; and the core's log (alog.rs).

pub mod alog;
pub mod lines;
pub mod model;

pub use model::*;

// Kotlin's exception carries no message (uniffi's JNI bindings give none), so its `toString` is this Display.
#[derive(Debug, thiserror::Error)]
#[cfg_attr(feature = "ffi", derive(uniffi::Error), uniffi::export(Display))]
pub enum CoreError {
    #[error("{reason}")]
    Api { code: i32, reason: String },
    #[error("bad response: {reason}")]
    Parse { reason: String },
    #[error("database: {reason}")]
    Db { reason: String },
}

impl From<rusqlite::Error> for CoreError {
    fn from(e: rusqlite::Error) -> Self {
        CoreError::Db { reason: e.to_string() }
    }
}

pub type Result<T> = std::result::Result<T, CoreError>;
