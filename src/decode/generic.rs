//! Formats decoded by the `image` crate: GIF and WebP (first frame;
//! animation comes with M7), BMP, ICO, TIFF, TGA, QOI, PNM, DDS, and the HDR
//! formats Radiance HDR and OpenEXR (tone-mapped later).

use std::io::Cursor;

use image::{DynamicImage, ImageDecoder, ImageFormat, ImageReader};

use super::{DecodeError, Raw, RawImage};
use crate::orientation::Orientation;

pub fn decode(data: &[u8], format: ImageFormat) -> Result<RawImage, DecodeError> {
    let mut reader = ImageReader::with_format(Cursor::new(data), format);
    reader.no_limits();
    let mut dec = reader.into_decoder().map_err(DecodeError::other)?;
    let icc = dec.icc_profile().ok().flatten();
    let orientation = dec.orientation().map_or(Orientation::IDENTITY, |o| {
        Orientation::from_exif(u32::from(o.to_exif()))
    });
    let img = DynamicImage::from_decoder(dec).map_err(DecodeError::other)?;
    let (w, h) = (img.width(), img.height());
    let data = match img {
        DynamicImage::ImageRgb32F(_) | DynamicImage::ImageRgba32F(_) => {
            Raw::RgbaF32(img.into_rgba32f().into_raw())
        }
        DynamicImage::ImageLuma16(_)
        | DynamicImage::ImageLumaA16(_)
        | DynamicImage::ImageRgb16(_)
        | DynamicImage::ImageRgba16(_) => Raw::Rgba16(img.into_rgba16().into_raw()),
        other => Raw::Rgba8(other.into_rgba8().into_raw()),
    };
    let mut out = RawImage::new(w, h, data);
    out.orientation = orientation;
    out.icc = icc;
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn encode(img: &DynamicImage, f: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, f).unwrap();
        out.into_inner()
    }

    #[test]
    fn decodes_common_formats() {
        let mut src = RgbaImage::new(7, 5);
        for (x, y, p) in src.enumerate_pixels_mut() {
            *p = Rgba([(x * 30) as u8, (y * 40) as u8, 200, 255]);
        }
        let img = DynamicImage::ImageRgba8(src);
        for f in [
            ImageFormat::Bmp,
            ImageFormat::Tiff,
            ImageFormat::Tga,
            ImageFormat::Qoi,
            ImageFormat::Gif,
            ImageFormat::WebP,
            ImageFormat::Ico,
            ImageFormat::Pnm,
        ] {
            let bytes = encode(&img, f);
            let out = decode(&bytes, f).unwrap_or_else(|e| panic!("{f:?}: {e}"));
            assert_eq!((out.width, out.height), (7, 5), "{f:?}");
        }
    }

    #[test]
    fn hdr_is_float() {
        let img = DynamicImage::ImageRgb32F(image::Rgb32FImage::from_pixel(
            3,
            2,
            image::Rgb([4.0, 0.5, 0.1]),
        ));
        let out = decode(&encode(&img, ImageFormat::OpenExr), ImageFormat::OpenExr).unwrap();
        assert!(matches!(out.data, Raw::RgbaF32(_)));
    }
}
