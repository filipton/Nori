//! What a demuxer hands the decoder besides the packets themselves: the codec's setup data, the RFC 6381
//! codec string an AAC stream is known by, and how large a packet buffer to keep. The decoder
//! (`decode.rs`) takes these as they are; a platform's demuxer names them in its own way.

/// Packet buffer for a stream whose demuxer does not say how large its packets get.
pub const DEFAULT_PACKET_BYTES: usize = 64 * 1024;
/// The least packet buffer made when measuring a song ahead, whatever the demuxer says.
pub const MIN_PACKET_BYTES: usize = 8 * 1024;

/// The codec's setup (Android's `csd-0`, `csd-1`, ... or media3's `initializationData`) as one block, the
/// parts joined in order; none when there are none. One part is taken as it is.
///
/// Twin of `setupData` in `AutoMixPrefetch` and in `RustAudioDecoder` (core/.../playback/AutoMixPrefetch.kt,
/// RustAudio.kt), which Android keeps: it runs once per stream on the platform's own objects.
pub fn join_setup<'a>(parts: impl IntoIterator<Item = &'a [u8]>) -> Option<Vec<u8>> {
    let mut all: Option<Vec<u8>> = None;
    for p in parts {
        all.get_or_insert_with(Vec::new).extend_from_slice(p);
    }
    all
}

/// The RFC 6381 codec string of an AAC stream a demuxer names only by its profile (2 is Low Complexity):
/// what `decode::takes` goes by.
///
/// Twin of the `codecs` in `AutoMixPrefetch.inCore` (core/.../playback/AutoMixPrefetch.kt).
pub fn aac_codecs(profile: Option<i32>) -> Option<String> {
    profile.map(|p| format!("mp4a.40.{p}"))
}

/// The packet buffer to keep for a stream whose demuxer says its packets are at most `max_input` bytes
/// (none when it does not say): its word for playback, at least [`MIN_PACKET_BYTES`] when measuring
/// ahead (`measuring`), which grows the buffer itself if a bigger packet comes.
///
/// Twin of the buffers of `RustAudioDecoder` (`maxInputSize`, or [`DEFAULT_PACKET_BYTES`] for
/// `Format.NO_VALUE`) and `AutoMixPrefetch.inCore` (`KEY_MAX_INPUT_SIZE`) (core/.../playback/RustAudio.kt,
/// AutoMixPrefetch.kt).
pub fn packet_bytes(max_input: Option<i32>, measuring: bool) -> i32 {
    match max_input {
        Some(n) if measuring => n.max(MIN_PACKET_BYTES as i32),
        Some(n) => n,
        None => DEFAULT_PACKET_BYTES as i32,
    }
}

/// How large the packet buffer has to be for a packet of `packet` bytes, from `capacity`: only ever
/// grown, and to exactly the packet.
///
/// Twin of the buffer growth in `AutoMixPrefetch.inCore` (core/.../playback/AutoMixPrefetch.kt).
pub fn grown(capacity: i32, packet: i64) -> i32 {
    if packet > capacity as i64 { packet as i32 } else { capacity }
}
