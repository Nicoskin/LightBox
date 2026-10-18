//! Colour management: embedded ICC profiles → sRGB (Little CMS).
//!
//! Transforms are created with `NO_CACHE` so one transform can run on
//! several threads at once (large images are split into bands).

use std::thread;

use bytemuck::Pod;
use lcms2::{
    DisallowCache, Flags, GlobalContext, InfoType, Intent, Locale, PixelFormat, Profile, Transform,
};

/// A transform that may run on several threads at once (no cache).
type SharedTransform<F, T> = Transform<F, T, GlobalContext, DisallowCache>;

fn shared<F: Copy + Pod, T: Copy + Pod>(
    src: &Profile,
    in_format: PixelFormat,
    dst: &Profile,
    out_format: PixelFormat,
) -> Option<SharedTransform<F, T>> {
    Transform::new_flags_context(
        GlobalContext::new(),
        src,
        in_format,
        dst,
        out_format,
        Intent::Perceptual,
        Flags::NO_CACHE,
    )
    .ok()
}

use crate::decode::Raw;

/// `true` for profiles that are sRGB already: the transform would be an
/// (expensive) identity.
/// Human-readable name of an ICC profile ("Adobe RGB (1998)").
pub fn profile_name(icc: &[u8]) -> Option<String> {
    let p = Profile::new_icc(icc).ok()?;
    p.info(InfoType::Description, Locale::none())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn is_srgb(p: &Profile) -> bool {
    p.info(InfoType::Description, Locale::none())
        .is_some_and(|d| d.to_ascii_lowercase().contains("srgb"))
}

/// Runs `f` on `data` split into bands of whole pixels across threads.
fn parallel<T: Send>(data: &mut [T], pixel: usize, f: impl Fn(&mut [T]) + Sync) {
    let threads = thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(16);
    let pixels = data.len() / pixel;
    if pixels < 1 << 18 {
        f(data);
        return;
    }
    let band = pixels.div_ceil(threads) * pixel;
    thread::scope(|s| {
        for chunk in data.chunks_mut(band) {
            let f = &f;
            s.spawn(move || f(chunk));
        }
    });
}

/// Converts `data` from the colour space of `icc` to sRGB, in place.
/// Unreadable or non-RGB profiles are ignored.
pub fn to_srgb(icc: &[u8], data: &mut Raw) {
    let Ok(src) = Profile::new_icc(icc) else {
        tracing::debug!("unreadable ICC profile ignored");
        return;
    };
    if is_srgb(&src) || src.color_space() != lcms2::ColorSpaceSignature::RgbData {
        return;
    }
    let dst = Profile::new_srgb();
    match data {
        Raw::Rgba8(v) => {
            let Some(t) =
                shared::<[u8; 4], [u8; 4]>(&src, PixelFormat::RGBA_8, &dst, PixelFormat::RGBA_8)
            else {
                return;
            };
            let px = v.as_chunks_mut::<4>().0;
            parallel(px, 1, |c| t.transform_in_place(c));
        }
        Raw::Rgba16(v) => {
            let Some(t) = shared::<[u16; 4], [u16; 4]>(
                &src,
                PixelFormat::RGBA_16,
                &dst,
                PixelFormat::RGBA_16,
            ) else {
                return;
            };
            let px = v.as_chunks_mut::<4>().0;
            parallel(px, 1, |c| t.transform_in_place(c));
        }
        // HDR data is scene-linear; profiles there are rare and not handled.
        Raw::RgbaF32(_) => {}
    }
}

/// CMYK (as stored in the JPEG; Adobe files are `inverted`) → sRGB RGBA8
/// through the embedded profile. `None` if the profile is unusable.
pub fn cmyk_to_srgb(icc: &[u8], cmyk: &[u8], inverted: bool) -> Option<Vec<u8>> {
    let src = Profile::new_icc(icc).ok()?;
    if src.color_space() != lcms2::ColorSpaceSignature::CmykData {
        return None;
    }
    let format = if inverted {
        PixelFormat::CMYK_8_REV
    } else {
        PixelFormat::CMYK_8
    };
    let t = shared::<[u8; 4], [u8; 3]>(&src, format, &Profile::new_srgb(), PixelFormat::RGB_8)?;
    let input = cmyk.as_chunks::<4>().0;
    let mut rgb = vec![[0u8; 3]; input.len()];
    let threads = thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(16);
    let band = input.len().div_ceil(threads).max(1);
    thread::scope(|s| {
        for (src, dst) in input.chunks(band).zip(rgb.chunks_mut(band)) {
            let t = &t;
            s.spawn(move || t.transform_pixels(src, dst));
        }
    });
    Some(rgb.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn srgb_profile_is_a_no_op() {
        let icc = Profile::new_srgb().icc().unwrap();
        let mut d = Raw::Rgba8(vec![10, 200, 30, 255]);
        to_srgb(&icc, &mut d);
        assert!(matches!(d, Raw::Rgba8(ref v) if v == &[10, 200, 30, 255]));
    }

    #[test]
    fn wide_gamut_green_shrinks_into_srgb() {
        // Pure green in a wide-gamut (Rec.2020-like) space is outside sRGB:
        // after conversion red/blue stay ~0 and green saturates.
        let profile = Profile::new_rgb(
            &lcms2::CIExyY {
                x: 0.3127,
                y: 0.3290,
                Y: 1.0,
            },
            &lcms2::CIExyYTRIPLE {
                Red: lcms2::CIExyY {
                    x: 0.708,
                    y: 0.292,
                    Y: 1.0,
                },
                Green: lcms2::CIExyY {
                    x: 0.170,
                    y: 0.797,
                    Y: 1.0,
                },
                Blue: lcms2::CIExyY {
                    x: 0.131,
                    y: 0.046,
                    Y: 1.0,
                },
            },
            &[
                &lcms2::ToneCurve::new(2.2),
                &lcms2::ToneCurve::new(2.2),
                &lcms2::ToneCurve::new(2.2),
            ],
        )
        .unwrap();
        let icc = profile.icc().unwrap();
        let mut d = Raw::Rgba8(vec![128, 128, 128, 255, 0, 255, 0, 255]);
        to_srgb(&icc, &mut d);
        let Raw::Rgba8(v) = d else { unreachable!() };
        // Grey stays grey (same white point).
        assert!((i32::from(v[0]) - i32::from(v[1])).abs() <= 2);
        assert!(v[5] > 200 && v[4] < 60, "{v:?}");
        assert_eq!(v[7], 255);
    }
}
