//! Rotation of the formats other than JPEG and PNG, without quality loss:
//! TIFF gets the Orientation tag (pixels and metadata untouched), GIF has
//! every frame's palette indices turned (palettes, timing and loop stay),
//! BMP / QOI / TGA / PNM and lossless WebP are re-encoded from the turned
//! pixels at their bit depth.

use std::io::Cursor;

use image::{DynamicImage, ImageEncoder, ImageFormat};

use super::{EncodeError, orient_pixels, tiff};
use crate::orientation::Orientation;

/// Writes new pixels (straight RGBA8, upright) in the original's format,
/// losslessly: TIFF, BMP, QOI, TGA, PNM and lossless WebP. Lossy and
/// palette formats are refused.
pub fn encode_like(
    original: &[u8],
    ext: &str,
    rgba: &[u8],
    w: usize,
    h: usize,
) -> Result<Vec<u8>, EncodeError> {
    let is_tiff = original.starts_with(b"II*\0") || original.starts_with(b"MM\0*");
    let format = if is_tiff && matches!(ext, "tif" | "tiff") {
        ImageFormat::Tiff
    } else if original.len() > 16 && &original[0..4] == b"RIFF" && &original[8..12] == b"WEBP" {
        if !webp_is_lossless(original) {
            return Err(EncodeError::Unsupported(
                "WebP со сжатием с потерями не пересохраняется. Сохраните копию через «Сохранить как»",
            ));
        }
        ImageFormat::WebP
    } else {
        match ext {
            "bmp" | "dib" if original.starts_with(b"BM") => ImageFormat::Bmp,
            "qoi" => ImageFormat::Qoi,
            "tga" => ImageFormat::Tga,
            "pbm" | "pgm" | "ppm" | "pnm" | "pam" => ImageFormat::Pnm,
            _ => {
                return Err(EncodeError::Unsupported(
                    "этот формат не пересохраняется. Сохраните копию через «Сохранить как»",
                ));
            }
        }
    };
    let img = image::RgbaImage::from_raw(w as u32, h as u32, rgba.to_vec())
        .ok_or_else(|| EncodeError::Format("размер".into()))?;
    let img = if img.pixels().all(|p| p[3] == 255) && format != ImageFormat::WebP {
        DynamicImage::ImageRgb8(DynamicImage::ImageRgba8(img).into_rgb8())
    } else {
        DynamicImage::ImageRgba8(img)
    };
    let mut out = Vec::new();
    img.write_to(&mut Cursor::new(&mut out), format)
        .map_err(|e| EncodeError::Format(e.to_string()))?;
    Ok(out)
}

