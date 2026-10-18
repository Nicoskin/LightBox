//! Image decoding. Formats are detected by content (magic bytes), not by
//! file extension, so a file with a wrong extension still opens.

mod alpha;
mod jpeg;
mod png;

use std::path::Path;

use crate::orientation::Orientation;

/// Largest texture side supported by D3D11 feature level 11.
pub const MAX_TEXTURE_SIDE: u32 = 16384;

/// File extensions listed when browsing a folder (lower case).
pub const EXTENSIONS: &[&str] = &["jpg", "jpeg", "jpe", "jfif", "png"];

/// `true` if the file name has one of the supported [`EXTENSIONS`].
pub fn is_supported(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("cannot read file: {0}")]
    Io(#[from] std::io::Error),
    #[error("unsupported or unrecognised image format")]
    Unsupported,
    #[error("JPEG: {0}")]
    Jpeg(#[from] turbojpeg::Error),
    #[error("PNG: {0}")]
    Png(String),
    #[error("image is too large ({0}×{1})")]
    TooLarge(u32, u32),
}

/// A decoded image ready for upload: tightly packed RGBA8 whose colour is
/// premultiplied in *linear* light and re-encoded as sRGB (see [`alpha`]).
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
    /// Size of the full-resolution source; differs from `width`/`height`
    /// when a reduced preview was decoded (JPEG DCT scaling).
    pub source_width: u32,
    pub source_height: u32,
    pub has_alpha: bool,
    /// How the pixels must be transformed for display (EXIF orientation).
    pub orientation: Orientation,
}

impl DecodedImage {
    /// `true` if this is the full-resolution image (not a reduced preview).
    /// Images larger than the texture limit count as full at the largest
    /// size that fits (tiling comes in M4).
    pub fn is_full(&self) -> bool {
        self.width == self.source_width || self.width.max(self.height) * 2 > MAX_TEXTURE_SIDE
    }

    /// Bytes of pixel memory, for cache accounting.
    pub fn bytes(&self) -> usize {
        self.pixels.len()
    }

    /// Full-resolution size in display orientation.
    pub fn display_size(&self) -> (u32, u32) {
        self.orientation
            .apply_size(self.source_width, self.source_height)
    }

    /// Builds an image from straight-alpha sRGB RGBA8, premultiplying if needed.
    fn from_straight_rgba(
        width: u32,
        height: u32,
        mut pixels: Vec<u8>,
        source: (u32, u32),
    ) -> Result<Self, DecodeError> {
        if width > MAX_TEXTURE_SIDE || height > MAX_TEXTURE_SIDE {
            // TODO(M4): tiling for very large images.
            return Err(DecodeError::TooLarge(source.0, source.1));
        }
        let has_alpha = alpha::premultiply_in_place(&mut pixels);
        Ok(Self {
            width,
            height,
            pixels,
            source_width: source.0,
            source_height: source.1,
            has_alpha,
            orientation: Orientation::default(),
        })
    }
}

/// The on-screen box the image will be fitted into. A decoder may return a
/// cheaper reduced image as long as fitting it into this box (in either
/// orientation, since EXIF rotation may swap the sides) never upscales.
#[derive(Clone, Copy, Debug)]
pub struct SizeHint {
    pub box_width: u32,
    pub box_height: u32,
}

impl SizeHint {
    /// Smallest scale factor (≤ 1) the source may be reduced by.
    pub fn min_scale(&self, src_w: u32, src_h: u32) -> f64 {
        let (w, h) = (f64::from(src_w), f64::from(src_h));
        let (bw, bh) = (f64::from(self.box_width), f64::from(self.box_height));
        let fit = crate::view::fit_scale;
        fit(w, h, bw, bh).max(fit(w, h, bh, bw))
    }
}

/// Decodes a file. With a `hint`, a cheaper reduced preview may be returned;
/// without one, the full resolution (capped at the texture limit).
pub fn decode_file(path: &Path, hint: Option<SizeHint>) -> Result<DecodedImage, DecodeError> {
    let data = std::fs::read(path)?;
    crate::bench::mark("file read");
    decode_bytes(&data, hint)
}

pub fn decode_bytes(data: &[u8], hint: Option<SizeHint>) -> Result<DecodedImage, DecodeError> {
    match sniff(data) {
        Format::Jpeg => {
            let mut img = jpeg::decode(data, hint)?;
            img.orientation = crate::metadata::jpeg_orientation(data);
            Ok(img)
        }
        Format::Png => png::decode(data),
        Format::Unknown => Err(DecodeError::Unsupported),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum Format {
    Jpeg,
    Png,
    Unknown,
}

fn sniff(data: &[u8]) -> Format {
    if data.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Format::Jpeg
    } else if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        Format::Png
    } else {
        Format::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sniff_formats() {
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0]), Format::Jpeg);
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n...."), Format::Png);
        assert_eq!(sniff(b"GIF89a"), Format::Unknown);
        assert_eq!(sniff(b""), Format::Unknown);
    }

    #[test]
    fn garbage_does_not_panic() {
        assert!(decode_bytes(&[0xFF, 0xD8, 0xFF, 0, 1, 2, 3], None).is_err());
        assert!(decode_bytes(b"\x89PNG\r\n\x1a\nbroken", None).is_err());
        assert!(decode_bytes(b"hello", None).is_err());
    }
}
