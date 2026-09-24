//! A cover's file (JPEG, PNG, WebP or GIF, told apart by its first bytes) decoded straight into the
//! caller's pixels at the size they are drawn at, turned the way its EXIF says. A picture already that
//! size is decoded into the caller's buffer with no copy in between; a JPEG at least twice the size is
//! decoded at 1/2, 1/4 or 1/8 of it by the IDCT itself, which skips most of the work; anything else is
//! decoded whole into a buffer the decoder keeps and filtered into place (scale.rs).
//!
//! A GIF is its first frame: a cover is a still picture, and one that moved would be a timer on every
//! screen that shows it. HEIF and AVIF are not decoded. The one pure-Rust HEVC decoder (the `heic`
//! crate, 0.1.6) is AGPL-3.0 or a paid licence, which an MIT app cannot take; it also decodes a cover
//! about 22 times slower than a JPEG of the same picture, gets alpha planes and 4:4:4 pictures wrong
//! against libheif, and adds 313 KB to the arm64 library. AV1's (rav1d, without its assembly) adds about
//! 1.5 MB and decodes a cover six times slower than a JPEG. Both for a format a Subsonic server does not
//! send when it is asked for a size (it resizes to JPEG or PNG). Either is an [`Error::Unknown`], a cover
//! with no picture.

use std::fmt;
use std::io::Cursor;

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

use crate::scale::{premultiply, Alpha, Scaler, Source, Target};

/// The largest picture decoded, in pixels a side and in all: a cover is a few thousand pixels at most, and
/// a file claiming more is broken or hostile. 4096² is 64 MB of RGBA on the worker decoding it, and each
/// of a loader's workers may be decoding one.
const MAX_SIDE: usize = 16384;
const MAX_PIXELS: usize = 4096 * 4096;
/// The longest side a picture asked for at its own size (0 x 0) is decoded to: past a phone's screen,
/// and a large file shrunk to it is kept in a fraction of the memory it would take whole.
pub const WHOLE_SIDE: usize = 2048;
/// A decoded picture's buffer above this many bytes (1448² RGBA) is given back after use rather than kept
/// for the next: one huge cover should not hold its memory for the life of the decoder.
const KEEP: usize = 8 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    WebP,
    Gif,
}

/// What a file is, from its first bytes.
pub fn format(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Format::WebP)
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some(Format::Gif)
    } else {
        None
    }
}

/// What a file's headers say before any of it is decoded: enough for a client to make the picture it
/// decodes into at the right size first (an Android Bitmap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub format: Format,
    /// The picture's size as it is shown: turned, when its EXIF turns it a quarter.
    pub width: usize,
    pub height: usize,
    /// The EXIF orientation, 1 to 8 (1 as stored), which the decoder applies.
    pub orientation: u8,
}

impl Header {
    /// Whether the picture has no transparency, from its format alone: a JPEG. A client can keep it in
    /// fewer bytes a pixel (Android's RGB_565) and draw it without blending.
    pub fn opaque(&self) -> bool {
        self.format == Format::Jpeg
    }

    /// The size to decode to for a view `width` x `height`: that size, or the same shape smaller when the
    /// picture has fewer pixels than it would fill (it is grown on screen, not in memory), or for 0 x 0
    /// the picture's own size, shrunk in its shape to at most [`WHOLE_SIDE`] a side.
    pub fn fill(&self, width: usize, height: usize) -> (usize, usize) {
        if width == 0 || height == 0 {
            let k = (WHOLE_SIDE as f64 / self.width.max(self.height) as f64).min(1.0);
            return (((self.width as f64 * k).round() as usize).max(1), ((self.height as f64 * k).round() as usize).max(1));
        }
        let k = (self.width as f64 / width as f64).min(self.height as f64 / height as f64);
        if k >= 1.0 {
            return (width, height);
        }
        (((width as f64 * k).round() as usize).max(1), ((height as f64 * k).round() as usize).max(1))
    }
}

