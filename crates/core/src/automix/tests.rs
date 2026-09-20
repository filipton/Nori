//! Whole-pipeline tests on synthetic tracks with a known tempo, downbeat, key and structure.

use std::f64::consts::PI;

use super::structure::camelot;
use super::*;
use crate::Core;

/// A deterministic noise source (xorshift), so tests never flake.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> f64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 11) as f64 / (1u64 << 53) as f64 * 2.0 - 1.0
    }
}

#[derive(Clone)]
struct Synth {
    rate: u32,
    bpm: f64,
    secs: f64,
    /// Time of beat 0, seconds.
    first_beat: f64,
    /// Which beat index (mod 4) carries the kick that marks the bar.
    downbeat: usize,
    /// Off-beat click at this fraction of the beat (0.5 straight, 0.67 swung); 0 for none.
    offbeat: f64,
    /// White noise level, linear.
    noise: f64,
    lead_silence: f64,
    tail_silence: f64,
    /// Chords (root pitch class, minor), one per bar, cycling; empty for no harmony.
    chords: Vec<(usize, bool)>,
    /// Bars at the start that only have the hats (no kick, no chords), and at the end.
    intro_bars: usize,
    outro_bars: usize,
    /// Tempo at the end, for drifting tracks (0 = steady).
    end_bpm: f64,
}

impl Synth {
    fn new(bpm: f64) -> Self {
        Synth {
            rate: 44100,
            bpm,
            secs: 60.0,
            first_beat: 0.25,
            downbeat: 0,
            offbeat: 0.0,
            noise: 0.0,
            lead_silence: 0.0,
            tail_silence: 0.0,
            chords: Vec::new(),
            intro_bars: 0,
            outro_bars: 0,
            end_bpm: 0.0,
        }
    }

    /// Beat times, seconds.
    fn beats(&self) -> Vec<f64> {
        let mut t = self.lead_silence + self.first_beat;
        let end = self.lead_silence + self.secs;
        let mut out = Vec::new();
        while t < end {
            out.push(t);
            let bpm = if self.end_bpm > 0.0 { self.bpm + (self.end_bpm - self.bpm) * (t - self.lead_silence) / self.secs } else { self.bpm };
            t += 60.0 / bpm;
        }
        out
    }

    fn render(&self) -> Vec<f32> {
        let rate = self.rate as f64;
        let total = ((self.lead_silence + self.secs + self.tail_silence) * rate) as usize;
        let music_end = ((self.lead_silence + self.secs) * rate) as usize;
        let mut x = vec![0f64; total];
        let beats = self.beats();
        let bars = beats.len() / 4;
        let add = |x: &mut Vec<f64>, at: f64, freq: f64, amp: f64, decay_s: f64, len_s: f64| {
            let s = (at * rate) as usize;
            let n = (len_s * rate) as usize;
            for i in 0..n {
                if s + i >= music_end {
                    break;
                }
                let t = i as f64 / rate;
                x[s + i] += amp * (-t / decay_s).exp() * (2.0 * PI * freq * t).sin();
            }
        };
        for (i, &t) in beats.iter().enumerate() {
            let bar = i / 4;
            let quiet = bar < self.intro_bars || bar + self.outro_bars >= bars;
            // Hat on every beat: a noisy high click.
            add(&mut x, t, 6000.0, 0.25, 0.01, 0.04);
            add(&mut x, t, 3100.0, 0.2, 0.012, 0.04);
            if !quiet && i % 4 == self.downbeat {
                add(&mut x, t, 55.0, 0.8, 0.12, 0.4);
            } else if !quiet {
                // A low tom under the chroma range, so the drums do not vote for a key.
                add(&mut x, t, 90.0, 0.25, 0.04, 0.12);
            }
            if self.offbeat > 0.0 {
                let period = beats.get(i + 1).map_or(60.0 / self.bpm, |n| n - t);
                add(&mut x, t + self.offbeat * period, 7000.0, 0.12, 0.008, 0.03);
            }
        }
        if !self.chords.is_empty() {
            // Chords change on the bar lines of the kick.
            let starts: Vec<f64> = beats.iter().enumerate().filter(|(i, _)| i % 4 == self.downbeat).map(|(_, t)| *t).collect();
            for (b, w) in starts.windows(2).enumerate() {
                let bar = b + usize::from(self.downbeat > 0);
                if bar < self.intro_bars || bar + self.outro_bars >= bars {
                    continue;
                }
                let (root, minor) = self.chords[b % self.chords.len()];
                let third = if minor { 3 } else { 4 };
                for (k, iv) in [0usize, third, 7].iter().enumerate() {
                    let midi = 48 + root + iv + if k == 0 { 0 } else { 12 };
                    let f = 440.0 * 2f64.powf((midi as f64 - 69.0) / 12.0);
                    for h in 1..=3 {
                        add(&mut x, w[0], f * h as f64, 0.08 / h as f64, 2.0, w[1] - w[0]);
                    }
                }
            }
        }
        let mut rng = Rng(0x9E3779B97F4A7C15);
        let start = (self.lead_silence * rate) as usize;
        for v in &mut x[start..music_end] {
            *v += self.noise * rng.next();
        }
        x.iter().map(|v| v.clamp(-1.0, 1.0) as f32).collect()
    }
}

