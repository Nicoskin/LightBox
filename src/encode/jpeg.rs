//! Lossless JPEG rotation (libjpeg-turbo `tj3Transform`, the jpegtran
//! engine): DCT coefficients are rearranged, never re-quantised.

use turbojpeg::{Transform, TransformOp, Transformer};

use super::{EncodeError, tiff};
use crate::orientation::Orientation;
use crate::settings::ImperfectJpeg;

/// The transform that turns stored pixels into what EXIF `o` displays.
pub fn op_for(o: Orientation) -> TransformOp {
    match o.exif() {
        2 => TransformOp::Hflip,
        3 => TransformOp::Rot180,
        4 => TransformOp::Vflip,
        5 => TransformOp::Transpose,
        6 => TransformOp::Rot90,
        7 => TransformOp::Transverse,
        8 => TransformOp::Rot270,
        _ => TransformOp::None,
    }
}

/// Bakes `display` (the full orientation the photo is shown with) into the
/// file. Returns `Ok(None)` when an exact lossless transform is impossible
/// (size not a multiple of the MCU) and no `imperfect` policy was given —
/// the caller should ask the user.
pub fn rotate(
    data: &[u8],
    display: Orientation,
    imperfect: Option<ImperfectJpeg>,
) -> Result<Option<Vec<u8>>, EncodeError> {
    if display == Orientation::IDENTITY {
        return Ok(Some(edit_exif(data, false, |t| {
            tiff::set_orientation(t, 1);
        })));
    }
    if imperfect == Some(ImperfectJpeg::ExifOnly) {
        let code = u16::from(display.exif());
        return Ok(Some(edit_exif(data, true, |t| {
            tiff::set_orientation(t, code);
        })));
    }

    let header = turbojpeg::read_header(data)?;
    let make = |perfect: bool, trim: bool| {
        let mut t = Transform::op(op_for(display));
        t.perfect = perfect;
        t.trim = trim;
        // Keep the coding style; optimised Huffman tables keep the size.
        t.progressive = header.is_progressive;
        t.arithmetic = header.is_arithmetic;
        t.optimize = !header.is_arithmetic;
        t
    };
    let mut tj = Transformer::new()?;
    let out = match imperfect {
        Some(ImperfectJpeg::Trim) => tj.transform_to_vec(&make(false, true), data)?,
        _ => match tj.transform_to_vec(&make(true, false), data) {
            Ok(v) => v,
            Err(e) => {
                // Distinguish "not perfect" from a broken file.
                return if tj.transform_to_vec(&make(false, true), data).is_ok() {
                    Ok(None)
                } else {
                    Err(e.into())
                };
            }
        },
    };

    let op = op_for(display);
    Ok(Some(edit_exif(&out, false, |t| {
        tiff::normalize(t, display.swaps_axes(), |thumb| {
            let mut tr = Transform::op(op);
            tr.trim = true;
            Transformer::new().ok()?.transform_to_vec(&tr, thumb).ok()
        });
    })))
}

/// Crops an upright JPEG to `c` (x, y, width, height) without re-encoding.
/// The left and top edges move out to the block grid (8 or 16 pixels);
/// returns the bytes and the rectangle kept. EXIF dimensions and the
/// embedded thumbnail follow.
pub fn crop(data: &[u8], c: [u32; 4]) -> Result<(Vec<u8>, [u32; 4]), EncodeError> {
    let header = turbojpeg::read_header(data)?;
    let (mw, mh) = (header.subsamp.mcu_width(), header.subsamp.mcu_height());
    let x = (c[0] as usize).min(header.width.saturating_sub(1)) / mw * mw;
    let y = (c[1] as usize).min(header.height.saturating_sub(1)) / mh * mh;
    let w = (c[0] as usize + c[2] as usize).min(header.width) - x;
    let h = (c[1] as usize + c[3] as usize).min(header.height) - y;
    if w == 0 || h == 0 {
        return Err(EncodeError::Format("пустая область обрезки".into()));
    }
    let mut t = Transform::op(TransformOp::None);
    t.crop = Some(turbojpeg::TransformCrop {
        x,
        y,
        width: Some(w),
        height: Some(h),
    });
    t.progressive = header.is_progressive;
    t.arithmetic = header.is_arithmetic;
    t.optimize = !header.is_arithmetic;
    let out = Transformer::new()?.transform_to_vec(&t, data)?;
    let thumb = small_thumbnail(&out);
    let out = edit_exif(&out, false, |t| {
        tiff::set_dims(t, w as u32, h as u32);
        tiff::normalize(t, false, |_| thumb.clone());
    });
    Ok((out, [x as u32, y as u32, w as u32, h as u32]))
}

