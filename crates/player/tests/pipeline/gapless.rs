//! Songs that follow each other with no transition: the ear gets every sample of one and then every
//! sample of the next, nothing added, nothing lost, nothing moved.

use nori_player::sim::{Audio, Player, Track};

use crate::common::*;

/// Plays the queue to its end and checks the output never ran dry or stuttered on the way.
fn play_all(p: &mut Player, max_ms: i64) {
    p.play_from(0);
    assert!(p.run_to_end(max_ms), "the queue played to its end");
    assert!(p.sink.gaps.is_empty(), "the output never ran dry: {:?}", p.sink.gaps);
    assert_eq!(p.sink.timestamp_jumps, 0, "no timestamp jumped (a stutter on a phone)");
    assert_eq!(p.sink.rebuilds, 0, "the output was never opened again");
}

#[test]
fn one_signal_cut_in_two_plays_back_as_the_whole() {
    // Cut at a frame no buffer size divides, so the join falls inside the output's buffers.
    let whole = music(20.0, 7);
    let cut = frames(9.0) * 2 + 2 * 377;
    let mut p = Player::new(vec![track("a", &whole[..cut]), track("b", &whole[cut..])]);
    play_all(&mut p, 60_000);
    let heard = p.sink.heard_samples();
    assert_eq!(heard.len(), whole.len(), "no sample added or dropped");
    assert!(heard == whole, "sample for sample the same, first difference at {:?}", heard.iter().zip(&whole).position(|(a, b)| a != b));
    assert_eq!(p.changes.iter().map(|c| c.1).collect::<Vec<_>>(), vec![0, 1], "the player moved on to the second song");
}

#[test]
fn through_the_limiter_the_join_is_still_exact_only_later() {
    let whole = music(12.0, 17);
    let cut = frames(5.0) * 2 + 2 * 91;
    let mut p = Player::new(vec![track("a", &whole[..cut]), track("b", &whole[cut..])]);
    p.set_sound(nori_player::sim::Sound { limiter: true, ..Default::default() });
    play_all(&mut p, 30_000);
    // The look-ahead is carried across the boundary: the join is where it was, 220 frames later, and
    // the end of the queue brings out the last 220 the limiter held back.
    let heard = p.sink.heard_samples();
    assert!(heard[..440].iter().all(|&v| v == 0) && heard[440..] == whole[..]);
}

#[test]
fn a_decoded_mp3_follows_itself_sample_for_sample() {
    let audio = Audio::mp3(&testdata("tone440.mp3"));
    let once = audio.decode_all();
    let mut p = Player::new(vec![Track::new("a", audio.clone()), Track::new("b", audio.clone()), Track::new("c", audio)]);
    play_all(&mut p, 30_000);
    let heard = p.sink.heard_samples();
    assert_eq!(heard.len(), once.len() * 3);
    for (k, part) in heard.chunks(once.len()).enumerate() {
        assert!(part == once, "song {k} is the decoder's output exactly");
    }
}

#[test]
fn a_decoded_opus_follows_itself_sample_for_sample() {
    let audio = Audio::opus(&testdata("tone440.opus"));
    let once = audio.decode_all();
    assert!(once.len() / 2 >= 48_000, "a second of tone, pre-skip gone: {}", once.len() / 2);
    let mut p = Player::new(vec![Track::new("a", audio.clone()), Track::new("b", audio)]);
    play_all(&mut p, 30_000);
    let heard = p.sink.heard_samples();
    assert!(heard.len() == once.len() * 2 && heard[..once.len()] == once[..] && heard[once.len()..] == once[..]);
}

#[test]
fn an_album_in_order_stays_gapless_with_a_crossfade_on() {
    let whole = music(40.0, 11);
    let cut = frames(21.0) * 2;
    let a = track("a", &whole[..cut]).on_album("x", 1);
    let b = track("b", &whole[cut..]).on_album("x", 2);
    let mut p = Player::with_prefs(vec![a, b], crossfade(6));
    play_all(&mut p, 90_000);
    assert!(p.app.logged("planFor: gapless (same album in order"), "{:?}", p.app.log);
    assert!(p.sink.heard_samples() == whole, "the album plays straight on, sample for sample");
}

#[test]
fn a_ten_minute_song_and_its_crossfade_take_no_time() {
    // Two ten-minute songs, a twelve-second crossfade: twenty minutes of music on the virtual clock.
    let started = std::time::Instant::now();
    let (a, b) = (sine(441.0, 0.5, 1.0), sine(551.25, 0.5, 1.0));
    let ten = frames(600.0) as u64;
    let tracks = vec![Track::new("a", Audio::looped(RATE, 2, &a, ten)), Track::new("b", Audio::looped(RATE, 2, &b, ten))];
    let mut p = Player::with_prefs(tracks, crossfade(12));
    // Only the join is kept: 30 s before the crossfade to 30 s after it.
    let join = ten - frames(12.0) as u64;
    p.sink.capture = join - frames(30.0) as u64..join + frames(42.0) as u64;
    p.play_from(0);
    assert!(p.run_to_end(1_300_000), "twenty minutes played");
    assert!(p.sink.gaps.is_empty(), "no hole anywhere: {:?}", p.sink.gaps);
    assert!(p.app.logged("transition a -> b: EqualPowerFade 12000 ms at 588000"), "{:?}", p.app.log);
    assert!(!p.app.logged("letting the ending play"), "the ending was held for the mix: {:?}", p.app.log);
    assert_eq!(p.sink.heard_frames, 2 * ten - frames(12.0) as u64, "the overlap is heard once, nothing else is lost");
    let heard = p.sink.heard_samples();
    let (a_run, b_run) = (a.repeat(30), b.repeat(42));
    let (at, overlap) = (frames(30.0) * 2, frames(12.0) * 2);
    assert!(heard[..at] == a_run[..at], "the ending of a alone, untouched, up to the planned sample");
    let mix = reference_mix(&a_run[..overlap], &b_run[..overlap], &blind_plan(600_000, 600_000, 12.0));
    assert!(heard[at..at + overlap] == mix[..], "the mix is the mixer's, starting on the planned sample");
    assert!(heard[at + overlap..] == b_run[overlap..frames(42.0) * 2], "then b alone, exactly where the overlap left it");
    // In a debug build: this is the whole point of a virtual clock.
    assert!(started.elapsed().as_secs_f64() < 10.0, "took {:?}", started.elapsed());
}
