//! Where a stretch's memory went, read with the stretch's other counters at its ends: the platform's
//! breakdown of the process's PSS (the app summary `dumpsys meminfo` prints), the native heap's
//! allocated bytes, and the app's own big holders - the Rust heap as a whole, the songs' bytes the
//! engine keeps, its ring, the beat model, the covers' Bitmaps and the moving cover's player. The
//! platform reads its part; the engine's is asked through the hook its client installs ([`install`]),
//! since this crate does not link the engine. Nothing here is sampled on a timer.

use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

/// What the Rust side holds, in KB.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfRust {
    /// The Rust heap's live bytes (`nori_model::heap`); -1 where they are not counted.
    #[serde(rename = "h")]
    pub heap_kb: i64,
    /// The songs' loaders alive, and the bytes they keep in memory.
    #[serde(rename = "sn")]
    pub songs: i32,
    #[serde(rename = "sk")]
    pub songs_kb: i64,
    /// Of those loaders, the songs read from their stream cache entry, their memory let go.
    #[serde(rename = "sd", default)]
    pub songs_on_disk: i32,
    /// The engine's ring (twelve seconds of float samples).
    #[serde(rename = "r")]
    pub ring_kb: i64,
    /// The beat model while it is loaded: what the Rust heap grew by as it was (1 where that is not
    /// counted); nought while it is not loaded.
    #[serde(rename = "m", default)]
    pub model_kb: i64,
}

/// One reading of the process's memory, in KB. The first seven are Android's app summary
/// (`Debug.MemoryInfo.getMemoryStats`), which add up to the total PSS; the rest say what of it is ours.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "ffi", derive(uniffi::Record))]
pub struct PerfMemory {
    #[serde(rename = "j")]
    pub java_kb: i64,
    #[serde(rename = "n")]
    pub native_kb: i64,
    #[serde(rename = "c")]
    pub code_kb: i64,
    #[serde(rename = "s")]
    pub stack_kb: i64,
    #[serde(rename = "g")]
    pub graphics_kb: i64,
    #[serde(rename = "o")]
    pub other_kb: i64,
    #[serde(rename = "y")]
    pub system_kb: i64,
    /// The native heap's bytes allocated now (mallinfo), Rust's and the platform's.
    #[serde(rename = "na")]
    pub native_alloc_kb: i64,
    /// The covers' Bitmaps kept in memory (hardware ones in the graphics figure on a phone), and how many.
    #[serde(rename = "ck")]
    pub covers_kb: i64,
    #[serde(rename = "cn")]
    pub covers: i32,
    /// Moving-cover players alive (each an ExoPlayer with its video decoder).
    #[serde(rename = "mv", default)]
    pub motion: i32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rust: Option<PerfRust>,
}

/// The engine's part of [`PerfRust`], from the client that runs the engine: songs (loaders, KB, on the
/// disk), ring KB and model KB.
pub struct EngineMemory {
    pub songs: i32,
    pub songs_kb: i64,
    pub songs_on_disk: i32,
    pub ring_kb: i64,
    pub model_kb: i64,
}

static ENGINE: OnceLock<fn() -> EngineMemory> = OnceLock::new();

/// Tells this how to ask the engine what it holds: called once by the client that links it.
pub fn install(engine: fn() -> EngineMemory) {
    let _ = ENGINE.set(engine);
}

/// What the Rust side holds now. A few locks taken, one per song loader; read at a stretch's ends.
#[cfg_attr(feature = "ffi", uniffi::export)]
pub fn perf_rust_memory() -> PerfRust {
    let e = ENGINE.get().map(|f| f());
    PerfRust {
        heap_kb: nori_model::heap::live_bytes().map_or(-1, |b| b / 1024),
        songs: e.as_ref().map_or(0, |e| e.songs),
        songs_kb: e.as_ref().map_or(0, |e| e.songs_kb),
        songs_on_disk: e.as_ref().map_or(0, |e| e.songs_on_disk),
        ring_kb: e.as_ref().map_or(0, |e| e.ring_kb),
        model_kb: e.as_ref().map_or(0, |e| e.model_kb),
    }
}