fn octave_ok(got: f64, want: f64, tol: f64) -> bool {
    [0.5, 1.0, 2.0].iter().any(|k| (got / (want * k) - 1.0).abs() < tol)
}

/// Tempo accuracy on the synthetic set: click tracks at 90/120/128/174 BPM, straight, swung, and with noise.
/// Prints the table the report quotes.
#[test]
fn tempo_accuracy_on_synthetic_tracks() {
    let mut acc1 = 0;
    let mut acc2 = 0;
    let mut n = 0;
    for bpm in [90.0, 120.0, 128.0, 174.0] {
        for (name, synth) in [
            ("plain", Synth::new(bpm)),
            ("swing", Synth { offbeat: 0.67, ..Synth::new(bpm) }),
            ("noise", Synth { noise: 0.15, ..Synth::new(bpm) }),
            ("swing+noise+chords", Synth { offbeat: 0.67, noise: 0.1, chords: vec![(0, false), (5, false)], ..Synth::new(bpm) }),
        ] {
            let a = analyse("t", &synth.render(), synth.rate);
            let t = &a.track;
            let err = (t.bpm / bpm - 1.0) * 100.0;
            println!("{bpm:>5} {name:<20} bpm {:>7.2} ({err:+.2} %) raw {:>7.2} conf {:.2} stab {:.2} downbeat {} ({:.2})", t.bpm, a.tempo.raw_bpm, t.bpm_confidence, t.stability, t.downbeat_phase, t.downbeat_confidence);
            n += 1;
            if (t.bpm / bpm - 1.0).abs() < 0.04 {
                acc1 += 1;
            }
            if octave_ok(t.bpm, bpm, 0.04) {
                acc2 += 1;
            }
            assert!(octave_ok(t.bpm, bpm, 0.01), "{bpm} {name}: {}", t.bpm);
            assert!(t.bpm_confidence >= 0.5, "{bpm} {name}: confidence {}", t.bpm_confidence);
            assert!(t.stability >= 0.6, "{bpm} {name}: stability {}", t.stability);
        }
    }
    println!("Acc1 {acc1}/{n}, Acc2 {acc2}/{n}");
    assert!(acc1 >= n - 2, "Acc1 {acc1}/{n}");
}

#[test]
fn grid_bpm_is_precise() {
    for bpm in [90.0, 123.0, 128.0, 140.5] {
        let s = Synth { secs: 90.0, ..Synth::new(bpm) };
        let t = analyse("t", &s.render(), s.rate).track;
        assert!((t.bpm - bpm).abs() < 0.05, "{bpm}: {}", t.bpm);
    }
}

