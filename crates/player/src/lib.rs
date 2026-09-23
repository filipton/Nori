//! How Nori plays music, independent of any platform: the sound chain (equalizer, crossfeed,
//! limiter), AutoMix (analysis, planning, per-sample mixing, time-stretch, resampling) and the
//! values they share. Nothing here decodes, fetches, stores or talks to an OS: a platform hands in
//! PCM and settings and gets PCM and decisions back, so Android and a desktop app sound the same.

pub mod automix;
pub mod burst;
pub mod dac;
pub mod device;
pub mod dsp;
pub mod engine;
pub mod outputs;
pub mod heard;
pub mod pcm;
pub mod seek;
pub mod playlist;
pub mod policy;
pub mod queue;
pub mod silence;
pub mod sound;
pub mod sonic;
pub mod speed;
pub mod transitions;
pub mod transport;
pub mod types;

#[cfg(test)]
mod no_alloc;
