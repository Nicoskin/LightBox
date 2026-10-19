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

    // JPEG with camera EXIF (info panel).
    {
        use exif::{Field, In, Rational, Tag, Value};
        let ascii = |s: &str| Value::Ascii(vec![s.as_bytes().to_vec()]);
        let r = |num: u32, denom: u32| Rational { num, denom };
        let fields = [
            (Tag::Make, ascii("Canon")),
            (Tag::Model, ascii("Canon EOS R6")),
            (Tag::Orientation, Value::Short(vec![1])),
            (Tag::LensModel, ascii("RF24-105mm F4 L IS USM")),
            (Tag::ExposureTime, Value::Rational(vec![r(1, 250)])),
            (Tag::FNumber, Value::Rational(vec![r(8, 1)])),
            (Tag::PhotographicSensitivity, Value::Short(vec![200])),
            (Tag::FocalLength, Value::Rational(vec![r(50, 1)])),
            (Tag::DateTimeOriginal, ascii("2026:07:14 18:32:05")),
            (Tag::GPSLatitudeRef, ascii("N")),
            (
                Tag::GPSLatitude,
                Value::Rational(vec![r(55, 1), r(45, 1), r(2088, 100)]),
            ),
            (Tag::GPSLongitudeRef, ascii("E")),
            (
                Tag::GPSLongitude,
                Value::Rational(vec![r(37, 1), r(37, 1), r(2028, 100)]),
            ),
        ]
        .map(|(tag, value)| Field {
            tag,
            ifd_num: In::PRIMARY,
            value,
        });
        let mut w = exif::experimental::Writer::new();
        for f in &fields {
            w.push_field(f);
        }
        let mut tiff = std::io::Cursor::new(Vec::new());
        w.write(&mut tiff, false).expect("exif");
        let tiff = tiff.into_inner();
        let mut seg = vec![0xFF, 0xE1];
        seg.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
        seg.extend_from_slice(b"Exif\0\0");
        seg.extend_from_slice(&tiff);
        let (w, h) = (1200, 800);
        let jpg = jpeg_bytes(w, h, &photo_rgb(w, h, 4), PixelFormat::RGB, 90);
        fs::write(dir.join("camera_exif.jpg"), insert_segment(&jpg, &seg)).expect("write");
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

/// 3D samples: a binary STL torus, an OBJ cube with MTL colours and a 3MF
/// with two coloured parts placed by the build.
fn models(dir: &Path) {
    use std::f32::consts::TAU;
    // Torus, Z-up like printer files.
    let (big, small, nu, nv) = (30.0f32, 10.0f32, 64, 32);
    let p = |i: usize, j: usize| {
        let (u, v) = (TAU * i as f32 / nu as f32, TAU * j as f32 / nv as f32);
        let r = big + small * v.cos();
        [r * u.cos(), r * u.sin(), small + small * v.sin()]
    };
    let mut tris = Vec::new();
    for i in 0..nu {
        for j in 0..nv {
            let (a, b, c, d) = (p(i, j), p(i + 1, j), p(i + 1, j + 1), p(i, j + 1));
            tris.push([a, b, c]);
            tris.push([a, c, d]);
        }
    }
    let mut stl = vec![0u8; 80];
    stl.extend_from_slice(&(tris.len() as u32).to_le_bytes());
    for t in &tris {
        stl.extend_from_slice(&[0u8; 12]);
        for v in t {
            for c in v {
                stl.extend_from_slice(&c.to_le_bytes());
            }
        }
        stl.extend_from_slice(&[0, 0]);
    }
    fs::write(dir.join("torus.stl"), stl).expect("write");

    // Cube with two materials, Y-up.
    fs::write(
        dir.join("cube.mtl"),
        "newmtl orange\nKd 1.0 0.5 0.1\nnewmtl teal\nKd 0.1 0.6 0.6\n",
    )
    .expect("write");
    fs::write(
        dir.join("cube.obj"),
        "mtllib cube.mtl\n\
         v -1 -1 -1\nv 1 -1 -1\nv 1 1 -1\nv -1 1 -1\nv -1 -1 1\nv 1 -1 1\nv 1 1 1\nv -1 1 1\n\
         usemtl orange\nf 5 6 7 8\nf 2 1 4 3\nf 4 8 7 3\n\
         usemtl teal\nf 1 2 6 5\nf 2 3 7 6\nf 1 5 8 4\n",
    )
    .expect("write");

    // 3MF: a red box and a blue box side by side.
    let model = r##"<?xml version="1.0" encoding="UTF-8"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <resources>
  <basematerials id="9"><base name="red" displaycolor="#D03030"/><base name="blue" displaycolor="#3060D0"/></basematerials>
  <object id="1" type="model" pid="9" pindex="0"><mesh>
   <vertices>
    <vertex x="0" y="0" z="0"/><vertex x="20" y="0" z="0"/><vertex x="20" y="20" z="0"/><vertex x="0" y="20" z="0"/>
    <vertex x="0" y="0" z="20"/><vertex x="20" y="0" z="20"/><vertex x="20" y="20" z="20"/><vertex x="0" y="20" z="20"/>
   </vertices>
   <triangles>
    <triangle v1="0" v2="2" v3="1"/><triangle v1="0" v2="3" v3="2"/><triangle v1="4" v2="5" v3="6"/><triangle v1="4" v2="6" v3="7"/>
    <triangle v1="0" v2="1" v3="5"/><triangle v1="0" v2="5" v3="4"/><triangle v1="1" v2="2" v3="6"/><triangle v1="1" v2="6" v3="5"/>
    <triangle v1="2" v2="3" v3="7"/><triangle v1="2" v2="7" v3="6"/><triangle v1="3" v2="0" v3="4"/><triangle v1="3" v2="4" v3="7"/>
   </triangles>
  </mesh></object>
  <object id="2" type="model"><components><component objectid="1"/></components></object>
 </resources>
 <build>
  <item objectid="1"/>
  <item objectid="1" transform="1 0 0 0 1 0 0 0 1.5 30 0 0"/>
 </build>
</model>"##;
    // The second box is blue: a copy of the mesh object with the other
    // material (a component keeps the material of the mesh it uses).
    let start = model.find(r#"<object id="1""#).expect("object 1");
    let end = start + model[start..].find("</object>").expect("end") + "</object>".len();
    let blue = model[start..end]
        .replacen(r#"id="1""#, r#"id="3""#, 1)
        .replacen(r#"pindex="0""#, r#"pindex="1""#, 1);
    let model = model
        .replace(
            r#"<item objectid="1" transform"#,
            r#"<item objectid="3" transform"#,
        )
        .replace("</resources>", &format!("{blue}\n </resources>"));
    fs::write(
        dir.join("boxes.3mf"),
        zip_stored(&[("3D/3dmodel.model", model.as_bytes())]),
    )
    .expect("write");
}

/// Minimal ZIP writer (stored entries) for the 3MF sample.
fn zip_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let off = out.len() as u32;
        let crc = crc32fast::hash(data);
        let common = |h: &mut Vec<u8>| {
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&0u32.to_le_bytes());
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
        };
        out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        common(&mut out);
        out.extend_from_slice(name.as_bytes());
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&20u16.to_le_bytes());
        common(&mut central);
        central.extend_from_slice(&[0; 6]);
        central.extend_from_slice(&0u32.to_le_bytes());
        central.extend_from_slice(&off.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd.to_le_bytes());
    out.extend_from_slice(&[0, 0]);
    out
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

/// Documents for the Space preview: a two-page PDF, text in UTF-8 and in
/// Windows-1251, and a minimal DOCX (shown by Word's preview handler).
fn documents(dir: &Path) {
    println!("documents…");
    // PDF: objects written in order, the cross-reference table from their
    // byte offsets.
    let pages = [
        "Lightbox - PDF, page 1",
        "Page 2: the second page of the sample",
    ];
    let mut objects: Vec<String> = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".into(),
        format!(
            "<< /Type /Pages /Kids [{}] /Count {} >>",
            (0..pages.len())
                .map(|i| format!("{} 0 R", 4 + i * 2))
                .collect::<Vec<_>>()
                .join(" "),
            pages.len()
        ),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".into(),
    ];
    for (i, text) in pages.iter().enumerate() {
        let content = format!(
            "BT /F1 28 Tf 72 720 Td ({text}) Tj ET\n\
             0.2 0.4 0.9 rg 72 300 451 300 re f\n\
             BT /F1 14 Tf 72 260 Td (A4, vector text and a filled rectangle) Tj ET"
        );
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] \
             /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            5 + i * 2
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ));
    }
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, o) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for o in offsets {
        pdf.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    fs::write(dir.join("sample.pdf"), pdf).expect("pdf");

    let text: String = (1..=150)
        .map(|i| format!("{i:3}. Строка текста для предпросмотра по пробелу — Lightbox.\n"))
        .collect();
    fs::write(dir.join("text_utf8.txt"), &text).expect("txt");
    // The same in Windows-1251 (Cyrillic letters only map to one byte).
    let cp1251: Vec<u8> = text
        .chars()
        .map(|c| match c {
            'А'..='я' => (c as u32 - 'А' as u32 + 0xC0) as u8,
            'Ё' => 0xA8,
            'ё' => 0xB8,
            '—' => 0x97,
            c if c.is_ascii() => c as u8,
            _ => b'?',
        })
        .collect();
    fs::write(dir.join("text_cp1251.txt"), cp1251).expect("txt");
    fs::write(
        dir.join("code.rs"),
        "// Code is shown in a monospaced font.\nfn main() {\n\tprintln!(\"Привет, Lightbox\");\n}\n",
    )
    .expect("code");

    let document = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<w:document xmlns:w="http://schemas.openxmlformats.org/wordprocessingml/2006/main"><w:body><w:p><w:r><w:t>Lightbox — DOCX для предпросмотра</w:t></w:r></w:p></w:body></w:document>"#;
    let types = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/word/document.xml" ContentType="application/vnd.openxmlformats-officedocument.wordprocessingml.document.main+xml"/></Types>"#;
    let rels = r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="word/document.xml"/></Relationships>"#;
    fs::write(
        dir.join("sample.docx"),
        zip_stored(&[
            ("[Content_Types].xml", types.as_bytes()),
            ("_rels/.rels", rels.as_bytes()),
            ("word/document.xml", document.as_bytes()),
        ]),
    )
    .expect("docx");
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
    models(&dir);
    documents(&dir);
    let huge = dir.join("huge_30000x20000.png");
    if !huge.exists() {
        println!("huge_30000x20000.png (takes a while)…");
        huge_png(&huge);
    }

    println!("done: {}", dir.display());
}
