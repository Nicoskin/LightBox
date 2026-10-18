//! Image decoding. Formats are detected by content (magic bytes) first and
//! by extension only where a format has no signature (TGA) or shares one
//! (camera RAW files are TIFF-like), so a file with a wrong extension still
//! opens.
//!
//! Every decoder produces a [`Raw`] image (straight alpha) that [`finish`]
//! turns into a [`DecodedImage`]: colour-managed to sRGB, premultiplied in
//! linear light, stored as 8-bit sRGB or 16-bit float, plus a reduced
//! "overview" for images too large for a single GPU texture.

mod alpha;
pub use alpha::{f16_to_straight_srgb8, unpremultiply};
mod avif;
mod generic;
mod jpeg;
mod png;
mod psd;
mod raw;
pub mod scale;
pub mod svg;
mod wic;

use std::path::Path;

use crate::color;
use crate::orientation::Orientation;

/// Largest texture side supported by D3D11 feature level 11.
pub const MAX_TEXTURE_SIDE: u32 = 16384;
/// Longest side of the overview of a huge image (drawn when zoomed out).
pub const OVERVIEW_SIDE: u32 = 8192;
/// Refuse images above this many pixels (≈ 4 GB as RGBA8).
const MAX_PIXELS: u64 = 1_000_000_000;

/// File extensions listed when browsing a folder (lower case).
pub const EXTENSIONS: &[&str] = &[
    "jpg", "jpeg", "jpe", "jfif", "png", "apng", "gif", "webp", "bmp", "dib", "ico", "cur", "tif",
    "tiff", "tga", "qoi", "avif", "heic", "heif", "hif", "svg", "svgz", "psd", "hdr", "exr", "pbm",
    "pgm", "ppm", "pnm", "pam", "dds", "jxr", "wdp", "dng", "cr2", "cr3", "nef", "nrw", "arw",
    "srf", "sr2", "orf", "rw2", "raf", "pef", "srw", "x3f", "3fr", "iiq", "erf", "mrw", "kdc",
    "dcr",
];

/// Camera RAW extensions (TIFF-like containers with embedded JPEG previews).
const RAW_EXTENSIONS: &[&str] = &[
    "dng", "cr2", "cr3", "nef", "nrw", "arw", "srf", "sr2", "orf", "rw2", "raf", "pef", "srw",
    "x3f", "3fr", "iiq", "erf", "mrw", "kdc", "dcr",
];

fn ext_of(path: &Path) -> String {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .unwrap_or_default()
}

/// `true` if the file name has one of the supported [`EXTENSIONS`].
pub fn is_supported(path: &Path) -> bool {
    EXTENSIONS.contains(&ext_of(path).as_str())
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("не удалось прочитать файл: {0}")]
    Io(#[from] std::io::Error),
    #[error("формат не поддерживается или файл повреждён")]
    Unsupported,
    #[error("JPEG: {0}")]
    Jpeg(#[from] turbojpeg::Error),
    #[error("PNG: {0}")]
    Png(String),
    #[error("{0}")]
    Other(String),
    #[error("изображение слишком большое ({0}×{1})")]
    TooLarge(u32, u32),
}

impl DecodeError {
    fn other(e: impl std::fmt::Display) -> Self {
        Self::Other(e.to_string())
    }
}

/// Decoded pixels, premultiplied in linear light.
pub enum Pixels {
    /// RGBA8, sRGB-encoded (see [`alpha`]): the common case.
    Srgb8(Vec<u8>),
    /// RGBA half floats, linear: 16-bit and HDR sources.
    Linear16F(Vec<u16>),
}

impl Pixels {
    pub fn bytes(&self) -> usize {
        match self {
            Pixels::Srgb8(v) => v.len(),
            Pixels::Linear16F(v) => v.len() * 2,
        }
    }
}

/// A decoded image ready for upload.
pub struct DecodedImage {
    pub width: u32,
    pub height: u32,
    pub pixels: Pixels,
    /// Size of the full-resolution source; differs from `width`/`height`
    /// when a reduced preview was decoded (JPEG DCT scaling, SVG).
    pub source_width: u32,
    pub source_height: u32,
    pub has_alpha: bool,
    /// How the pixels must be transformed for display (EXIF orientation).
    pub orientation: Orientation,
    /// Reduced copy for images larger than one texture (drawn when the
    /// image is shown smaller than its overview's resolution).
    pub overview: Option<Box<DecodedImage>>,
    /// A tiny stand-in (the EXIF thumbnail) shown until the preview decodes.
    pub thumbnail: bool,
    pub info: SourceInfo,
}

/// What the source file holds (inspector).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct SourceInfo {
    /// "JPEG", "PNG", "WebP"…
    pub format: String,
    /// Bits per channel as decoded (8, 16 or 32).
    pub bits: u8,
    /// Floating-point samples (HDR / EXR).
    pub float: bool,
    /// Embedded colour profile, by name.
    pub icc: Option<String>,
}

