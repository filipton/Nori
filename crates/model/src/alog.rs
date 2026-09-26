//! Logging to Android's log (logcat, tag `nori`) straight from Rust, so the core can say what it is
//! doing from any thread without calling back into Kotlin. Elsewhere (tests, a desktop app) it goes to
//! stderr. Only for events - never per buffer - since each line is formatted and copied once.
//!
//! The last [`KEPT`] lines are also kept in memory, each with the wall clock it was said at, whatever
//! logcat does with them: logcat's buffer is shared with the whole system and a codec's chatter turns it
//! over in minutes, so a report written after something broke would no longer find what the app said as
//! it broke. The perf build's invariant watch takes a copy of them the moment one breaks ([`recent`]).
//! Keeping one costs a lock and the line's copy, on a line that was formatted anyway.

use std::collections::VecDeque;
use std::sync::Mutex;

#[cfg(target_os = "android")]
mod sys {
    use std::ffi::{c_char, c_int};
    #[link(name = "log")]
    extern "C" {
        pub fn __android_log_write(prio: c_int, tag: *const c_char, text: *const c_char) -> c_int;
    }
}

/// How many of the latest lines are kept in memory.
pub const KEPT: usize = 500;

/// The latest lines, oldest first, each with when it was said (wall clock ms).
static LINES: Mutex<VecDeque<(i64, String)>> = Mutex::new(VecDeque::new());

fn wall_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

/// A line kept in memory only, as said elsewhere (the app's own Kotlin, which writes to logcat itself).
pub fn keep(message: &str) {
    let line = (wall_ms(), message.to_string());
    // A panic while it was held leaves the lines as they were: the log must never stop the app.
    let mut kept = LINES.lock().unwrap_or_else(|e| e.into_inner());
    if kept.len() >= KEPT {
        kept.pop_front();
    }
    kept.push_back(line);
}

/// The lines kept, oldest first, with when each was said (wall clock ms).
pub fn recent() -> Vec<(i64, String)> {
    LINES.lock().unwrap_or_else(|e| e.into_inner()).iter().cloned().collect()
}

/// One line at INFO under the app's tag.
pub fn info(message: &str) {
    keep(message);
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

/// One line the app's Kotlin wrote to logcat under the `nori` tag, kept with the core's own so that a
/// copy of the latest lines has both.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn alog_keep(line: String) {
    keep(&line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_latest_lines_are_kept_in_order_and_no_more_than_kept() {
        for k in 0..KEPT + 20 {
            info(&format!("alog-test line {k}"));
        }
        let mine: Vec<String> = recent().into_iter().map(|(_, l)| l).filter(|l| l.starts_with("alog-test line ")).collect();
        assert!(mine.len() <= KEPT);
        assert_eq!(mine.last().map(String::as_str), Some(format!("alog-test line {}", KEPT + 19).as_str()));
        // In order, and the oldest gone first.
        let numbers: Vec<usize> = mine.iter().map(|l| l.rsplit(' ').next().unwrap().parse().unwrap()).collect();
        assert!(numbers.windows(2).all(|w| w[1] == w[0] + 1), "in order: {numbers:?}");
        assert!(!mine.contains(&"alog-test line 0".to_string()), "the oldest went first");
    }
}
