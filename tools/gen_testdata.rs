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

/// APP1 "Exif" segment: IFD0 { Orientation = 1 } → IFD1 { thumbnail }.
fn exif_with_thumbnail(thumb: &[u8]) -> Vec<u8> {
    let entry = |t: &mut Vec<u8>, tag: u16, ty: u16, v: u32| {
        t.extend_from_slice(&tag.to_le_bytes());
        t.extend_from_slice(&ty.to_le_bytes());
        t.extend_from_slice(&1u32.to_le_bytes());
        t.extend_from_slice(&v.to_le_bytes());
    };
    let mut t = b"II*\0".to_vec();
    t.extend_from_slice(&8u32.to_le_bytes());
    // IFD0 at 8 (18 bytes), IFD1 at 26 (30 bytes), thumbnail at 56.
    t.extend_from_slice(&1u16.to_le_bytes());
    entry(&mut t, 0x0112, 3, 1);
    t.extend_from_slice(&26u32.to_le_bytes());
    t.extend_from_slice(&2u16.to_le_bytes());
    entry(&mut t, 0x0201, 4, 56);
    entry(&mut t, 0x0202, 4, thumb.len() as u32);
    t.extend_from_slice(&0u32.to_le_bytes());
    t.extend_from_slice(thumb);
    let mut seg = vec![0xFF, 0xE1];
    seg.extend_from_slice(&((t.len() + 8) as u16).to_be_bytes());
    seg.extend_from_slice(b"Exif\0\0");
    seg.extend_from_slice(&t);
    seg
}

/// APP2 ICC_PROFILE segment (single chunk).
fn icc_segment(icc: &[u8]) -> Vec<u8> {
    let mut seg = vec![0xFF, 0xE2];
    seg.extend_from_slice(&((icc.len() + 16) as u16).to_be_bytes());
    seg.extend_from_slice(b"ICC_PROFILE\0");
    seg.extend_from_slice(&[1, 1]);
    seg.extend_from_slice(icc);
    seg
}

/// Inserts a marker segment right after SOI.
fn insert_segment(jpeg: &[u8], seg: &[u8]) -> Vec<u8> {
    let mut out = jpeg[..2].to_vec();
    out.extend_from_slice(seg);
    out.extend_from_slice(&jpeg[2..]);
    out
}

fn jpeg_bytes(w: usize, h: usize, px: &[u8], format: PixelFormat, quality: i32) -> Vec<u8> {
    let mut c = Compressor::new().expect("compressor");
    c.set_quality(quality).expect("quality");
    c.compress_to_vec(Image {
        pixels: px,
        width: w,
        pitch: w * format.size(),
        height: h,
        format,
    })
    .expect("compress")
}

fn save(img: &image::DynamicImage, path: &Path) {
    img.save(path)
        .unwrap_or_else(|e| panic!("{}: {e}", path.display()));
}

