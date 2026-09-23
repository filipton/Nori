//! What Jetpack Compose does to a colour, reproduced step for step so a colour worked out here is the
//! one the Android app used to work out itself: `Color.copy(alpha)`, the app's own `blend` and `over`,
//! and Compose's `lerp`, which mixes in Oklab (through its half-float colour storage) rather than in
//! sRGB. Every rule that was tuned by eye on screen was tuned against these, so they are kept exact.

use crate::color::{alpha, argb, blue, green, red};

/// A channel as Compose reads it back: 8 bits over 255, in float.
fn ch(v: i32) -> f32 {
    v as f32 / 255.0
}

/// Compose's `Color(red, green, blue, alpha)` in sRGB: each channel clamped and rounded half up.
pub fn from_floats_a(r: f32, g: f32, b: f32, a: f32) -> u32 {
    let q = |v: f32| (v.clamp(0.0, 1.0) * 255.0 + 0.5) as i32;
    argb(q(a), q(r), q(g), q(b))
}

/// `color.copy(alpha = a)`: the same colour with its alpha stored in 8 bits.
pub fn with_alpha(c: u32, a: f32) -> u32 {
    from_floats_a(ch(red(c)), ch(green(c)), ch(blue(c)), a)
}

/// The app's `blend(a, b, t)`: two opaque colours mixed channel by channel in sRGB, opaque.
pub fn blend(a: u32, b: u32, t: f32) -> u32 {
    let (ar, ag, ab) = (ch(red(a)), ch(green(a)), ch(blue(a)));
    from_floats_a(ar + (ch(red(b)) - ar) * t, ag + (ch(green(b)) - ag) * t, ab + (ch(blue(b)) - ab) * t, 1.0)
}

/// The app's `Color.over(background)`: a translucent colour laid on an opaque one, as one opaque colour.
pub fn over(fg: u32, bg: u32) -> u32 {
    blend(bg, fg | 0xFF00_0000, ch(alpha(fg)))
}

/// `on.copy(alpha = a).over(bg)`, the way every tinted surface in the app is made.
pub fn veil(on: u32, a: f32, bg: u32) -> u32 {
    over(with_alpha(on, a), bg)
}

// Oklab, as Compose's colour space classes compute it. Matrices are the exact floats Compose holds at
// run time (its sRGB matrix is adapted to D50 by the connector, so it is not the textbook one).

const SRGB_TO_XYZ: [f32; 9] = [
    f32::from_bits(0x3edf3e3e), f32::from_bits(0x3e63d085), f32::from_bits(0x3c6432cf),
    f32::from_bits(0x3ec52cfc), f32::from_bits(0x3f378732), f32::from_bits(0x3dc6dd2b),
    f32::from_bits(0x3e1283e7), f32::from_bits(0x3d784ad4), f32::from_bits(0x3f36d31c),
];
const XYZ_TO_SRGB: [f32; 9] = [
    f32::from_bits(0x40489893), f32::from_bits(0xbf7a8ef3), f32::from_bits(0x3d93598f),
    f32::from_bits(0xbfcf017d), f32::from_bits(0x3ff54339), f32::from_bits(0xbe6a7b62),
    f32::from_bits(0xbefb3b32), f32::from_bits(0x3d0902ac), f32::from_bits(0x3fb3dfe8),
];
const M1: [f32; 9] = [
    f32::from_bits(0x3f454c06), f32::from_bits(0x3bb90975), f32::from_bits(0x3d3ded66),
    f32::from_bits(0x3eb2cde6), f32::from_bits(0x3f6fe3a5), f32::from_bits(0x3e817c3d),
    f32::from_bits(0xbde5717d), f32::from_bits(0x3d8eba37), f32::from_bits(0x3f5a0574),
];
const M2: [f32; 9] = [
    f32::from_bits(0x3e578152), f32::from_bits(0x3ffd2f0e), f32::from_bits(0x3cd434b4),
    f32::from_bits(0x3f4b2a89), f32::from_bits(0xc01b6e0e), f32::from_bits(0x3f4863bb),
    f32::from_bits(0xbb856ece), f32::from_bits(0x3ee6b438), f32::from_bits(0xbf4f0560),
];
const INV_M1: [f32; 9] = [
    f32::from_bits(0x3fa4f1d5), f32::from_bits(0xbb2ab843), f32::from_bits(0xbd8e1b17),
    f32::from_bits(0xbf09b253), f32::from_bits(0x3f8bd206), f32::from_bits(0xbe971673),
    f32::from_bits(0x3e5aa850), f32::from_bits(0xbdb7c4b3), f32::from_bits(0x3f983844),
];
const INV_M2: [f32; 9] = [
    f32::from_bits(0x3f800001), f32::from_bits(0x3f800000), f32::from_bits(0x3f800001),
    f32::from_bits(0x3ecaecca), f32::from_bits(0xbdd8308c), f32::from_bits(0xbdb7437b),
    f32::from_bits(0x3e5cfba9), f32::from_bits(0xbd82c5fb), f32::from_bits(0xbfa54f66),
];

