//! Box-filter downscaling (overviews of huge images, thumbnails), split
//! into horizontal bands across threads.

use std::thread;

use super::{DecodedImage, Pixels};

/// Target size fitting `(w, h)` into `side` × `side`, keeping the aspect.
pub fn fit_size(w: u32, h: u32, side: u32) -> (u32, u32) {
    let k = (f64::from(side) / f64::from(w.max(h))).min(1.0);
    (
        ((f64::from(w) * k).round() as u32).max(1),
        ((f64::from(h) * k).round() as u32).max(1),
    )
}

/// Generic box filter over 4-channel pixels of type `T`, accumulated in f32.
fn box_filter<T: Copy + Send + Sync>(
    src: &[T],
    (sw, sh): (u32, u32),
    (tw, th): (u32, u32),
    to_f: impl Fn(T) -> f32 + Sync,
    from_f: impl Fn(f32) -> T + Sync,
    zero: T,
) -> Vec<T> {
    let mut out = vec![zero; (tw * th * 4) as usize];
    let threads = thread::available_parallelism()
        .map_or(4, std::num::NonZero::get)
        .min(16) as u32;
    let band = th.div_ceil(threads).max(1);
    thread::scope(|s| {
        for (bi, chunk) in out.chunks_mut((band * tw * 4) as usize).enumerate() {
            let (to_f, from_f) = (&to_f, &from_f);
            s.spawn(move || {
                let ty0 = bi as u32 * band;
                for (row, line) in chunk.chunks_mut((tw * 4) as usize).enumerate() {
                    let ty = ty0 + row as u32;
                    let y0 = (u64::from(ty) * u64::from(sh) / u64::from(th)) as u32;
                    let y1 = ((u64::from(ty + 1) * u64::from(sh) / u64::from(th)) as u32)
                        .max(y0 + 1)
                        .min(sh);
                    for tx in 0..tw {
                        let x0 = (u64::from(tx) * u64::from(sw) / u64::from(tw)) as u32;
                        let x1 = ((u64::from(tx + 1) * u64::from(sw) / u64::from(tw)) as u32)
                            .max(x0 + 1)
                            .min(sw);
                        let mut acc = [0f32; 4];
                        for y in y0..y1 {
                            let base = (y as usize * sw as usize + x0 as usize) * 4;
                            for i in 0..(x1 - x0) as usize * 4 {
                                acc[i % 4] += to_f(src[base + i]);
                            }
                        }
                        let n = ((y1 - y0) * (x1 - x0)) as f32;
                        let o = tx as usize * 4;
                        for c in 0..4 {
                            line[o + c] = from_f(acc[c] / n);
                        }
                    }
                }
            });
        }
    });
    out
}

/// Downscales straight or premultiplied RGBA8 (values averaged as stored).
pub fn rgba8(src: &[u8], sw: u32, sh: u32, tw: u32, th: u32) -> Vec<u8> {
    box_filter(src, (sw, sh), (tw, th), f32::from, |v| (v + 0.5) as u8, 0u8)
}

/// Reduced copy of `img` whose longer side is at most `side`.
pub fn overview(img: &DecodedImage, side: u32) -> DecodedImage {
    let (tw, th) = fit_size(img.width, img.height, side);
    let pixels = match &img.pixels {
        Pixels::Srgb8(v) => Pixels::Srgb8(rgba8(v, img.width, img.height, tw, th)),
        Pixels::Linear16F(v) => Pixels::Linear16F(box_filter(
            v,
            (img.width, img.height),
            (tw, th),
            |x| half::f16::from_bits(x).to_f32(),
            |x| half::f16::from_f32(x).to_bits(),
            0u16,
        )),
    };
    DecodedImage {
        width: tw,
        height: th,
        pixels,
        source_width: img.source_width,
        source_height: img.source_height,
        has_alpha: img.has_alpha,
        orientation: img.orientation,
        overview: None,
        thumbnail: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn averages_blocks() {
        let src = [
            0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
        ];
        assert_eq!(rgba8(&src, 2, 2, 1, 1), vec![128, 128, 128, 255]);
        assert_eq!(rgba8(&src, 2, 2, 2, 2), src.to_vec());
    }

    #[test]
    fn many_bands_cover_every_row() {
        let (sw, sh) = (37, 101);
        let src: Vec<u8> = (0..sw * sh * 4).map(|i| (i % 251) as u8).collect();
        let out = rgba8(&src, sw, sh, 17, 53);
        assert_eq!(out.len(), 17 * 53 * 4);
        assert_eq!(fit_size(30_000, 20_000, 8192), (8192, 5461));
    }
}
