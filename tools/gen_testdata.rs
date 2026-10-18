//! Generates synthetic test images into `testdata/`.
//!
//!     cargo run --release --example gen_testdata
//!
//! Everything is procedural (no downloads), so the set is reproducible.

use std::fs;
use std::io::BufWriter;
use std::path::Path;

use turbojpeg::{Compressor, Image, PixelFormat, Subsamp};

/// Small deterministic PRNG (xorshift32) for film-grain-like noise.
struct Rng(u32);
impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// Photo-like RGB content: sky gradient, "hills", rings and noise. `noise` is
/// the grain amplitude: 16 gives a worst-case ~10 MB JPEG (entropy decoding
/// dominates), 4 a more typical ~4 MB camera-like file.
fn photo_rgb(w: usize, h: usize, noise: u32) -> Vec<u8> {
    let mut rng = Rng(0x1234_5678);
    let mut px = vec![0u8; w * h * 3];
    for y in 0..h {
        let fy = y as f32 / h as f32;
        for x in 0..w {
            let fx = x as f32 / w as f32;
            let hill = 0.6 + 0.08 * (fx * 9.0).sin() + 0.05 * (fx * 23.0 + 1.0).sin();
            let (mut r, mut g, mut b) = if fy < hill {
                (90.0 + 120.0 * fy, 140.0 + 90.0 * fy, 230.0 - 40.0 * fy)
            } else {
                let t = (fy - hill) * 3.0;
                (60.0 + 40.0 * t, 110.0 - 30.0 * t, 40.0 + 10.0 * t)
            };
            let dx = fx - 0.7;
            let dy = fy - 0.3;
            let ring = ((dx * dx + dy * dy).sqrt() * 120.0).sin();
            if ring > 0.92 {
                r = 250.0;
                g = 230.0;
                b = 120.0;
            }
            let n = (rng.next() % (2 * noise + 1)) as f32 - noise as f32;
            let i = (y * w + x) * 3;
            px[i] = (r + n).clamp(0.0, 255.0) as u8;
            px[i + 1] = (g + n).clamp(0.0, 255.0) as u8;
            px[i + 2] = (b + n).clamp(0.0, 255.0) as u8;
        }
    }
    px
}

fn write_jpeg(path: &Path, w: usize, h: usize, rgb: &[u8], progressive: bool) {
    let mut c = Compressor::new().expect("compressor");
    c.set_quality(92).expect("quality");
    c.set_subsamp(Subsamp::Sub2x2).expect("subsamp");
    c.set_progressive(progressive).expect("progressive");
    let jpeg = c
        .compress_to_vec(Image {
            pixels: rgb,
            width: w,
            pitch: w * 3,
            height: h,
            format: PixelFormat::RGB,
        })
        .expect("compress");
    fs::write(path, jpeg).expect("write jpeg");
}

fn write_png(path: &Path, w: u32, h: u32, color: png::ColorType, data: &[u8]) {
    let file = fs::File::create(path).expect("create png");
    let mut enc = png::Encoder::new(BufWriter::new(file), w, h);
    enc.set_color(color);
    enc.set_depth(png::BitDepth::Eight);
    let mut writer = enc.write_header().expect("png header");
    writer.write_image_data(data).expect("png data");
}

/// RGBA with a horizontal alpha gradient, fully transparent holes and a
/// semi-transparent coloured disc — exercises premultiplication and the
/// checkerboard.
fn alpha_rgba(w: usize, h: usize) -> Vec<u8> {
    let mut px = vec![0u8; w * h * 4];
    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) * 4;
            let fx = x as f32 / w as f32;
            let fy = y as f32 / h as f32;
            let mut rgba = [
                (255.0 * fx) as u8,
                (255.0 * fy) as u8,
                (255.0 * (1.0 - fx)) as u8,
                (255.0 * fx) as u8,
            ];
            // Transparent windows.
            if (x / 64 + y / 64) % 5 == 0 {
                rgba[3] = 0;
            }
            // Semi-transparent white disc with a hard edge.
            let (dx, dy) = (fx - 0.5, fy - 0.5);
            if dx * dx + dy * dy < 0.04 {
                rgba = [255, 255, 255, 128];
            }
            px[i..i + 4].copy_from_slice(&rgba);
        }
    }
    px
}

/// Display UV → stored UV for EXIF orientation `o` (same as the shader).
fn orient_uv(o: u16, x: f32, y: f32) -> (f32, f32) {
    match o {
        2 => (1.0 - x, y),
        3 => (1.0 - x, 1.0 - y),
        4 => (x, 1.0 - y),
        5 => (y, x),
        6 => (y, 1.0 - x),
        7 => (1.0 - y, 1.0 - x),
        8 => (1.0 - y, x),
        _ => (x, y),
    }
}

/// Upright test card: a big "F" (asymmetric in every direction) and a
/// coloured bar along the top edge.
fn letter_f(w: usize, h: usize) -> Vec<u8> {
    let mut px = vec![235u8; w * h * 3];
    let mut fill = |x0: usize, y0: usize, x1: usize, y1: usize, c: [u8; 3]| {
        for y in y0..y1 {
            for x in x0..x1 {
                px[(y * w + x) * 3..][..3].copy_from_slice(&c);
            }
        }
    };
    fill(0, 0, w, h / 12, [220, 40, 40]); // red top bar
    fill(w / 4, h / 5, w / 4 + w / 10, h * 9 / 10, [20, 20, 20]); // stem
    fill(w / 4, h / 5, w * 3 / 4, h / 5 + h / 8, [20, 20, 20]); // top arm
    fill(w / 4, h / 2, w * 3 / 5, h / 2 + h / 8, [20, 20, 20]); // middle arm
    px
}