/// The grid lands on the clicks: `offset + n * period` within a few ms of every true beat.
#[test]
fn beat_times_line_up_with_the_clicks() {
    for bpm in [90.0, 128.0, 174.0] {
        for first in [0.1, 0.33] {
            let s = Synth { first_beat: first, lead_silence: 1.5, ..Synth::new(bpm) };
            let a = analyse("t", &s.render(), s.rate);
            let t = &a.track;
            let period = 60_000.0 / t.bpm;
            let mut worst = 0f64;
            let mut mean = 0f64;
            let truth = s.beats();
            for b in &truth {
                let ms = b * 1000.0;
                let n = ((ms - t.beat_offset_ms) / period).round();
                let d = ms - (t.beat_offset_ms + n * period);
                worst = worst.max(d.abs());
                mean += d;
            }
            mean /= truth.len() as f64;
            println!("{bpm} first {first}: grid error mean {mean:+.2} ms, worst {worst:.2} ms");
            assert!(worst < 8.0, "{bpm}/{first}: grid is {worst} ms off (mean {mean})");
            assert!(t.beat_offset_ms >= 0.0 && t.beat_offset_ms < period);
        }
    }
}

#[test]
fn downbeats_follow_the_kick() {
    for phase in 0..4 {
        let s = Synth { downbeat: phase, chords: vec![(0, false), (7, false), (9, true), (5, false)], ..Synth::new(124.0) };
        let t = analyse("t", &s.render(), s.rate).track;
        // Beat 0 of the synth is the first grid beat: the offset is its time.
        let period = 60_000.0 / t.bpm;
        let first_grid = ((s.first_beat * 1000.0 - t.beat_offset_ms) / period).round() as i64;
        let want = (phase as i64 + first_grid).rem_euclid(4) as i32;
        assert_eq!(t.downbeat_phase, want, "kick on beat {phase}");
        assert!(t.downbeat_confidence >= 0.5, "confidence {}", t.downbeat_confidence);
    }
}

#[test]
fn noise_is_reported_as_unreliable() {
    let rate = 44100;
    let mut rng = Rng(12345);
    let x: Vec<f32> = (0..rate * 60).map(|_| (0.3 * rng.next()) as f32).collect();
    let t = analyse("n", &x, rate as u32).track;
    println!("noise: bpm {:.1} conf {:.2} stab {:.2} key {} ({:.2}) downbeat conf {:.2}", t.bpm, t.bpm_confidence, t.stability, t.key, t.key_confidence, t.downbeat_confidence);
    assert!(t.bpm_confidence < 0.3, "noise got a confident tempo: {}", t.bpm_confidence);
    assert!(t.key_confidence < 0.2, "noise got a confident key: {}", t.key_confidence);
    let p = plan_transition(Some(t.clone()), Some(t.clone()), 60_000, 60_000, crate::AutoMixSettings::default());
    assert_ne!(p.kind, crate::TransitionKind::BeatMatched);

    // Brown-ish noise with slow swells: still no confident beat.
    let mut y = 0f64;
    let x: Vec<f32> = (0..rate * 60)
        .map(|i| {
            y = 0.995 * y + 0.05 * rng.next();
            (y * (0.6 + 0.4 * (i as f64 / rate as f64 * 0.37).sin())) as f32
        })
        .collect();
    let t = analyse("n", &x, rate as u32).track;
    assert!(t.bpm_confidence < 0.3, "brown noise: {}", t.bpm_confidence);
}

#[test]
fn silence_and_trims() {    let t = analyse("s", &vec![0f32; 44100 * 20], 44100).track;
    assert_eq!((t.bpm, t.bpm_confidence, t.lufs, t.key), (0.0, 0.0, -70.0, 0));
    assert_eq!((t.silence_start_ms, t.silence_end_ms), (0, 0));
    assert_eq!(t.duration_ms, 20_000);

    let s = Synth { lead_silence: 3.0, tail_silence: 5.0, ..Synth::new(120.0) };
    let t = analyse("t", &s.render(), s.rate).track;
    assert!((t.silence_start_ms - 3200).abs() <= 100, "lead {}", t.silence_start_ms); // first hat at 3.25 s
    assert!((t.silence_end_ms - 63_000).abs() <= 200, "tail {}", t.silence_end_ms);
    assert!(t.mixramp_start_ms >= t.silence_start_ms && t.mixramp_start_ms < t.silence_start_ms + 1000, "{t:?}");
    assert!(t.mixramp_end_ms <= t.silence_end_ms && t.mixramp_end_ms > t.silence_end_ms - 1000, "{t:?}");
    assert!(t.lufs > -30.0 && t.lufs < -5.0, "lufs {}", t.lufs);
    assert!((t.bpm - 120.0).abs() < 0.1);
}

