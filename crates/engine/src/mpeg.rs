//! A live MP3 stream's frames, found and checked here rather than by symphonia's reader.
//!
//! A station's bytes are joined wherever its server's buffer began, and may hold anything between two
//! songs (a relay's dropped bytes, a tag, noise), and the next song may be at another rate or channel
//! count. symphonia's reader takes the first sync word it finds as a frame and the frame's length as
//! where the next one is, so once it lands on a false header inside the music it can walk from false
//! frame to false frame (a tone's frames repeat, and so do the false headers in them) and never find the
//! music again; and it has no bound on how far it looks, so on a long run of noise it waits on the
//! network with the engine's thread. Here a frame counts only when the stream around it says so:
//!
//! - a frame right after the one before, of the same rate and channels, is taken as it is;
//! - any other (the first, one found after skipping bytes, one of another shape) only when the next
//!   frame's header starts right where it ends, with the same rate and channels: a change of shape is
//!   followed once two frames agree on it, and a false header in noise seldom has a second behind it;
//! - no more than [`SCAN`] bytes are looked through in one call: past that the call gives up for now
//!   ([`io::ErrorKind::WouldBlock`]), and the engine asks again once its bytes are there, so its thread
//!   never waits on the network for a frame that may not come;
//! - a frame cut short by the end of the stream is not handed out.
//!
//! Only layer III is taken (a stream symphonia's probe called MP3). Each frame is handed out as a
//! symphonia packet, which (as symphonia's own readers do) allocates once a frame.

use std::io::{self, Read};

use symphonia::core::errors::{Error, Result};
use symphonia::core::io::MediaSourceStream;
use symphonia::core::packet::Packet;
use symphonia::core::units::{Duration, Timestamp};

/// The most bytes looked through for a frame in one call: well under what a live stream has buffered
/// before the engine reads it (`source::LIVE_READY`, 32 KiB), so a call never reads past what is there.
pub(crate) const SCAN: usize = 16 * 1024;
/// Bytes read from the stream at a time.
const CHUNK: usize = 4096;

/// What a layer III frame header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Header {
    pub rate: u32,
    pub channels: usize,
    /// The whole frame's length, header included.
    pub len: usize,
    /// Frames of audio it decodes to.
    pub samples: u64,
}

impl Header {
    /// The layer III header at the start of `b`; none for anything else (another layer, a free-format
    /// or reserved bit rate, a reserved rate or version).
    pub fn parse(b: &[u8]) -> Option<Header> {
        let h = u32::from_be_bytes(b.get(..4)?.try_into().ok()?);
        let (version, layer, bitrate, rate_index, padding) = ((h >> 19) & 3, (h >> 17) & 3, (h >> 12) & 0xf, (h >> 10) & 3, (h >> 9) & 1);
        if h >> 21 != 0x7ff || version == 1 || layer != 1 || bitrate == 0 || bitrate == 0xf || rate_index == 3 {
            return None;
        }
        let mpeg1 = version == 3;
        let kbps: u32 = if mpeg1 {
            [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320][bitrate as usize]
        } else {
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160][bitrate as usize]
        };
        let base = [44_100, 48_000, 32_000][rate_index as usize];
        let rate = match version {
            3 => base,
            2 => base / 2,
            _ => base / 4,
        };
        let per = if mpeg1 { 144 } else { 72 };
        let len = (per * kbps * 1000 / rate + padding) as usize;
        let channels = if (h >> 6) & 3 == 3 { 1 } else { 2 };
        Some(Header { rate, channels, len, samples: if mpeg1 { 1152 } else { 576 } })
    }

    fn shape(&self) -> (u32, usize) {
        (self.rate, self.channels)
    }
}

/// A live MP3 stream read frame by frame.
pub(crate) struct Frames {
    source: MediaSourceStream<'static>,
    /// Bytes read and not yet handed out, from `at`.
    buf: Vec<u8>,
    at: usize,
    eof: bool,
    /// The rate and channels of the last frame handed out.
    shape: Option<(u32, usize)>,
    /// The next byte is the one right after the last frame handed out.
    synced: bool,
    pts: u64,
    track: u32,
}

