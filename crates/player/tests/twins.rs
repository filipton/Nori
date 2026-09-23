//! nori-player's twins held to their Kotlin originals: testdata/twins/player_twins.tsv is what the Kotlin
//! (copied into testdata/twins/PlayerTwins.kt) answered on the JVM, written by tools/twins.sh. Every
//! answer must match exactly.

use nori_player::{packets, sink};

const TABLE: &str = include_str!("../testdata/twins/player_twins.tsv");

fn rows(case: &str) -> Vec<Vec<&'static str>> {
    let rows: Vec<Vec<&str>> =
        TABLE.lines().map(|l| l.split('\t').collect::<Vec<_>>()).filter(|f| f[0] == case).map(|f| f[1..].to_vec()).collect();
    assert!(!rows.is_empty(), "no vectors for {case}");
    rows
}

/// Bytes in hex: "_" none of them, "-" no array at all.
fn bytes(hex: &str) -> Option<Vec<u8>> {
    match hex {
        "-" => None,
        "_" => Some(Vec::new()),
        _ => Some((0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()),
    }
}

fn maybe(s: &str) -> Option<i32> {
    (s != "-").then(|| s.parse().unwrap())
}

#[test]
fn setup_data_joins_in_order() {
    for case in ["setup_extracted", "setup_format"] {
        for r in rows(case) {
            let parts: Vec<Vec<u8>> = if r[0] == "-" { Vec::new() } else { r[0].split(',').map(|p| bytes(p).unwrap()).collect() };
            assert_eq!(packets::join_setup(parts.iter().map(Vec::as_slice)), bytes(r[1]), "{case} {r:?}");
        }
    }
}

#[test]
fn aac_codec_strings() {
    for r in rows("codecs") {
        assert_eq!(packets::aac_codecs(maybe(r[0])).as_deref(), (r[1] != "-").then_some(r[1]), "{r:?}");
    }
}

#[test]
fn packet_buffers() {
    for r in rows("measuring_buffer") {
        assert_eq!(packets::packet_bytes(maybe(r[0]), true), r[1].parse::<i32>().unwrap(), "{r:?}");
    }
    // media3 says "not known" with Format.NO_VALUE, -1: that is none here.
    for r in rows("decoder_buffer") {
        let max: i32 = r[0].parse().unwrap();
        assert_eq!(packets::packet_bytes((max != -1).then_some(max), false), r[1].parse::<i32>().unwrap(), "{r:?}");
    }
    for r in rows("grown") {
        assert_eq!(packets::grown(r[0].parse().unwrap(), r[1].parse().unwrap()), r[2].parse::<i32>().unwrap(), "{r:?}");
    }
}

#[test]
fn what_the_engine_is_told_a_stream_is() {
    for r in rows("encoding") {
        assert_eq!(sink::sample_encoding(r[0] == "audio/raw", r[1].parse().unwrap()), r[2].parse::<i32>().unwrap(), "{r:?}");
    }
}

#[test]
fn formats_kept_by_token() {
    let (mut kept, mut next) = (Vec::<i32>::new(), 0);
    for r in rows("configs") {
        if r[0] == "40" {
            kept.clear();
        } else {
            let token = next;
            next += 1;
            kept.push(token);
            if let Some(below) = sink::formats_below(kept.len(), token) {
                kept.retain(|&t| t >= below);
            }
        }
        let want: Vec<i32> = if r[1] == "_" { Vec::new() } else { r[1].split(',').map(|t| t.parse().unwrap()).collect() };
        assert_eq!(kept, want, "{r:?}");
    }
}