/// A file's format, size and orientation from its headers: only the headers are read.
pub fn header(bytes: &[u8]) -> Result<Header, Error> {
    let format = format(bytes).ok_or(Error::Unknown)?;
    let (width, height) = match format {
        Format::Jpeg => {
            let mut z = zune_jpeg::JpegDecoder::new(ZCursor::new(bytes));
            z.decode_headers().map_err(corrupt)?;
            let info = z.info().ok_or_else(|| Error::Corrupt("no header".into()))?;
            (info.width as usize, info.height as usize)
        }
        // IHDR is always the first chunk: width and height, big-endian, at 16 and 20.
        Format::Png => {
            let be = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
            if bytes.get(12..16) != Some(b"IHDR") {
                return Err(Error::Corrupt("no IHDR".into()));
            }
            (be(16).unwrap_or(0), be(20).unwrap_or(0))
        }
        Format::WebP => {
            let d = image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(corrupt)?;
            let (w, h) = d.dimensions();
            (w as usize, h as usize)
        }
        // The logical screen: width and height, little-endian, at 6 and 8.
        Format::Gif => {
            let le = |at: usize| bytes.get(at..at + 2).map_or(0, |b| u16::from_le_bytes([b[0], b[1]]) as usize);
            (le(6), le(8))
        }
    };
    check(width, height)?;
    let orientation = orientation_of(bytes, format);
    let (width, height) = if orientation >= 5 { (height, width) } else { (width, height) };
    Ok(Header { format, width, height, orientation })
}

/// A file's EXIF orientation (1, as stored, when it says none), found by walking its container to the
/// EXIF block: a JPEG's APP1 segments before the picture, a PNG's eXIf chunk before its data, a WebP's
/// EXIF chunk. Nothing is decoded or copied. A GIF has none.
fn orientation_of(bytes: &[u8], format: Format) -> u8 {
    let tiff = match format {
        Format::Jpeg => jpeg_exif(bytes),
        Format::Png => png_exif(bytes),
        Format::WebP => webp_exif(bytes),
        Format::Gif => None,
    };
    match tiff.map(orientation) {
        Some(o @ 1..=8) => o as u8,
        _ => 1,
    }
}

// The walkers below add lengths the file gives (up to 4 GB) to offsets into it with checked sums: on a
// 32-bit phone a plain sum could wrap around to an offset back inside the file.

/// `len` bytes of `bytes` from `at`, if the file holds them.
fn span(bytes: &[u8], at: usize, len: usize) -> Option<&[u8]> {
    bytes.get(at..at.checked_add(len)?)
}

fn jpeg_exif(bytes: &[u8]) -> Option<&[u8]> {
    let mut at = 2usize;
    loop {
        // A marker may be preceded by fill bytes.
        while bytes.get(at.checked_add(1)?) == Some(&0xFF) {
            at += 1;
        }
        let [0xFF, marker] = *span(bytes, at, 2)? else { return None };
        // The picture itself, or its end: no EXIF comes after.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let len = u16::from_be_bytes(span(bytes, at + 2, 2)?.try_into().ok()?) as usize;
        // The length counts its own two bytes.
        let data = span(bytes, at + 4, len.saturating_sub(2))?;
        if marker == 0xE1 {
            if let Some(tiff) = data.strip_prefix(b"Exif\0\0") {
                return Some(tiff);
            }
        }
        at = at.checked_add(2 + len)?;
    }
}

fn png_exif(bytes: &[u8]) -> Option<&[u8]> {
    let mut at = 8usize;
    loop {
        let len = u32::from_be_bytes(span(bytes, at, 4)?.try_into().ok()?) as usize;
        let kind = span(bytes, at.checked_add(4)?, 4)?;
        if kind == b"IDAT" || kind == b"IEND" {
            return None;
        }
        if kind == b"eXIf" {
            return span(bytes, at + 8, len);
        }
        // Length, kind, data and CRC.
        at = at.checked_add(len)?.checked_add(12)?;
    }
}

fn webp_exif(bytes: &[u8]) -> Option<&[u8]> {
    let mut at = 12usize;
    loop {
        let kind = span(bytes, at, 4)?;
        let len = u32::from_le_bytes(span(bytes, at.checked_add(4)?, 4)?.try_into().ok()?) as usize;
        if kind == b"EXIF" {
            let data = span(bytes, at + 8, len)?;
            return Some(data.strip_prefix(b"Exif\0\0").unwrap_or(data));
        }
        // Chunks are padded to an even length.
        at = at.checked_add(len)?.checked_add(8 + (len & 1))?;
    }
}

