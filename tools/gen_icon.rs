//! Draws the Lightbox logo and writes `assets/lightbox.ico` (16–256 px) and
//! `assets/lightbox.png` (512 px).
//!
//!     cargo run --release --example gen_icon
//!
//! The logo is procedural (supersampled shapes), so every size is rendered
//! natively rather than scaled from one bitmap.

use std::fs;
use std::io::BufWriter;
use std::path::Path;

type Rgba = [f32; 4]; // straight alpha, 0..1

fn mix(a: Rgba, b: Rgba, t: f32) -> Rgba {
    [
        a[0] + (b[0] - a[0]) * t,
        a[1] + (b[1] - a[1]) * t,
        a[2] + (b[2] - a[2]) * t,
        a[3] + (b[3] - a[3]) * t,
    ]
}

/// "src over dst" with straight alpha.
fn over(dst: Rgba, src: Rgba) -> Rgba {
    let a = src[3] + dst[3] * (1.0 - src[3]);
    if a <= 0.0 {
        return [0.0; 4];
    }
    let c = |i: usize| (src[i] * src[3] + dst[i] * dst[3] * (1.0 - src[3])) / a;
    [c(0), c(1), c(2), a]
}

fn hex(h: u32) -> Rgba {
    [
        ((h >> 16) & 0xff) as f32 / 255.0,
        ((h >> 8) & 0xff) as f32 / 255.0,
        (h & 0xff) as f32 / 255.0,
        1.0,
    ]
}

/// Inside a rounded rectangle.
fn in_rrect(x: f32, y: f32, x0: f32, y0: f32, x1: f32, y1: f32, r: f32) -> bool {
    let cx = x.clamp(x0 + r, x1 - r);
    let cy = y.clamp(y0 + r, y1 - r);
    x >= x0 && x <= x1 && y >= y0 && y <= y1 && (x - cx).powi(2) + (y - cy).powi(2) <= r * r
}

fn in_triangle(p: (f32, f32), a: (f32, f32), b: (f32, f32), c: (f32, f32)) -> bool {
    let s = |p1: (f32, f32), p2: (f32, f32), p3: (f32, f32)| {
        (p1.0 - p3.0) * (p2.1 - p3.1) - (p2.0 - p3.0) * (p1.1 - p3.1)
    };
    let (d1, d2, d3) = (s(p, a, b), s(p, b, c), s(p, c, a));
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

/// Colour of the logo at normalised position (x, y) in 0..1.
fn sample(x: f32, y: f32) -> Rgba {
    let mut c: Rgba = [0.0; 4];
    // Tile: rounded square, diagonal blue → violet gradient with a soft glow.
    if in_rrect(x, y, 0.04, 0.04, 0.96, 0.96, 0.21) {
        let t = ((x + y) / 2.0).clamp(0.0, 1.0);
        let mut bg = mix(hex(0x2F7BFF), hex(0x7B3CF0), t);
        let glow = (1.0 - ((x - 0.42).powi(2) + (y - 0.36).powi(2)).sqrt() / 0.55).max(0.0);
        bg = mix(bg, hex(0x9FD0FF), glow * 0.35);
        c = over(c, bg);
    }
    // Photo card: white frame with a dark sky inside.
    let (fx0, fy0, fx1, fy1) = (0.19, 0.25, 0.81, 0.77);
    if in_rrect(x, y, fx0, fy0, fx1, fy1, 0.07) {
        c = over(c, hex(0xFFFFFF));
        let inner = (fx0 + 0.055, fy0 + 0.055, fx1 - 0.055, fy1 - 0.055);
        if in_rrect(x, y, inner.0, inner.1, inner.2, inner.3, 0.025) {
            let t = ((y - inner.1) / (inner.3 - inner.1)).clamp(0.0, 1.0);
            c = over(c, mix(hex(0x1C2A5C), hex(0x3E5BB8), t));
            // Sun.
            if (x - 0.62).powi(2) + (y - 0.41).powi(2) <= 0.058f32.powi(2) {
                c = over(c, hex(0xFFD45A));
            }
            // Mountains.
            let p = (x, y);
            let back = in_triangle(p, (0.42, 0.72), (0.60, 0.47), (0.78, 0.72));
            let front = in_triangle(p, (0.22, 0.72), (0.40, 0.44), (0.60, 0.72));
            if back {
                c = over(c, hex(0x8FB4FF));
            }
            if front {
                c = over(c, hex(0xFFFFFF));
            }
        }
    }
    c
}

/// Renders the logo at `size` px with 6×6 supersampling (premultiplied
/// averaging, straight-alpha output).
fn render(size: u32) -> Vec<u8> {
    const SS: u32 = 6;
    let mut px = Vec::with_capacity((size * size * 4) as usize);
    for py in 0..size {
        for pxx in 0..size {
            let mut acc = [0f32; 4];
            for sy in 0..SS {
                for sx in 0..SS {
                    let x = (pxx as f32 + (sx as f32 + 0.5) / SS as f32) / size as f32;
                    let y = (py as f32 + (sy as f32 + 0.5) / SS as f32) / size as f32;
                    let c = sample(x, y);
                    acc[0] += c[0] * c[3];
                    acc[1] += c[1] * c[3];
                    acc[2] += c[2] * c[3];
                    acc[3] += c[3];
                }
            }
            let n = (SS * SS) as f32;
            let a = acc[3] / n;
            let un = |v: f32| {
                if a > 0.0 {
                    ((v / n / a) * 255.0 + 0.5).clamp(0.0, 255.0) as u8
                } else {
                    0
                }
            };
            px.extend_from_slice(&[un(acc[0]), un(acc[1]), un(acc[2]), (a * 255.0 + 0.5) as u8]);
        }
    }
    px
}

fn png_bytes(size: u32, rgba: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, size, size);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut w = enc.write_header().expect("png header");
        w.write_image_data(rgba).expect("png data");
    }
    out
}

fn main() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    fs::create_dir_all(&dir).expect("mkdir assets");

    let sizes = [16u32, 20, 24, 32, 40, 48, 64, 128, 256];
    let images: Vec<Vec<u8>> = sizes.iter().map(|&s| png_bytes(s, &render(s))).collect();

    // ICO container with PNG-compressed entries (Windows Vista and later).
    let mut ico = Vec::new();
    ico.extend_from_slice(&0u16.to_le_bytes());
    ico.extend_from_slice(&1u16.to_le_bytes());
    ico.extend_from_slice(&(sizes.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * sizes.len() as u32;
    for (s, data) in sizes.iter().zip(&images) {
        let dim = if *s >= 256 { 0 } else { *s as u8 };
        ico.extend_from_slice(&[dim, dim, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes()); // planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bpp
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for data in &images {
        ico.extend_from_slice(data);
    }
    fs::write(dir.join("lightbox.ico"), ico).expect("write ico");

    let big = 512;
    let file = fs::File::create(dir.join("lightbox.png")).expect("create png");
    let mut enc = png::Encoder::new(BufWriter::new(file), big, big);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header()
        .expect("png header")
        .write_image_data(&render(big))
        .expect("png data");
    println!("wrote {}", dir.display());
}
