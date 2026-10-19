//! Writing images: lossless rotation of existing files and "Save as".

pub mod jpeg;
pub mod other;
pub mod png;
pub mod tiff;

use crate::orientation::Orientation;

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("{0}")]
    Format(String),
    #[error("JPEG: {0}")]
    Jpeg(#[from] turbojpeg::Error),
    #[error("изображение слишком большое")]
    TooLarge,
    #[error("формат не поддерживает запись: {0}")]
    Unsupported(&'static str),
}

/// Reorders pixels (`bpp` bytes each) so that the result shows what
/// orientation `o` displays: the inverse of the mapping the shader uses
/// when drawing (display coordinate → stored coordinate).
pub fn orient_pixels(
    src: &[u8],
    w: usize,
    h: usize,
    bpp: usize,
    o: Orientation,
) -> (Vec<u8>, usize, usize) {
    let (dw, dh) = if o.swaps_axes() { (h, w) } else { (w, h) };
    let mut dst = vec![0u8; dw * dh * bpp];
    for y in 0..dh {
        for x in 0..dw {
            let (sx, sy) = match o.exif() {
                2 => (dw - 1 - x, y),
                3 => (dw - 1 - x, dh - 1 - y),
                4 => (x, dh - 1 - y),
                5 => (y, x),
                6 => (y, dw - 1 - x),
                7 => (dh - 1 - y, dw - 1 - x),
                8 => (dh - 1 - y, x),
                _ => (x, y),
            };
            let s = (sy * w + sx) * bpp;
            let d = (y * dw + x) * bpp;
            dst[d..d + bpp].copy_from_slice(&src[s..s + bpp]);
        }
    }
    (dst, dw, dh)
}

/// JPEG from straight sRGB RGBA8 (alpha is dropped), 4:2:0 below quality 90,
/// 4:4:4 above (keeps colour edges crisp).
pub fn encode_jpeg(rgba: &[u8], w: usize, h: usize, quality: u8) -> Result<Vec<u8>, EncodeError> {
    let mut c = turbojpeg::Compressor::new()?;
    c.set_quality(i32::from(quality.clamp(1, 100)))?;
    c.set_subsamp(if quality >= 90 {
        turbojpeg::Subsamp::None
    } else {
        turbojpeg::Subsamp::Sub2x2
    })?;
    c.set_optimize(true)?;
    Ok(c.compress_to_vec(turbojpeg::Image {
        pixels: rgba,
        width: w,
        pitch: w * 4,
        height: h,
        format: turbojpeg::PixelFormat::RGBX,
    })?)
}

/// PNG from straight sRGB RGBA8 (RGB when fully opaque).
pub fn encode_png(rgba: &[u8], w: usize, h: usize) -> Result<Vec<u8>, EncodeError> {
    let opaque = rgba.as_chunks::<4>().0.iter().all(|p| p[3] == 255);
    let (data, color): (Vec<u8>, ::png::ColorType) = if opaque {
        (
            rgba.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|p| [p[0], p[1], p[2]])
                .collect(),
            ::png::ColorType::Rgb,
        )
    } else {
        (rgba.to_vec(), ::png::ColorType::Rgba)
    };
    let mut out = Vec::new();
    {
        let mut e = ::png::Encoder::new(&mut out, w as u32, h as u32);
        e.set_color(color);
        e.set_depth(::png::BitDepth::Eight);
        e.set_compression(::png::Compression::High);
        let mut wr = e
            .write_header()
            .map_err(|e| EncodeError::Format(e.to_string()))?;
        wr.write_image_data(&data)
            .map_err(|e| EncodeError::Format(e.to_string()))?;
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn orient_pixels_inverse_pairs() {
        let src: Vec<u8> = (0..12).collect(); // 4×3, 1 byte per pixel
        for code in 1..=8 {
            let o = Orientation::from_exif(code);
            let (a, w, h) = orient_pixels(&src, 4, 3, 1, o);
            // Undo with the inverse operation.
            let inv = (1..=8)
                .map(Orientation::from_exif)
                .find(|i| o.then(*i) == Orientation::IDENTITY)
                .unwrap();
            let (b, w2, h2) = orient_pixels(&a, w, h, 1, inv);
            assert_eq!((b, w2, h2), (src.clone(), 4, 3), "code {code}");
        }
    }

    #[test]
    fn export_encoders_roundtrip() {
        let rgba: Vec<u8> = (0..(8 * 8))
            .flat_map(|i| [i as u8, 0, 255 - i as u8, 255])
            .collect();
        let png = encode_png(&rgba, 8, 8).unwrap();
        let img = crate::decode::decode_bytes(&png, None).unwrap();
        assert_eq!((img.width, img.height), (8, 8));
        let jpg = encode_jpeg(&rgba, 8, 8, 95).unwrap();
        assert!(crate::decode::decode_bytes(&jpg, None).is_ok());
    }
}
