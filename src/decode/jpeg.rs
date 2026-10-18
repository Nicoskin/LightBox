//! JPEG via libjpeg-turbo (SIMD), with DCT-domain downscaling for previews.

use turbojpeg::{Colorspace, Decompressor, Image, PixelFormat, ScalingFactor};

use super::{DecodeError, Raw, RawImage, SizeHint};
use crate::color;

/// Power-of-two factors have SIMD IDCT paths; the others are much slower.
const FACTORS: [ScalingFactor; 4] = [
    ScalingFactor::ONE_EIGHTH,
    ScalingFactor::ONE_QUARTER,
    ScalingFactor::ONE_HALF,
    ScalingFactor::ONE,
];

/// Picks the smallest factor that still does not need upscaling to fill
/// `hint`; full size without a hint.
fn pick_factor(width: usize, height: usize, hint: Option<SizeHint>) -> ScalingFactor {
    let Some(h) = hint else {
        return ScalingFactor::ONE;
    };
    let need = h.min_scale(width as u32, height as u32);
    FACTORS
        .into_iter()
        .find(|f| f.num() as f64 / f.denom() as f64 >= need - 1e-9)
        .unwrap_or(ScalingFactor::ONE)
}

pub fn decode(data: &[u8], hint: Option<SizeHint>) -> Result<RawImage, DecodeError> {
    let mut d = Decompressor::new()?;
    let header = d.read_header(data)?;
    let factor = pick_factor(header.width, header.height, hint);
    d.set_scaling_factor(factor)?;
    let scaled = header.scaled(factor);
    let (w, h) = (scaled.width, scaled.height);

    let cmyk = matches!(header.colorspace, Colorspace::CMYK | Colorspace::YCCK);
    let format = if cmyk {
        PixelFormat::CMYK
    } else {
        PixelFormat::RGBA
    };
    let mut pixels = vec![0u8; w * h * 4];
    d.decompress(
        data,
        Image {
            pixels: pixels.as_mut_slice(),
            width: w,
            pitch: w * 4,
            height: h,
            format,
        },
    )?;
    let mut icc = icc_profile(data);
    if cmyk {
        // Adobe (APP14) CMYK JPEGs store inverted values.
        let inverted = has_adobe_marker(data);
        pixels = match icc.take() {
            Some(p) => color::cmyk_to_srgb(&p, &pixels, inverted)
                .unwrap_or_else(|| naive_cmyk(&pixels, inverted)),
            None => naive_cmyk(&pixels, inverted),
        };
    }
    crate::bench::mark("decoded (jpeg)");

    let mut img = RawImage::new(w as u32, h as u32, Raw::Rgba8(pixels));
    img.source = (header.width as u32, header.height as u32);
    img.orientation = crate::metadata::jpeg_orientation(data);
    img.icc = icc;
    Ok(img)
}

/// CMYK → RGB without a profile.
fn naive_cmyk(px: &[u8], inverted: bool) -> Vec<u8> {
    px.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| {
            let v = |c: u8| {
                if inverted {
                    u32::from(c)
                } else {
                    255 - u32::from(c)
                }
            };
            let k = v(p[3]);
            let ch = |c: u8| ((v(c) * k + 127) / 255) as u8;
            [ch(p[0]), ch(p[1]), ch(p[2]), 255]
        })
        .collect()
}

/// JPEG segments before the image data: (marker, body).
fn segments(data: &[u8]) -> impl Iterator<Item = (u8, &[u8])> {
    let mut pos = 2;
    std::iter::from_fn(move || {
        while pos + 4 <= data.len() {
            if data[pos] != 0xFF {
                return None;
            }
            let m = data[pos + 1];
            if m == 0xFF {
                pos += 1;
                continue;
            }
            if m == 0xDA || m == 0xD9 {
                return None;
            }
            if (0xD0..=0xD7).contains(&m) || m == 0x01 {
                pos += 2;
                continue;
            }
            let len = usize::from(u16::from_be_bytes([data[pos + 2], data[pos + 3]]));
            let end = (pos + 2 + len).min(data.len());
            let body = &data[(pos + 4).min(end)..end];
            pos += 2 + len;
            return Some((m, body));
        }
        None
    })
}

/// Embedded ICC profile (APP2 "ICC_PROFILE" chunks, joined in order).
pub fn icc_profile(data: &[u8]) -> Option<Vec<u8>> {
    let mut parts: Vec<(u8, &[u8])> = segments(data)
        .filter(|(m, b)| *m == 0xE2 && b.starts_with(b"ICC_PROFILE\0") && b.len() > 14)
        .map(|(_, b)| (b[12], &b[14..]))
        .collect();
    if parts.is_empty() {
        return None;
    }
    parts.sort_by_key(|p| p.0);
    Some(
        parts
            .into_iter()
            .flat_map(|p| p.1.iter().copied())
            .collect(),
    )
}

fn has_adobe_marker(data: &[u8]) -> bool {
    segments(data).any(|(m, b)| m == 0xEE && b.starts_with(b"Adobe"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factor_selection() {
        let hint = |w, h| {
            Some(SizeHint {
                box_width: w,
                box_height: h,
            })
        };
        assert_eq!(pick_factor(6000, 4000, None), ScalingFactor::ONE);
        assert_eq!(
            pick_factor(6000, 4000, hint(700, 466)),
            ScalingFactor::ONE_EIGHTH
        );
        // 1920×1080 box: needs 0.27 → 1/2 (1/4 = 0.25 would upscale).
        assert_eq!(
            pick_factor(6000, 4000, hint(1920, 1080)),
            ScalingFactor::ONE_HALF
        );
        // Portrait source in the same box: the rotated fit (0.27) still wins.
        assert_eq!(
            pick_factor(4000, 6000, hint(1920, 1080)),
            ScalingFactor::ONE_HALF
        );
        assert_eq!(
            pick_factor(6000, 4000, hint(1400, 900)),
            ScalingFactor::ONE_QUARTER
        );
        assert_eq!(
            pick_factor(6000, 4000, hint(3840, 2160)),
            ScalingFactor::ONE
        );
        assert_eq!(pick_factor(100, 100, hint(1920, 1080)), ScalingFactor::ONE);
        // Full resolution without a hint (huge images are tiled).
        assert_eq!(pick_factor(30_000, 20_000, None), ScalingFactor::ONE);
    }
}
