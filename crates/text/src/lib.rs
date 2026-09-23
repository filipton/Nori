//! How Nori writes a number with a fraction in it, for every crate that words something: the way
//! Java's `String.format` did it in the app these strings came from, so nothing on screen changed when
//! they moved here.
//!
//! Two things make that more than `{:.1}`. `%f` and `%g` round half up on the shortest decimal form of
//! the number, not on its exact binary value (which is what Rust rounds): 62.5 Hz is "63", 0.15 is
//! "0.2". And they write the fraction with the phone's own decimal separator - "12,4 MB" on a Polish
//! phone. The platform says which separator once at start and again when the locale changes
//! ([`set_style`]); until it does (tests, a desktop that never calls) it is a point.
//!
//! None of the app's formats asked for grouping, so no number here is grouped; the grouping separator is
//! kept with the style for the first one that does.

use std::sync::atomic::{AtomicU32, Ordering};

static DECIMAL: AtomicU32 = AtomicU32::new('.' as u32);
static GROUPING: AtomicU32 = AtomicU32::new(',' as u32);

/// The platform's number style: its decimal and grouping separators (the first character of each; an
/// empty one keeps what was there).
pub fn set_style(decimal: &str, grouping: &str) {
    if let Some(c) = decimal.chars().next() {
        DECIMAL.store(c as u32, Ordering::Relaxed);
    }
    if let Some(c) = grouping.chars().next() {
        GROUPING.store(c as u32, Ordering::Relaxed);
    }
}

/// The decimal separator in use.
pub fn decimal() -> char {
    char::from_u32(DECIMAL.load(Ordering::Relaxed)).unwrap_or('.')
}

/// The grouping separator in use.
pub fn grouping() -> char {
    char::from_u32(GROUPING.load(Ordering::Relaxed)).unwrap_or(',')
}

/// Decimal digits and the power of ten of the first, on the stack: a number is worded without a heap
/// allocation beyond the string it is written into (a download's speed is worded every second).
struct Digits {
    d: [u8; 32],
    len: usize,
    /// The power of ten of `d[0]`.
    exp: i32,
}

/// Collects `{:e}`'s output without a `String`.
struct Sci {
    b: [u8; 40],
    n: usize,
}

impl std::fmt::Write for Sci {
    fn write_str(&mut self, s: &str) -> std::fmt::Result {
        let end = self.n + s.len();
        if end > self.b.len() {
            return Err(std::fmt::Error);
        }
        self.b[self.n..end].copy_from_slice(s.as_bytes());
        self.n = end;
        Ok(())
    }
}

impl Digits {
    /// The shortest decimal digits of `v` (> 0, finite): 0.15 -> [1, 5] at -1.
    fn shortest(v: f64) -> Self {
        use std::fmt::Write;
        let mut sci = Sci { b: [0; 40], n: 0 };
        let _ = write!(sci, "{v:e}");
        let text = &sci.b[..sci.n];
        let mut out = Digits { d: [0; 32], len: 0, exp: 0 };
        let mut i = 0;
        while i < text.len() && text[i] != b'e' {
            if text[i].is_ascii_digit() && out.len < out.d.len() - 1 {
                out.d[out.len] = text[i] - b'0';
                out.len += 1;
            }
            i += 1;
        }
        let (neg, mut e) = (text.get(i + 1) == Some(&b'-'), 0i32);
        for &c in text.iter().skip(i + 1) {
            if c.is_ascii_digit() {
                e = e * 10 + (c - b'0') as i32;
            }
        }
        out.exp = if neg { -e } else { e };
        out
    }

    /// Rounded half up to `keep` digits; a carry can raise the power of the first by one.
    fn round(&mut self, keep: i32) {
        if keep < 0 {
            self.d[0] = 0;
            self.len = 1;
            return;
        }
        let keep = keep as usize;
        if self.len <= keep {
            return;
        }
        let up = self.d[keep] >= 5;
        self.len = keep;
        if !up {
            return;
        }
        let mut i = self.len;
        loop {
            if i == 0 {
                // All nines: one more digit in front.
                self.d.copy_within(0..self.len, 1);
                self.d[0] = 1;
                self.len += 1;
                self.exp += 1;
                return;
            }
            i -= 1;
            if self.d[i] == 9 {
                self.d[i] = 0;
            } else {
                self.d[i] += 1;
                return;
            }
        }
    }

    /// The digit at power `p` of ten, 0 outside those held.
    fn at(&self, p: i32) -> u8 {
        let i = self.exp - p;
        if i >= 0 && (i as usize) < self.len { self.d[i as usize] } else { 0 }
    }
}