#[test]
fn overlap_windows_describe_vocals_and_brightness() {
    // Sustained chords put pitched energy in the voice band; drums alone are clicks and hats.
    let sung = analyse("t", &Synth { chords: vec![(0, false), (5, false)], ..Synth::new(128.0) }.render(), 44100).track;
    let drums = analyse("t", &Synth::new(128.0).render(), 44100).track;
    assert!(sung.outro_vocal > drums.outro_vocal, "{} vs {}", sung.outro_vocal, drums.outro_vocal);
    assert!(sung.intro_vocal > drums.intro_vocal, "{} vs {}", sung.intro_vocal, drums.intro_vocal);
    // The kick carries drums-only power (low centroid); chords pull it up into the voice band.
    // Either way the windows disagree by well over half an octave - the mismatch signal is real.
    assert!((sung.outro_centroid / drums.outro_centroid).log2().abs() > 0.5, "{} vs {}", sung.outro_centroid, drums.outro_centroid);
    for t in [&sung, &drums] {
        assert!((0.0..=1.0).contains(&t.outro_vocal) && (0.0..=1.0).contains(&t.intro_vocal));
        assert!(t.outro_centroid > 0.0 && t.intro_centroid > 0.0);
    }
    let silent = analyse("s", &vec![0f32; 44100 * 20], 44100).track;
    assert_eq!((silent.outro_vocal, silent.intro_vocal, silent.outro_centroid, silent.intro_centroid), (0.0, 0.0, 0.0, 0.0));
}

#[test]
fn the_sample_rate_does_not_matter() {
    let mut got = Vec::new();
    for rate in [22050, 32000, 44100, 48000, 96000] {
        let s = Synth { rate, chords: vec![(9, true), (2, true)], ..Synth::new(126.0) };
        let t = analyse("t", &s.render(), rate).track;
        got.push((rate, t.bpm, t.beat_offset_ms, t.key));
    }
    println!("{got:?}");
    for (rate, bpm, offset, key) in &got {
        assert!((bpm - 126.0).abs() < 0.1, "{rate}: {bpm}");
        assert!((offset - got[2].2).abs() < 6.0, "{rate}: offset {offset} vs {}", got[2].2);
        assert_eq!(*key, got[2].3, "{rate}");
    }
}

#[test]
fn streaming_in_odd_chunks_gives_the_same_answer() {
    let s = Synth { chords: vec![(0, false), (5, false)], ..Synth::new(128.0) };
    let x = s.render();
    let whole = analyse("t", &x, s.rate).track;
    let mut a = analysis::Analyzer::new(s.rate, 0);
    let mut i = 0;
    let mut step = 1;
    while i < x.len() {
        let n = step.min(x.len() - i);
        a.feed(&x[i..i + n]);
        i += n;
        step = step * 7 % 4093 + 1;
    }
    let streamed = finish("t", &a.take_features()).track;
    assert_eq!(TrackAnalysis { analysed_ms: 0, ..whole }, TrackAnalysis { analysed_ms: 0, ..streamed });

    // Interleaved stereo 16-bit through the same door the JNI tap uses.
    let pcm: Vec<i16> = x.iter().flat_map(|v| {
        let s = (v * 32767.0) as i16;
        [s, s]
    }).collect();
    a.feed_interleaved(&pcm, 2, |v| v as f32 / 32768.0);
    let t = finish("t", &a.take_features()).track;
    assert!((t.bpm - 128.0).abs() < 0.05);
}