/// A new EXIF thumbnail (at most 160 px) from a JPEG.
fn small_thumbnail(jpeg: &[u8]) -> Option<Vec<u8>> {
    let mut d = turbojpeg::Decompressor::new().ok()?;
    d.set_scaling_factor(turbojpeg::ScalingFactor::ONE_EIGHTH)
        .ok()?;
    let header = d
        .read_header(jpeg)
        .ok()?
        .scaled(turbojpeg::ScalingFactor::ONE_EIGHTH);
    let (w, h) = (header.width, header.height);
    let mut px = vec![0u8; w * h * 4];
    d.decompress(
        jpeg,
        turbojpeg::Image {
            pixels: px.as_mut_slice(),
            width: w,
            pitch: w * 4,
            height: h,
            format: turbojpeg::PixelFormat::RGBA,
        },
    )
    .ok()?;
    let img = image::RgbaImage::from_raw(w as u32, h as u32, px)?;
    let k = 160.0 / w.max(h) as f64;
    let img = if k < 1.0 {
        image::imageops::resize(
            &img,
            ((w as f64 * k).round() as u32).max(1),
            ((h as f64 * k).round() as u32).max(1),
            image::imageops::FilterType::Triangle,
        )
    } else {
        img
    };
    let (tw, th) = (img.width() as usize, img.height() as usize);
    super::encode_jpeg(&img.into_raw(), tw, th, 80).ok()
}

/// Location of the EXIF block: (APP1 marker offset, TIFF start, TIFF end).
fn find_exif(data: &[u8]) -> Option<(usize, usize, usize)> {
    let mut pos = 2;
    while pos + 4 <= data.len() {
        if data[pos] != 0xFF {
            return None;
        }
        let marker = data[pos + 1];
        if marker == 0xFF {
            pos += 1; // fill byte
            continue;
        }
        if marker == 0xDA || marker == 0xD9 {
            return None; // start of scan / end of image
        }
        if (0xD0..=0xD7).contains(&marker) || marker == 0x01 {
            pos += 2;
            continue;
        }
        let len = usize::from(u16::from_be_bytes([data[pos + 2], data[pos + 3]]));
        let body = pos + 4;
        let end = pos + 2 + len;
        if end > data.len() || len < 2 {
            return None;
        }
        if marker == 0xE1 && data[body..end].starts_with(b"Exif\0\0") {
            return Some((pos, body + 6, end));
        }
        pos = end;
    }
    None
}