/// Column-major 3x3 times a vector, summed in the order Compose sums it.
fn mul(m: &[f32; 9], x: f32, y: f32, z: f32) -> (f32, f32, f32) {
    (m[0] * x + m[3] * y + m[6] * z, m[1] * x + m[4] * y + m[7] * z, m[2] * x + m[5] * y + m[8] * z)
}

// sRGB's transfer parameters, as Compose's `TransferParameters` holds them.
const TA: f64 = 0.9478672985781991;
const TB: f64 = 0.05213270142180095;
const TC: f64 = 0.07739938080495357;
const TD: f64 = 0.04045;
const TG: f64 = 2.4;

fn eotf(x: f32) -> f32 {
    let x = (x as f64).clamp(0.0, 1.0);
    (if x >= TD { (TA * x + TB).powf(TG) } else { TC * x }) as f32
}

fn oetf(x: f32) -> f32 {
    let x = x as f64;
    let v = if x >= TD * TC { (x.powf(1.0 / TG) - TB) / TA } else { x / TC };
    v.clamp(0.0, 1.0) as f32
}

/// `androidx.compose.ui.util.fastCbrt`: a bit trick and two Newton steps, not `cbrt`.
fn fast_cbrt(x: f32) -> f32 {
    let bits = (x.to_bits() as i32 as i64) & 0x1_FFFF_FFFF;
    let mut y = f32::from_bits(709_952_852i32.wrapping_add((bits / 3) as i32) as u32);
    y = y - (y - x / (y * y)) * 0.333_333_34;
    y - (y - x / (y * y)) * 0.333_333_34
}

/// Compose's float to half-float, rounding the way `Color` packs a component (half up on the 13th bit).
fn half(f: f32) -> u16 {
    let bits = f.to_bits() as i32;
    let s = ((bits as u32) >> 31) as i32;
    let mut e = ((bits as u32) >> 23) as i32 & 0xFF;
    let mut m = bits & 0x7F_FFFF;
    let (mut out_e, mut out_m) = (0, 0);
    if e == 0xFF {
        out_e = 0x1F;
        out_m = if m != 0 { 0x200 } else { 0 };
    } else {
        e = e - 127 + 15;
        if e >= 0x1F {
            out_e = 0x31;
        } else if e <= 0 {
            if e >= -10 {
                m = (m | 0x80_0000) >> (1 - e);
                if m & 0x1000 != 0 {
                    m += 0x2000;
                }
                out_m = m >> 13;
            }
        } else {
            out_e = e;
            out_m = m >> 13;
            if m & 0x1000 != 0 {
                return ((((out_e << 10) | out_m) + 1) | (s << 15)) as u16;
            }
        }
    }
    ((s << 15) | (out_e << 10) | out_m) as u16
}

fn unhalf(h: u16) -> f32 {
    let h = h as i32;
    let s = h & 0x8000;
    let e = (h >> 10) & 0x1F;
    let m = h & 0x3FF;
    let (mut out_e, mut out_m) = (0, 0);
    if e == 0 {
        if m != 0 {
            let v = f32::from_bits((0x3F00_0000 + m) as u32) - f32::from_bits(0x3F00_0000);
            return if s == 0 { v } else { -v };
        }
    } else {
        out_m = m << 13;
        if e == 0x1F {
            out_e = 0xFF;
            if out_m != 0 {
                out_m |= 0x40_0000;
            }
        } else {
            out_e = e - 15 + 127;
        }
    }
    f32::from_bits(((s << 16) | (out_e << 23) | out_m) as u32)
}

