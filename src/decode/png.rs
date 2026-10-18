//! PNG via the `png` crate (fdeflate + SIMD unfiltering). Benchmarked ~2×
//! faster than zune-png on a 24 Mp image (see benches/decode.rs).
//! 16-bit images keep their precision (half-float textures); the embedded
//! ICC profile is applied.

use std::io::Cursor;

use png::{BitDepth, ColorType, Decoder, Limits, Transformations};

use super::{DecodeError, Raw, RawImage};

pub fn decode(data: &[u8]) -> Result<RawImage, DecodeError> {
    let mut dec = Decoder::new_with_limits(
        Cursor::new(data),
        Limits {
            bytes: 8 << 30, // the pixel-count check in `finish` is the real limit
        },
    );
    // Palette / low bit depths / tRNS → 8 bit; add alpha where missing;
    // 16-bit data stays 16-bit.
    dec.set_transformations(Transformations::EXPAND | Transformations::ALPHA);
    let mut reader = dec.read_info().map_err(png_err)?;
    let (w, h) = (reader.info().width, reader.info().height);
    let icc = reader.info().icc_profile.as_ref().map(|p| p.to_vec());
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| DecodeError::Png("image too large".into()))?;
    let mut buf = vec![0u8; size];
    let frame = reader.next_frame(&mut buf).map_err(png_err)?;
    buf.truncate(frame.buffer_size());

    let px = w as usize * h as usize;
    let data = match (frame.color_type, frame.bit_depth) {
        (ColorType::Rgba, BitDepth::Eight) => Raw::Rgba8(buf),
        (ColorType::GrayscaleAlpha, BitDepth::Eight) => Raw::Rgba8(
            buf.as_chunks::<2>()
                .0
                .iter()
                .flat_map(|&[l, a]| [l, l, l, a])
                .collect(),
        ),
        (ColorType::Rgba, BitDepth::Sixteen) => Raw::Rgba16(
            buf.as_chunks::<2>()
                .0
                .iter()
                .map(|&b| u16::from_be_bytes(b))
                .collect(),
        ),
        (ColorType::GrayscaleAlpha, BitDepth::Sixteen) => Raw::Rgba16(
            buf.as_chunks::<4>()
                .0
                .iter()
                .flat_map(|&[l0, l1, a0, a1]| {
                    let (l, a) = (u16::from_be_bytes([l0, l1]), u16::from_be_bytes([a0, a1]));
                    [l, l, l, a]
                })
                .collect(),
        ),
        (c, d) => {
            return Err(DecodeError::Png(format!(
                "unexpected output format {c:?} {d:?}"
            )));
        }
    };
    let n = match &data {
        Raw::Rgba8(v) => v.len(),
        Raw::Rgba16(v) => v.len(),
        Raw::RgbaF32(v) => v.len(),
    };
    if n != px * 4 {
        return Err(DecodeError::Png("unexpected buffer size".into()));
    }
    crate::bench::mark("decoded (png)");
    let mut img = RawImage::new(w, h, data);
    img.icc = icc;
    Ok(img)
}

fn png_err(e: png::DecodingError) -> DecodeError {
    DecodeError::Png(e.to_string())
}