fn mb(kb: i64) -> String {
    let mb = kb as f64 / 1024.0;
    if mb < 10.0 { format!("{mb:.1}") } else { format!("{mb:.0}") }
}

/// The memory line under a stretch's figures, in MB: the PSS as Android breaks it down, then what of it
/// is the app's own.
pub fn memory_line(m: &PerfMemory) -> String {
    let total = m.java_kb + m.native_kb + m.code_kb + m.stack_kb + m.graphics_kb + m.other_kb + m.system_kb;
    let mut out = format!(
        "memory: PSS {} MB = Java {}, native {}, code {}, stack {}, graphics {}, other {}, system {}; native heap allocated {}",
        mb(total),
        mb(m.java_kb),
        mb(m.native_kb),
        mb(m.code_kb),
        mb(m.stack_kb),
        mb(m.graphics_kb),
        mb(m.other_kb),
        mb(m.system_kb),
        mb(m.native_alloc_kb),
    );
    if let Some(r) = &m.rust {
        if r.heap_kb >= 0 {
            out.push_str(&format!(", of it Rust {}", mb(r.heap_kb)));
        }
        out.push_str(&format!("; songs {} in {} loaders", mb(r.songs_kb), r.songs));
        if r.songs_on_disk > 0 {
            out.push_str(&format!(" ({} read from the disk)", r.songs_on_disk));
        }
        out.push_str(&format!(", ring {}", mb(r.ring_kb)));
        if r.model_kb > 0 {
            out.push_str(&format!(", beat model {}", mb(r.model_kb)));
        }
    }
    out.push_str(&format!("; covers {} in {} Bitmaps", mb(m.covers_kb), m.covers));
    if m.motion > 0 {
        out.push_str(&format!(", moving cover playing ({})", m.motion));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_adds_the_summary_up_and_says_what_is_ours() {
        let m = PerfMemory {
            java_kb: 16 * 1024,
            native_kb: 50 * 1024,
            code_kb: 11 * 1024,
            stack_kb: 2 * 1024 + 512,
            graphics_kb: 0,
            other_kb: 8 * 1024,
            system_kb: 10 * 1024,
            native_alloc_kb: 46 * 1024,
            covers_kb: 12 * 1024,
            covers: 30,
            motion: 0,
            rust: Some(PerfRust { heap_kb: 18 * 1024, songs: 2, songs_kb: 21 * 1024, songs_on_disk: 1, ring_kb: 4134, model_kb: 0 }),
        };
        assert_eq!(
            memory_line(&m),
            "memory: PSS 98 MB = Java 16, native 50, code 11, stack 2.5, graphics 0.0, other 8.0, system 10; native heap allocated 46, \
             of it Rust 18; songs 21 in 2 loaders (1 read from the disk), ring 4.0; covers 12 in 30 Bitmaps"
        );
    }

    #[test]
    fn without_the_rust_part_or_its_count_the_line_says_only_what_is_known() {
        let m = PerfMemory { rust: Some(PerfRust { heap_kb: -1, model_kb: 60 * 1024, ..Default::default() }), motion: 1, ..Default::default() };
        let line = memory_line(&m);
        assert!(!line.contains("of it Rust"), "{line}");
        assert!(line.contains("beat model 60"), "{line}");
        assert!(line.ends_with("moving cover playing (1)"), "{line}");
    }

    #[test]
    fn a_row_keeps_its_memory_under_short_names_and_reads_back() {
        let m = PerfMemory { java_kb: 1, rust: Some(PerfRust { heap_kb: 2, ..Default::default() }), ..Default::default() };
        let json = serde_json::to_string(&m).unwrap();
        assert!(json.contains("\"j\":1") && json.contains("\"h\":2"), "{json}");
        assert_eq!(serde_json::from_str::<PerfMemory>(&json).unwrap(), m);
    }
}
