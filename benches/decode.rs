//! Decoder shoot-out on the 24 Mp test images (run `gen_testdata` first):
//!
//!     cargo bench --bench decode
//!
//! JPEG: libjpeg-turbo (full and DCT-scaled) vs zune-jpeg.
//! PNG:  zune-png vs png.

use std::hint::black_box;
use std::io::Cursor;
use std::path::Path;
use std::time::Duration;

use criterion::{Criterion, criterion_group, criterion_main};
use turbojpeg::{Decompressor, Image, PixelFormat, ScalingFactor};
use zune_core::bytestream::ZCursor;
use zune_core::colorspace::ColorSpace;
use zune_core::options::DecoderOptions;

fn load(name: &str) -> Option<Vec<u8>> {
    let p = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("testdata")
        .join(name);
    match std::fs::read(&p) {
        Ok(d) => Some(d),
        Err(_) => {
            eprintln!(
                "missing {}, run: cargo run --release --example gen_testdata",
                p.display()
            );
            None
        }
    }
}

fn turbo(data: &[u8], factor: ScalingFactor) -> Vec<u8> {
    let mut d = Decompressor::new().unwrap();
    let h = d.read_header(data).unwrap().scaled(factor);
    d.set_scaling_factor(factor).unwrap();
    let mut out = vec![0u8; h.width * h.height * 4];
    d.decompress(
        data,
        Image {
            pixels: out.as_mut_slice(),
            width: h.width,
            pitch: h.width * 4,
            height: h.height,
            format: PixelFormat::RGBA,
        },
    )
    .unwrap();
    out
}

fn jpeg(c: &mut Criterion) {
    for file in ["photo_24mp.jpg", "photo_24mp_progressive.jpg"] {
        let Some(data) = load(file) else { return };
        let mut g = c.benchmark_group(format!("jpeg/{file}"));
        g.sample_size(15).measurement_time(Duration::from_secs(8));
        for (name, f) in [
            ("turbojpeg 1/1", ScalingFactor::ONE),
            ("turbojpeg 1/2", ScalingFactor::ONE_HALF),
            ("turbojpeg 1/4", ScalingFactor::ONE_QUARTER),
            ("turbojpeg 1/8", ScalingFactor::ONE_EIGHTH),
        ] {
            g.bench_function(name, |b| b.iter(|| black_box(turbo(&data, f))));
        }
        g.bench_function("zune-jpeg 1/1", |b| {
            b.iter(|| {
                let opts = DecoderOptions::default().jpeg_set_out_colorspace(ColorSpace::RGBA);
                let mut d = zune_jpeg::JpegDecoder::new_with_options(ZCursor::new(&data), opts);
                black_box(d.decode().unwrap())
            })
        });
        g.finish();
    }
}

fn png(c: &mut Criterion) {
    let Some(data) = load("photo_24mp.png") else {
        return;
    };
    let mut g = c.benchmark_group("png/photo_24mp.png");
    g.sample_size(10).measurement_time(Duration::from_secs(10));
    g.bench_function("zune-png", |b| {
        b.iter(|| {
            let opts = DecoderOptions::default().png_set_add_alpha_channel(true);
            let mut d = zune_png::PngDecoder::new_with_options(ZCursor::new(&data), opts);
            black_box(d.decode_raw().unwrap())
        })
    });
    g.bench_function("png", |b| {
        b.iter(|| {
            let mut d = png::Decoder::new(Cursor::new(&data));
            d.set_transformations(png::Transformations::ALPHA);
            let mut r = d.read_info().unwrap();
            let mut buf = vec![0u8; r.output_buffer_size().unwrap()];
            r.next_frame(&mut buf).unwrap();
            black_box(buf)
        })
    });
    g.finish();
}

criterion_group!(benches, jpeg, png);
criterion_main!(benches);
