//! Premultiplication in linear light for sRGB-encoded textures.
//!
//! The GPU texture is `R8G8B8A8_UNORM_SRGB`: sampling decodes sRGB to linear.
//! Storing `srgb_encode(srgb_decode(c) * a)` therefore yields premultiplied
//! *linear* colour after sampling, so bilinear filtering, mipmaps and blending
//! are all correct at transparent edges (no dark or bright fringes).

use std::sync::OnceLock;

fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(l: f32) -> f32 {
    if l <= 0.003_130_8 {
        l * 12.92
    } else {
        1.055 * l.powf(1.0 / 2.4) - 0.055
    }
}

/// 64 KiB table indexed by `alpha << 8 | colour`.
fn lut() -> &'static [u8; 65536] {
    static LUT: OnceLock<Box<[u8; 65536]>> = OnceLock::new();
    LUT.get_or_init(|| {
        let mut t = Box::new([0u8; 65536]);
        for a in 0..256usize {
            let af = a as f32 / 255.0;
            for c in 0..256usize {
                let l = srgb_to_linear(c as f32 / 255.0) * af;
                t[(a << 8) | c] = (linear_to_srgb(l) * 255.0 + 0.5) as u8;
            }
        }
        t
    })
}

/// Premultiplies straight-alpha sRGB RGBA8 in place.
/// Returns `false` (and leaves pixels untouched) when the image is fully opaque.
pub fn premultiply_in_place(rgba: &mut [u8]) -> bool {
    if rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255) {
        return false;
    }
    let lut = lut();
    for p in rgba.as_chunks_mut::<4>().0 {
        let a = usize::from(p[3]);
        if a != 255 {
            let row = a << 8;
            p[0] = lut[row | usize::from(p[0])];
            p[1] = lut[row | usize::from(p[1])];
            p[2] = lut[row | usize::from(p[2])];
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn opaque_is_untouched() {
        let mut px = vec![10, 20, 30, 255, 200, 100, 0, 255];
        let orig = px.clone();
        assert!(!premultiply_in_place(&mut px));
        assert_eq!(px, orig);
    }

    #[test]
    fn transparent_becomes_zero_and_half_alpha_is_linear() {
        let mut px = vec![255, 255, 255, 0, 255, 255, 255, 128];
        assert!(premultiply_in_place(&mut px));
        assert_eq!(&px[0..4], &[0, 0, 0, 0]);
        // White at ~50 % alpha: linear 0.502 -> sRGB ~0.737 -> 188.
        assert!((187..=189).contains(&px[4]), "{}", px[4]);
        assert_eq!(px[7], 128);
    }

    #[test]
    fn roundtrip_identity_at_full_alpha() {
        let t = lut();
        for c in 0..256 {
            assert_eq!(t[(255 << 8) | c], c as u8);
        }
    }
}
