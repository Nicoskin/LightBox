//! PNG via the `png` crate (fdeflate + SIMD unfiltering). Benchmarked ~2×
//! faster than zune-png on a 24 Mp image (see benches/decode.rs).

use std::io::Cursor;

use png::{ColorType, Decoder, Limits, Transformations};

use super::{DecodeError, DecodedImage, MAX_TEXTURE_SIDE};

pub fn decode(data: &[u8]) -> Result<DecodedImage, DecodeError> {
    // TODO(M4): keep 16-bit precision (R16G16B16A16_FLOAT), ICC/gAMA, APNG.
    let mut dec = Decoder::new_with_limits(
        Cursor::new(data),
        Limits {
            bytes: 4 << 30, // the texture-size check below is the real limit
        },
    );
    // Palette/low-bit-depth → 8 bit, 16 bit → 8 bit, add alpha where missing.
    dec.set_transformations(Transformations::normalize_to_color8() | Transformations::ALPHA);
    let mut reader = dec.read_info().map_err(png_err)?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w > MAX_TEXTURE_SIDE || h > MAX_TEXTURE_SIDE {
        return Err(DecodeError::TooLarge(w, h));
    }
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| DecodeError::Png("image too large".into()))?;
    let mut buf = vec![0u8; size];
    let frame = reader.next_frame(&mut buf).map_err(png_err)?;
    buf.truncate(frame.buffer_size());

    let rgba = match frame.color_type {
        ColorType::Rgba => buf,
        ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[l, a]| [l, l, l, a])
            .collect(),
        other => {
            return Err(DecodeError::Png(format!(
                "unexpected output colour type {other:?}"
            )));
        }
    };
    if rgba.len() != (w as usize) * (h as usize) * 4 {
        return Err(DecodeError::Png("unexpected buffer size".into()));
    }
    crate::bench::mark("decoded (png)");
    DecodedImage::from_straight_rgba(w, h, rgba, (w, h))
}

fn png_err(e: png::DecodingError) -> DecodeError {
    DecodeError::Png(e.to_string())
}
