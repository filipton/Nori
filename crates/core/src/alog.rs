//! Logging to Android's log (logcat, tag `nori`) straight from Rust, so the core can say what it is
//! doing from any thread without calling back into Kotlin. Elsewhere (tests, a desktop app) it goes to
//! stderr. Only for events - never per buffer - since each line is formatted and copied once.

#[cfg(target_os = "android")]
mod sys {
    use std::ffi::{c_char, c_int};
    #[link(name = "log")]
    extern "C" {
        pub fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }
}

/// One line at INFO under the app's tag.
pub fn info(message: &str) {
    #[cfg(target_os = "android")]
    {
        const INFO: std::ffi::c_int = 4;
        // Interior NULs would end the line early; there are none in what the core logs, but never panic.
        if let Ok(text) = std::ffi::CString::new(message) {
            unsafe { sys::__android_log_write(INFO, c"nori".as_ptr(), text.as_ptr()) };
        }
    }
    #[cfg(not(target_os = "android"))]
    eprintln!("nori: {message}");
}
