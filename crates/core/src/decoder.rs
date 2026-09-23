//! Which streams the core's decoder (`nori_player::decode`) takes from the platform's player, and which
//! it leaves to the platform's own decoder.

use nori_player::decode::Codec;

/// Whether this decoder takes a stream of `mime` (with the RFC 6381 `codecs` string, when known), or
/// leaves it to the platform's own: what it cannot decode (Opus, HE-AAC), and anything the output
/// would rather hand to the audio chip whole - offload is cheaper still than decoding here.
pub fn takes(mime: &str, codecs: Option<&str>, channels: i32, chip_takes_it: bool) -> Option<Codec> {
    let codec = Codec::from_mime(mime)?;
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
    takes(mime, codecs, channels, false).filter(|c| matches!(c, Codec::Mp3 | Codec::Aac | Codec::Vorbis | Codec::Opus))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_what_it_decodes_and_not_what_the_chip_would_take() {
        assert_eq!(takes("audio/mpeg", None, 2, false), Some(Codec::Mp3));
        assert_eq!(takes("audio/opus", None, 2, false), Some(Codec::Opus));
        assert_eq!(takes("audio/opus", None, 6, false), None, "surround Opus is the platform's");
        assert_eq!(takes("audio/ac3", None, 2, false), None);
        assert_eq!(takes("audio/mp4a-latm", Some("mp4a.40.2"), 2, false), Some(Codec::Aac));
        assert_eq!(takes("audio/mp4a-latm", Some("mp4a.40.5"), 2, false), None, "HE-AAC is the platform's");
        assert_eq!(takes("audio/mp4a-latm", None, 2, false), None, "an AAC of unknown profile too");
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