/// Edits the EXIF TIFF block with `f` and rebuilds the APP1 segment. With
/// `create`, a minimal EXIF block is inserted when there is none.
pub fn edit_exif(data: &[u8], create: bool, f: impl FnOnce(&mut Vec<u8>)) -> Vec<u8> {
    let (seg, start, end, mut tiff) = match find_exif(data) {
        Some((seg, start, end)) => (seg, start, end, data[start..end].to_vec()),
        None if create => {
            // After SOI and a JFIF APP0, if present.
            let mut at = 2;
            if data.get(2..4) == Some(&[0xFF, 0xE0]) && data.len() > 6 {
                at = 4 + usize::from(u16::from_be_bytes([data[4], data[5]]));
            }
            let mut empty = b"II*\0".to_vec();
            empty.extend_from_slice(&8u32.to_le_bytes());
            empty.extend_from_slice(&[0, 0, 0, 0, 0, 0]); // 0 entries, no next IFD
            (at, at, at, empty)
        }
        None => return data.to_vec(),
    };
    f(&mut tiff);
    let seg_len = tiff.len() + 6 + 2;
    if seg_len > 0xFFFF {
        return data.to_vec(); // cannot grow beyond one segment: leave as is
    }
    let mut out = Vec::with_capacity(data.len() + 64);
    out.extend_from_slice(&data[..seg]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&(seg_len as u16).to_be_bytes());
    out.extend_from_slice(b"Exif\0\0");
    out.extend_from_slice(&tiff);
    out.extend_from_slice(&data[end.max(start)..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use turbojpeg::{Compressor, Image, PixelFormat, Subsamp};

    /// Test JPEG `w × h` with a gradient so that orientation is visible.
    fn jpeg(w: usize, h: usize) -> Vec<u8> {
        let mut px = vec![0u8; w * h * 3];
        for y in 0..h {
            for x in 0..w {
                let i = (y * w + x) * 3;
                px[i] = (x * 255 / w) as u8;
                px[i + 1] = (y * 255 / h) as u8;
                px[i + 2] = 128;
            }
        }
        let mut c = Compressor::new().unwrap();
        c.set_quality(90).unwrap();
        c.set_subsamp(Subsamp::Sub2x2).unwrap();
        c.compress_to_vec(Image {
            pixels: px.as_slice(),
            width: w,
            pitch: w * 3,
            height: h,
            format: PixelFormat::RGB,
        })
        .unwrap()
    }

    fn pixels(data: &[u8]) -> (usize, usize, Vec<u8>) {
        let img = turbojpeg::decompress(data, PixelFormat::RGB).unwrap();
        (img.width, img.height, img.pixels)
    }

    /// Cropping moves the left / top edges out to the 16-pixel grid of a
    /// 4:2:0 JPEG and keeps the pixels: the kept part decodes like the
    /// same part of the original.
    #[test]
    fn crop_snaps_to_blocks_and_keeps_pixels() {
        let original = jpeg(96, 64);
        let (out, kept) = crop(&original, [21, 18, 40, 30]).unwrap();
        assert_eq!(kept, [16, 16, 45, 32]);
        let (w, h, px) = pixels(&out);
        assert_eq!((w, h), (45, 32));
        let (ow, _, opx) = pixels(&original);
        // Away from the block edges, the pixels match the original's.
        for (x, y) in [(5, 5), (20, 10), (40, 30)] {
            let a = &px[(y * w + x) * 3..][..3];
            let b = &opx[((y + 16) * ow + x + 16) * 3..][..3];
            assert!(a.iter().zip(b).all(|(p, q)| p.abs_diff(*q) <= 2), "{x},{y}");
        }
    }

    #[test]
    fn four_quarter_turns_are_lossless() {
        let original = jpeg(64, 48);
        let mut cur = original.clone();
        for _ in 0..4 {
            cur = rotate(&cur, Orientation::ROTATE_CW, None).unwrap().unwrap();
        }
        // Coefficients only moved around: identical pixels after 360°.
        assert_eq!(pixels(&cur), pixels(&original));
        let (w, h, _) = pixels(
            &rotate(&original, Orientation::ROTATE_CW, None)
                .unwrap()
                .unwrap(),
        );
        assert_eq!((w, h), (48, 64));
    }

    #[test]
    fn rotation_matches_reference_transform() {
        // Same result as libjpeg-turbo's own transform (what jpegtran does).
        let src = jpeg(64, 32);
        let ours = rotate(&src, Orientation::from_exif(3), None)
            .unwrap()
            .unwrap();
        let reference = turbojpeg::transform(&Transform::op(TransformOp::Rot180), &src).unwrap();
        assert_eq!(pixels(&ours), pixels(&reference));
    }

    #[test]
    fn imperfect_size_asks_then_trims_or_writes_exif() {
        let src = jpeg(70, 50); // not a multiple of the 16×16 MCU
        assert!(
            rotate(&src, Orientation::ROTATE_CW, None)
                .unwrap()
                .is_none()
        );
        let trimmed = rotate(&src, Orientation::ROTATE_CW, Some(ImperfectJpeg::Trim))
            .unwrap()
            .unwrap();
        let (w, h, _) = pixels(&trimmed);
        assert!(w <= 50 && h <= 70 && w >= 48 && h >= 64, "{w}×{h}");
        let tagged = rotate(&src, Orientation::ROTATE_CW, Some(ImperfectJpeg::ExifOnly))
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::metadata::jpeg_orientation(&tagged),
            Orientation::ROTATE_CW
        );
        assert_eq!(pixels(&tagged), pixels(&src));
    }

    #[test]
    fn exif_orientation_is_reset_after_rotation() {
        let src = jpeg(64, 48);
        let tagged = edit_exif(&src, true, |t| {
            tiff::set_orientation(t, 6);
        });
        assert_eq!(
            crate::metadata::jpeg_orientation(&tagged),
            Orientation::from_exif(6)
        );
        // Shown as EXIF 6 → bake it: pixels rotate, tag becomes 1.
        let baked = rotate(&tagged, Orientation::from_exif(6), None)
            .unwrap()
            .unwrap();
        assert_eq!(
            crate::metadata::jpeg_orientation(&baked),
            Orientation::IDENTITY
        );
        assert_eq!(pixels(&baked).0, 48);
    }
}
