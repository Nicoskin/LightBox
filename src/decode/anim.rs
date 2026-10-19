//! Animated images (GIF, animated WebP, APNG): every frame composited to
//! the full canvas, premultiplied for the GPU, with its delay.

use std::io::{BufReader, Cursor};

use image::AnimationDecoder;

use super::{DecodeError, alpha};

/// Frames kept in memory at most (bytes); longer animations show their
/// first frame only.
const MAX_BYTES: usize = 1 << 30;

pub struct Frame {
    /// Premultiplied RGBA8 like `Pixels::Srgb8`.
    pub pixels: Vec<u8>,
    pub delay_ms: u32,
}

pub struct Animation {
    pub width: u32,
    pub height: u32,
    pub frames: Vec<Frame>,
}

/// Browsers show delays under 20 ms as 100 ms (old GIFs rely on it).
fn delay(ms: u32) -> u32 {
    if ms < 20 { 100 } else { ms }
}

/// All frames of an animated GIF / WebP / PNG; `Ok(None)` when the file is
/// not animated (or too long to hold).
pub fn decode(data: &[u8]) -> Result<Option<Animation>, DecodeError> {
    let other = |e: image::ImageError| DecodeError::Other(e.to_string());
    let frames = if data.starts_with(b"GIF8") {
        let d = image::codecs::gif::GifDecoder::new(BufReader::new(Cursor::new(data)))
            .map_err(other)?;
        Some(collect(d.into_frames())?)
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        let d = image::codecs::webp::WebPDecoder::new(BufReader::new(Cursor::new(data)))
            .map_err(other)?;
        if d.has_animation() {
            Some(collect(d.into_frames())?)
        } else {
            None
        }
    } else if data.starts_with(b"\x89PNG") {
        apng(data)?
    } else {
        None
    };
    let Some(Some((width, height, mut list))) = frames else {
        return Ok(None);
    };
    if list.len() < 2 {
        return Ok(None);
    }
    for f in &mut list {
        alpha::premultiply_in_place(&mut f.pixels);
    }
    Ok(Some(Animation {
        width,
        height,
        frames: list,
    }))
}

type Collected = Option<(u32, u32, Vec<Frame>)>;

/// Frames from the `image` crate (already composited), within the budget.
fn collect(frames: image::Frames<'_>) -> Result<Collected, DecodeError> {
    let mut out = Vec::new();
    let mut size = (0, 0);
    let mut bytes = 0usize;
    for f in frames {
        let f = f.map_err(|e| DecodeError::Other(e.to_string()))?;
        let (n, d) = f.delay().numer_denom_ms();
        let buf = f.into_buffer();
        size = buf.dimensions();
        bytes += buf.len();
        if bytes > MAX_BYTES {
            tracing::info!("animation too long to hold: first frame only");
            return Ok(None);
        }
        out.push(Frame {
            pixels: buf.into_raw(),
            delay_ms: delay(n / d.max(1)),
        });
    }
    Ok(Some((size.0, size.1, out)))
}

/// APNG: frames are sub-rectangles blended onto a canvas and then disposed
/// of as each frame says.
fn apng(data: &[u8]) -> Result<Option<Collected>, DecodeError> {
    let err = |e: png::DecodingError| DecodeError::Png(e.to_string());
    let mut decoder = png::Decoder::new(Cursor::new(data));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::ALPHA);
    let mut reader = decoder.read_info().map_err(err)?;
    let info = reader.info();
    let Some(actl) = info.animation_control else {
        return Ok(None);
    };
    if actl.num_frames < 2 {
        return Ok(None);
    }
    let (w, h) = (info.width as usize, info.height as usize);
    // The default image is an animation frame only if it has an fcTL.
    let default_is_frame = info.frame_control.is_some();
    let (color, depth) = reader.output_color_type();
    if depth != png::BitDepth::Eight && depth != png::BitDepth::Sixteen {
        return Ok(None);
    }
    let mut canvas = vec![0u8; w * h * 4];
    let mut out = Vec::new();
    let mut buf = vec![0u8; reader.output_buffer_size().unwrap_or(w * h * 8)];
    let mut first = true;
    loop {
        let frame = match reader.next_frame(&mut buf) {
            Ok(f) => f,
            Err(_) if !out.is_empty() => break, // truncated: keep what played
            Err(e) => return Err(err(e)),
        };
        let skip = first && !default_is_frame;
        first = false;
        let Some(fc) = reader.info().frame_control else {
            if skip {
                continue;
            }
            break;
        };
        if skip {
            continue;
        }
        let rgba = to_rgba8(&buf[..frame.buffer_size()], color, depth);
        let (fx, fy, fw, fh) = (
            fc.x_offset as usize,
            fc.y_offset as usize,
            fc.width as usize,
            fc.height as usize,
        );
        if fx + fw > w || fy + fh > h || rgba.len() < fw * fh * 4 {
            break;
        }
        let saved =
            (fc.dispose_op == png::DisposeOp::Previous).then(|| region(&canvas, w, fx, fy, fw, fh));
        for y in 0..fh {
            for x in 0..fw {
                let s = &rgba[(y * fw + x) * 4..][..4];
                let d = &mut canvas[((fy + y) * w + fx + x) * 4..][..4];
                if fc.blend_op == png::BlendOp::Source || s[3] == 255 {
                    d.copy_from_slice(s);
                } else if s[3] > 0 {
                    over(d, s);
                }
            }
        }
        out.push(Frame {
            pixels: canvas.clone(),
            delay_ms: delay(
                (u32::from(fc.delay_num) * 1000)
                    / u32::from(if fc.delay_den == 0 { 100 } else { fc.delay_den }),
            ),
        });
        if out.len() * w * h * 4 > MAX_BYTES {
            return Ok(Some(None));
        }
        match fc.dispose_op {
            png::DisposeOp::Background => {
                for y in fy..fy + fh {
                    canvas[(y * w + fx) * 4..(y * w + fx + fw) * 4].fill(0);
                }
            }
            png::DisposeOp::Previous => {
                if let Some(saved) = saved {
                    for (row, y) in (fy..fy + fh).enumerate() {
                        canvas[(y * w + fx) * 4..(y * w + fx + fw) * 4]
                            .copy_from_slice(&saved[row * fw * 4..(row + 1) * fw * 4]);
                    }
                }
            }
            png::DisposeOp::None => {}
        }
        if out.len() as u32 >= actl.num_frames {
            break;
        }
    }
    Ok(Some(Some((w as u32, h as u32, out))))
}

