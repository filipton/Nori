//! A cover's file (JPEG, PNG or WebP, told apart by its first bytes) decoded straight into the caller's
//! pixels at the size they are drawn at. A picture already that size is decoded into the caller's buffer
//! with no copy in between; a JPEG at least twice the size is decoded at 1/2, 1/4 or 1/8 of it by the IDCT
//! itself, which skips most of the work; anything else is decoded whole into a buffer the decoder keeps
//! and filtered into place (scale.rs).

use std::fmt;
use std::io::Cursor;

use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

use crate::scale::{premultiply, Alpha, Scaler, Source, Target};

/// The largest picture decoded, in pixels a side and in all: a cover is a few thousand pixels at most, and
/// a file claiming more is broken or hostile.
const MAX_SIDE: usize = 16384;
const MAX_PIXELS: usize = 64 << 20;
/// A decoded picture's buffer above this many bytes (1448² RGBA) is given back after use rather than kept
/// for the next: one huge cover should not hold its memory for the life of the decoder.
const KEEP: usize = 8 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    WebP,
}

/// What a file is, from its first bytes.
pub fn format(bytes: &[u8]) -> Option<Format> {
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some(Format::Jpeg)
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(Format::Png)
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(Format::WebP)
    } else {
        None
    }
}

/// What a file's headers say before any of it is decoded: enough for a client that sizes the picture it
/// decodes into (Android's Coil, which makes the Bitmap first) to work that size out the way it always has.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub format: Format,
    pub width: usize,
    pub height: usize,
    /// The file's EXIF turns or mirrors the picture. The decoder draws the pixels as they are stored, so
    /// a client that honours the turn (Coil does, for JPEG and WebP) decodes these its own way.
    pub oriented: bool,
}

/// A file's format and size from its headers, and whether its EXIF turns it: only the headers are read.
pub fn header(bytes: &[u8]) -> Result<Header, Error> {
    let format = format(bytes).ok_or(Error::Unknown)?;
    let (width, height, oriented) = match format {
        Format::Jpeg => {
            let mut z = zune_jpeg::JpegDecoder::new(ZCursor::new(bytes));
            z.decode_headers().map_err(corrupt)?;
            let info = z.info().ok_or_else(|| Error::Corrupt("no header".into()))?;
            (info.width as usize, info.height as usize, z.exif().is_some_and(|e| orientation(e) > 1))
        }
        // IHDR is always the first chunk: width and height, big-endian, at 16 and 20.
        Format::Png => {
            let be = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize);
            if bytes.get(12..16) != Some(b"IHDR") {
                return Err(Error::Corrupt("no IHDR".into()));
            }
            (be(16).unwrap_or(0), be(20).unwrap_or(0), false)
        }
        Format::WebP => {
            let mut d = image_webp::WebPDecoder::new(Cursor::new(bytes)).map_err(corrupt)?;
            let (w, h) = d.dimensions();
            let exif = d.exif_metadata().ok().flatten();
            (w as usize, h as usize, exif.is_some_and(|e| orientation(e.strip_prefix(b"Exif\0\0").unwrap_or(&e)) > 1))
        }
    };
    check(width, height)?;
    Ok(Header { format, width, height, oriented })
}

/// The EXIF orientation (1 to 8, 1 as stored) in a TIFF block, or 0 when there is none or it cannot be
/// read: the Orientation tag (0x0112) of the first directory.
fn orientation(tiff: &[u8]) -> u16 {
    let little = match tiff.get(..4) {
        Some(b"II*\0") => true,
        Some(b"MM\0*") => false,
        _ => return 0,
    };
    let u16_at = |at: usize| tiff.get(at..at + 2).map(|b| if little { u16::from_le_bytes([b[0], b[1]]) } else { u16::from_be_bytes([b[0], b[1]]) });
    let u32_at = |at: usize| {
        tiff.get(at..at + 4).map(|b| {
            let b = [b[0], b[1], b[2], b[3]];
            (if little { u32::from_le_bytes(b) } else { u32::from_be_bytes(b) }) as usize
        })
    };
    let Some(dir) = u32_at(4) else { return 0 };
    let Some(entries) = u16_at(dir) else { return 0 };
    (0..entries as usize)
        .map(|i| dir + 2 + i * 12)
        .find(|&e| u16_at(e) == Some(0x0112))
        .and_then(|e| u16_at(e + 8))
        .unwrap_or(0)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// Not a JPEG, PNG or WebP file.
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
            Error::Unknown => f.write_str("not a JPEG, PNG or WebP picture"),
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

/// `len` bytes of a kept buffer, grown to hold them.
fn grow(v: &mut Vec<u8>, len: usize) -> &mut [u8] {
    if v.len() < len {
        v.resize(len, 0);
    }
    &mut v[..len]
}

/// A decoder with the buffers it keeps between pictures: one per thread that decodes.
pub struct Decoder {
    src: Vec<u8>,
    scaler: Scaler,
    idct_scaling: bool,
}

impl Default for Decoder {
    fn default() -> Decoder {
        Decoder { src: Vec::new(), scaler: Scaler::default(), idct_scaling: true }
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

    /// Decodes the file `bytes` into `t`, filling it (the middle kept when the shapes differ).
    pub fn decode_into(&mut self, bytes: &[u8], mut t: Target, alpha: Alpha) -> Result<(), Error> {
        if !t.fits() {
            return Err(Error::Target);
        }
        let r = match format(bytes) {
            Some(Format::Jpeg) => self.jpeg(bytes, &mut t),
            Some(Format::Png) => self.png(bytes, &mut t, alpha),
            Some(Format::WebP) => self.webp(bytes, &mut t, alpha),
            None => Err(Error::Unknown),
        };
        if self.src.capacity() > KEEP {
            self.src = Vec::new();
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn files_are_told_apart_by_their_first_bytes() {
        assert_eq!(format(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
        assert_eq!(format(b"\x89PNG\r\n\x1a\n...."), Some(Format::Png));
        assert_eq!(format(b"RIFF\0\0\0\0WEBPVP8 "), Some(Format::WebP));
        assert_eq!(format(b"GIF89a"), None);
        assert_eq!(format(b""), None);
    }

    #[test]
    fn a_header_is_read_without_decoding_the_picture() {
        let mut png = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR".to_vec();
        png.extend_from_slice(&600u32.to_be_bytes());
        png.extend_from_slice(&400u32.to_be_bytes());
        assert_eq!(header(&png), Ok(Header { format: Format::Png, width: 600, height: 400, oriented: false }));
        assert_eq!(header(&png[..20]), Err(Error::Corrupt("empty picture".into())), "cut short");
        assert_eq!(header(b"GIF89a"), Err(Error::Unknown));
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