/// An sRGB colour in Oklab the way Compose stores it: L, a, b as half floats, alpha in ten bits.
fn to_oklab(c: u32) -> ([u16; 3], i32) {
    let (r, g, b) = (eotf(ch(red(c))), eotf(ch(green(c))), eotf(ch(blue(c))));
    let (x, y, z) = mul(&SRGB_TO_XYZ, r, g, b);
    let (l, m, s) = mul(&M1, x, y, z);
    let (l, a, bb) = mul(&M2, fast_cbrt(l), fast_cbrt(m), fast_cbrt(s));
    let a10 = (ch(alpha(c)).clamp(0.0, 1.0) * 1023.0 + 0.5) as i32;
    ([half(l.clamp(0.0, 1.0)), half(a.clamp(-0.5, 0.5)), half(bb.clamp(-0.5, 0.5))], a10)
}

fn from_oklab(lab: [u16; 3], a10: i32) -> u32 {
    let l = unhalf(lab[0]).clamp(0.0, 1.0);
    let a = unhalf(lab[1]).clamp(-0.5, 0.5);
    let b = unhalf(lab[2]).clamp(-0.5, 0.5);
    let (l, m, s) = mul(&INV_M2, l, a, b);
    let (x, y, z) = mul(&INV_M1, l * l * l, m * m * m, s * s * s);
    let (r, g, b) = mul(&XYZ_TO_SRGB, x, y, z);
    from_floats_a(oetf(r), oetf(g), oetf(b), (a10 & 0x3FF) as f32 / 1023.0)
}

fn mix(a: f32, b: f32, t: f32) -> f32 {
    (1.0 - t) * a + t * b
}

/// Compose's `lerp(start, stop, fraction)` for two sRGB colours: mixed in Oklab and brought back.
pub fn lerp(start: u32, stop: u32, fraction: f32) -> u32 {
    let t = fraction.clamp(0.0, 1.0);
    let (s, sa) = to_oklab(start);
    let (e, ea) = to_oklab(stop);
    let sa = sa as f32 / 1023.0;
    let ea = ea as f32 / 1023.0;
    let lab = [0, 1, 2].map(|i| half(mix(unhalf(s[i]), unhalf(e[i]), t)));
    from_oklab(lab, (mix(sa, ea, t) * 1023.0 + 0.5) as i32)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Worked out by Compose itself (ui-graphics 1.12, on the JVM): start, stop, fraction bits, result.
    const LERPS: &[(u32, u32, u32, u32)] = include!("compose_lerps.in");

    #[test]
    fn lerp_is_composes() {
        let mut wrong = 0;
        for &(a, b, t, want) in LERPS {
            let got = lerp(a, b, f32::from_bits(t));
            if got != want {
                wrong += 1;
                eprintln!("lerp({a:08x}, {b:08x}, {}) = {got:08x}, Compose {want:08x}", f32::from_bits(t));
            }
        }
        assert_eq!(wrong, 0);
    }

    #[test]
    fn copy_over_and_blend_are_composes() {
        // Worked out by Compose: Color(0xFFEEDDCC).copy(alpha = 0.10f).over(Color(0xFF102030)).
        assert_eq!(with_alpha(0xFFEE_DDCC, 0.10), 0x1AEE_DDCC);
        assert_eq!(veil(0xFFEE_DDCC, 0.10, 0xFF10_2030), OVER_10);
        assert_eq!(blend(0xFF10_2030, 0xFFEE_DDCC, 0.30), BLEND_30);
    }

    /// Worked out by Compose: the veil above, and `blend(Color(0xFF102030), Color(0xFFEEDDCC), 0.30f)`.
    const OVER_10: u32 = 0xFF27_3340;
    const BLEND_30: u32 = 0xFF53_595F;
}
