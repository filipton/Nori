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
    for name in ["photo.jpg", "alpha.png", "photo.webp", "grey.jpg", "palette.png", "photo.jpg"] {
        for side in [7, 12, 30, 64] {
            let px = d.decode(&file(name), side, side, Alpha::Premultiplied).unwrap();
            assert_eq!(px.len(), side * side * 4);
        }
    }
}

#[test]
fn a_broken_file_is_an_error_not_a_panic() {
    let mut d = Decoder::new();
    for name in ["photo.jpg", "photo.png", "photo.webp", "alpha.webp"] {
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
    ] {
        let h = header(&file(name)).unwrap();
        assert_eq!((h.format, h.width, h.height, h.oriented), (format, 40, 30, false), "{name}");
    }
}
