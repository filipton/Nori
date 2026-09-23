//! The core's twins held to their Kotlin originals: testdata/twins/core_twins.tsv is what the Kotlin
//! (copied into testdata/twins/CoreTwins.kt) answered on the JVM, written by tools/twins.sh. Every answer
//! must match exactly; the only floats are volume fractions, compared bit for bit.

use norimusic::automix::ahead;
use norimusic::rows::{merge_rows, Row};
use norimusic::{autoeq, car, covers, heard, menus, rules, search, stream_cache};

const TABLE: &str = include_str!("../testdata/twins/core_twins.tsv");

/// The rows for `case`, each split into its fields after the name.
fn rows(case: &str) -> Vec<Vec<&'static str>> {
    let rows: Vec<Vec<&str>> =
        TABLE.lines().map(|l| l.split('\t').collect::<Vec<_>>()).filter(|f| f[0] == case).map(|f| f[1..].to_vec()).collect();
    assert!(!rows.is_empty(), "no vectors for {case}");
    rows
}

/// A string written as its UTF-8 bytes in hex: "_" empty, "-" none.
fn text(s: &str) -> Option<String> {
    match s {
        "-" => None,
        "_" => Some(String::new()),
        _ => Some(String::from_utf8(bytes(s)).unwrap()),
    }
}

fn bytes(hex: &str) -> Vec<u8> {
    (0..hex.len()).step_by(2).map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap()).collect()
}

fn list(s: &str) -> Vec<&str> {
    if s == "_" { Vec::new() } else { s.split(',').collect() }
}

fn flag(s: &str) -> bool {
    s.parse().unwrap()
}

fn maybe<T: std::str::FromStr>(s: &str) -> Option<T>
where
    T::Err: std::fmt::Debug,
{
    (s != "-").then(|| s.parse().unwrap())
}

#[test]
fn provider_covers() {
    for r in rows("provider") {
        assert_eq!(covers::is_provider_cover(&text(r[0]).unwrap()), flag(r[1]), "{r:?}");
    }
}

#[test]
fn cover_urls() {
    let mut out = String::new();
    for r in rows("cover_url") {
        covers::cover_url_into(&mut out, &text(r[0]).unwrap(), &text(r[1]).unwrap(), r[2].parse().unwrap());
        assert_eq!(Some(&out), text(r[3]).as_ref(), "{r:?}");
    }
}

#[test]
fn live_search_waits() {
    for r in rows("live_delay") {
        assert_eq!(search::live_delay_ms(&text(r[0]).unwrap(), 350), r[1].parse::<i64>().unwrap(), "{r:?}");
    }
    let kotlin: Vec<u32> = rows("whitespace").iter().map(|r| u32::from_str_radix(r[0], 16).unwrap()).collect();
    let rust: Vec<u32> = (0..=0x10FFFFu32).filter(|&c| char::from_u32(c).is_some_and(search::kotlin_whitespace)).collect();
    assert_eq!(rust, kotlin);
}

#[test]
fn bodies_read_as_the_jvm_reads_them() {
    for r in rows("text") {
        assert_eq!(Some(autoeq::text(&bytes(r[0]))), text(r[1]), "{r:?}");
    }
}

#[test]
fn measuring_ahead() {
    for r in rows("on_device") {
        assert_eq!(ahead::whole_on_device(flag(r[0]), r[1].parse().unwrap(), r[2].parse().unwrap()), flag(r[3]), "{r:?}");
    }
    for r in rows("ahead") {
        let here = list(r[1]);
        let p = ahead::plan(list(r[0]).into_iter().map(String::from).collect(), |id| here.contains(&id));
        assert_eq!((p.measure.iter().map(String::as_str).collect::<Vec<_>>(), p.waiting), (list(r[2]), r[3].parse().unwrap()), "{r:?}");
    }
}

#[test]
fn precaching() {
    for r in rows("precache") {
        let fetching = list(r[1]);
        let got = rules::precache_list(list(r[0]).into_iter().map(String::from).collect(), |id| fetching.contains(&id));
        assert_eq!(got.iter().map(String::as_str).collect::<Vec<_>>(), list(r[2]), "{r:?}");
    }
}

#[test]
fn the_row_the_ear_is_on() {
    for r in rows("shown") {
        let heard = maybe::<i64>(r[0]).and_then(|h| usize::try_from(h).ok());
        let (before, now) = (list(r[2]), list(r[3]));
        let playing = (r[4] != "-").then_some(r[4]);
        assert_eq!(heard::shown_index(heard, flag(r[1]), &before, &now, playing), maybe(r[5]), "{r:?}");
    }
}

#[test]
fn the_heard_word_reads_back() {
    for at in [
        heard::HeardAt { index: None, changed: false, ms: 0 },
        heard::HeardAt { index: Some(0), changed: true, ms: 61_500 },
        heard::HeardAt { index: Some(4_000), changed: false, ms: (1 << 43) - 1 },
    ] {
        assert_eq!(heard::HeardAt::unpack(at.pack()), at);
    }
}

