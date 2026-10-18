//! JPEG via libjpeg-turbo (SIMD), with DCT-domain downscaling for previews.

use turbojpeg::{Colorspace, Decompressor, Image, PixelFormat, ScalingFactor};

use super::{DecodeError, DecodedImage, MAX_TEXTURE_SIDE, SizeHint};

/// Power-of-two factors have SIMD IDCT paths; the others are much slower.
const FACTORS: [ScalingFactor; 4] = [
    ScalingFactor::ONE_EIGHTH,
    ScalingFactor::ONE_QUARTER,
    ScalingFactor::ONE_HALF,
    ScalingFactor::ONE,
];

/// Picks the smallest factor that still does not need upscaling to fill `hint`.
/// Without a hint: full size, or the largest factor that fits a texture.
fn pick_factor(width: usize, height: usize, hint: Option<SizeHint>) -> ScalingFactor {
    let Some(h) = hint else {
        let max = MAX_TEXTURE_SIDE as usize;
        return FACTORS
            .into_iter()
            .rev()
            .find(|f| f.scale(width) <= max && f.scale(height) <= max)
            .unwrap_or(ScalingFactor::ONE_EIGHTH);
    };
    let need = h.min_scale(width as u32, height as u32);
    FACTORS
        .into_iter()
        .find(|f| f.num() as f64 / f.denom() as f64 >= need - 1e-9)
        .unwrap_or(ScalingFactor::ONE)
}

pub fn decode(data: &[u8], hint: Option<SizeHint>) -> Result<DecodedImage, DecodeError> {
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
    if cmyk {
        cmyk_to_rgba(&mut pixels);
    }
    crate::bench::mark("decoded (jpeg)");

    DecodedImage::from_straight_rgba(
        w as u32,
        h as u32,
        pixels,
        (header.width as u32, header.height as u32),
    )
}

/// Naive CMYK → RGB. Adobe CMYK JPEGs store inverted values, which is what
/// libjpeg returns as-is. Proper ICC-based conversion comes with colour
/// management (M4).
fn cmyk_to_rgba(px: &mut [u8]) {
    for p in px.as_chunks_mut::<4>().0 {
        let k = u32::from(p[3]);
        p[0] = ((u32::from(p[0]) * k + 127) / 255) as u8;
        p[1] = ((u32::from(p[1]) * k + 127) / 255) as u8;
        p[2] = ((u32::from(p[2]) * k + 127) / 255) as u8;
        p[3] = 255;
    }
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
        // Full resolution, capped at the texture limit.
        assert_eq!(pick_factor(30_000, 20_000, None), ScalingFactor::ONE_HALF);
        assert_eq!(pick_factor(16_384, 100, None), ScalingFactor::ONE);
    }
}