/// Every additional format: one small file each.
fn formats(dir: &Path) {
    use image::{DynamicImage, ImageBuffer, Rgb, Rgba};

    // JPEG with an EXIF thumbnail (progressive loading).
    println!("photo_24mp_exifthumb.jpg…");
    let (w, h) = (6000, 4000);
    let rgb = photo_rgb(w, h, 4);
    let main = jpeg_bytes(w, h, &rgb, PixelFormat::RGB, 92);
    let full = ImageBuffer::<Rgb<u8>, _>::from_raw(w as u32, h as u32, rgb).expect("buffer");
    let small = image::imageops::thumbnail(&full, 240, 160);
    let thumb = jpeg_bytes(240, 160, small.as_raw(), PixelFormat::RGB, 80);
    fs::write(
        dir.join("photo_24mp_exifthumb.jpg"),
        insert_segment(&main, &exif_with_thumbnail(&thumb)),
    )
    .expect("write");

    let rgba = DynamicImage::ImageRgba8(ImageBuffer::from_fn(640, 400, |x, y| {
        Rgba([(x * 255 / 640) as u8, (y * 255 / 400) as u8, 160, 255])
    }));

    // JPEG with a wide-gamut (Adobe RGB-like) profile: saturated green must
    // map to a different sRGB green.
    let xy = |x, y| lcms2::CIExyY { x, y, Y: 1.0 };
    let curve = lcms2::ToneCurve::new(2.2);
    let adobe = lcms2::Profile::new_rgb(
        &xy(0.3127, 0.3290),
        &lcms2::CIExyYTRIPLE {
            Red: xy(0.64, 0.33),
            Green: xy(0.21, 0.71),
            Blue: xy(0.15, 0.06),
        },
        &[&curve, &curve, &curve],
    )
    .expect("profile")
    .icc()
    .expect("icc");
    let green: Vec<u8> = (0..320 * 200).flat_map(|_| [30, 220, 40]).collect();
    let jpg = jpeg_bytes(320, 200, &green, PixelFormat::RGB, 95);
    fs::write(
        dir.join("adobe_rgb.jpg"),
        insert_segment(&jpg, &icc_segment(&adobe)),
    )
    .expect("write");

    // CMYK JPEG without a profile (simple conversion). libjpeg writes an
    // Adobe marker, so readers expect inverted ink values (Photoshop style).
    let cmyk: Vec<u8> = (0..320 * 200)
        .flat_map(|i| [((i % 320) * 255 / 320) as u8, 40, 200, 20].map(|v| 255 - v))
        .collect();
    fs::write(
        dir.join("cmyk.jpg"),
        jpeg_bytes(320, 200, &cmyk, PixelFormat::CMYK, 95),
    )
    .expect("write");

    // 16-bit PNG and TIFF: smooth gradients (banding test).
    let g16 = DynamicImage::ImageRgba16(ImageBuffer::from_fn(1024, 256, |x, y| {
        let v = (x * 65535 / 1023) as u16;
        Rgba([v, v / 2, 65535 - v, if y < 128 { 65535 } else { 40000 }])
    }));
    save(&g16, &dir.join("gradient_16bit.png"));
    save(&g16, &dir.join("gradient_16bit.tif"));

    // PNG with A1111 / ComfyUI generation metadata.
    {
        let file = fs::File::create(dir.join("comfyui.png")).expect("create");
        let mut e = png::Encoder::new(BufWriter::new(file), 64, 64);
        e.set_color(png::ColorType::Rgb);
        e.set_depth(png::BitDepth::Eight);
        e.add_text_chunk(
            "parameters".into(),
            "a lighthouse at dusk, oil painting\nNegative prompt: blurry\n\
             Steps: 30, Sampler: DPM++ 2M, CFG scale: 7, Seed: 1234, Model: sdxl"
                .into(),
        )
        .expect("text");
        e.add_text_chunk(
            "prompt".into(),
            r#"{"3":{"class_type":"KSampler","inputs":{"seed":1234}}}"#.into(),
        )
        .expect("text");
        e.add_text_chunk("workflow".into(), r#"{"nodes":[],"links":[]}"#.into())
            .expect("text");
        let mut wr = e.write_header().expect("header");
        wr.write_image_data(&vec![120u8; 64 * 64 * 3])
            .expect("data");
    }

    // Simple formats via the image crate.
    for (name, img) in [
        ("sample.bmp", rgba.clone()),
        ("sample.tga", rgba.clone()),
        ("sample.qoi", rgba.clone()),
        ("sample.tif", rgba.clone()),
        ("sample.webp", rgba.clone()),
        ("sample.ppm", DynamicImage::ImageRgb8(rgba.to_rgb8())),
        (
            "sample.ico",
            rgba.resize_exact(256, 256, image::imageops::FilterType::Triangle),
        ),
    ] {
        save(&img, &dir.join(name));
    }
    // 0..8× over white: needs tone mapping.
    let hdr = DynamicImage::ImageRgb32F(ImageBuffer::from_fn(512, 256, |x, _| {
        let e = 8.0 * x as f32 / 512.0;
        Rgb([e, e * 0.8, e * 0.6])
    }));
    save(&hdr, &dir.join("sample.hdr"));
    save(
        &DynamicImage::ImageRgba32F(hdr.to_rgba32f()),
        &dir.join("sample.exr"),
    );

    // Animated GIF (first frame for now; playback in M7).
    {
        use image::codecs::gif::{GifEncoder, Repeat};
        use image::{Delay, Frame};
        let file = fs::File::create(dir.join("animated.gif")).expect("create");
        let mut enc = GifEncoder::new(file);
        enc.set_repeat(Repeat::Infinite).expect("repeat");
        for i in 0..8u32 {
            let frame = ImageBuffer::from_fn(200, 120, |x, y| {
                if (x / 25 + y / 30 + i) % 2 == 0 {
                    Rgba([255, 80, 40, 255])
                } else {
                    Rgba([20, 40, 200, 255])
                }
            });
            enc.encode_frame(Frame::from_parts(
                frame,
                0,
                0,
                Delay::from_numer_denom_ms(100, 1),
            ))
            .expect("frame");
        }
    }

    fs::write(
        dir.join("sample.svg"),
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="400" height="300" viewBox="0 0 400 300">
  <rect width="400" height="300" fill="#1d3557"/>
  <circle cx="200" cy="150" r="110" fill="#e63946" fill-opacity="0.85"/>
  <path d="M60 260 L200 60 L340 260 Z" fill="none" stroke="#f1faee" stroke-width="10"/>
  <text x="200" y="285" font-family="Segoe UI" font-size="22" fill="#f1faee" text-anchor="middle">Lightbox SVG</text>
</svg>"##,
    )
    .expect("write");

    // AVIF via ffmpeg (libaom), if it is installed.
    let ok = std::process::Command::new("ffmpeg")
        .args(["-y", "-loglevel", "error", "-i"])
        .arg(dir.join("alpha_gradient.png"))
        .args(["-c:v", "libaom-av1", "-still-picture", "1", "-crf", "30"])
        .arg(dir.join("sample.avif"))
        .status()
        .is_ok_and(|s| s.success());
    println!(
        "sample.avif: {}",
        if ok { "ok" } else { "skipped (no ffmpeg)" }
    );
}

/// 30 000 × 20 000 PNG for tiling, streamed row by row.
fn huge_png(path: &Path) {
    let (w, h) = (30_000u32, 20_000u32);
    let file = fs::File::create(path).expect("create");
    let mut e = png::Encoder::new(BufWriter::new(file), w, h);
    e.set_color(png::ColorType::Rgb);
    e.set_depth(png::BitDepth::Eight);
    e.set_compression(png::Compression::Fast);
    let mut wr = e
        .write_header()
        .expect("header")
        .into_stream_writer()
        .expect("stream");
    let mut row = vec![0u8; w as usize * 3];
    for y in 0..h {
        for x in 0..w {
            let i = x as usize * 3;
            // A white grid every 1000 px; cells coloured by their position.
            let line = x % 1000 < 6 || y % 1000 < 6;
            row[i] = if line { 255 } else { (x / 1000 * 8) as u8 };
            row[i + 1] = if line { 255 } else { (y / 1000 * 12) as u8 };
            row[i + 2] = if line {
                255
            } else {
                ((x ^ y) & 0x3F) as u8 + 60
            };
        }
        std::io::Write::write_all(&mut wr, &row).expect("row");
    }
    wr.finish().expect("finish");
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

    formats(&dir);
    let huge = dir.join("huge_30000x20000.png");
    if !huge.exists() {
        println!("huge_30000x20000.png (takes a while)…");
        huge_png(&huge);
    }

    println!("done: {}", dir.display());
}