/// The EXIF orientation (1 to 8, 1 as stored) in a TIFF block, or 0 when there is none or it cannot be
/// read: the Orientation tag (0x0112) of the first directory.
fn orientation(tiff: &[u8]) -> u16 {
    let little = match tiff.get(..4) {
        Some(b"II*\0") => true,
        Some(b"MM\0*") => false,
        _ => return 0,
    };
    let u16_at = |at: usize| span(tiff, at, 2).map(|b| if little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |at: usize| {
        span(tiff, at, 4).map(|b| {
            let b = [b[0], b[1], b[2], b[3]];
            (if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }) as usize
        })
    };
    let Some(dir) = u32_at(4) else { return 0 };
    let Some(entries) = u16_at(dir) else { return 0 };
    // A directory must lie in the block, so an entry's offset is below its length and the sums below
    // cannot wrap.
    if dir.checked_add(2 + entries as usize * 12).is_none_or(|end| end > tiff.len()) {
        return 0;
    }
    (0..entries as usize)
        .map(|i| dir + 2 + i * 12)
        .find(|&e| u16_at(e) == Some(0x0112))
        .and_then(|e| u16_at(e + 8))
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a JPEG, PNG, WebP or GIF file (a HEIF or AVIF one, say).
    Unknown,
    /// Larger than any cover (`MAX_SIDE`, `MAX_PIXELS`).
    TooLarge,
    /// The pixels given do not hold the size asked for.
    Target,
    /// The decoder's own words for what is wrong with the file.
    Corrupt(String),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Unknown => f.write_str("not a JPEG, PNG, WebP or GIF picture"),
            Error::TooLarge => f.write_str("picture too large"),
            Error::Target => f.write_str("the target does not hold the size asked for"),
            Error::Corrupt(why) => write!(f, "bad picture: {why}"),
        }
    }
}

impl std::error::Error for Error {}

fn corrupt(e: impl fmt::Display) -> Error {
    Error::Corrupt(e.to_string())
}

fn check(width: usize, height: usize) -> Result<(), Error> {
    if width == 0 || height == 0 {
        Err(Error::Corrupt("empty picture".into()))
    } else if width > MAX_SIDE || height > MAX_SIDE || width * height > MAX_PIXELS {
        Err(Error::TooLarge)
    } else {
        Ok(())
    }
}

/// How far a JPEG's IDCT may shrink a `width` x `height` picture that is to fill `tw` x `th`: 8, 4, 2 or
/// 1 (not at all), the most that still leaves at least the pixels drawn.
fn reduction(width: usize, height: usize, tw: usize, th: usize) -> usize {
    let s = (width as f64 / tw as f64).min(height as f64 / th as f64);
    [8, 4, 2].into_iter().find(|&k| s >= k as f64).unwrap_or(1)
}

/// `len` bytes of a kept buffer, grown to hold exactly them: grown by doubling, it would be kept at up to
/// twice the largest picture decoded.
fn grow(v: &mut Vec<u8>, len: usize) -> &mut [u8] {
    if v.len() < len {
        v.reserve_exact(len - v.len());
        v.resize(len, 0);
    }
    &mut v[..len]
}

/// A decoder with the buffers it keeps between pictures: one per thread that decodes.
pub struct Decoder {
    src: Vec<u8>,
    /// A turned picture as it is stored, before it is turned into the caller's pixels.
    unturned: Vec<u8>,
    scaler: Scaler,
    idct_scaling: bool,
}

impl Default for Decoder {
    fn default() -> Decoder {
        Decoder { src: Vec::new(), unturned: Vec::new(), scaler: Scaler::default(), idct_scaling: true }
    }
}

impl Decoder {
    pub fn new() -> Decoder {
        Decoder::default()
    }

    /// Whether a JPEG at least twice the size drawn is shrunk by jpeg-decoder's IDCT (the default) or
    /// decoded whole by zune-jpeg and averaged. The IDCT is a third faster on a desktop and its pictures
    /// differ from the exact average about as much as libjpeg-turbo's scaled decode (Android's
    /// `inSampleSize`) does, with sharper errors at strong colour edges; decoded whole, the average is
    /// exact. Android's `coverbench` measures both on the phone.
    pub fn set_idct_scaling(&mut self, on: bool) {
        self.idct_scaling = on;
    }

