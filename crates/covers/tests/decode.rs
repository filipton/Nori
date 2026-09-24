//! The decoders and the scaler against Pillow's (libjpeg-turbo, libpng, libwebp): testdata/make.py wrote
//! the pictures and what Pillow decodes them to.

use nori_covers::{header, Alpha, Decoder, Format, Target};

fn file(name: &str) -> Vec<u8> {
    std::fs::read(format!("{}/testdata/{name}", env!("CARGO_MANIFEST_DIR"))).unwrap()
}

fn decode(name: &str, w: usize, h: usize, alpha: Alpha) -> Vec<u8> {
    Decoder::new().decode(&file(name), w, h, alpha).unwrap()
}

/// The mean and the largest difference between two pictures, over every channel.
fn diff(a: &[u8], b: &[u8]) -> (f64, u8) {
    assert_eq!(a.len(), b.len());
    let d = a.iter().zip(b).map(|(x, y)| x.abs_diff(*y));
    (d.clone().map(f64::from).sum::<f64>() / a.len() as f64, d.max().unwrap())
}

#[track_caller]
fn close(name: &str, got: &[u8], want: &[u8], mean: f64, max: u8) {
    let (m, x) = diff(got, want);
    assert!(m <= mean && x <= max, "{name}: mean {m:.2} (at most {mean}), largest {x} (at most {max})");
}

#[test]
fn lossless_pictures_decode_to_exactly_their_pixels() {
    for name in ["alpha.png", "alpha.webp", "palette.png"] {
        close(name, &decode(name, 40, 30, Alpha::Straight), &file(&format!("{name}.rgba")), 0.0, 0);
    }
    let photo = decode("photo.png", 40, 30, Alpha::Straight);
    assert!(photo.chunks_exact(4).all(|p| p[3] == 255));
}

#[test]
fn jpegs_decode_as_libjpeg_turbo_does() {
    // The two differ only in how the halved colour is brought back up: a step or two on a few pixels.
    close("baseline", &decode("photo.jpg", 40, 30, Alpha::Straight), &file("photo.jpg.rgba"), 1.0, 12);
    close("progressive", &decode("photo-progressive.jpg", 40, 30, Alpha::Straight), &file("photo-progressive.jpg.rgba"), 1.0, 12);
    close("grey", &decode("grey.jpg", 40, 30, Alpha::Straight), &file("grey.jpg.rgba"), 0.5, 2);
}

#[test]
fn a_lossy_webp_decodes_as_libwebp_does() {
    close("webp", &decode("photo.webp", 40, 30, Alpha::Straight), &file("photo.webp.rgba"), 1.5, 16);
}

#[test]
fn premultiplied_colours_are_the_straight_ones_times_alpha() {
    let straight = decode("alpha.png", 40, 30, Alpha::Straight);
    let pre = decode("alpha.png", 40, 30, Alpha::Premultiplied);
    for (s, p) in straight.chunks_exact(4).zip(pre.chunks_exact(4)) {
        let a = s[3] as u32;
        assert_eq!(p, [0, 1, 2].map(|i| ((s[i] as u32 * a + 127) / 255) as u8).iter().chain([&s[3]]).copied().collect::<Vec<_>>());
    }
}

#[test]
fn shrinking_averages_exactly_the_area_under_each_pixel() {
    // 40x30 filling 12x12: the middle 30x30, 2.5 source pixels to one.
    close("area", &decode("photo.png", 12, 12, Alpha::Straight), &file("photo-12-area.rgba"), 0.1, 1);
}

#[test]
fn growing_is_pillows_bilinear() {
    close("bilinear", &decode("photo.png", 50, 50, Alpha::Straight), &file("photo-50-bilinear.rgba"), 0.1, 1);
}

#[test]
fn a_jpeg_decoded_whole_is_averaged_exactly() {
    let mut d = Decoder::new();
    d.set_idct_scaling(false);
    let d = d.decode(&file("photo.jpg"), 12, 12, Alpha::Straight).unwrap();
    close("whole", &d, &file("photo.jpg-12-area.rgba"), 0.5, 3);
}

#[test]
fn a_jpeg_shrunk_by_its_idct_stays_near_libjpeg_turbos_own_shrinking() {
    // 2.5 times too big: decoded at half size by the IDCT, then averaged the rest of the way. The
    // reference is libjpeg-turbo's half-size decode averaged the same way. This picture is small and busy
    // (half its blocks are cut by an edge); on real covers the two differ by about one step on average.
    close("idct", &decode("photo.jpg", 12, 12, Alpha::Straight), &file("photo.jpg-12-half.rgba"), 4.5, 32);
}

