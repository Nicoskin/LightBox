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

/// Inverse of [`premultiply_in_place`]: straight-alpha sRGB RGBA8.
pub fn unpremultiply(rgba: &[u8]) -> Vec<u8> {
    let mut out = rgba.to_vec();
    for p in out.as_chunks_mut::<4>().0 {
        let a = f32::from(p[3]) / 255.0;
        if p[3] != 255 && a > 0.0 {
            for c in &mut p[..3] {
                let l = srgb_to_linear(f32::from(*c) / 255.0) / a;
                *c = (linear_to_srgb(l.min(1.0)) * 255.0 + 0.5) as u8;
            }
        }
    }
    out
}

/// sRGB-encoded 16-bit value → linear, as a 65536-entry table.
fn lut16() -> &'static [f32] {
    static LUT: OnceLock<Vec<f32>> = OnceLock::new();
    LUT.get_or_init(|| {
        (0..65536u32)
            .map(|v| srgb_to_linear(v as f32 / 65535.0))
            .collect()
    })
}

/// RGBA16 (gamma-encoded, straight) → premultiplied linear half floats.
pub fn rgba16_to_linear_f16(v: &[u16]) -> (super::Pixels, bool) {
    let lut = lut16();
    let mut has_alpha = false;
    let mut out = Vec::with_capacity(v.len());
    for p in v.as_chunks::<4>().0 {
        let a = f32::from(p[3]) / 65535.0;
        has_alpha |= p[3] != 65535;
        for &c in &p[..3] {
            out.push(half::f16::from_f32(lut[usize::from(c)] * a).to_bits());
        }
        out.push(half::f16::from_f32(a).to_bits());
    }
    (super::Pixels::Linear16F(out), has_alpha)
}

/// Tone curve for HDR (ACES filmic fit, Narkowicz 2015): linear in,
/// linear 0..1 out.
fn tonemap(x: f32) -> f32 {
    let x = x.max(0.0) * 0.8;
    ((x * (2.51 * x + 0.03)) / (x * (2.43 * x + 0.59) + 0.14)).clamp(0.0, 1.0)
}

/// Linear RGBA32F (HDR) → tone-mapped, premultiplied linear half floats.
pub fn hdr_to_linear_f16(v: &[f32]) -> (super::Pixels, bool) {
    let mut has_alpha = false;
    let mut out = Vec::with_capacity(v.len());
    for p in v.as_chunks::<4>().0 {
        let a = p[3].clamp(0.0, 1.0);
        has_alpha |= a < 1.0;
        for &c in &p[..3] {
            out.push(half::f16::from_f32(tonemap(c) * a).to_bits());
        }
        out.push(half::f16::from_f32(a).to_bits());
    }
    (super::Pixels::Linear16F(out), has_alpha)
}

/// Linear RGBA32F (HDR) → tone-mapped straight sRGB RGBA8.
pub fn hdr_to_srgb8(v: &[f32]) -> Vec<u8> {
    v.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let e = |c: f32| (linear_to_srgb(tonemap(c)) * 255.0 + 0.5) as u8;
            [
                e(p[0]),
                e(p[1]),
                e(p[2]),
                (p[3].clamp(0.0, 1.0) * 255.0 + 0.5) as u8,
            ]
        })
        .collect()
}

/// Premultiplied linear half floats → straight sRGB RGBA8.
pub fn f16_to_straight_srgb8(v: &[u16]) -> Vec<u8> {
    v.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let f = |x: u16| half::f16::from_bits(x).to_f32();
            let a = f(p[3]).clamp(0.0, 1.0);
            let e = |x: u16| {
                let l = if a > 0.0 { f(x) / a } else { 0.0 };
                (linear_to_srgb(l.clamp(0.0, 1.0)) * 255.0 + 0.5) as u8
            };
            [e(p[0]), e(p[1]), e(p[2]), (a * 255.0 + 0.5) as u8]
        })
        .collect()
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