    /// Decodes the file `bytes` into `t`, filling it (the middle kept when the shapes differ), turned
    /// the way its EXIF says.
    pub fn decode_into(&mut self, bytes: &[u8], mut t: Target, alpha: Alpha) -> Result<(), Error> {
        if !t.fits() {
            return Err(Error::Target);
        }
        let format = format(bytes).ok_or(Error::Unknown)?;
        let turn = orientation_of(bytes, format);
        let r = if turn == 1 { self.stored(bytes, format, &mut t, alpha) } else { self.turned(bytes, format, turn, &mut t, alpha) };
        if self.src.capacity() > KEEP {
            self.src = Vec::new();
        }
        if self.unturned.capacity() > KEEP {
            self.unturned = Vec::new();
        }
        r
    }

    /// The picture as it is stored, into `t`.
    fn stored(&mut self, bytes: &[u8], format: Format, t: &mut Target, alpha: Alpha) -> Result<(), Error> {
        match format {
            Format::Jpeg => self.jpeg(bytes, t),
            Format::Png => self.png(bytes, t, alpha),
            Format::WebP => self.webp(bytes, t, alpha),
            Format::Gif => self.gif(bytes, t, alpha),
        }
    }

    /// A picture its EXIF turns or mirrors: decoded as it is stored, filling `t`'s size turned back
    /// (the middle of the stored picture is the middle of the turned one), then turned into `t`.
    fn turned(&mut self, bytes: &[u8], format: Format, turn: u8, t: &mut Target, alpha: Alpha) -> Result<(), Error> {
        let (w, h) = if turn >= 5 { (t.height, t.width) } else { (t.width, t.height) };
        let mut stored = std::mem::take(&mut self.unturned);
        let px = grow(&mut stored, w * h * 4);
        let r = self.stored(bytes, format, &mut Target { px, width: w, height: h, stride: w * 4 }, alpha);
        if r.is_ok() {
            orient(px, w, h, turn, t);
        }
        self.unturned = stored;
        r
    }