#[test]
fn pictures_are_written_into_padded_rows_as_a_bitmap_has_them() {
    let want = decode("photo.jpg", 40, 30, Alpha::Premultiplied);
    let stride = 40 * 4 + 16;
    let mut px = vec![0xAB; stride * 30];
    Decoder::new().decode_into(&file("photo.jpg"), Target { px: &mut px, width: 40, height: 30, stride }, Alpha::Premultiplied).unwrap();
    for y in 0..30 {
        close("row", &px[y * stride..][..160], &want[y * 160..][..160], 0.0, 0);
        assert!(px[y * stride + 160..][..16].iter().all(|&b| b == 0xAB));
    }
}

#[test]
fn one_decoder_serves_picture_after_picture() {
    let mut d = Decoder::new();
    for name in ["photo.jpg", "alpha.png", "photo.webp", "grey.jpg", "palette.png", "photo.gif", "turned-6.jpg", "photo.jpg"] {
        for side in [7, 12, 30, 64] {
            let px = d.decode(&file(name), side, side, Alpha::Premultiplied).unwrap();
            assert_eq!(px.len(), side * side * 4);
        }
    }
}

#[test]
fn a_broken_file_is_an_error_not_a_panic() {
    let mut d = Decoder::new();
    for name in ["photo.jpg", "photo.png", "photo.webp", "alpha.webp", "photo.gif", "turned-6.jpg", "turned-6.webp"] {
        let f = file(name);
        for cut in [4, 20, f.len() / 2, f.len() - 3] {
            let _ = d.decode(&f[..cut], 16, 16, Alpha::Straight);
            let _ = header(&f[..cut]);
        }
        let mut junk = f.clone();
        for b in junk.iter_mut().skip(30).step_by(7) {
            *b ^= 0x5A;
        }
        let _ = d.decode(&junk, 16, 16, Alpha::Straight);
        let _ = header(&junk);
    }
}

#[test]
fn every_format_says_its_size_from_its_headers() {
    for (name, format) in [
        ("photo.jpg", Format::Jpeg),
        ("photo-progressive.jpg", Format::Jpeg),
        ("grey.jpg", Format::Jpeg),
        ("photo.png", Format::Png),
        ("palette.png", Format::Png),
        ("photo.webp", Format::WebP),
        ("alpha.webp", Format::WebP),
        ("photo.gif", Format::Gif),
        ("part.gif", Format::Gif),
        ("turned-3.jpg", Format::Jpeg),
        ("turned-2.jpg", Format::Jpeg),
    ] {
        let h = header(&file(name)).unwrap();
        assert_eq!((h.format, h.width, h.height), (format, 40, 30), "{name}");
    }
    // A quarter turn is a picture the other way up: its size as it is shown.
    for name in ["turned-6.jpg", "turned-8.jpg", "turned-5.jpg", "turned-7.jpg", "turned-6.png", "turned-6.webp"] {
        let h = header(&file(name)).unwrap();
        assert_eq!((h.width, h.height), (30, 40), "{name}");
    }
    assert_eq!(header(&file("turned-7.jpg")).unwrap().orientation, 7);
    assert_eq!(header(&file("photo.jpg")).unwrap().orientation, 1);
}

#[test]
fn a_gif_is_its_first_frame() {
    close("gif", &decode("photo.gif", 40, 30, Alpha::Straight), &file("photo.gif.rgba"), 0.0, 0);
}

#[test]
fn a_gif_frame_smaller_than_its_screen_leaves_the_rest_transparent() {
    let px = decode("part.gif", 40, 30, Alpha::Premultiplied);
    for y in 0..30 {
        for x in 0..40 {
            let p = &px[(y * 40 + x) * 4..][..4];
            let inside = (8..28).contains(&x) && (6..16).contains(&y);
            assert_eq!(p, if inside { [250, 40, 60, 255] } else { [0, 0, 0, 0] }, "({x}, {y})");
        }
    }
}

/// A GIF with a `sw` x `sh` screen and one frame `fw` x `fh` at `left`, `top`, whose pixels are one
/// black pixel's worth of LZW (a frame claiming more is cut short, which is not what these tests reach).
fn gif(sw: u16, sh: u16, left: u16, top: u16, fw: u16, fh: u16) -> Vec<u8> {
    let mut g = b"GIF89a".to_vec();
    g.extend([sw, sh].iter().flat_map(|v| v.to_le_bytes()));
    // A global table of two colours, black and white.
    g.extend([0x80, 0, 0, 0, 0, 0, 255, 255, 255]);
    g.push(0x2C);
    g.extend([left, top, fw, fh].iter().flat_map(|v| v.to_le_bytes()));
    // No local table; LZW with 2-bit codes: clear, 0, end.
    g.extend([0, 2, 2, 0x44, 0x01, 0, 0x3B]);
    g
}