/// The coarse uniffi entry: decoder bytes, 16-bit stereo or float mono, give the same answer as the f32 path.
#[test]
fn decoder_bytes_in_any_layout() {
    let s = Synth { rate: 48000, chords: vec![(9, true), (4, false)], ..Synth::new(96.0) };
    let x = s.render();
    let reference = analyse("t", &x, s.rate).track;
    let stereo16: Vec<u8> = x.iter().flat_map(|v| {
        let b = ((v * 32767.0).round() as i16).to_le_bytes();
        [b[0], b[1], b[0], b[1]]
    }).collect();
    let t = automix_analyse("t".into(), stereo16, 48000, 2, PCM_16);
    assert!((t.bpm - reference.bpm).abs() < 0.01 && t.key == reference.key, "{t:?}");
    assert!((t.beat_offset_ms - reference.beat_offset_ms).abs() < 1.0);
    assert_eq!(t.duration_ms, reference.duration_ms);
    let mono: Vec<u8> = x.iter().flat_map(|v| v.to_le_bytes()).collect();
    let t = automix_analyse("t".into(), mono, 48000, 1, PCM_FLOAT);
    assert_eq!(TrackAnalysis { analysed_ms: 0, ..t }, TrackAnalysis { analysed_ms: 0, ..reference });
    // Garbage in: nothing to analyse, no panic.
    let t = automix_analyse("t".into(), vec![1, 2, 3], 0, 0, 99);
    assert!(t.bpm == 0.0 && t.duration_ms <= 1, "{t:?}");
}

#[test]
fn keys_of_simple_progressions() {
    for (chords, want) in [
        (vec![(0, false), (5, false), (7, false), (0, false)], camelot(0, false)), // C F G C
        (vec![(9, true), (2, true), (4, false), (9, true)], camelot(9, true)),     // Am Dm E Am
        (vec![(7, false), (0, false), (2, false), (7, false)], camelot(7, false)), // G C D G
        (vec![(2, true), (7, true), (9, false), (2, true)], camelot(2, true)),     // Dm Gm A Dm
    ] {
        let s = Synth { chords, ..Synth::new(110.0) };
        let t = analyse("t", &s.render(), s.rate).track;
        println!("key want {} got {} ({:.2})", structure::camelot_name(want), structure::camelot_name(t.key), t.key_confidence);
        assert_eq!(t.key, want, "got {}", structure::camelot_name(t.key));
        assert!(t.key_confidence >= 0.3, "confidence {}", t.key_confidence);
    }
    // Drums only: no key worth trusting.
    let s = Synth::new(110.0);
    let t = analyse("t", &s.render(), s.rate).track;
    assert!(t.key_confidence < 0.3, "drums got key confidence {}", t.key_confidence);
}

#[test]
fn a_drifting_tempo_is_flagged_unstable() {
    let s = Synth { secs: 120.0, end_bpm: 132.0, ..Synth::new(120.0) };
    let t = analyse("t", &s.render(), s.rate).track;
    println!("drift: bpm {:.2} conf {:.2} stability {:.2}", t.bpm, t.bpm_confidence, t.stability);
    assert!(t.stability < 0.6, "stability {}", t.stability);
}

#[test]
fn phrase_cues_land_on_the_structure() {
    // 16 bars of hats only, full groove, 16 bars of hats only at the end. 128 BPM: a bar is 1.875 s.
    let s = Synth { secs: 150.0, intro_bars: 16, outro_bars: 16, chords: vec![(0, false), (5, false)], ..Synth::new(128.0) };
    let t = analyse("t", &s.render(), s.rate).track;
    let bar = 4.0 * 60_000.0 / 128.0;
    let intro_want = (s.first_beat * 1000.0) + 16.0 * bar;
    let bars = s.beats().len() / 4;
    let outro_want = (s.first_beat * 1000.0) + (bars - 16) as f64 * bar;
    println!("intro {} (want {intro_want:.0}), outro {} (want {outro_want:.0})", t.intro_end_ms, t.outro_start_ms);
    assert!((t.intro_end_ms as f64 - intro_want).abs() < 60.0, "intro {}", t.intro_end_ms);
    assert!((t.outro_start_ms as f64 - outro_want).abs() < 60.0, "outro {}", t.outro_start_ms);

    // A steady track: no intro, the outro cue is a phrase boundary at least 16 bars before the end.
    let s = Synth { secs: 120.0, ..Synth::new(128.0) };
    let t = analyse("t", &s.render(), s.rate).track;
    assert!(t.intro_end_ms <= t.silence_start_ms + 1, "{t:?}");
    let from_first = (t.outro_start_ms as f64 - s.first_beat * 1000.0) / bar;
    assert!((from_first / 8.0 - (from_first / 8.0).round()).abs() < 0.02, "on an 8-bar line: {from_first}");
    assert!(t.silence_end_ms as f64 - t.outro_start_ms as f64 >= 16.0 * bar - 100.0);
}

