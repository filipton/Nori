//! A light or dark scheme from one colour: tones of the same hue, like Material's own generator but
//! tiny. The platform maps them onto its theme's roles.

use crate::color::{color_to_hsl, hsl_to_color, WHITE};

/// The tones, in this order: primary, on primary, primary container, on primary container, secondary,
/// secondary container, on secondary container, surface, background, surface variant, on surface variant.
pub fn seeded(seed: u32, dark: bool) -> [u32; 11] {
    let hsl = color_to_hsl(seed);
    let tone = |l: f32, s: f32| hsl_to_color([hsl[0], s.clamp(0.0, 1.0), l]);
    let s = hsl[1];
    if dark {
        [
            tone(0.80, s),
            tone(0.20, s),
            tone(0.30, s),
            tone(0.90, s),
            tone(0.78, s * 0.4),
            tone(0.28, s * 0.4),
            tone(0.90, s * 0.4),
            tone(0.07, s * 0.12),
            tone(0.07, s * 0.12),
            tone(0.22, s * 0.15),
            tone(0.80, s * 0.15),
        ]
    } else {
        [
            tone(0.40, s),
            WHITE,
            tone(0.90, s),
            tone(0.12, s),
            tone(0.40, s * 0.4),
            tone(0.90, s * 0.4),
            tone(0.12, s * 0.4),
            tone(0.98, s * 0.2),
            tone(0.98, s * 0.2),
            tone(0.90, s * 0.15),
            tone(0.30, s * 0.15),
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::color::calculate_contrast;

    #[test]
    fn tones_keep_the_seed_hue_and_read_on_each_other() {
        for dark in [true, false] {
            let t = seeded(0xFF3F_51B5, dark);
            assert!(calculate_contrast(t[0], t[1]) >= 4.5, "primary / on primary, dark={dark}");
            assert!(calculate_contrast(t[10], t[9]) >= 3.0, "on surface variant, dark={dark}");
        }
    }
}