#[test]
fn a_gif_frame_is_decoded_only_where_it_lies_on_its_screen_and_within_the_limits() {
    let one = Decoder::new().decode(&gif(1, 1, 0, 0, 1, 1), 1, 1, Alpha::Straight).unwrap();
    assert_eq!(one, [0, 0, 0, 255], "the builder's GIF is a black pixel");
    // A 1x1 screen carrying a 65535x65535 frame: 17 GB of RGBA, refused before any of it is made.
    assert!(Decoder::new().decode(&gif(1, 1, 0, 0, 65535, 65535), 1, 1, Alpha::Straight).is_err());
    // Frames that stick out past the screen's right or bottom edge, or start beyond it.
    for (left, top, fw, fh) in [(200, 9, 1, 1), (9, 200, 1, 1), (5, 0, 8, 1), (0, 5, 1, 8), (65535, 65535, 1, 1)] {
        let r = Decoder::new().decode(&gif(10, 10, left, top, fw, fh), 10, 10, Alpha::Straight);
        assert!(r.is_err(), "a frame {fw}x{fh} at {left},{top} on a 10x10 screen");
    }
}

/// `px`, `w` x `h` RGBA rows, as EXIF orientation `turn` shows them, written out plainly: the reference
/// for the decoder's turns.
fn turn(px: &[u8], w: usize, h: usize, turn: u8) -> Vec<u8> {
    let quarter = turn >= 5;
    let (ow, oh) = if quarter { (h, w) } else { (w, h) };
    let mut out = Vec::with_capacity(px.len());
    for y in 0..oh {
        for x in 0..ow {
            // Where this pixel is in the stored picture: mirrored first (2, 4, 5, 7), then turned.
            let (sx, sy) = match turn {
                2 => (w - 1 - x, y),
                3 => (w - 1 - x, h - 1 - y),
                4 => (x, h - 1 - y),
                5 => (y, x),
                6 => (y, h - 1 - x),
                7 => (w - 1 - y, h - 1 - x),
                8 => (w - 1 - y, x),
                _ => (x, y),
            };
            out.extend_from_slice(&px[(sy * w + sx) * 4..][..4]);
        }
    }
    out
}

#[test]
fn a_picture_is_turned_and_mirrored_the_way_its_exif_says() {
    // The same pixels as photo.jpg, stored with each orientation: decoded, each is photo.jpg turned.
    let plain = decode("photo.jpg", 40, 30, Alpha::Straight);
    for o in [2u8, 3, 5, 6, 7, 8] {
        let (w, h) = if o >= 5 { (30, 40) } else { (40, 30) };
        close(&format!("turned-{o}"), &decode(&format!("turned-{o}.jpg"), w, h, Alpha::Straight), &turn(&plain, 40, 30, o), 0.0, 0);
    }
    // And each is what Pillow shows for the file (exif_transpose over libjpeg-turbo's decode).
    for o in [2u8, 3, 5, 6, 7, 8] {
        let (w, h) = if o >= 5 { (30, 40) } else { (40, 30) };
        let name = format!("turned-{o}.jpg");
        close(&name, &decode(&name, w, h, Alpha::Straight), &file(&format!("{name}.rgba")), 1.0, 12);
    }
    // A quarter turn clockwise puts the stored picture's bottom left corner at the top left.
    let turned = decode("turned-6.jpg", 30, 40, Alpha::Straight);
    assert_eq!(&turned[..4], &plain[29 * 40 * 4..][..4]);
    // PNG keeps its EXIF in an eXIf chunk and WebP in an EXIF chunk.
    close("png", &decode("turned-6.png", 30, 40, Alpha::Straight), &turn(&decode("photo.png", 40, 30, Alpha::Straight), 40, 30, 6), 0.0, 0);
    let webp = decode("photo.webp", 40, 30, Alpha::Straight);
    close("webp", &decode("turned-6.webp", 30, 40, Alpha::Straight), &turn(&webp, 40, 30, 6), 0.0, 0);
}

#[test]
fn a_turned_picture_is_cut_and_scaled_as_it_is_shown() {
    // Filling a square: the middle of the turned picture, which is the middle of the stored one turned.
    let plain = decode("photo.jpg", 30, 30, Alpha::Straight);
    close("square", &decode("turned-8.jpg", 30, 30, Alpha::Straight), &turn(&plain, 30, 30, 8), 0.0, 0);
    let mut d = Decoder::new();
    let small = d.decode(&file("turned-6.jpg"), 12, 16, Alpha::Straight).unwrap();
    close("scaled", &small, &turn(&d.decode(&file("photo.jpg"), 16, 12, Alpha::Straight).unwrap(), 16, 12, 6), 0.0, 0);
}
