//! The engine's tests on the virtual clock, in one test binary so that they run side by side instead of
//! one binary after another (Cargo.toml lists the test targets). Each file stays a test file of its own
//! with its own helpers, and `cargo test -p nori-engine --test engine <name>` picks any of them.

#[path = "engine.rs"]
mod engine;
#[path = "paths.rs"]
mod paths;
#[path = "radio.rs"]
mod radio;
#[path = "tempo.rs"]
mod tempo;
#[path = "estimated.rs"]
mod estimated;
