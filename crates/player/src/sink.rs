//! The platform's side of the transition engine's formats: which streams it hands over as samples, and
//! how long it keeps the formats it named by token (the engine asks for one back when it sends that
//! format downstream).

/// 16-bit PCM, as media3 and the engine number encodings.
pub const PCM_16: i32 = 2;
/// 32-bit float PCM.
pub const PCM_FLOAT: i32 = 4;

/// How a stream's encoding is named to the engine: 16-bit or float PCM as it is, anything else - a
/// compressed stream passed through, 24-bit PCM - as 0, not samples, which the engine never mixes.
///
/// Twin of the encoding in `TransitionSink.configure` (core/.../playback/TransitionSink.kt).
pub fn sample_encoding(raw_pcm: bool, encoding: i32) -> i32 {
    if raw_pcm && (encoding == PCM_16 || encoding == PCM_FLOAT) { encoding } else { 0 }
}

/// How many formats before the newest the platform keeps: only a few are ever live (the one below, a
/// pending one, those staged ahead).
pub const KEEP_FORMATS: i32 = 16;

/// With `kept` formats held and `token` just handed out, the tokens below which the held formats are let
/// go, or none while there are few enough.
///
/// Twin of the pruning in `TransitionSink.configure` (core/.../playback/TransitionSink.kt).
pub fn formats_below(kept: usize, token: i32) -> Option<i32> {
    (kept > KEEP_FORMATS as usize).then(|| token - KEEP_FORMATS)
}