    /// Decodes into a new buffer of tight `width` x `height` RGBA rows.
    pub fn decode(&mut self, bytes: &[u8], width: usize, height: usize, alpha: Alpha) -> Result<Vec<u8>, Error> {
        if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
            return Err(Error::Target);
        }
        let mut px = vec![0; width * height * 4];
        self.decode_into(bytes, Target { px: &mut px, width, height, stride: width * 4 }, alpha)?;
        Ok(px)
    }

    fn jpeg(&mut self, bytes: &[u8], t: &mut Target) -> Result<(), Error> {
        let options = DecoderOptions::default().set_max_width(MAX_SIDE).set_max_height(MAX_SIDE);
        let mut z = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options.jpeg_set_out_colorspace(ColorSpace::RGB));
        z.decode_headers().map_err(corrupt)?;
        let info = z.info().ok_or_else(|| Error::Corrupt("no header".into()))?;
        let (w, h) = (info.width as usize, info.height as usize);
        check(w, h)?;
        let k = if self.idct_scaling { reduction(w, h, t.width, t.height) } else { 1 };
        if k > 1 && self.jpeg_reduced(bytes, w.div_ceil(k), h.div_ceil(k), t)? {
            return Ok(());
        }
        if t.takes(w, h) {
            // RGBA straight into the target. The output colour space is fixed once headers are read, so
            // this is a second decoder; its headers are a few hundred bytes to parse again.
            let mut z = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(bytes), options.jpeg_set_out_colorspace(ColorSpace::RGBA));
            return z.decode_into(&mut t.px[..w * h * 4]).map_err(corrupt);
        }
        let src = grow(&mut self.src, w * h * 3);
        z.decode_into(src).map_err(corrupt)?;
        self.scaler.fill(Source { px: src, width: w, height: h, stride: w * 3, channels: 3 }, t, Alpha::Straight);
        Ok(())
    }

    /// A JPEG decoded at a fraction of its size by jpeg-decoder's scaled IDCT, then filtered the rest of
    /// the way. False (and nothing written) for a JPEG it does not decode to grey or RGB, CMYK: zune-jpeg
    /// takes those whole.
    fn jpeg_reduced(&mut self, bytes: &[u8], w: usize, h: usize, t: &mut Target) -> Result<bool, Error> {
        let mut d = jpeg_decoder::Decoder::new(bytes);
        let (w, h) = d.scale(w as u16, h as u16).map_err(corrupt)?;
        let info = d.info().ok_or_else(|| Error::Corrupt("no header".into()))?;
        let c = match info.pixel_format {
            jpeg_decoder::PixelFormat::L8 => 1,
            jpeg_decoder::PixelFormat::RGB24 => 3,
            _ => return Ok(false),
        };
        let mut px = d.decode().map_err(corrupt)?;
        let (w, h) = (w as usize, h as usize);
        if px.len() < w * h * c {
            return Err(Error::Corrupt("short picture".into()));
        }
        self.scaler.fill(Source { px: &mut px, width: w, height: h, stride: w * c, channels: c }, t, Alpha::Straight);
        Ok(true)
    }

    fn png(&mut self, bytes: &[u8], t: &mut Target, alpha: Alpha) -> Result<(), Error> {
        let mut d = png::Decoder::new(Cursor::new(bytes));
        // Palettes, low bit depths and tRNS to 8-bit grey, grey and alpha, RGB or RGBA.
        d.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
        let mut r = d.read_info().map_err(corrupt)?;
        let (w, h) = (r.info().width as usize, r.info().height as usize);
        check(w, h)?;
        let c = r.output_color_type().0.samples();
        let len = r.output_buffer_size().ok_or(Error::TooLarge)?;
        if c == 4 && t.takes(w, h) && len == w * h * 4 {
            r.next_frame(&mut t.px[..len]).map_err(corrupt)?;
            if alpha == Alpha::Premultiplied {
                premultiply(&mut t.px[..len], 4);
            }
            return Ok(());
        }
        let src = grow(&mut self.src, len);
        let frame = r.next_frame(src).map_err(corrupt)?;
        self.scaler.fill(Source { px: src, width: w, height: h, stride: frame.line_size, channels: c }, t, alpha);
        Ok(())
    }

    /// The first frame as it is shown on the screen the file describes: at its place, with whatever of
    /// the screen it leaves uncovered transparent.
    fn gif(&mut self, bytes: &[u8], t: &mut Target, alpha: Alpha) -> Result<(), Error> {
        let mut o = gif::DecodeOptions::new();
        o.set_color_output(gif::ColorOutput::RGBA);
        o.set_memory_limit(gif::MemoryLimit::Bytes(std::num::NonZeroU64::new((MAX_PIXELS * 4) as u64).expect("not zero")));
        // A frame that does not lie on the screen is refused while its descriptor is read, before any of
        // it is decoded or anything is made for it.
        o.check_frame_consistency(true);
        let mut d = o.read_info(bytes).map_err(corrupt)?;
        let (w, h) = (d.width() as usize, d.height() as usize);
        check(w, h)?;
        let (left, top, fw, fh) = {
            let f = d.next_frame_info().map_err(corrupt)?.ok_or_else(|| Error::Corrupt("no frame".into()))?;
            (f.left as usize, f.top as usize, f.width as usize, f.height as usize)
        };
        // The frame is sized on its own, before anything is made for it: a 1x1 screen may carry a
        // 65535x65535 frame, 17 GB of RGBA.
        check(fw, fh)?;
        if left + fw > w || top + fh > h {
            return Err(Error::Corrupt("frame off the screen".into()));
        }
        let len = d.buffer_size();
        if len != fw * fh * 4 {
            return Err(Error::Corrupt("frame size".into()));
        }
        let whole = (left, top, fw, fh) == (0, 0, w, h);
        // The screen, and after it (for a frame smaller than the screen) the frame.
        let src = grow(&mut self.src, w * h * 4 + if whole { 0 } else { len });
        let (screen, frame) = src.split_at_mut(w * h * 4);
        if whole {
            d.read_into_buffer(screen).map_err(corrupt)?;
        } else {
            screen.fill(0);
            d.read_into_buffer(frame).map_err(corrupt)?;
            // The frame lies wholly on the screen (checked above).
            for y in 0..fh {
                screen[((top + y) * w + left) * 4..][..fw * 4].copy_from_slice(&frame[y * fw * 4..][..fw * 4]);
            }
        }
        self.scaler.fill(Source { px: screen, width: w, height: h, stride: w * 4, channels: 4 }, t, alpha);
        Ok(())
    }

    fn webp(&mut self, bytes: &[u8], t: &mut Target, alpha: Alpha) -> Result<(), Error> {
        let mut d = image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(corrupt)?;
        let (w, h) = d.dimensions();
        let (w, h) = (w as usize, h as usize);
        check(w, h)?;
        let c = if d.has_alpha() { 4 } else { 3 };
        let len = d.output_buffer_size().ok_or(Error::TooLarge)?;
        if c == 4 && t.takes(w, h) {
            d.read_image(&mut t.px[..len]).map_err(corrupt)?;
            if alpha == Alpha::Premultiplied {
                premultiply(&mut t.px[..len], 4);
            }
            return Ok(());
        }
        let src = grow(&mut self.src, len);
        d.read_image(src).map_err(corrupt)?;
        self.scaler.fill(Source { px: src, width: w, height: h, stride: w * c, channels: c }, t, alpha);
        Ok(())
    }
}