impl DecodedImage {
    /// `true` if this is the full-resolution image (not a reduced preview).
    pub fn is_full(&self) -> bool {
        self.width >= self.source_width && self.height >= self.source_height
    }

    /// Bytes of pixel memory, for cache accounting.
    pub fn bytes(&self) -> usize {
        self.pixels.bytes() + self.overview.as_ref().map_or(0, |o| o.bytes())
    }

    /// Full-resolution size in display orientation.
    pub fn display_size(&self) -> (u32, u32) {
        self.orientation
            .apply_size(self.source_width, self.source_height)
    }

    /// One pixel as straight-alpha sRGB RGBA8 (colour picker). `x`, `y` in
    /// stored pixels of this image.
    pub fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        if x >= self.width || y >= self.height {
            return None;
        }
        let i = (y as usize * self.width as usize + x as usize) * 4;
        let v = match &self.pixels {
            Pixels::Srgb8(v) => alpha::unpremultiply(v.get(i..i + 4)?),
            Pixels::Linear16F(v) => alpha::f16_to_straight_srgb8(v.get(i..i + 4)?),
        };
        v.try_into().ok()
    }

    /// Straight-alpha sRGB RGBA8 copy (for export and thumbnails).
    pub fn to_straight_rgba8(&self) -> Vec<u8> {
        match &self.pixels {
            Pixels::Srgb8(v) => alpha::unpremultiply(v),
            Pixels::Linear16F(v) => alpha::f16_to_straight_srgb8(v),
        }
    }
}

/// What a format decoder returns: straight alpha, before colour management.
pub enum Raw {
    /// RGBA8, sRGB (or the embedded ICC profile's space).
    Rgba8(Vec<u8>),
    /// RGBA16, gamma-encoded like 8-bit data.
    Rgba16(Vec<u16>),
    /// RGBA32F, linear light, possibly above 1.0 (HDR / EXR): tone-mapped.
    RgbaF32(Vec<f32>),
}

pub struct RawImage {
    pub width: u32,
    pub height: u32,
    pub data: Raw,
    /// Full-resolution size (when `data` is reduced).
    pub source: (u32, u32),
    pub orientation: Orientation,
    pub icc: Option<Vec<u8>>,
}

impl RawImage {
    pub fn new(width: u32, height: u32, data: Raw) -> Self {
        Self {
            width,
            height,
            data,
            source: (width, height),
            orientation: Orientation::IDENTITY,
            icc: None,
        }
    }
}