/// Java's `"%.{places}f"` with the decimal separator `sep`, and a '+' in front of a non-negative number
/// when `plus`, onto the end of `out`.
pub fn push_fixed_in(out: &mut String, v: f64, places: i32, plus: bool, sep: char) {
    if v.is_sign_negative() {
        out.push('-');
    } else if plus {
        out.push('+');
    }
    let a = v.abs();
    let mut n = Digits { d: [0; 32], len: 1, exp: 0 };
    if a != 0.0 {
        n = Digits::shortest(a);
        // Digits kept: those down to 10^-places.
        n.round(n.exp + 1 + places);
    }
    for p in (0..=n.exp.max(0)).rev() {
        out.push((b'0' + n.at(p)) as char);
    }
    if places > 0 {
        out.push(sep);
        for p in 1..=places {
            out.push((b'0' + n.at(-p)) as char);
        }
    }
}

/// [`push_fixed_in`] as a string of its own.
pub fn fixed_in(v: f64, places: i32, plus: bool, sep: char) -> String {
    let mut out = String::new();
    push_fixed_in(&mut out, v, places, plus, sep);
    out
}

/// Java's `"%.{places}f"` in the platform's style, onto the end of `out`.
pub fn push_fixed(out: &mut String, v: f64, places: i32, plus: bool) {
    push_fixed_in(out, v, places, plus, decimal());
}

/// Java's `"%.{places}f"` in the platform's style.
pub fn fixed(v: f64, places: i32, plus: bool) -> String {
    fixed_in(v, places, plus, decimal())
}

/// Java's `"%.{sig}g"` with the separator `sep`, for a positive number between 10^-4 and 10^sig (which
/// is all it is used for).
pub fn general_in(v: f64, sig: i32, sep: char) -> String {
    if v == 0.0 {
        return fixed_in(0.0, sig - 1, false, sep);
    }
    let mut n = Digits::shortest(v.abs());
    n.round(sig);
    fixed_in(v, (sig - 1 - n.exp).max(0), false, sep)
}

/// Java's `"%.{sig}g"` in the platform's style.
pub fn general(v: f64, sig: i32) -> String {
    general_in(v, sig, decimal())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Printed by Java's `String.format` (OpenJDK 21, root locale): value bits (f64), places, plus, result.
    const FIXED: &[(u64, i32, bool, &str)] = include!("fixed.in");

    #[test]
    fn fixed_rounds_as_java_does() {
        for &(bits, places, plus, want) in FIXED {
            assert_eq!(fixed_in(f64::from_bits(bits), places, plus, '.'), want, "{} to {places}", f64::from_bits(bits));
        }
        assert_eq!(fixed_in(0.15, 1, false, '.'), "0.2", "half up on the shortest form, not on the binary value");
        assert_eq!(fixed_in(62.5, 0, false, '.'), "63");
        assert_eq!(fixed_in(-0.04, 1, false, '.'), "-0.0");
        assert_eq!(fixed_in(9.96, 1, false, '.'), "10.0", "a carry adds a digit");
        assert_eq!(fixed_in(0.000_04, 1, false, '.'), "0.0");
        assert_eq!(fixed_in(1e-7, 3, false, '.'), "0.000");
        assert_eq!(fixed_in(0.0005, 3, false, '.'), "0.001");
    }

    #[test]
    fn a_comma_is_where_the_point_was_and_nowhere_else() {
        // What `"%.1f".format(12.4)` and friends print with a Polish default locale.
        assert_eq!(fixed_in(12.4, 1, false, ','), "12,4");
        assert_eq!(fixed_in(-3.25, 2, true, ','), "-3,25");
        assert_eq!(fixed_in(3.0, 1, true, ','), "+3,0");
        assert_eq!(fixed_in(1234.5, 1, false, ','), "1234,5", "no grouping: the formats never asked for it");
        assert_eq!(fixed_in(62.5, 0, false, ','), "63");
        assert_eq!(general_in(2.5, 4, ','), "2,500");
        assert_eq!(general_in(12.5, 4, '.'), "12.50");
    }

    #[test]
    fn a_point_until_the_platform_says_otherwise() {
        // Only ever set to the default here: the style is process-wide and the other tests run beside
        // this one. An empty separator keeps what was there.
        set_style("", "");
        assert_eq!((decimal(), grouping()), ('.', ','));
        set_style(".", ",");
        assert_eq!(fixed(12.44, 1, false), "12.4");
        assert_eq!(general(1.0, 4), "1.000");
    }
}