fn region(c: &[u8], w: usize, x: usize, y: usize, rw: usize, rh: usize) -> Vec<u8> {
    let mut v = Vec::with_capacity(rw * rh * 4);
    for row in y..y + rh {
        v.extend_from_slice(&c[(row * w + x) * 4..(row * w + x + rw) * 4]);
    }
    v
}

/// Straight-alpha "over" in 8 bits.
fn over(d: &mut [u8], s: &[u8]) {
    let sa = u32::from(s[3]);
    let da = u32::from(d[3]) * (255 - sa) / 255;
    let oa = sa + da;
    if oa == 0 {
        d.fill(0);
        return;
    }
    for c in 0..3 {
        d[c] = ((u32::from(s[c]) * sa + u32::from(d[c]) * da) / oa) as u8;
    }
    d[3] = oa as u8;
}

/// Decoder output (after EXPAND | ALPHA) to RGBA8.
fn to_rgba8(b: &[u8], color: png::ColorType, depth: png::BitDepth) -> Vec<u8> {
    let v8: Vec<u8> = if depth == png::BitDepth::Sixteen {
        b.as_chunks::<2>().0.iter().map(|c| c[0]).collect()
    } else {
        b.to_vec()
    };
    match color {
        png::ColorType::Rgba => v8,
        png::ColorType::GrayscaleAlpha => v8
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Rgb => v8
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        _ => v8.iter().flat_map(|&g| [g, g, g, 255]).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gif_frames_and_delays() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/animated.gif");
        let Ok(data) = std::fs::read(&p) else {
            return;
        };
        let a = decode(&data).unwrap().expect("animated");
        assert_eq!((a.width, a.height, a.frames.len()), (200, 120, 8));
        assert_eq!(a.frames[0].delay_ms, 100);
        // Frames differ (the checkerboard shifts).
        assert_ne!(a.frames[0].pixels, a.frames[1].pixels);
    }

    #[test]
    fn apng_composites_frames() {
        // 2×1 APNG: frame 1 red / green, frame 2 paints only x = 1 blue.
        let mut bytes = Vec::new();
        {
            let mut e = png::Encoder::new(&mut bytes, 2, 1);
            e.set_color(png::ColorType::Rgba);
            e.set_animated(2, 0).unwrap();
            let mut w = e.write_header().unwrap();
            w.set_frame_delay(1, 10).unwrap();
            w.write_image_data(&[255, 0, 0, 255, 0, 255, 0, 255])
                .unwrap();
            w.set_frame_dimension(1, 1).unwrap();
            w.set_frame_position(1, 0).unwrap();
            w.write_image_data(&[0, 0, 255, 255]).unwrap();
        }
        let a = decode(&bytes).unwrap().expect("animated");
        assert_eq!(a.frames.len(), 2);
        assert_eq!(&a.frames[1].pixels[..8], &[255, 0, 0, 255, 0, 0, 255, 255]);
        assert_eq!(a.frames[0].delay_ms, 100);
        // A still PNG is not an animation.
        let mut still = Vec::new();
        {
            let mut e = png::Encoder::new(&mut still, 1, 1);
            e.set_color(png::ColorType::Rgba);
            e.write_header().unwrap().write_image_data(&[0; 4]).unwrap();
        }
        assert!(decode(&still).unwrap().is_none());
    }
}
