//! Where a song's audio comes from and under which name it is cached. Songs are resolved when they are
//! opened, not when they are queued, so the quality follows the network the phone is on at that moment.
//! Both caches are keyed by song id and quality, never by URL, so a replayed track costs no radio time.

use crate::client::Client;

/// A quality setting: `bit_rate` 0 and an empty `format` mean the original file.
#[derive(Debug, Clone, Default, uniffi::Record)]
pub struct StreamQuality {
    pub bit_rate: u32,
    pub format: String,
}

#[derive(Debug, Clone, uniffi::Record)]
pub struct StreamTarget {
    pub url: String,
    /// The cache key: `<id>:<bit rate><format>` in the rolling stream cache, `dl:<id>` for downloads.
    pub key: String,
}

/// A finished download's cache key.
#[uniffi::export]
pub fn download_key(id: String) -> String {
    format!("dl:{id}")
}

/// The streamed copies of `id` among `keys`, whatever quality they were fetched at. Keys are
/// `<id>:<quality>` and the quality never holds a colon, so this is exact.
#[uniffi::export]
pub fn stream_copies(id: String, keys: Vec<String>) -> Vec<String> {
    keys.into_iter().filter(|k| k.rsplit_once(':').is_some_and(|(before, _)| before == id)).collect()
}

impl Client {
    /// The quality to stream at: the metered or the Wi-Fi setting, and through the profile's second
    /// (usually public) address an optional ceiling on top, as opus unless a format was chosen.
    fn quality(&self, metered: bool, wifi: StreamQuality, mobile: StreamQuality) -> StreamQuality {
        let q = if metered { mobile } else { wifi };
        let cap = self.profile.read().alt_max_bit_rate;
        if self.on_second_address() && cap > 0 && (q.bit_rate == 0 || q.bit_rate > cap) {
            return StreamQuality { bit_rate: cap, format: if q.format.is_empty() { "opus".into() } else { q.format } };
        }
        q
    }
}

#[uniffi::export]
impl Client {
    /// The URL and cache key to stream `id` from now.
    pub fn stream_target(&self, id: String, metered: bool, wifi: StreamQuality, mobile: StreamQuality) -> StreamTarget {
        let q = self.quality(metered, wifi, mobile);
        let key = format!("{id}:{}{}", q.bit_rate, q.format);
        StreamTarget { url: self.core.stream_url(id, q.bit_rate, q.format), key }
    }

    /// Only the cache key [`Self::stream_target`] would give: for asking whether a song is already cached.
    pub fn stream_key(&self, id: String, metered: bool, wifi: StreamQuality, mobile: StreamQuality) -> String {
        let q = self.quality(metered, wifi, mobile);
        format!("{id}:{}{}", q.bit_rate, q.format)
    }

    /// The URL and cache key to open `id` from now: a finished download is the permanent copy (`downloaded`),
    /// fetched at the download quality; anything else streams at the quality for the network the phone is
    /// on (`metered`). The qualities are the user's settings.
    pub fn resolve(&self, id: String, downloaded: bool, metered: bool) -> StreamTarget {
        let q = |s: &crate::settings::SavedQuality| StreamQuality { bit_rate: s.bit_rate.max(0) as u32, format: s.format.clone() };
        let (wifi, mobile, download) = crate::rules::prefs(|p| (q(&p.wifi), q(&p.mobile), q(&p.download)));
        if downloaded {
            self.download_target(id, download)
        } else {
            self.stream_target(id, metered, wifi, mobile)
        }
    }

    /// The URL and cache key a download of `id` is fetched and kept under.
    pub fn download_target(&self, id: String, quality: StreamQuality) -> StreamTarget {
        let key = download_key(id.clone());
        StreamTarget { url: self.core.stream_url(id, quality.bit_rate, quality.format), key }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::tests::{block, client, two_addresses};
    use crate::client::NetProfile;
    use crate::transport::FailureKind;

    fn q(bit_rate: u32, format: &str) -> StreamQuality {
        StreamQuality { bit_rate, format: format.into() }
    }

    #[test]
    fn quality_follows_the_network_and_the_second_address_caps_it() {
        let (c, fake) = client(NetProfile { alt_max_bit_rate: 128, ..two_addresses() });
        let t = c.stream_target("s1".into(), false, q(0, ""), q(192, "opus"));
        assert_eq!(t.key, "s1:0");
        assert!(t.url.contains("/rest/stream?") && t.url.ends_with("&id=s1"));
        assert_eq!(c.stream_key("s1".into(), true, q(0, ""), q(192, "opus")), "s1:192opus");

        fake.fail(FailureKind::Connect);
        assert!(block(c.choose_address()));
        let t = c.stream_target("s1".into(), false, q(0, ""), q(192, "opus"));
        assert_eq!(t.key, "s1:128opus");
        assert!(t.url.starts_with("https://wan.example/rest/stream?") && t.url.ends_with("&id=s1&maxBitRate=128&format=opus&estimateContentLength=true"));
        assert_eq!(c.stream_key("s1".into(), false, q(96, "mp3"), q(0, "")), "s1:96mp3", "already under the cap");
        assert_eq!(c.stream_key("s1".into(), false, q(320, "mp3"), q(0, "")), "s1:128mp3");
    }

    #[test]
    fn a_download_opens_as_itself_and_anything_else_streams() {
        let (c, _) = client(NetProfile { url: "h".into(), ..Default::default() });
        assert_eq!(c.resolve("s1".into(), true, true).key, "dl:s1", "the permanent copy, whatever the network");
        // The settings' defaults: the original on Wi-Fi, 192k opus on a metered network.
        assert_eq!(c.resolve("s1".into(), false, false).key, "s1:0");
        assert_eq!(c.resolve("s1".into(), false, true).key, "s1:192opus");
    }

    #[test]
    fn downloads_and_streamed_copies() {
        let (c, _) = client(NetProfile { url: "h".into(), ..Default::default() });
        let t = c.download_target("s:1".into(), q(0, ""));
        assert_eq!(t.key, "dl:s:1");
        assert!(t.url.ends_with("&id=s%3A1"));
        let keys = vec!["a:0".into(), "a:192opus".into(), "ab:0".into(), "dl:a".into(), "x:a:0".into()];
        assert_eq!(stream_copies("a".into(), keys), vec!["a:0", "a:192opus"]);
    }
}