#[test]
fn analysis_is_stored_and_reported_missing() {
    let core = Core::new(String::new()).unwrap();
    assert_eq!(core.analysis_get("a".into()).unwrap(), None);
    assert_eq!(core.analysis_missing(vec!["a".into(), "b".into()]).unwrap(), vec!["a".to_string(), "b".to_string()]);
    let s = Synth::new(120.0);
    let bytes: Vec<u8> = s.render().iter().flat_map(|v| v.to_le_bytes()).collect();
    let t = core.analysis_run("a".into(), bytes, s.rate as i32, 1, PCM_FLOAT).unwrap();
    assert_eq!(core.analysis_get("a".into()).unwrap(), Some(t.clone()));
    assert_eq!(core.analysis_missing(vec!["a".into(), "b".into()]).unwrap(), vec!["b".to_string()]);
    core.analysis_store(TrackAnalysis { song_id: "b".into(), analysis_version: ANALYSIS_VERSION - 1, ..t.clone() }).unwrap();
    assert_eq!(core.analysis_missing(vec!["a".into(), "b".into()]).unwrap(), vec!["b".to_string()], "old versions are redone");
    core.analysis_store(TrackAnalysis { bpm: 99.5, ..t.clone() }).unwrap();
    assert_eq!(core.analysis_get("a".into()).unwrap().unwrap().bpm, 99.5, "store replaces");
}

