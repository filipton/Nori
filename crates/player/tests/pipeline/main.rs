//! The player end to end, on a simulated output and a virtual clock (`nori_player::sim`): decoder,
//! transition engine fed in bursts, sound chain, and what the ear gets, asserted sample by sample.
//! These are the checks `tools/audio-e2e.sh` makes on an emulator, where the Rust decides the answer.

mod automix;
mod chain;
mod common;
mod controls;
mod crossfade;
mod gain;
mod gapless;
mod golden;
mod levels;
mod output;
mod stages;
