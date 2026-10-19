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
#[cfg(test)]
pub fn rotate(data: &[u8], display: Orientation) -> Result<Vec<u8>, EncodeError> {
    transform(data, display, None)
}

/// [`rotate`], then keeps only `crop` (x, y, width, height in the turned
/// image), losslessly.
pub fn transform(
    data: &[u8],
    display: Orientation,
    crop: Option<[u32; 4]>,
) -> Result<Vec<u8>, EncodeError> {
    let list = chunks(data)?;
    if list.iter().any(|c| &c.kind == b"acTL") {
        if crop.is_some() {
            return Err(EncodeError::Unsupported(
                "обрезка анимированных PNG не поддерживается",
            ));
        }
        return rotate_apng(&list, display);
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

    let spp = samples_per_pixel(color);
    let (unpacked, bpp) = unpack(&raw, line, w, h, bits, spp);
    let (rot, nw, nh) = orient_pixels(&unpacked, w, h, bpp, display);
    let (rot, nw, nh) = match crop {
        Some(c) => crop_units(&rot, nw, nh, bpp, c)?,
        None => (rot, nw, nh),
    };
    let idat = pack(&rot, nw, nh, bits, spp, bpp);

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

/// Samples of unfiltered rows (`line` bytes each): one byte per sample for
/// sub-byte depths, otherwise whole pixels. Returns them and the bytes per
/// unit.
fn unpack(
    raw: &[u8],
    line: usize,
    w: usize,
    h: usize,
    bits: usize,
    spp: usize,
) -> (Vec<u8>, usize) {
    if bits < 8 {
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
    }
}

/// The `c` = (x, y, width, height) part of a `w` × `h` grid of `bpp`-byte
/// units, clamped to the grid.
pub(super) fn crop_units(
    px: &[u8],
    w: usize,
    h: usize,
    bpp: usize,
    c: [u32; 4],
) -> Result<(Vec<u8>, usize, usize), EncodeError> {
    let x0 = (c[0] as usize).min(w);
    let y0 = (c[1] as usize).min(h);
    let cw = (c[2] as usize).min(w - x0);
    let ch = (c[3] as usize).min(h - y0);
    if cw == 0 || ch == 0 {
        return Err(EncodeError::Format("пустая область обрезки".into()));
    }
    let mut out = Vec::with_capacity(cw * ch * bpp);
    for y in y0..y0 + ch {
        let row = (y * w + x0) * bpp;
        out.extend_from_slice(&px[row..row + cw * bpp]);
    }
    Ok((out, cw, ch))
}

/// Re-packs samples into rows (sub-byte depths), filters and compresses
/// them: the zlib stream of an IDAT / fdAT.
fn pack(px: &[u8], w: usize, h: usize, bits: usize, spp: usize, bpp: usize) -> Vec<u8> {
    let row_bytes = row_bytes(w, bits, spp);
    let filter_bpp = ((spp * bits).div_ceil(8)).max(1);
    let mut filtered = Vec::with_capacity((row_bytes + 1) * h);
    let mut prev = vec![0u8; row_bytes];
    let mut row = vec![0u8; row_bytes];
    for y in 0..h {
        if bits < 8 {
            row.iter_mut().for_each(|b| *b = 0);
            for x in 0..w {
                let bit = x * bits;
                row[bit / 8] |= px[y * w + x] << (8 - bits - (bit % 8));
            }
        } else {
            row.copy_from_slice(&px[y * w * bpp..(y + 1) * w * bpp]);
        }
        filter_row(&row, &prev, filter_bpp, &mut filtered);
        std::mem::swap(&mut prev, &mut row);
    }
    miniz_oxide::deflate::compress_to_vec_zlib(&filtered, 8)
}

fn row_bytes(w: usize, bits: usize, spp: usize) -> usize {
    (w * spp * bits).div_ceil(8)
}

/// Undoes the PNG row filters of a decompressed (non-interlaced) image.
fn unfilter(data: &[u8], w: usize, h: usize, bits: usize, spp: usize) -> Option<Vec<u8>> {
    let line = row_bytes(w, bits, spp);
    let bpp = ((spp * bits).div_ceil(8)).max(1);
    let mut out = vec![0u8; line * h];
    for y in 0..h {
        let src = data.get(y * (line + 1)..(y + 1) * (line + 1))?;
        let (filter, src) = (src[0], &src[1..]);
        let (done, rest) = out.split_at_mut(y * line);
        let prev = if y > 0 {
            &done[(y - 1) * line..]
        } else {
            &[][..]
        };
        let up = |i: usize| prev.get(i).copied().unwrap_or(0);
        let row = &mut rest[..line];
        for i in 0..line {
            let a = if i >= bpp { row[i - bpp] } else { 0 };
            let b = up(i);
            let c = if i >= bpp { up(i - bpp) } else { 0 };
            let p = match filter {
                0 => 0,
                1 => a,
                2 => b,
                3 => ((u16::from(a) + u16::from(b)) / 2) as u8,
                4 => {
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
                }
                _ => return None,
            };
            row[i] = src[i].wrapping_add(p);
        }
    }
    Some(out)
}

/// Where stored pixel (`sx`, `sy`) of a `w` × `h` image ends up when shown
/// with orientation `o` (the inverse of the mapping in `orient_pixels`).
pub(super) fn display_of(
    o: Orientation,
    w: usize,
    h: usize,
    sx: usize,
    sy: usize,
) -> (usize, usize) {
    match o.exif() {
        2 => (w - 1 - sx, sy),
        3 => (w - 1 - sx, h - 1 - sy),
        4 => (sx, h - 1 - sy),
        5 => (sy, sx),
        6 => (h - 1 - sy, sx),
        7 => (h - 1 - sy, w - 1 - sx),
        8 => (sy, w - 1 - sx),
        _ => (sx, sy),
    }
}

/// Animated PNG: every frame is turned at its original bit depth and its
/// position on the canvas moved with it; timing, disposal and blending stay.
fn rotate_apng(list: &[Chunk], display: Orientation) -> Result<Vec<u8>, EncodeError> {
    let bad = || EncodeError::Format("повреждённый APNG".into());
    let ihdr = list
        .first()
        .filter(|c| &c.kind == b"IHDR")
        .ok_or_else(bad)?;
    let d = ihdr.data;
    if d.len() < 13 {
        return Err(bad());
    }
    let be = |i: usize| u32::from_be_bytes([d[i], d[i + 1], d[i + 2], d[i + 3]]) as usize;
    let (cw, ch) = (be(0), be(4));
    let bits = usize::from(d[8]);
    let color = ColorType::from_u8(d[9]).ok_or_else(bad)?;
    if d[12] != 0 {
        return Err(EncodeError::Format(
            "чересстрочные анимированные PNG не поддерживаются".into(),
        ));
    }
    let spp = samples_per_pixel(color);
    let (ncw, nch) = if display.swaps_axes() {
        (ch, cw)
    } else {
        (cw, ch)
    };

    // Turns one frame: its zlib data (`w` × `h` at `x`, `y` on the canvas).
    let turn = |z: &[u8], w: usize, h: usize, x: usize, y: usize| {
        let raw = miniz_oxide::inflate::decompress_to_vec_zlib(z).map_err(|_| bad())?;
        let rows = unfilter(&raw, w, h, bits, spp).ok_or_else(bad)?;
        let (px, bpp) = unpack(&rows, row_bytes(w, bits, spp), w, h, bits, spp);
        let (rot, nw, nh) = orient_pixels(&px, w, h, bpp, display);
        // The frame's corners on the turned canvas.
        let a = display_of(display, cw, ch, x, y);
        let b = display_of(display, cw, ch, x + w - 1, y + h - 1);
        let pos = (a.0.min(b.0), a.1.min(b.1));
        Ok::<_, EncodeError>((pack(&rot, nw, nh, bits, spp, bpp), nw, nh, pos))
    };

    let mut out = Vec::new();
    out.extend_from_slice(SIGNATURE);
    let mut ihdr_new = d.to_vec();
    ihdr_new[0..4].copy_from_slice(&(ncw as u32).to_be_bytes());
    ihdr_new[4..8].copy_from_slice(&(nch as u32).to_be_bytes());
    write_chunk(&mut out, b"IHDR", &ihdr_new);

    let mut seq = 0u32;
    let mut i = 1;
    // The control chunk of the frame whose data comes next.
    let mut pending: Option<&[u8]> = None;
    while i < list.len() {
        let c = &list[i];
        match &c.kind {
            b"IEND" => break,
            b"fcTL" => {
                if c.data.len() < 26 {
                    return Err(bad());
                }
                pending = Some(c.data);
                i += 1;
            }
            kind @ (b"IDAT" | b"fdAT") => {
                // Gather the run of data chunks of this frame.
                let mut z = Vec::new();
                let mut j = i;
                while j < list.len() && &list[j].kind == kind {
                    let body = list[j].data;
                    z.extend_from_slice(if kind == b"fdAT" {
                        body.get(4..).ok_or_else(bad)?
                    } else {
                        body
                    });
                    j += 1;
                }
                i = j;
                let fc = pending.take();
                let (w, h, x, y) = match fc {
                    Some(f) => {
                        let u = |k: usize| {
                            u32::from_be_bytes([f[k], f[k + 1], f[k + 2], f[k + 3]]) as usize
                        };
                        (u(4), u(8), u(12), u(16))
                    }
                    None if kind == b"IDAT" => (cw, ch, 0, 0),
                    None => return Err(bad()),
                };
                if w == 0 || h == 0 || x + w > cw || y + h > ch {
                    return Err(bad());
                }
                let (z, nw, nh, (nx, ny)) = turn(&z, w, h, x, y)?;
                if let Some(f) = fc {
                    let mut f = f.to_vec();
                    f[0..4].copy_from_slice(&seq.to_be_bytes());
                    f[4..8].copy_from_slice(&(nw as u32).to_be_bytes());
                    f[8..12].copy_from_slice(&(nh as u32).to_be_bytes());
                    f[12..16].copy_from_slice(&(nx as u32).to_be_bytes());
                    f[16..20].copy_from_slice(&(ny as u32).to_be_bytes());
                    write_chunk(&mut out, b"fcTL", &f);
                    seq += 1;
                }
                if kind == b"IDAT" {
                    write_chunk(&mut out, b"IDAT", &z);
                } else {
                    let mut body = seq.to_be_bytes().to_vec();
                    body.extend_from_slice(&z);
                    write_chunk(&mut out, b"fdAT", &body);
                    seq += 1;
                }
            }
            b"eXIf" => {
                let mut t = c.data.to_vec();
                tiff::set_orientation(&mut t, 1);
                write_chunk(&mut out, b"eXIf", &t);
                i += 1;
            }
            b"pHYs" if display.swaps_axes() && c.data.len() == 9 => {
                let mut p = c.data.to_vec();
                p.swap(0, 4);
                p.swap(1, 5);
                p.swap(2, 6);
                p.swap(3, 7);
                write_chunk(&mut out, b"pHYs", &p);
                i += 1;
            }
            k if k != b"acTL" && !is_known(k) && k[3].is_ascii_uppercase() => i += 1,
            k => {
                write_chunk(&mut out, k, c.data);
                i += 1;
            }
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

    /// `display_of` is the inverse of `orient_pixels` for all orientations.
    #[test]
    fn display_of_matches_orient_pixels() {
        let (w, h) = (5, 3);
        let src: Vec<u8> = (0..(w * h) as u8).collect();
        for e in 1..=8u32 {
            let o = Orientation::from_exif(e);
            let (dst, dw, _) = orient_pixels(&src, w, h, 1, o);
            for sy in 0..h {
                for sx in 0..w {
                    let (x, y) = display_of(o, w, h, sx, sy);
                    assert_eq!(dst[y * dw + x], src[sy * w + sx], "exif {e} ({sx},{sy})");
                }
            }
        }
    }

    /// 6×4 canvas: a full first frame, then a 2×1 frame at (3, 2) and a 1×3
    /// frame at (0, 1), each with its own delay.
    fn apng() -> Vec<u8> {
        let mut out = Vec::new();
        {
            let mut e = png::Encoder::new(&mut out, 6, 4);
            e.set_color(ColorType::Rgba);
            e.set_depth(BitDepth::Eight);
            e.set_animated(3, 0).unwrap();
            e.add_text_chunk("parameters".into(), "a cat".into())
                .unwrap();
            let mut wr = e.write_header().unwrap();
            let px = |n: usize, base: u8| -> Vec<u8> {
                (0..n as u8).flat_map(|i| [base + i, 0, 0, 255]).collect()
            };
            wr.set_frame_delay(1, 10).unwrap();
            wr.write_image_data(&px(24, 0)).unwrap();
            wr.set_frame_dimension(2, 1).unwrap();
            wr.set_frame_position(3, 2).unwrap();
            wr.set_frame_delay(2, 10).unwrap();
            wr.write_image_data(&px(2, 100)).unwrap();
            wr.set_frame_position(0, 1).unwrap();
            wr.set_frame_dimension(1, 3).unwrap();
            wr.set_frame_delay(3, 10).unwrap();
            wr.write_image_data(&px(3, 200)).unwrap();
        }
        out
    }

    /// Width, height, x, y, delay numerator and red channel of a frame.
    type Frame = (u32, u32, u32, u32, u16, Vec<u8>);

    /// The canvas size and every frame.
    fn frames(data: &[u8]) -> ((u32, u32), Vec<Frame>) {
        let mut d = Decoder::new(Cursor::new(data));
        d.set_transformations(Transformations::IDENTITY);
        let mut r = d.read_info().unwrap();
        let canvas = (r.info().width, r.info().height);
        let n = r.info().animation_control().unwrap().num_frames;
        let mut out = Vec::new();
        for _ in 0..n {
            let mut buf = vec![0; r.output_buffer_size().unwrap()];
            let f = r.next_frame(&mut buf).unwrap();
            let fc = r.info().frame_control().unwrap();
            buf.truncate(f.buffer_size());
            let red = buf.chunks(4).map(|p| p[0]).collect();
            out.push((
                fc.width,
                fc.height,
                fc.x_offset,
                fc.y_offset,
                fc.delay_num,
                red,
            ));
        }
        (canvas, out)
    }

    #[test]
    fn animated_png_turns_every_frame() {
        let src = apng();
        let out = rotate(&src, Orientation::ROTATE_CW).unwrap();
        let (canvas, f) = frames(&out);
        assert_eq!(canvas, (4, 6));
        assert_eq!(f.len(), 3);
        // Frame 2 (2×1 at (3, 2) of 6×4) turned clockwise: 1×2 at (1, 3).
        let head = |k: usize| (f[k].0, f[k].1, f[k].2, f[k].3, f[k].4);
        assert_eq!(head(1), (1, 2, 1, 3, 2));
        assert_eq!(f[1].5, [100, 101]);
        // Frame 3 (1×3 at (0, 1)): 3×1 at (0, 0), bottom pixel first.
        assert_eq!(head(2), (3, 1, 0, 0, 3));
        assert_eq!(f[2].5, [202, 201, 200]);
        // The full frame matches a still rotation, and back is identical.
        let back = rotate(&out, Orientation::ROTATE_CCW).unwrap();
        assert_eq!(frames(&back), frames(&src));
        assert!(decode_raw(&out).3.contains(&"parameters=a cat".to_string()));
    }

    #[test]
    fn crop_after_turning() {
        // 3×2: pixels numbered 0..5; turned clockwise [3 0 / 4 1 / 5 2].
        let px: Vec<u8> = (0..6u8).flat_map(|i| [i, 0, 0]).collect();
        let src = encode(3, 2, ColorType::Rgb, BitDepth::Eight, &px, None);
        let out = transform(&src, Orientation::ROTATE_CW, Some([1, 1, 1, 2])).unwrap();
        let (w, h, data, texts) = decode_raw(&out);
        assert_eq!((w, h), (1, 2));
        assert_eq!(data.chunks(3).map(|p| p[0]).collect::<Vec<_>>(), [1, 2]);
        assert_eq!(texts, ["parameters=a cat, masterpiece"]);
        assert!(transform(&apng(), Orientation::IDENTITY, Some([0, 0, 2, 2])).is_err());
    }

    #[test]
    fn garbage_is_an_error() {
        assert!(rotate(b"not a png", Orientation::ROTATE_CW).is_err());
    }
}