/// Colour management, premultiplication, storage format, overview.
fn finish(mut img: RawImage, premultiply: bool) -> Result<DecodedImage, DecodeError> {
    let (w, h) = (img.width, img.height);
    if u64::from(w) * u64::from(h) > MAX_PIXELS {
        return Err(DecodeError::TooLarge(w, h));
    }
    let info = SourceInfo {
        format: String::new(),
        bits: match &img.data {
            Raw::Rgba8(_) => 8,
            Raw::Rgba16(_) => 16,
            Raw::RgbaF32(_) => 32,
        },
        float: matches!(img.data, Raw::RgbaF32(_)),
        icc: img.icc.as_deref().and_then(color::profile_name),
    };
    if let Some(icc) = &img.icc {
        color::to_srgb(icc, &mut img.data);
    }
    let (pixels, has_alpha) = match img.data {
        Raw::Rgba8(mut v) => {
            let a = if premultiply {
                alpha::premultiply_in_place(&mut v)
            } else {
                v.as_chunks::<4>().0.iter().any(|p| p[3] != 255)
            };
            (Pixels::Srgb8(v), a)
        }
        Raw::Rgba16(v) if premultiply => alpha::rgba16_to_linear_f16(&v),
        Raw::Rgba16(v) => {
            let v8: Vec<u8> = v
                .iter()
                .map(|&x| ((u32::from(x) + 128) / 257) as u8)
                .collect();
            let a = v8.as_chunks::<4>().0.iter().any(|p| p[3] != 255);
            (Pixels::Srgb8(v8), a)
        }
        Raw::RgbaF32(v) if premultiply => alpha::hdr_to_linear_f16(&v),
        Raw::RgbaF32(v) => (Pixels::Srgb8(alpha::hdr_to_srgb8(&v)), false),
    };
    let mut out = DecodedImage {
        width: w,
        height: h,
        pixels,
        source_width: img.source.0,
        source_height: img.source.1,
        has_alpha,
        orientation: img.orientation,
        overview: None,
        thumbnail: false,
        info,
    };
    if premultiply && w.max(h) > MAX_TEXTURE_SIDE {
        out.overview = Some(Box::new(scale::overview(&out, OVERVIEW_SIDE)));
    }
    Ok(out)
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
/// without one, the full resolution.
pub fn decode_file(path: &Path, hint: Option<SizeHint>) -> Result<DecodedImage, DecodeError> {
    let data = std::fs::read(path)?;
    crate::bench::mark("file read");
    decode_data(&data, path, hint)
}

/// [`decode_file`] for bytes already read from `path`.
pub fn decode_data(
    data: &[u8],
    path: &Path,
    hint: Option<SizeHint>,
) -> Result<DecodedImage, DecodeError> {
    let ext = ext_of(path);
    let mut img = finish(decode_raw(data, &ext, hint, Some(path))?, true)?;
    img.info.format = format_name(sniff(data, &ext));
    Ok(img)
}

/// Instant low-resolution stand-in: the EXIF thumbnail of a JPEG, shown at
/// the photo's size (blurry for a moment, but on screen immediately).
/// Letterboxed thumbnails (other aspect ratio) are not used.
pub fn decode_thumbnail_bytes(data: &[u8]) -> Option<DecodedImage> {
    if sniff(data, "") != Format::Jpeg {
        return None;
    }
    let main = turbojpeg::read_header(data).ok()?;
    let thumb = crate::metadata::exif_thumbnail(data)?;
    let mut raw = jpeg::decode(&thumb, None).ok()?;
    let aspect = |w: f64, h: f64| w / h;
    let (a, b) = (
        aspect(main.width as f64, main.height as f64),
        aspect(f64::from(raw.width), f64::from(raw.height)),
    );
    if (a - b).abs() > a * 0.03 {
        return None;
    }
    raw.source = (main.width as u32, main.height as u32);
    raw.orientation = crate::metadata::jpeg_orientation(data);
    raw.icc = None;
    let mut img = finish(raw, true).ok()?;
    img.thumbnail = true;
    crate::bench::mark("decoded (exif thumbnail)");
    Some(img)
}

/// [`decode_thumbnail_bytes`] for a file.
pub fn decode_thumbnail(path: &Path) -> Result<DecodedImage, DecodeError> {
    let data = std::fs::read(path)?;
    decode_thumbnail_bytes(&data).ok_or(DecodeError::Unsupported)
}

/// Full-resolution straight-alpha sRGB RGBA8 for re-encoding ("Save as");
/// `orientation` tells how it must be turned for display.
pub fn decode_for_export(data: &[u8], path: &Path) -> Result<DecodedImage, DecodeError> {
    finish(decode_raw(data, &ext_of(path), None, Some(path))?, false)
}

/// Decodes bytes whose file name is unknown (tests, embedded images).
#[cfg(test)]
pub fn decode_bytes(data: &[u8], hint: Option<SizeHint>) -> Result<DecodedImage, DecodeError> {
    finish(decode_raw(data, "", hint, None)?, true)
}

fn decode_raw(
    data: &[u8],
    ext: &str,
    hint: Option<SizeHint>,
    path: Option<&Path>,
) -> Result<RawImage, DecodeError> {
    let format = sniff(data, ext);
    let result = match format {
        Format::Jpeg => jpeg::decode(data, hint),
        Format::Png => png::decode(data),
        Format::Avif => avif::decode(data),
        Format::Svg => svg::decode(data, hint),
        Format::Psd => psd::decode(data),
        Format::Raw => raw::decode(data, hint),
        Format::Image(f) => generic::decode(data, f),
        Format::Heic | Format::Unknown => Err(DecodeError::Unsupported),
    };
    // Windows Imaging Component as the universal fallback: HEIC (with the
    // HEVC extension), JPEG XR, camera codecs, anything else installed.
    match (result, path) {
        (Ok(img), _) => Ok(img),
        (Err(e), Some(p)) => wic::decode(p).map_err(|_| e),
        (Err(e), None) => Err(e),
    }
}

/// Display name of a sniffed format.
fn format_name(f: Format) -> String {
    match f {
        Format::Jpeg => "JPEG".into(),
        Format::Png => "PNG".into(),
        Format::Avif => "AVIF".into(),
        Format::Heic => "HEIC".into(),
        Format::Svg => "SVG".into(),
        Format::Psd => "PSD".into(),
        Format::Raw => "RAW (встроенное превью)".into(),
        Format::Image(i) => i
            .extensions_str()
            .first()
            .map_or_else(|| format!("{i:?}"), |e| e.to_uppercase()),
        Format::Unknown => "WIC".into(),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Format {
    Jpeg,
    Png,
    Avif,
    Heic,
    Svg,
    Psd,
    Raw,
    /// Decoded by the `image` crate.
    Image(image::ImageFormat),
    Unknown,
}

fn sniff(data: &[u8], ext: &str) -> Format {
    use image::ImageFormat as F;
    let starts = |m: &[u8]| data.starts_with(m);
    let is_raw_ext = RAW_EXTENSIONS.contains(&ext);
    if starts(&[0xFF, 0xD8, 0xFF]) {
        return Format::Jpeg;
    }
    if starts(b"\x89PNG\r\n\x1a\n") {
        return Format::Png;
    }
    if starts(b"GIF87a") || starts(b"GIF89a") {
        return Format::Image(F::Gif);
    }
    if starts(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        return Format::Image(F::WebP);
    }
    if starts(b"II*\0") || starts(b"MM\0*") || starts(b"IIRO") || starts(b"IIU\0") {
        return if is_raw_ext {
            Format::Raw
        } else {
            Format::Image(F::Tiff)
        };
    }
    if starts(b"FUJIFILMCCD-RAW") {
        return Format::Raw;
    }
    if data.get(4..8) == Some(b"ftyp") {
        let brands = &data[8..data.len().min(64)];
        let has = |b: &[u8]| brands.windows(4).any(|w| w == b);
        if has(b"crx ") {
            return Format::Raw;
        }
        if has(b"avif") || has(b"avis") {
            return Format::Avif;
        }
        if has(b"heic")
            || has(b"heix")
            || has(b"hevc")
            || has(b"heim")
            || has(b"heis")
            || has(b"mif1")
            || has(b"msf1")
        {
            return Format::Heic;
        }
    }
    if starts(b"BM") {
        return Format::Image(F::Bmp);
    }
    // TGA has no signature and its common header looks like a cursor.
    if ext == "tga" {
        return Format::Image(F::Tga);
    }
    if starts(&[0, 0, 1, 0]) || starts(&[0, 0, 2, 0]) {
        return Format::Image(F::Ico);
    }
    if starts(b"qoif") {
        return Format::Image(F::Qoi);
    }
    if starts(b"8BPS") {
        return Format::Psd;
    }
    if starts(b"#?RADIANCE") || starts(b"#?RGBE") {
        return Format::Image(F::Hdr);
    }
    if starts(&[0x76, 0x2F, 0x31, 0x01]) {
        return Format::Image(F::OpenExr);
    }
    if starts(b"DDS ") {
        return Format::Image(F::Dds);
    }
    if data.len() >= 2 && data[0] == b'P' && (b'1'..=b'7').contains(&data[1]) {
        return Format::Image(F::Pnm);
    }
    if ext == "svgz" && starts(&[0x1F, 0x8B]) || ext == "svg" || looks_like_svg(data) {
        return Format::Svg;
    }
    if is_raw_ext {
        return Format::Raw;
    }
    match ext {
        "tga" => Format::Image(F::Tga),
        "heic" | "heif" | "hif" => Format::Heic,
        _ => Format::Unknown,
    }
}

fn looks_like_svg(data: &[u8]) -> bool {
    // Editors write long prologues (comments, DOCTYPE, metadata).
    let head = &data[..data.len().min(8192)];
    let text = String::from_utf8_lossy(head);
    let t = text.trim_start_matches('\u{feff}').trim_start();
    t.starts_with('<') && text.contains("<svg")
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::ImageFormat as F;

    #[test]
    fn sniff_formats() {
        assert_eq!(sniff(&[0xFF, 0xD8, 0xFF, 0xE0], ""), Format::Jpeg);
        assert_eq!(sniff(b"\x89PNG\r\n\x1a\n....", ""), Format::Png);
        assert_eq!(sniff(b"GIF89a", ""), Format::Image(F::Gif));
        assert_eq!(sniff(b"RIFF\0\0\0\0WEBPVP8 ", ""), Format::Image(F::WebP));
        assert_eq!(sniff(b"II*\0", "tif"), Format::Image(F::Tiff));
        assert_eq!(sniff(b"II*\0", "nef"), Format::Raw);
        assert_eq!(sniff(b"\0\0\0\x1cftypavif\0\0\0\0", ""), Format::Avif);
        assert_eq!(sniff(b"\0\0\0\x18ftypheic\0\0\0\0", ""), Format::Heic);
        assert_eq!(sniff(b"<?xml version=\"1.0\"?><svg>", ""), Format::Svg);
        assert_eq!(sniff(b"8BPS", ""), Format::Psd);
        assert_eq!(sniff(b"\0\0\x02\0", "tga"), Format::Image(F::Tga));
        assert_eq!(sniff(b"hello", ""), Format::Unknown);
        assert_eq!(sniff(b"", ""), Format::Unknown);
    }

    #[test]
    fn garbage_does_not_panic() {
        for data in [
            &[0xFF, 0xD8, 0xFF, 0, 1, 2, 3][..],
            b"\x89PNG\r\n\x1a\nbroken",
            b"GIF89a\x01\x00",
            b"RIFF\x10\0\0\0WEBPVP8 \0\0",
            b"II*\0\x08\0\0\0",
            b"\0\0\0\x1cftypavif\0\0\0\0mif1avif",
            b"8BPS\0\x01",
            b"<svg",
            b"hello",
        ] {
            assert!(decode_bytes(data, None).is_err());
        }
    }

    fn testdata(name: &str) -> Option<std::path::PathBuf> {
        let p = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("testdata")
            .join(name);
        p.exists().then_some(p)
    }

    const SCREEN: SizeHint = SizeHint {
        box_width: 1920,
        box_height: 1080,
    };

    /// Every generated sample decodes (run `gen_testdata` first; skipped
    /// when the folder is missing).
    #[test]
    fn all_testdata_decodes() {
        let Some(dir) = testdata("") else {
            return;
        };
        let mut n = 0;
        for entry in std::fs::read_dir(dir).expect("testdata") {
            let path = entry.expect("entry").path();
            let name = path.file_name().unwrap_or_default().to_string_lossy();
            if name.starts_with("broken_") || name.starts_with("huge") || name.ends_with(".md") {
                continue;
            }
            let img = decode_file(&path, Some(SCREEN)).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert!(img.width > 0 && img.height > 0, "{name}");
            assert_eq!(
                img.bytes(),
                img.width as usize
                    * img.height as usize
                    * match img.pixels {
                        Pixels::Srgb8(_) => 4,
                        Pixels::Linear16F(_) => 8,
                    },
                "{name}"
            );
            n += 1;
        }
        assert!(n >= 25, "only {n} samples");
        // A truncated JPEG shows what arrived (like other viewers); garbage
        // is an error. Neither may panic.
        if let Some(p) = testdata("broken_truncated.jpg") {
            let _ = decode_file(&p, None);
        }
        if let Some(p) = testdata("broken_garbage.png") {
            assert!(decode_file(&p, None).is_err());
        }
    }

    fn pixel(img: &DecodedImage, x: u32, y: u32) -> [u8; 4] {
        let Pixels::Srgb8(v) = &img.pixels else {
            panic!("not 8-bit");
        };
        let i = (y * img.width + x) as usize * 4;
        [v[i], v[i + 1], v[i + 2], v[i + 3]]
    }

    #[test]
    fn colour_management_and_depth() {
        let Some(p) = testdata("adobe_rgb.jpg") else {
            return;
        };
        // Stored (30, 220, 40) in a wider gamut: outside sRGB, so red and
        // blue fall to (almost) zero instead of staying at 30 / 40.
        let px = pixel(&decode_file(&p, None).expect("adobe"), 160, 100);
        assert!(px[0] < 10 && px[2] < 10 && px[1] > 200, "{px:?}");

        let cmyk = decode_file(&testdata("cmyk.jpg").expect("cmyk"), None).expect("cmyk");
        assert_eq!((cmyk.width, cmyk.height), (320, 200));
        // Little cyan, much yellow on the left: yellow-green, not dark.
        let px = pixel(&cmyk, 10, 100);
        assert!(px[0] > 180 && px[1] > 150 && px[2] < 100, "{px:?}");

        for name in ["gradient_16bit.png", "gradient_16bit.tif", "sample.exr"] {
            let img = decode_file(&testdata(name).expect(name), None).expect(name);
            assert!(matches!(img.pixels, Pixels::Linear16F(_)), "{name}");
        }
        let gif = decode_file(&testdata("animated.gif").expect("gif"), None).expect("gif");
        assert_eq!((gif.width, gif.height), (200, 120));
    }

    #[test]
    fn exif_thumbnail_first() {
        let Some(p) = testdata("photo_24mp_exifthumb.jpg") else {
            return;
        };
        let t = decode_thumbnail(&p).expect("thumbnail");
        assert!(t.thumbnail && !t.is_full());
        assert_eq!((t.width, t.height), (240, 160));
        assert_eq!((t.source_width, t.source_height), (6000, 4000));
        // Files without one report it as unsupported (the preview follows).
        let plain = testdata("photo_24mp.jpg").expect("plain");
        assert!(decode_thumbnail(&plain).is_err());
    }

    /// 30 000 × 20 000: decoded at full size with an overview for zoom-out.
    /// Slow and needs ~3 GB of RAM: `cargo test --release -- --ignored`.
    #[test]
    #[ignore = "needs the 544 MB huge sample and ~3 GB RAM"]
    fn huge_image_has_overview() {
        let Some(p) = testdata("huge_30000x20000.png") else {
            return;
        };
        let img = decode_file(&p, None).expect("huge");
        assert_eq!((img.width, img.height), (30_000, 20_000));
        let ov = img.overview.as_ref().expect("overview");
        assert_eq!((ov.width, ov.height), (OVERVIEW_SIDE, 5461));
    }
}