/// `src`, `w` x `h` tight RGBA rows as stored, into `t` as EXIF orientation `turn` (2 to 8) shows them: 2
/// mirrored, 3 turned half way, 4 flipped, 5 mirrored across the diagonal, 6 turned a quarter clockwise,
/// 7 mirrored across the other diagonal, 8 turned a quarter anticlockwise. `t` is `h` x `w` for 5 to 8.
fn orient(src: &[u8], w: usize, h: usize, turn: u8, t: &mut Target) {
    let px = |x: usize, y: usize| -> [u8; 4] { src[(y * w + x) * 4..][..4].try_into().expect("four bytes") };
    for y in 0..t.height {
        let row = &mut t.px[y * t.stride..][..t.width * 4];
        for (x, out) in row.chunks_exact_mut(4).enumerate() {
            let p = match turn {
                2 => px(w - 1 - x, y),
                3 => px(w - 1 - x, h - 1 - y),
                4 => px(x, h - 1 - y),
                5 => px(y, x),
                6 => px(y, h - 1 - x),
                7 => px(w - 1 - y, h - 1 - x),
                _ => px(w - 1 - y, x),
            };
            out.copy_from_slice(&p);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_told_apart_by_their_first_bytes() {
        assert_eq!(format(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
        assert_eq!(format(b"\x89PNG\r\n\x1a\n...."), Some(Format::Png));
        assert_eq!(format(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::WebP));
        assert_eq!(format(b"GIF89a"), Some(Format::Gif));
        assert_eq!(format(b"\0\0\0\x1cftypheic"), None, "HEIF");
        assert_eq!(format(b""), None);
    }

    #[test]
    fn a_header_is_read_without_decoding_the_picture() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&600u32.to_be_bytes());
        png.extend_from_slice(&400u32.to_be_bytes());
        assert_eq!(header(&png), Ok(Header { format: Format::Png, width: 600, height: 400, orientation: 1 }));
        assert_eq!(header(&png[..20]), Err(Error::Corrupt("empty picture".into())), "cut short");
        assert_eq!(header(b"GIF89a\x02\x01\x03\x01"), Ok(Header { format: Format::Gif, width: 258, height: 259, orientation: 1 }));
        assert_eq!(header(b"\0\0\0\x1cftypheic"), Err(Error::Unknown));
        let size = |w: u32, h: u32| {
            let mut png = png[..16].to_vec();
            png.extend_from_slice(&w.to_be_bytes());
            png.extend_from_slice(&h.to_be_bytes());
            header(&png)
        };
        assert!(size(4096, 4096).is_ok());
        assert_eq!(size(4097, 4096), Err(Error::TooLarge), "past 16 MP: 64 MB of RGBA on one worker");
        assert_eq!(size(16384, 1024), Ok(Header { format: Format::Png, width: 16384, height: 1024, orientation: 1 }));
    }

    #[test]
    fn a_view_is_filled_at_its_size_or_smaller_in_its_shape_never_grown() {
        let h = Header { format: Format::Jpeg, width: 320, height: 320, orientation: 1 };
        assert_eq!(h.fill(138, 138), (138, 138));
        assert_eq!(h.fill(900, 900), (320, 320));
        assert_eq!(h.fill(1000, 500), (320, 160), "fills by the width");
        assert_eq!(h.fill(0, 0), (320, 320), "its own size");
        let huge = Header { width: 4000, height: 3000, ..h };
        assert_eq!(huge.fill(0, 0), (2048, 1536), "its own shape, at most WHOLE_SIDE a side");
        assert_eq!(Header { width: 1000, height: 3000, ..h }.fill(0, 0), (683, 2048));
        let wide = Header { width: 800, height: 400, ..h };
        assert_eq!(wide.fill(600, 600), (400, 400), "fills by the height");
        assert!(h.opaque() && !Header { format: Format::Png, ..h }.opaque());
    }

    #[test]
    fn the_exif_orientation_is_read_in_either_byte_order() {
        // One directory at 8 with one entry: tag 0x0112, type SHORT, count 1, value 6 (turned right).
        let le = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0";
        let be = b"MM\0*\0\0\0\x08\0\x01\x01\x12\0\x03\0\0\0\x01\0\x06\0\0";
        assert_eq!(orientation(le), 6);
        assert_eq!(orientation(be), 6);
        assert_eq!(orientation(&le[..16]), 0, "cut short");
        assert_eq!(orientation(b"nope"), 0);
        // A directory claimed at the end of the address space, and one claiming more entries than fit.
        assert_eq!(orientation(b"II*\0\xF0\xFF\xFF\xFF\x01\0"), 0);
        assert_eq!(orientation(b"II*\0\x08\0\0\0\xFF\xFF\x12\x01\x03\0\x01\0\0\0\x06\0\0\0"), 0);
    }

    #[test]
    fn lengths_that_run_past_the_file_or_the_address_space_end_the_walk() {
        let huge = u32::MAX.to_be_bytes();
        // A PNG chunk 4 GB long before an eXIf one, and an eXIf chunk that long.
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&huge);
        png.extend_from_slice(b"tEXt\0\0\0\0eXIf");
        assert_eq!(png_exif(&png), None);
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&huge);
        png.extend_from_slice(b"eXIfII*\0");
        assert_eq!(png_exif(&png), None);
        // The same in a WebP, little-endian.
        let mut webp = b"RIFF\0\0\0\0WEBPVP8X".to_vec();
        webp.extend_from_slice(&u32::MAX.to_le_bytes());
        webp.extend_from_slice(b"EXIF\x04\0\0\0II*\0");
        assert_eq!(webp_exif(&webp), None);
        let mut webp = b"RIFF\0\0\0\0WEBPEXIF".to_vec();
        webp.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(webp_exif(&webp), None);
        // A JPEG segment whose length runs past the end, and one too short to count itself.
        assert_eq!(jpeg_exif(b"\xFF\xD8\xFF\xE1\xFF\xFFExif\0\0"), None);
        assert_eq!(jpeg_exif(b"\xFF\xD8\xFF\xE1\0\0\xFF\xD9"), None);
        // Fill bytes up to the end of the file.
        assert_eq!(jpeg_exif(b"\xFF\xD8\xFF\xFF\xFF"), None);
        // What they should find, they still find.
        let mut jpeg = b"\xFF\xD8\xFF\xE1\0\x0cExif\0\0II*\0".to_vec();
        jpeg.extend_from_slice(b"\xFF\xDA");
        assert_eq!(jpeg_exif(&jpeg), Some(&b"II*\0"[..]));
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x04eXIfII*\0".to_vec();
        png.extend_from_slice(b"CRC!");
        assert_eq!(png_exif(&png), Some(&b"II*\0"[..]));
    }

    #[test]
    fn the_idct_shrinks_as_far_as_still_leaves_every_pixel_drawn() {
        assert_eq!(reduction(800, 800, 300, 300), 2);
        assert_eq!(reduction(800, 800, 400, 400), 2);
        assert_eq!(reduction(800, 800, 401, 401), 1);
        assert_eq!(reduction(3000, 3000, 300, 300), 8);
        assert_eq!(reduction(1600, 1600, 300, 300), 4);
        // A wide picture fills by its height.
        assert_eq!(reduction(2400, 600, 300, 300), 2);
        assert_eq!(reduction(320, 320, 1080, 1080), 1);
    }

    #[test]
    fn a_target_that_does_not_hold_its_size_is_refused() {
        let mut px = [0u8; 15];
        let t = Target { px: &mut px, width: 2, height: 2, stride: 8 };
        assert_eq!(Decoder::new().decode_into(&[0xFF, 0xD8, 0xFF], t, Alpha::Straight), Err(Error::Target));
        let mut px = [0u8; 16];
        let t = Target { px: &mut px, width: 2, height: 2, stride: 8 };
        assert_eq!(Decoder::new().decode_into(b"nope", t, Alpha::Straight), Err(Error::Unknown));
    }
}
