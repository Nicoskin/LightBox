//! Lossless PNG rotation: the samples are moved at their original bit depth
//! (palette indices, 1/2/4/8/16-bit), and every other chunk — text
//! (ComfyUI / A1111 prompts), iCCP, eXIf, pHYs… — is carried over.

use std::io::Cursor;

use png::{ColorType, Decoder, Limits, Transformations};

use super::{EncodeError, orient_pixels, tiff};
use crate::orientation::Orientation;

const SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";

struct Chunk<'a> {
    kind: [u8; 4],
    data: &'a [u8],
}

fn chunks(data: &[u8]) -> Result<Vec<Chunk<'_>>, EncodeError> {
    let bad = || EncodeError::Format("повреждённый PNG".into());
    if !data.starts_with(SIGNATURE) {
        return Err(bad());
    }
    let mut out = Vec::new();
    let mut pos = 8;
    while pos + 12 <= data.len() {
        let len = u32::from_be_bytes(data[pos..pos + 4].try_into().map_err(|_| bad())?) as usize;
        let kind: [u8; 4] = data[pos + 4..pos + 8].try_into().map_err(|_| bad())?;
        let body = data.get(pos + 8..pos + 8 + len).ok_or_else(bad)?;
        out.push(Chunk { kind, data: body });
        pos += 12 + len;
        if &kind == b"IEND" {
            break;
        }
    }
    Ok(out)
}

fn write_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut h = crc32fast::Hasher::new();
    h.update(kind);
    h.update(data);
    out.extend_from_slice(&h.finalize().to_be_bytes());
}

fn samples_per_pixel(c: ColorType) -> usize {
    match c {
        ColorType::Grayscale | ColorType::Indexed => 1,
        ColorType::GrayscaleAlpha => 2,
        ColorType::Rgb => 3,
        ColorType::Rgba => 4,
    }
}