/// JPEG whose pixels are stored transformed so that applying EXIF
/// orientation `o` shows the upright card.
fn oriented_jpeg(o: u16) -> Vec<u8> {
    let (dw, dh) = (600usize, 400usize);
    let display = letter_f(dw, dh);
    let (sw, sh) = if o >= 5 { (dh, dw) } else { (dw, dh) };
    let mut stored = vec![0u8; sw * sh * 3];
    for dy in 0..dh {
        for dx in 0..dw {
            let (u, v) = orient_uv(
                o,
                (dx as f32 + 0.5) / dw as f32,
                (dy as f32 + 0.5) / dh as f32,
            );
            let (sx, sy) = ((u * sw as f32) as usize, (v * sh as f32) as usize);
            stored[(sy * sw + sx) * 3..][..3].copy_from_slice(&display[(dy * dw + dx) * 3..][..3]);
        }
    }
    let mut c = Compressor::new().expect("compressor");
    c.set_quality(90).expect("quality");
    let jpeg = c
        .compress_to_vec(Image {
            pixels: stored.as_slice(),
            width: sw,
            pitch: sw * 3,
            height: sh,
            format: PixelFormat::RGB,
        })
        .expect("compress");
    insert_exif_orientation(&jpeg, o)
}

/// Inserts an APP1 Exif segment with only the Orientation tag after SOI.
fn insert_exif_orientation(jpeg: &[u8], o: u16) -> Vec<u8> {
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"MM\0*");
    tiff.extend_from_slice(&8u32.to_be_bytes());
    tiff.extend_from_slice(&1u16.to_be_bytes());
    tiff.extend_from_slice(&0x0112u16.to_be_bytes());
    tiff.extend_from_slice(&3u16.to_be_bytes());
    tiff.extend_from_slice(&1u32.to_be_bytes());
    tiff.extend_from_slice(&o.to_be_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0u32.to_be_bytes());
    let mut app1 = b"Exif\0\0".to_vec();
    app1.extend_from_slice(&tiff);
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
    out.extend_from_slice(&app1);
    out.extend_from_slice(&jpeg[2..]);
    out
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
    fs::create_dir_all(&dir).expect("mkdir testdata");

    println!("photo_24mp.jpg (6000×4000)…");
    let (w, h) = (6000, 4000);
    write_jpeg(
        &dir.join("photo_24mp_typical.jpg"),
        w,
        h,
        &photo_rgb(w, h, 4),
        false,
    );
    let rgb = photo_rgb(w, h, 16);
    write_jpeg(&dir.join("photo_24mp.jpg"), w, h, &rgb, false);
    write_jpeg(&dir.join("photo_24mp_progressive.jpg"), w, h, &rgb, true);

    println!("alpha_gradient.png…");
    let (aw, ah) = (1600, 1000);
    write_png(
        &dir.join("alpha_gradient.png"),
        aw as u32,
        ah as u32,
        png::ColorType::Rgba,
        &alpha_rgba(aw, ah),
    );

    // Opaque 24 Mp PNG for decoder benchmarks.
    println!("photo_24mp.png…");
    write_png(
        &dir.join("photo_24mp.png"),
        w as u32,
        h as u32,
        png::ColorType::Rgb,
        &rgb,
    );

    // 1×1 and small pixel art (nearest-neighbour checks).
    write_png(
        &dir.join("tiny_1x1.png"),
        1,
        1,
        png::ColorType::Rgba,
        &[255, 0, 0, 255],
    );
    let mut art = vec![0u8; 16 * 16 * 4];
    for (i, p) in art.as_chunks_mut::<4>().0.iter_mut().enumerate() {
        let (x, y) = (i % 16, i / 16);
        p.copy_from_slice(if (x + y) % 3 == 0 {
            &[255, 200, 0, 255]
        } else if x == y {
            &[0, 0, 0, 0]
        } else {
            &[30, 60, 200, 255]
        });
    }
    write_png(
        &dir.join("pixel_art_16.png"),
        16,
        16,
        png::ColorType::Rgba,
        &art,
    );

    // All 8 EXIF orientations: each must display as the same upright "F".
    println!("orientation_1..8.jpg…");
    for o in 1..=8u16 {
        fs::write(dir.join(format!("orientation_{o}.jpg")), oriented_jpeg(o)).expect("write");
    }

    // Broken and mislabelled files.
    let jpeg = fs::read(dir.join("photo_24mp.jpg")).expect("read back");
    fs::write(dir.join("broken_truncated.jpg"), &jpeg[..2000]).expect("write");
    fs::write(
        dir.join("broken_garbage.png"),
        b"\x89PNG\r\n\x1a\nthis is not a png",
    )
    .expect("write");
    fs::copy(
        dir.join("alpha_gradient.png"),
        dir.join("png_with_jpg_ext.jpg"),
    )
    .expect("copy");

    println!("done: {}", dir.display());
}