#[test]
fn a_v1_database_migrates_to_v2() {
    // A database from before the overlap-window columns: migrate keeps the row readable with
    // zeroed windows, and the old version still reports the track for re-analysis.
    let dir = std::env::temp_dir().join(format!("flint-mig-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("mig.db").to_string_lossy().into_owned();
    {
        let c = rusqlite::Connection::open(&path).unwrap();
        c.execute_batch("CREATE TABLE track_analysis(song_id TEXT PRIMARY KEY, analysis_version INTEGER NOT NULL, duration_ms INTEGER NOT NULL, bpm REAL NOT NULL, bpm_confidence REAL NOT NULL, beat_offset_ms REAL NOT NULL, stability REAL NOT NULL, downbeat_phase INTEGER NOT NULL, downbeat_confidence REAL NOT NULL, lufs REAL NOT NULL, key INTEGER NOT NULL, key_confidence REAL NOT NULL, silence_start_ms INTEGER NOT NULL, silence_end_ms INTEGER NOT NULL, mixramp_start_ms INTEGER NOT NULL, mixramp_end_ms INTEGER NOT NULL, intro_end_ms INTEGER NOT NULL, outro_start_ms INTEGER NOT NULL, analysed_ms INTEGER NOT NULL) WITHOUT ROWID").unwrap();
        c.execute("INSERT INTO track_analysis VALUES('v1', 1, 240000, 128.0, 0.9, 120.0, 0.9, 0, 0.8, -9.0, 8, 0.8, 100, 238500, 400, 236000, 15000, 220000, 0)", []).unwrap();
    }
    let core = Core::new(path.clone()).unwrap();
    let a = core.analysis_get("v1".into()).unwrap().unwrap();
    assert_eq!((a.bpm, a.outro_vocal, a.intro_centroid), (128.0, 0.0, 0.0));
    assert_eq!(core.analysis_missing(vec!["v1".into()]).unwrap(), vec!["v1".to_string()]);
    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn the_streaming_handle_finishes_into_the_store() {
    use jni::sys::jlong;
    let core = Core::new(String::new()).unwrap();
    // The handle a JNI create would return, built the same way.
    let s = Synth::new(128.0);
    let x = s.render();
    let h = {
        let mut a = analysis::Analyzer::new(s.rate, 60_000);
        a.feed(&x[..s.rate as usize * 10]);
        store_handle(a)
    };
    assert_eq!(core.analysis_finish_stream("x".into(), h).unwrap(), None, "10 s is not a track");
    let t = {
        with_handle(h, |a| a.feed(&x));
        core.analysis_finish_stream("x".into(), h).unwrap().unwrap()
    };
    assert!((t.bpm - 128.0).abs() < 0.05);
    assert_eq!(core.analysis_get("x".into()).unwrap(), Some(t));
    assert_eq!(core.analysis_finish_stream("x".into(), 0).unwrap(), None);
    store::test_destroy(h as jlong);
}

fn store_handle(a: analysis::Analyzer) -> i64 {
    store::test_handle(a)
}

fn with_handle(h: i64, f: impl FnOnce(&mut analysis::Analyzer)) {
    store::test_with(h, f)
}

#[test]
fn plan_from_real_analyses() {
    let a = analyse("a", &Synth { secs: 120.0, ..Synth::new(128.0) }.render(), 44100).track;
    let b = analyse("b", &Synth { secs: 120.0, ..Synth::new(125.0) }.render(), 44100).track;
    let p = plan_transition(Some(a.clone()), Some(b.clone()), a.duration_ms, b.duration_ms, crate::AutoMixSettings::default());
    assert_eq!(p.kind, crate::TransitionKind::BeatMatched, "{}", p.reason);
    assert!((p.tempo_ratio - a.bpm / b.bpm).abs() < 1e-6);
    let params = automix_mixer_params(p.clone());
    assert_eq!(params.len(), mixer::param::COUNT);
    assert_eq!(params[mixer::param::DURATION], p.duration_ms as f32);
}

/// `cargo test --release -p flintmusic analysis_cost -- --ignored --nocapture`
#[test]
#[ignore]
fn analysis_cost() {
    for rate in [44100u32, 48000] {
        let s = Synth { rate, secs: 240.0, offbeat: 0.5, noise: 0.05, chords: vec![(0, false), (5, false), (7, false), (9, true)], ..Synth::new(124.0) };
        let x = s.render();
        let t0 = std::time::Instant::now();
        let a = analyse("t", &x, rate);
        let el = t0.elapsed();
        println!("{rate} Hz, 4 min mono: {:.0} ms ({:.2} BPM, conf {:.2})", el.as_secs_f64() * 1000.0, a.track.bpm, a.track.bpm_confidence);
        // Split: the streaming front end vs the whole-track steps.
        let mut an = analysis::Analyzer::new(rate, 240_000);
        let t1 = std::time::Instant::now();
        an.feed(&x);
        let f = an.take_features();
        let front = t1.elapsed();
        let t2 = std::time::Instant::now();
        let _ = finish("t", &f);
        println!("  front end {:.0} ms, finish {:.1} ms", front.as_secs_f64() * 1000.0, t2.elapsed().as_secs_f64() * 1000.0);
    }
}

/// Analyses a raw PCM file, to compare what the app measures with what the file really is:
/// `FLINT_PCM=/path/file.s16 FLINT_RATE=44100 FLINT_CH=2 cargo test --release -p flintmusic -- --ignored --nocapture pcm_file`
#[test]
#[ignore]
fn pcm_file() {
    let Ok(path) = std::env::var("FLINT_PCM") else { return };
    let rate: i32 = std::env::var("FLINT_RATE").map(|v| v.parse().unwrap()).unwrap_or(44100);
    let ch: i32 = std::env::var("FLINT_CH").map(|v| v.parse().unwrap()).unwrap_or(2);
    let enc: i32 = std::env::var("FLINT_ENC").map(|v| v.parse().unwrap()).unwrap_or(2);
    for p in path.split(',') {
        let bytes = std::fs::read(p).expect("pcm file");
        let t = std::time::Instant::now();
        let a = crate::automix::automix_analyse("probe".into(), bytes, rate, ch, enc);
        println!(
            "{p}: {:.2} bpm (conf {:.2}, stab {:.2}) offset {:.1} ms, downbeat {} ({:.2}), key {}, {:.1} LUFS, {} ms audio, took {:?}",
            a.bpm, a.bpm_confidence, a.stability, a.beat_offset_ms, a.downbeat_phase, a.downbeat_confidence, a.key, a.lufs, a.duration_ms, t.elapsed(),
        );
    }
}