#[test]
fn volume_steps() {
    for r in rows("volume_step") {
        let f = f32::from_bits(u32::from_str_radix(r[0], 16).unwrap());
        assert_eq!(rules::volume_step(f, r[1].parse().unwrap()), maybe(r[2]), "{r:?}");
    }
    for r in rows("volume_fraction") {
        let got = rules::volume_fraction(r[0].parse().unwrap(), r[1].parse().unwrap());
        assert_eq!(got.to_bits(), u32::from_str_radix(r[2], 16).unwrap(), "{r:?}");
    }
}

#[test]
fn pages_of_a_browse_list() {
    for r in rows("page") {
        let n: Vec<usize> = r.iter().map(|x| x.parse().unwrap()).collect();
        assert_eq!(car::page(n[0], n[1] as u32, n[2] as u32), n[3]..n[4], "{r:?}");
    }
}

#[test]
fn songs_still_to_download() {
    for r in rows("missing") {
        let done = list(r[1]);
        let got = menus::download_missing(list(r[0]), |id| done.contains(&id));
        assert_eq!(got.iter().map(usize::to_string).collect::<Vec<_>>(), list(r[2]), "{r:?}");
    }
}

#[test]
fn rows_coming_and_going() {
    let mut shown: Option<Vec<Row<String>>> = None;
    let mut scenario = "";
    for r in rows("rows") {
        if r[0] != scenario {
            (scenario, shown) = (r[0], None);
        }
        if r[1] == "settle" {
            for row in shown.iter_mut().flatten() {
                row.shown = row.wanted;
            }
        } else {
            let keys: Vec<String> = list(r[1].trim_start_matches("set:")).into_iter().map(String::from).collect();
            shown = Some(merge_rows(shown.as_deref(), &keys));
        }
        let got: Vec<String> = shown.iter().flatten().map(|r| format!("{}:{}:{}", r.key, r.shown, r.wanted)).collect();
        assert_eq!(got, list(r[2]), "{r:?}");
    }
}

#[test]
fn trimming_the_stream_cache() {
    for r in rows("trim") {
        let order: Vec<(&str, i64)> = list(r[0]).into_iter().map(|e| e.split_once(':').unwrap()).map(|(k, n)| (k, n.parse().unwrap())).collect();
        // The core names the least recently used first: touched in this order, that is this order.
        stream_cache::clear();
        for (k, _) in &order {
            stream_cache::touch(k);
        }
        let space = std::cell::Cell::new(order.iter().map(|o| o.1).sum::<i64>());
        let mut removed = Vec::new();
        stream_cache::trim(
            r[1].parse().unwrap(),
            || space.get(),
            |k| {
                space.set(space.get() - order.iter().find(|o| o.0 == k).unwrap().1);
                removed.push(k.to_string());
            },
        );
        assert_eq!(removed, list(r[2]), "{r:?}");
    }
}

/// Allocations made on the calling thread, for the twins a list asks per row.
mod counting {
    use std::alloc::{GlobalAlloc, Layout, System};
    use std::cell::Cell;

    struct Counting;

    thread_local! {
        static ALLOCS: Cell<u64> = const { Cell::new(0) };
    }

    unsafe impl GlobalAlloc for Counting {
        unsafe fn alloc(&self, l: Layout) -> *mut u8 {
            let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
            unsafe { System.alloc(l) }
        }
        unsafe fn realloc(&self, p: *mut u8, l: Layout, n: usize) -> *mut u8 {
            let _ = ALLOCS.try_with(|c| c.set(c.get() + 1));
            unsafe { System.realloc(p, l, n) }
        }
        unsafe fn dealloc(&self, p: *mut u8, l: Layout) {
            unsafe { System.dealloc(p, l) }
        }
    }

    #[global_allocator]
    static GLOBAL: Counting = Counting;

    pub fn allocations(f: impl FnOnce()) -> u64 {
        let before = ALLOCS.with(Cell::get);
        f();
        ALLOCS.with(Cell::get) - before
    }
}

#[test]
fn a_rows_cover_allocates_nothing() {
    let prefix = "https://m.example/rest/getCoverArt.view?u=admin&t=26719a1196d2a940705a59634eb18eab&s=c19b2d&f=json&v=1.16.1&c=nori";
    let mut url = String::with_capacity(512);
    let mut provider = 0;
    let n = counting::allocations(|| {
        for i in 0..1_000 {
            covers::cover_url_into(&mut url, prefix, if i % 2 == 0 { "al-12 (live)" } else { "ext-deezer-1" }, 320);
            provider += covers::is_provider_cover(&url) as u32;
        }
    });
    assert_eq!((n, provider), (0, 500));
}