/// `true` for a still WebP with lossless (VP8L) data.
fn webp_is_lossless(data: &[u8]) -> bool {
    let mut pos = 12;
    let mut lossless = false;
    while pos + 8 <= data.len() {
        let kind = &data[pos..pos + 4];
        let len = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
            as usize;
        match kind {
            b"ANIM" | b"VP8 " => return false,
            b"VP8L" => lossless = true,
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    lossless
}

/// Rotates `data` (file extension `ext`, lower case) so that it shows what
/// `display` shows today.
#[cfg(test)]
pub fn rotate(data: &[u8], ext: &str, display: Orientation) -> Result<Vec<u8>, EncodeError> {
    transform(data, ext, display, None)
}

/// [`rotate`], then keeps only `crop` (x, y, width, height in the turned
/// image). A cropped TIFF is written from its pixels (its metadata is not
/// carried over); GIFs are not cropped.
pub fn transform(
    data: &[u8],
    ext: &str,
    display: Orientation,
    crop: Option<[u32; 4]>,
) -> Result<Vec<u8>, EncodeError> {
    let is_tiff = data.starts_with(b"II*\0") || data.starts_with(b"MM\0*");
    if is_tiff && !matches!(ext, "tif" | "tiff") {
        return Err(only_view());
    }
    if is_tiff && crop.is_none() {
        let mut out = data.to_vec();
        if !tiff::set_orientation(&mut out, u16::from(display.exif())) {
            return Err(EncodeError::Format("повреждённый TIFF".into()));
        }
        return Ok(out);
    }
    if data.starts_with(b"GIF8") {
        if crop.is_some() {
            return Err(EncodeError::Unsupported("обрезка GIF не поддерживается"));
        }
        return gif(data, display);
    }
    if data.len() > 16 && &data[0..4] == b"RIFF" && &data[8..12] == b"WEBP" {
        return webp(data, display, crop);
    }
    let format = match ext {
        _ if is_tiff => ImageFormat::Tiff,
        "bmp" | "dib" if data.starts_with(b"BM") => ImageFormat::Bmp,
        "qoi" => ImageFormat::Qoi,
        "tga" => ImageFormat::Tga,
        "pbm" | "pgm" | "ppm" | "pnm" | "pam" => ImageFormat::Pnm,
        "avif" | "heic" | "heif" | "hif" => {
            return Err(EncodeError::Unsupported(
                "поворот AVIF и HEIC сохраняется только при просмотре: \
                 в программе нет их кодировщика. Сохраните копию через «Сохранить как»",
            ));
        }
        _ => return Err(only_view()),
    };
    let img = image::load_from_memory_with_format(data, format)
        .map_err(|e| EncodeError::Format(e.to_string()))?;
    let mut out = Vec::new();
    cut(turn(img, display), crop)
        .write_to(&mut Cursor::new(&mut out), format)
        .map_err(|e| EncodeError::Format(e.to_string()))?;
    Ok(out)
}

/// The `crop` part of `img` (clamped), or all of it.
fn cut(img: DynamicImage, crop: Option<[u32; 4]>) -> DynamicImage {
    match crop {
        Some([x, y, w, h]) => {
            let x = x.min(img.width().saturating_sub(1));
            let y = y.min(img.height().saturating_sub(1));
            img.crop_imm(
                x,
                y,
                w.min(img.width() - x).max(1),
                h.min(img.height() - y).max(1),
            )
        }
        None => img,
    }
}

fn only_view() -> EncodeError {
    EncodeError::Unsupported(
        "этот формат поворачивается только при просмотре. Сохраните копию через «Сохранить как»",
    )
}

/// `img` as `display` shows it.
fn turn(img: DynamicImage, display: Orientation) -> DynamicImage {
    match display.exif() {
        2 => img.fliph(),
        3 => img.rotate180(),
        4 => img.flipv(),
        5 => img.rotate90().fliph(),
        6 => img.rotate90(),
        7 => img.rotate270().fliph(),
        8 => img.rotate270(),
        _ => img,
    }
}

/// Lossless WebP (VP8L) stills only: lossy ones would be re-compressed, and
/// animations are not encoded by the WebP library in use.
fn webp(data: &[u8], display: Orientation, crop: Option<[u32; 4]>) -> Result<Vec<u8>, EncodeError> {
    let mut lossless = false;
    let mut pos = 12;
    while pos + 8 <= data.len() {
        let kind = &data[pos..pos + 4];
        let len = u32::from_le_bytes([data[pos + 4], data[pos + 5], data[pos + 6], data[pos + 7]])
            as usize;
        match kind {
            b"ANIM" => {
                return Err(EncodeError::Unsupported(
                    "поворот анимированного WebP сохраняется только при просмотре",
                ));
            }
            b"VP8 " => {
                return Err(EncodeError::Unsupported(
                    "поворот WebP со сжатием с потерями сохраняется только при просмотре: \
                     пересохранение ухудшило бы качество. Сохраните копию через «Сохранить как»",
                ));
            }
            b"VP8L" => lossless = true,
            _ => {}
        }
        pos += 8 + len + (len & 1);
    }
    if !lossless {
        return Err(EncodeError::Format("повреждённый WebP".into()));
    }
    let mut reader = image::ImageReader::with_format(Cursor::new(data), ImageFormat::WebP);
    reader.no_limits();
    let mut dec = reader
        .into_decoder()
        .map_err(|e| EncodeError::Format(e.to_string()))?;
    let icc = image::ImageDecoder::icc_profile(&mut dec).ok().flatten();
    let img = DynamicImage::from_decoder(dec).map_err(|e| EncodeError::Format(e.to_string()))?;
    let img = cut(turn(img, display), crop);
    // The encoder takes 8-bit RGB(A) only; lossless WebP is 8-bit anyway.
    let img = if img.color().has_alpha() {
        DynamicImage::ImageRgba8(img.into_rgba8())
    } else {
        DynamicImage::ImageRgb8(img.into_rgb8())
    };
    let mut out = Vec::new();
    let mut enc = image::codecs::webp::WebPEncoder::new_lossless(&mut out);
    if let Some(icc) = icc {
        let _ = enc.set_icc_profile(icc);
    }
    enc.write_image(
        img.as_bytes(),
        img.width(),
        img.height(),
        img.color().into(),
    )
    .map_err(|e| EncodeError::Format(e.to_string()))?;
    Ok(out)
}

/// Every frame turned at the palette-index level: no colours change.
fn gif(data: &[u8], display: Orientation) -> Result<Vec<u8>, EncodeError> {
    let err = |e: &dyn std::fmt::Display| EncodeError::Format(e.to_string());
    let mut opts = gif::DecodeOptions::new();
    opts.set_color_output(gif::ColorOutput::Indexed);
    let mut dec = opts.read_info(Cursor::new(data)).map_err(|e| err(&e))?;
    let (cw, ch) = (usize::from(dec.width()), usize::from(dec.height()));
    let (ncw, nch) = if display.swaps_axes() {
        (ch, cw)
    } else {
        (cw, ch)
    };
    let global = dec.global_palette().map(<[u8]>::to_vec).unwrap_or_default();
    let repeat = dec.repeat();
    let mut out = Vec::new();
    {
        let mut enc =
            gif::Encoder::new(&mut out, ncw as u16, nch as u16, &global).map_err(|e| err(&e))?;
        enc.set_repeat(repeat).map_err(|e| err(&e))?;
        while let Some(f) = dec.read_next_frame().map_err(|e| err(&e))? {
            let (w, h) = (usize::from(f.width), usize::from(f.height));
            let (x, y) = (usize::from(f.left), usize::from(f.top));
            if w == 0 || h == 0 || f.buffer.len() < w * h {
                continue;
            }
            let (px, nw, nh) = orient_pixels(&f.buffer[..w * h], w, h, 1, display);
            // Frames may hang over the canvas edge: clamp before turning.
            let a = super::png::display_of(display, cw.max(x + w), ch.max(y + h), x, y);
            let b =
                super::png::display_of(display, cw.max(x + w), ch.max(y + h), x + w - 1, y + h - 1);
            let mut g = f.clone();
            g.width = nw as u16;
            g.height = nh as u16;
            g.left = a.0.min(b.0) as u16;
            g.top = a.1.min(b.1) as u16;
            g.interlaced = false;
            g.buffer = px.into();
            enc.write_frame(&g).map_err(|e| err(&e))?;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gif_sample() -> Vec<u8> {
        // 4×2 canvas, palette black/red/green; frame 1 full, frame 2 a 1×2
        // column at (3, 0) with its own delay.
        let mut out = Vec::new();
        {
            let pal = [0, 0, 0, 255, 0, 0, 0, 255, 0];
            let mut e = gif::Encoder::new(&mut out, 4, 2, &pal).unwrap();
            e.set_repeat(gif::Repeat::Infinite).unwrap();
            let mut f = gif::Frame {
                width: 4,
                height: 2,
                buffer: vec![0, 1, 2, 0, 1, 1, 1, 2].into(),
                delay: 10,
                ..Default::default()
            };
            e.write_frame(&f).unwrap();
            f.width = 1;
            f.height = 2;
            f.left = 3;
            f.buffer = vec![2, 1].into();
            f.delay = 20;
            e.write_frame(&f).unwrap();
        }
        out
    }

    /// Width, height, left, top, delay and palette indices of a frame.
    type GifFrame = (u16, u16, u16, u16, u16, Vec<u8>);

    fn gif_frames(data: &[u8]) -> (u16, u16, Vec<GifFrame>) {
        let mut o = gif::DecodeOptions::new();
        o.set_color_output(gif::ColorOutput::Indexed);
        let mut d = o.read_info(Cursor::new(data)).unwrap();
        let (w, h) = (d.width(), d.height());
        let mut v = Vec::new();
        while let Some(f) = d.read_next_frame().unwrap() {
            v.push((f.width, f.height, f.left, f.top, f.delay, f.buffer.to_vec()));
        }
        (w, h, v)
    }

    #[test]
    fn gif_frames_turn_with_their_positions() {
        let out = rotate(&gif_sample(), "gif", Orientation::ROTATE_CW).unwrap();
        let (w, h, f) = gif_frames(&out);
        assert_eq!((w, h), (2, 4));
        // Rows [0 1 2 0 / 1 1 1 2] turned clockwise.
        assert_eq!(f[0].5, [1, 0, 1, 1, 1, 2, 2, 0]);
        // The 1×2 column at (3, 0) becomes a 2×1 row at (0, 3), delay kept.
        assert_eq!((f[1].0, f[1].1, f[1].2, f[1].3, f[1].4), (2, 1, 0, 3, 20));
        assert_eq!(f[1].5, [1, 2]);
        let back = rotate(&out, "gif", Orientation::ROTATE_CCW).unwrap();
        assert_eq!(gif_frames(&back), gif_frames(&gif_sample()));
    }

    #[test]
    fn bmp_and_qoi_round_trip() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::from_fn(3, 2, |x, y| {
            image::Rgb([(x * 10 + y) as u8, 7, 9])
        }));
        for (ext, fmt) in [("bmp", ImageFormat::Bmp), ("qoi", ImageFormat::Qoi)] {
            let mut src = Vec::new();
            img.write_to(&mut Cursor::new(&mut src), fmt).unwrap();
            let cw = rotate(&src, ext, Orientation::ROTATE_CW).unwrap();
            let t = image::load_from_memory_with_format(&cw, fmt).unwrap();
            assert_eq!((t.width(), t.height()), (2, 3), "{ext}");
            let back = rotate(&cw, ext, Orientation::ROTATE_CCW).unwrap();
            let b = image::load_from_memory_with_format(&back, fmt).unwrap();
            assert_eq!(b.to_rgb8(), img.to_rgb8(), "{ext}");
        }
    }

    #[test]
    fn webp_lossless_turns_lossy_refuses() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_fn(3, 2, |x, y| {
            image::Rgba([(x * 40) as u8, (y * 90) as u8, 5, 200])
        }));
        let mut src = Vec::new();
        img.write_to(&mut Cursor::new(&mut src), ImageFormat::WebP)
            .unwrap();
        let cw = rotate(&src, "webp", Orientation::ROTATE_CW).unwrap();
        let t = image::load_from_memory(&cw).unwrap();
        assert_eq!((t.width(), t.height()), (2, 3));
        assert_eq!(t.to_rgba8(), turn(img, Orientation::ROTATE_CW).to_rgba8());
        // A lossy VP8 chunk is refused.
        let mut lossy = b"RIFF\x0c\0\0\0WEBPVP8 \0\0\0\0".to_vec();
        lossy[4] = (lossy.len() - 8) as u8;
        assert!(matches!(
            rotate(&lossy, "webp", Orientation::ROTATE_CW),
            Err(EncodeError::Unsupported(_))
        ));
    }

    #[test]
    fn tiff_gets_the_tag_only() {
        let img = DynamicImage::ImageRgb8(image::RgbImage::new(3, 2));
        let mut src = Vec::new();
        img.write_to(&mut Cursor::new(&mut src), ImageFormat::Tiff)
            .unwrap();
        let out = rotate(&src, "tif", Orientation::ROTATE_CW).unwrap();
        let mut d = out.clone();
        assert_eq!(tiff::orientation(&mut d), Some(6));
        // Raw camera files are TIFF too, but are never written.
        assert!(rotate(&src, "dng", Orientation::ROTATE_CW).is_err());
    }
}
