//! Which streams the core's decoder (`nori_player::decode`) takes from the platform's player, and which
//! it leaves to the platform's own decoder.

use nori_player::decode::Codec;

/// Whether an AAC stream announced as Low Complexity at `rate` may be HE-AAC whose SBR (and PS) is
/// signalled only inside the stream: an ADTS stream, as internet radio sends AAC+, can only say AAC-LC
/// at the core's rate (half the rate it plays at). At 24 kHz and below that is what it almost always
/// is, so such a stream is treated as HE-AAC. Decoded here, only its core would be heard (nothing above
/// a quarter of the core's rate: muffled); handed to the audio chip, the chip would be set up for AAC-LC
/// at half the real rate. The platform's own decoder finds the SBR and says the real rate.
pub fn implicit_sbr(mime: &str, codecs: Option<&str>, rate: i32) -> bool {
    Codec::from_mime(mime) == Some(Codec::Aac) && codecs.is_none_or(|c| c.eq_ignore_ascii_case("mp4a.40.2")) && rate > 0 && rate <= 24_000
}

/// Whether this decoder takes a stream of `mime` (with the RFC 6381 `codecs` string, when known) at
/// `rate` Hz (0 unknown), or leaves it to the platform's own: what it cannot decode (Opus, HE-AAC,
/// including HE-AAC that says it is AAC-LC, see [`implicit_sbr`]), and anything the output would rather
/// hand to the audio chip whole - offload is cheaper still than decoding here.
pub fn takes(mime: &str, codecs: Option<&str>, channels: i32, rate: i32, chip_takes_it: bool) -> Option<Codec> {
    let codec = Codec::from_mime(mime)?;
    if implicit_sbr(mime, codecs, rate) {
        return None;
    }
    // Opus: mono and stereo; surround Opus is laid out as several streams, which is the platform's.
    if codec == Codec::Opus && !(1..=2).contains(&channels) {
        return None;
    }
    // AAC: only Low Complexity; SBR and PS (HE-AAC, HE-AACv2) are the platform's.
    if codec == Codec::Aac && !codecs.is_some_and(|c| c.eq_ignore_ascii_case("mp4a.40.2")) {
        return None;
    }
    if chip_takes_it && crate::dsp::offload_wanted() {
        return None;
    }
    Some(codec)
}

/// Whether this decoder takes a stream split by the platform's own extractor (Android's MediaExtractor,
/// which measures tracks ahead) rather than media3's. Their setup data is laid out alike for MP3 (none),
/// AAC (the AudioSpecificConfig), Vorbis (identification header, then setup header) and Opus (OpusHead,
/// then pre-skip and pre-roll); FLAC and ALAC come in other shapes (Android's FLAC extractor hands out
/// PCM outright), so those stay with the platform's decoder.
pub fn takes_extracted(mime: &str, codecs: Option<&str>, channels: i32) -> Option<Codec> {
    // Measured ahead, the core of an HE-AAC stream is as good as the whole: its beats are all there.
    takes(mime, codecs, channels, 0, false).filter(|c| matches!(c, Codec::Mp3 | Codec::Aac | Codec::Vorbis | Codec::Opus))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_it_decodes_and_not_what_the_chip_would_take() {
        assert_eq!(takes("audio/mpeg", None, 2, 44_100, false), Some(Codec::Mp3));
        assert_eq!(takes("audio/opus", None, 2, 44_100, false), Some(Codec::Opus));
        assert_eq!(takes("audio/opus", None, 6, 44_100, false), None, "surround Opus is the platform's");
        assert_eq!(takes("audio/ac3", None, 2, 44_100, false), None);
        assert_eq!(takes("audio/mp4a-latm", Some("mp4a.40.2"), 2, 44_100, false), Some(Codec::Aac));
        assert_eq!(takes("audio/mp4a-latm", Some("mp4a.40.5"), 2, 44_100, false), None, "HE-AAC is the platform's");
        assert_eq!(takes("audio/mp4a-latm", None, 2, 44_100, false), None, "an AAC of unknown profile too");
    }

    #[test]
    fn aac_lc_at_a_core_rate_is_taken_for_he_aac() {
        // SomaFM's AAC+ streams: ADTS saying AAC-LC, 22.05 kHz, stereo; they play at 44.1 kHz.
        assert!(implicit_sbr("audio/mp4a-latm", Some("mp4a.40.2"), 22_050));
        assert!(implicit_sbr("audio/mp4a-latm", Some("mp4a.40.2"), 24_000));
        assert!(!implicit_sbr("audio/mp4a-latm", Some("mp4a.40.2"), 44_100));
        assert!(!implicit_sbr("audio/mp4a-latm", Some("mp4a.40.2"), 0), "unknown: as it says");
        assert!(!implicit_sbr("audio/mp4a-latm", Some("mp4a.40.5"), 22_050), "HE-AAC that says so is not taken anyway");
        assert!(!implicit_sbr("audio/mpeg", None, 22_050), "an MP3 at 22.05 kHz is just that");
        assert_eq!(takes("audio/mp4a-latm", Some("mp4a.40.2"), 2, 22_050, false), None, "the platform's decoder plays its SBR");
        assert_eq!(takes("audio/mp4a-latm", Some("mp4a.40.2"), 2, 48_000, false), Some(Codec::Aac));
        assert_eq!(takes("audio/mpeg", None, 1, 22_050, false), Some(Codec::Mp3));
        assert_eq!(takes_extracted("audio/mp4a-latm", Some("mp4a.40.2"), 2), Some(Codec::Aac), "measured ahead, the core will do");
    }

    #[test]
    fn a_track_measured_ahead_is_decoded_here_where_its_setup_reads_the_same() {
        assert_eq!(takes_extracted("audio/mpeg", None, 2), Some(Codec::Mp3));
        assert_eq!(takes_extracted("audio/opus", None, 2), Some(Codec::Opus));
        assert_eq!(takes_extracted("audio/mp4a-latm", Some("mp4a.40.2"), 2), Some(Codec::Aac));
        assert_eq!(takes_extracted("audio/flac", None, 2), None);
        assert_eq!(takes_extracted("audio/alac", None, 2), None);
        assert_eq!(takes_extracted("audio/raw", None, 2), None);
    }
}