impl Frames {
    pub fn new(source: MediaSourceStream<'static>, track: u32) -> Frames {
        Frames { source, buf: Vec::with_capacity(4 * CHUNK), at: 0, eof: false, shape: None, synced: false, pts: 0, track }
    }

    /// Reads until `need` bytes are here from `at`, or the stream ends: whether they are.
    fn fill(&mut self, need: usize) -> io::Result<bool> {
        while self.buf.len() - self.at < need && !self.eof {
            if self.at > 0 {
                // What was handed out goes, so the buffer stays the size of a frame or two and a chunk.
                self.buf.drain(..self.at);
                self.at = 0;
            }
            let len = self.buf.len();
            self.buf.resize(len + CHUNK, 0);
            let n = loop {
                match self.source.read(&mut self.buf[len..]) {
                    Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                    other => break other,
                }
            };
            self.buf.truncate(len + *n.as_ref().unwrap_or(&0));
            match n {
                Ok(0) => self.eof = true,
                Ok(_) => {}
                Err(e) => return Err(e),
            }
        }
        Ok(self.buf.len() - self.at >= need)
    }

    /// The next frame; none at the end of the stream. An error of kind `WouldBlock` is a call that
    /// looked through [`SCAN`] bytes without finding one: ask again.
    pub fn next_packet(&mut self) -> Result<Option<Packet>> {
        let mut skipped = 0usize;
        loop {
            if !self.fill(4)? {
                return Ok(None);
            }
            let found = Header::parse(&self.buf[self.at..]);
            // A frame cut short by the end of the stream is not one: looked past, to the end.
            if let Some(f) = found {
                if !self.fill(f.len)? {
                    self.at += 1;
                    self.synced = false;
                    skipped += 1;
                    continue;
                }
                let next = if self.fill(f.len + 4)? { Header::parse(&self.buf[self.at + f.len..]) } else { None };
                let known = self.shape == Some(f.shape());
                if (self.synced && known) || next.is_some_and(|n| n.shape() == f.shape()) {
                    let data: Box<[u8]> = self.buf[self.at..self.at + f.len].into();
                    self.at += f.len;
                    self.synced = true;
                    self.shape = Some(f.shape());
                    let pts = self.pts;
                    self.pts += f.samples;
                    return Ok(Some(Packet::new(self.track, Timestamp::new(pts as i64), Duration::new(f.samples), data)));
                }
            }
            // Not a frame here: on to the next byte that could start one.
            self.synced = false;
            let step = self.buf[self.at + 1..].iter().position(|&b| b == 0xff).map_or(self.buf.len() - self.at, |p| p + 1);
            self.at += step;
            skipped += step;
            if skipped >= SCAN {
                return Err(Error::IoError(io::Error::new(io::ErrorKind::WouldBlock, "no MPEG frame found yet")));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use symphonia::core::io::MediaSourceStreamOptions;

    /// An MPEG-1 layer III frame at 44.1 kHz stereo, 128 kbps (417 bytes), or MPEG-2 at 22.05 kHz mono,
    /// 32 kbps (104 bytes), its body `fill`.
    fn frame(mpeg1: bool, fill: u8) -> Vec<u8> {
        let head: [u8; 4] = if mpeg1 { [0xff, 0xfb, 0x90, 0x00] } else { [0xff, 0xf3, 0x40, 0xc0] };
        let len = Header::parse(&head).unwrap().len;
        let mut f = head.to_vec();
        f.resize(len, fill);
        f
    }

    fn frames_of(bytes: Vec<u8>) -> Frames {
        let mss = MediaSourceStream::new(Box::new(io::Cursor::new(bytes)), MediaSourceStreamOptions::default());
        Frames::new(mss, 0)
    }

    /// Every frame handed out, `(rate, channels, first body byte)`, and the calls that gave up for now.
    fn read_all(mut f: Frames) -> (Vec<(u32, usize, u8)>, usize) {
        let (mut got, mut yields) = (Vec::new(), 0);
        loop {
            match f.next_packet() {
                Ok(Some(p)) => {
                    let h = Header::parse(&p.data).unwrap();
                    got.push((h.rate, h.channels, p.data[4]));
                }
                Ok(None) => return (got, yields),
                Err(Error::IoError(e)) if e.kind() == io::ErrorKind::WouldBlock => yields += 1,
                Err(e) => panic!("{e}"),
            }
        }
    }

    #[test]
    fn headers_say_their_rate_channels_and_length() {
        assert_eq!(Header::parse(&[0xff, 0xfb, 0x90, 0x00]), Some(Header { rate: 44_100, channels: 2, len: 417, samples: 1152 }));
        assert_eq!(Header::parse(&[0xff, 0xfb, 0x92, 0x00]).map(|h| h.len), Some(418), "padded");
        assert_eq!(Header::parse(&[0xff, 0xf3, 0x40, 0xc0]), Some(Header { rate: 22_050, channels: 1, len: 104, samples: 576 }));
        assert_eq!(Header::parse(&[0xff, 0xe3, 0x40, 0x00]).map(|h| (h.rate, h.len)), Some((11_025, 208)), "MPEG-2.5");
        assert_eq!(Header::parse(&[0xff, 0xfd, 0x90, 0x00]), None, "layer II");
        assert_eq!(Header::parse(&[0xff, 0xfb, 0x00, 0x00]), None, "free format");
        assert_eq!(Header::parse(&[0xff, 0xfb, 0x9c, 0x00]), None, "reserved rate");
        assert_eq!(Header::parse(&[0xff, 0xeb, 0x90, 0x00]), None, "reserved version");
    }

    #[test]
    fn a_stream_joined_mid_frame_is_read_from_its_first_whole_frame() {
        let mut bytes = frame(true, 1)[200..].to_vec();
        for i in 2..6 {
            bytes.extend_from_slice(&frame(true, i));
        }
        let (got, _) = read_all(frames_of(bytes));
        assert_eq!(got.iter().map(|g| g.2).collect::<Vec<_>>(), [2, 3, 4, 5]);
    }

    #[test]
    fn a_change_of_shape_is_followed_once_two_frames_agree_on_it() {
        let mut bytes = Vec::new();
        for i in 1..4 {
            bytes.extend_from_slice(&frame(true, i));
        }
        // A lone header of another shape, as noise holds: not a frame.
        bytes.extend_from_slice(&[0xff, 0xf3, 0x40, 0xc0, 9, 9, 9]);
        for i in 4..6 {
            bytes.extend_from_slice(&frame(true, i));
        }
        for i in 6..9 {
            bytes.extend_from_slice(&frame(false, i));
        }
        let (got, _) = read_all(frames_of(bytes));
        assert_eq!(
            got,
            [(44_100, 2, 1), (44_100, 2, 2), (44_100, 2, 3), (44_100, 2, 4), (44_100, 2, 5), (22_050, 1, 6), (22_050, 1, 7), (22_050, 1, 8)]
        );
    }

    #[test]
    fn noise_is_looked_through_a_bounded_stretch_at_a_time_and_a_cut_frame_is_dropped() {
        let mut bytes = frame(true, 1);
        bytes.extend_from_slice(&frame(true, 2));
        // 100 kB of 0xff-strewn noise, then two frames, the last cut short.
        bytes.extend((0..100_000u32).map(|i| if i % 7 == 0 { 0xff } else { (i * 31 % 251) as u8 }));
        bytes.extend_from_slice(&frame(false, 3));
        bytes.extend_from_slice(&frame(false, 4));
        bytes.extend_from_slice(&frame(false, 5)[..50]);
        let (got, yields) = read_all(frames_of(bytes));
        assert_eq!(got.iter().map(|g| g.2).collect::<Vec<_>>(), [1, 2, 3, 4]);
        assert!(yields >= 100_000 / SCAN, "{yields} calls gave up for now");
    }
}