/// Rotates / mirrors a PNG so that it looks like `display` with no
/// orientation metadata needed.
pub fn rotate(data: &[u8], display: Orientation) -> Result<Vec<u8>, EncodeError> {
    let list = chunks(data)?;
    if list.iter().any(|c| &c.kind == b"acTL") {
        return Err(EncodeError::Format(
            "поворот анимированных PNG пока не поддерживается".into(),
        ));
    }

    // Raw samples, no transformations.
    let mut dec = Decoder::new_with_limits(Cursor::new(data), Limits { bytes: 4 << 30 });
    dec.set_transformations(Transformations::IDENTITY);
    let mut reader = dec
        .read_info()
        .map_err(|e| EncodeError::Format(e.to_string()))?;
    let info = reader.info();
    let (w, h) = (info.width as usize, info.height as usize);
    let (color, depth) = (info.color_type, info.bit_depth);
    let bits = depth as usize;
    let mut raw = vec![0u8; reader.output_buffer_size().ok_or(EncodeError::TooLarge)?];
    let frame = reader
        .next_frame(&mut raw)
        .map_err(|e| EncodeError::Format(e.to_string()))?;
    let line = frame.line_size;

    // One byte per sample for sub-byte depths, otherwise whole pixels.
    let spp = samples_per_pixel(color);
    let (unpacked, bpp) = if bits < 8 {
        let mut v = Vec::with_capacity(w * h);
        for row in raw.chunks(line).take(h) {
            for x in 0..w {
                let bit = x * bits;
                let byte = row[bit / 8];
                let shift = 8 - bits - (bit % 8);
                v.push((byte >> shift) & ((1u8 << bits) - 1));
            }
        }
        (v, 1)
    } else {
        let bpp = spp * bits / 8;
        let mut v = Vec::with_capacity(w * h * bpp);
        for row in raw.chunks(line).take(h) {
            v.extend_from_slice(&row[..w * bpp]);
        }
        (v, bpp)
    };
    let (rot, nw, nh) = orient_pixels(&unpacked, w, h, bpp, display);

    // Re-pack rows (sub-byte depths) and filter them.
    let row_bytes = if bits < 8 {
        (nw * bits).div_ceil(8)
    } else {
        nw * bpp
    };
    let filter_bpp = ((spp * bits).div_ceil(8)).max(1);
    let mut filtered = Vec::with_capacity((row_bytes + 1) * nh);
    let mut prev = vec![0u8; row_bytes];
    let mut row = vec![0u8; row_bytes];
    for y in 0..nh {
        if bits < 8 {
            row.iter_mut().for_each(|b| *b = 0);
            for x in 0..nw {
                let bit = x * bits;
                row[bit / 8] |= rot[y * nw + x] << (8 - bits - (bit % 8));
            }
        } else {
            row.copy_from_slice(&rot[y * row_bytes..(y + 1) * row_bytes]);
        }
        filter_row(&row, &prev, filter_bpp, &mut filtered);
        std::mem::swap(&mut prev, &mut row);
    }
    let idat = miniz_oxide::deflate::compress_to_vec_zlib(&filtered, 8);

    // Assemble: IHDR, chunks before the image data, IDAT, the rest, IEND.
    let mut out = Vec::with_capacity(idat.len() + 4096);
    out.extend_from_slice(SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&(nw as u32).to_be_bytes());
    ihdr.extend_from_slice(&(nh as u32).to_be_bytes());
    ihdr.push(depth as u8);
    ihdr.push(color as u8);
    ihdr.extend_from_slice(&[0, 0, 0]); // deflate, adaptive filtering, no interlace
    write_chunk(&mut out, b"IHDR", &ihdr);
    let mut idat_written = false;
    for c in &list {
        match &c.kind {
            b"IHDR" | b"IEND" => {}
            b"IDAT" => {
                if !idat_written {
                    write_chunk(&mut out, b"IDAT", &idat);
                    idat_written = true;
                }
            }
            b"eXIf" => {
                let mut t = c.data.to_vec();
                tiff::set_orientation(&mut t, 1);
                write_chunk(&mut out, b"eXIf", &t);
            }
            b"pHYs" if display.swaps_axes() && c.data.len() == 9 => {
                let mut p = c.data.to_vec();
                p.swap(0, 4);
                p.swap(1, 5);
                p.swap(2, 6);
                p.swap(3, 7);
                write_chunk(&mut out, b"pHYs", &p);
            }
            // Unknown chunks marked "unsafe to copy" (4th letter upper
            // case) depend on the image data: the spec says drop them.
            k if !is_known(k) && k[3].is_ascii_uppercase() => {}
            k => write_chunk(&mut out, k, c.data),
        }
    }
    write_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

fn is_known(k: &[u8; 4]) -> bool {
    matches!(
        k,
        b"PLTE"
            | b"tRNS"
            | b"gAMA"
            | b"cHRM"
            | b"sRGB"
            | b"iCCP"
            | b"sBIT"
            | b"bKGD"
            | b"hIST"
            | b"pHYs"
            | b"sPLT"
            | b"tIME"
            | b"tEXt"
            | b"zTXt"
            | b"iTXt"
            | b"eXIf"
            | b"cICP"
    )
}

/// Appends one filtered row: the PNG filter (0–4) with the smallest sum of
/// absolute residuals (the standard heuristic).
fn filter_row(row: &[u8], prev: &[u8], bpp: usize, out: &mut Vec<u8>) {
    let n = row.len();
    let left = |i: usize| if i >= bpp { row[i - bpp] } else { 0 };
    let up_left = |i: usize| if i >= bpp { prev[i - bpp] } else { 0 };
    let paeth = |a: u8, b: u8, c: u8| {
        let p = i16::from(a) + i16::from(b) - i16::from(c);
        let (pa, pb, pc) = (
            (p - i16::from(a)).abs(),
            (p - i16::from(b)).abs(),
            (p - i16::from(c)).abs(),
        );
        if pa <= pb && pa <= pc {
            a
        } else if pb <= pc {
            b
        } else {
            c
        }
    };
    let predict = |f: u8, i: usize| -> u8 {
        match f {
            1 => left(i),
            2 => prev[i],
            3 => ((u16::from(left(i)) + u16::from(prev[i])) / 2) as u8,
            4 => paeth(left(i), prev[i], up_left(i)),
            _ => 0,
        }
    };
    let cost = |f: u8| -> u64 {
        (0..n)
            .map(|i| u64::from((row[i].wrapping_sub(predict(f, i)) as i8).unsigned_abs()))
            .sum()
    };
    let best = (0u8..5).min_by_key(|&f| cost(f)).unwrap_or(0);
    out.push(best);
    out.extend((0..n).map(|i| row[i].wrapping_sub(predict(best, i))));
}

#[cfg(test)]
mod tests {
    use super::*;
    use png::BitDepth;

    fn encode(
        w: u32,
        h: u32,
        color: ColorType,
        depth: BitDepth,
        data: &[u8],
        palette: Option<&[u8]>,
    ) -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, w, h);
            e.set_color(color);
            e.set_depth(depth);
            if let Some(p) = palette {
                e.set_palette(p.to_vec());
            }
            e.add_text_chunk("parameters".into(), "a cat, masterpiece".into())
                .unwrap();
            let mut wr = e.write_header().unwrap();
            wr.write_image_data(data).unwrap();
        }
        out
    }

    fn decode_raw(data: &[u8]) -> (u32, u32, Vec<u8>, Vec<String>) {
        let mut d = Decoder::new(Cursor::new(data));
        d.set_transformations(Transformations::IDENTITY);
        let mut r = d.read_info().unwrap();
        let mut buf = vec![0; r.output_buffer_size().unwrap()];
        let f = r.next_frame(&mut buf).unwrap();
        buf.truncate(f.buffer_size());
        let texts = r
            .info()
            .uncompressed_latin1_text
            .iter()
            .map(|t| format!("{}={}", t.keyword, t.text))
            .collect();
        (f.width, f.height, buf, texts)
    }

    #[test]
    fn rgb8_rotates_and_keeps_text() {
        // 3×2: pixels numbered 0..5 in the red channel.
        let px: Vec<u8> = (0..6u8).flat_map(|i| [i, 0, 0]).collect();
        let src = encode(3, 2, ColorType::Rgb, BitDepth::Eight, &px, None);
        let out = rotate(&src, Orientation::ROTATE_CW).unwrap();
        let (w, h, data, texts) = decode_raw(&out);
        assert_eq!((w, h), (2, 3));
        let reds: Vec<u8> = data.chunks(3).map(|p| p[0]).collect();
        // Stored rows [0 1 2 / 3 4 5] turned clockwise: [3 0 / 4 1 / 5 2].
        assert_eq!(reds, [3, 0, 4, 1, 5, 2]);
        assert_eq!(texts, ["parameters=a cat, masterpiece"]);
    }

    #[test]
    fn sixteen_bit_and_back_is_identical() {
        let px: Vec<u8> = (0..(5 * 4 * 8)).map(|i| (i * 7 % 251) as u8).collect();
        let src = encode(5, 4, ColorType::Rgba, BitDepth::Sixteen, &px, None);
        let cw = rotate(&src, Orientation::ROTATE_CW).unwrap();
        let back = rotate(&cw, Orientation::ROTATE_CCW).unwrap();
        assert_eq!(decode_raw(&back).2, decode_raw(&src).2);
    }

    #[test]
    fn one_bit_palette_flips() {
        // 10×1, 1-bit palette: bits 1100000001 → mirrored 1000000011.
        let src = encode(
            10,
            1,
            ColorType::Indexed,
            BitDepth::One,
            &[0b1100_0000, 0b0100_0000],
            Some(&[0, 0, 0, 255, 255, 255]),
        );
        let out = rotate(&src, Orientation::FLIP_H).unwrap();
        let (w, h, data, _) = decode_raw(&out);
        assert_eq!((w, h), (10, 1));
        assert_eq!(data, [0b1000_0000, 0b1100_0000]);
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(rotate(b"not a png", Orientation::ROTATE_CW).is_err());
    }
}
