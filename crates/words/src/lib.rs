//! What the app says: its confirmations, labels and every sentence on screen (words.rs), and what it
//! writes about numbers - times, decibels, frequencies, sizes, captions (fmt.rs). Each is worked out here
//! once, so every front end says the same. The few words that also read the app's state (the settings,
//! the queue playing) are the core's.

pub mod fmt;
pub mod words;
