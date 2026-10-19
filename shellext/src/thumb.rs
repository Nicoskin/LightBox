//! Thumbnail pixels for a model file: the preview a slicer saved in a 3MF,
//! else the mesh drawn by the viewer's CPU rasteriser.

use std::io::Cursor;

use png::{ColorType, Decoder, Limits, Transformations};

use crate::model;

/// Larger files are not read: a mesh takes several times its file size in
/// memory, and Explorer would wait for it.
pub const MAX_FILE: u64 = 256 << 20;
/// Largest thumbnail edge (Explorer asks for up to 256, more on huge icons).
const MAX_SIZE: u32 = 1024;
/// Largest embedded preview decoded.
const MAX_PREVIEW: u32 = 4096;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Stl,
    Obj,
    ThreeMf,
}

impl Kind {
    fn ext(self) -> &'static str {
        match self {
            Kind::Stl => "stl",
            Kind::Obj => "obj",
            Kind::ThreeMf => "3mf",
        }
    }

    /// From the file name's extension, else from the content.
    pub fn detect(name: Option<&str>, data: &[u8]) -> Kind {
        let ext = name
            .and_then(|n| n.rsplit_once('.'))
            .map(|(_, e)| e.to_ascii_lowercase());
        match ext.as_deref() {
            Some("stl") => return Kind::Stl,
            Some("obj") => return Kind::Obj,
            Some("3mf") => return Kind::ThreeMf,
            _ => {}
        }
        let binary_stl = data
            .get(80..84)
            .map(|c| 84 + 50 * u32::from_le_bytes([c[0], c[1], c[2], c[3]]) as u64)
            == Some(data.len() as u64);
        if data.starts_with(b"PK\x03\x04") {
            Kind::ThreeMf
        } else if binary_stl
            || data.trim_ascii_start().starts_with(b"solid")
            || data[..data.len().min(4096)].contains(&0)
        {
            Kind::Stl
        } else {
            Kind::Obj
        }
    }
}

/// Premultiplied BGRA, top row first: what a 32-bit DIB section holds.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Image {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

impl Image {
    /// From straight-alpha RGBA.
    pub fn from_rgba(width: u32, height: u32, rgba: &[u8]) -> Image {
        let bgra = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[r, g, b, a]| {
                let pm = |c: u8| ((u32::from(c) * u32::from(a) + 127) / 255) as u8;
                [pm(b), pm(g), pm(r), a]
            })
            .collect();
        Image {
            width,
            height,
            bgra,
        }
    }

    /// Shrunk with a box filter so that neither side exceeds `max`, keeping
    /// the aspect ratio; never enlarged.
    pub fn fit(self, max: u32) -> Image {
        let (w, h) = (self.width as usize, self.height as usize);
        let long = w.max(h);
        let max = max.max(1) as usize;
        if long <= max {
            return self;
        }
        let dw = (w * max).div_ceil(long).max(1);
        let dh = (h * max).div_ceil(long).max(1);
        let mut out = Vec::with_capacity(dw * dh * 4);
        for y in 0..dh {
            let (y0, y1) = (y * h / dh, ((y + 1) * h / dh).max(y * h / dh + 1));
            for x in 0..dw {
                let (x0, x1) = (x * w / dw, ((x + 1) * w / dw).max(x * w / dw + 1));
                let mut sum = [0u32; 4];
                for sy in y0..y1 {
                    for px in self.bgra[(sy * w + x0) * 4..(sy * w + x1) * 4]
                        .as_chunks::<4>()
                        .0
                    {
                        for (s, &c) in sum.iter_mut().zip(px) {
                            *s += u32::from(c);
                        }
                    }
                }
                let n = ((y1 - y0) * (x1 - x0)) as u32;
                out.extend(sum.map(|s| ((s + n / 2) / n) as u8));
            }
        }
        Image {
            width: dw as u32,
            height: dh as u32,
            bgra: out,
        }
    }
}

/// The thumbnail of a model file, at most `cx` pixels on the long side.
pub fn render(data: &[u8], kind: Kind, cx: u32) -> Result<Image, String> {
    let cx = cx.clamp(1, MAX_SIZE);
    if kind == Kind::ThreeMf
        && let Some(img) = model::embedded_thumbnail(data).and_then(|b| decode_png(&b))
    {
        return Ok(img.fit(cx));
    }
    let mesh = model::parse(data, kind.ext(), None)?;
    // Drawn at twice the size and shrunk: smooth edges.
    let size = if cx <= MAX_SIZE / 2 { cx * 2 } else { cx };
    let rgba = model::raster::thumbnail(&mesh, size);
    Ok(Image::from_rgba(size, size, &rgba).fit(cx))
}

/// A PNG (8 or 16 bit, any colour type) as an [`Image`]; `None` for other
/// formats (a JPEG preview falls back to rendering the mesh).
fn decode_png(data: &[u8]) -> Option<Image> {
    let mut dec = Decoder::new_with_limits(Cursor::new(data), Limits { bytes: 64 << 20 });
    dec.set_transformations(
        Transformations::EXPAND | Transformations::STRIP_16 | Transformations::ALPHA,
    );
    let mut reader = dec.read_info().ok()?;
    let (w, h) = (reader.info().width, reader.info().height);
    if w == 0 || h == 0 || w > MAX_PREVIEW || h > MAX_PREVIEW {
        return None;
    }
    let mut buf = vec![0u8; reader.output_buffer_size()?];
    let frame = reader.next_frame(&mut buf).ok()?;
    buf.truncate(frame.buffer_size());
    let rgba: Vec<u8> = match frame.color_type {
        ColorType::Rgba => buf,
        ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|&[l, a]| [l, l, l, a])
            .collect(),
        _ => return None,
    };
    (rgba.len() == w as usize * h as usize * 4).then(|| Image::from_rgba(w, h, &rgba))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A PNG of `w` × `h` straight RGBA pixels.
    pub(crate) fn png(w: u32, h: u32, rgba: &[u8]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut enc = png::Encoder::new(&mut out, w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        let mut writer = enc.write_header().unwrap();
        writer.write_image_data(rgba).unwrap();
        writer.finish().unwrap();
        out
    }

    /// A stored ZIP (no CRCs: the reader does not check them).
    fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let off = out.len() as u32;
            let fields = |central: bool| {
                let mut h = Vec::new();
                if central {
                    h.extend_from_slice(&20u16.to_le_bytes()); // made by
                }
                h.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
                h.extend_from_slice(&(data.len() as u32).to_le_bytes());
                h.extend_from_slice(&(data.len() as u32).to_le_bytes());
                h.extend_from_slice(&(name.len() as u16).to_le_bytes());
                h.extend_from_slice(&0u16.to_le_bytes());
                if central {
                    h.extend_from_slice(&[0; 10]);
                    h.extend_from_slice(&off.to_le_bytes());
                }
                h.extend_from_slice(name.as_bytes());
                h
            };
            out.extend_from_slice(b"PK\x03\x04");
            out.extend(fields(false));
            out.extend_from_slice(data);
            central.extend_from_slice(b"PK\x01\x02");
            central.extend(fields(true));
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

    /// A binary STL tetrahedron.
    fn tetrahedron() -> Vec<u8> {
        let v = [[0f32, 0., 0.], [10., 0., 0.], [0., 10., 0.], [0., 0., 10.]];
        let faces = [[0, 2, 1], [0, 1, 3], [0, 3, 2], [1, 2, 3]];
        let mut out = vec![0u8; 80];
        out.extend_from_slice(&(faces.len() as u32).to_le_bytes());
        for f in faces {
            out.extend_from_slice(&[0; 12]);
            for i in f {
                for c in v[i] {
                    out.extend_from_slice(&c.to_le_bytes());
                }
            }
            out.extend_from_slice(&[0, 0]);
        }
        out
    }

    #[test]
    fn premultiplied_bgra() {
        let img = Image::from_rgba(2, 1, &[255, 128, 0, 255, 200, 100, 50, 128]);
        assert_eq!(img.bgra, [0, 128, 255, 255, 25, 50, 100, 128]);
    }

    #[test]
    fn fit_shrinks_keeping_aspect() {
        let img = Image::from_rgba(4, 2, &[255; 4 * 2 * 4]);
        let small = img.clone().fit(2);
        assert_eq!((small.width, small.height), (2, 1));
        assert_eq!(small.bgra, [255; 8]);
        assert_eq!(img.clone().fit(256), img); // never enlarged
        // Averaging: opaque white next to transparent → half-covered white.
        let half = Image::from_rgba(2, 1, &[255, 255, 255, 255, 0, 0, 0, 0]).fit(1);
        assert_eq!(half.bgra, [128, 128, 128, 128]);
    }

    #[test]
    fn detects_the_format() {
        let stl = tetrahedron();
        assert_eq!(Kind::detect(Some("a.STL"), b"v 1 2 3"), Kind::Stl);
        assert_eq!(Kind::detect(Some("x.3mf"), b""), Kind::ThreeMf);
        assert_eq!(Kind::detect(None, &stl), Kind::Stl);
        assert_eq!(Kind::detect(None, b"solid x\nfacet normal"), Kind::Stl);
        assert_eq!(Kind::detect(None, b"PK\x03\x04..."), Kind::ThreeMf);
        assert_eq!(Kind::detect(Some("noext"), b"v 0 0 0\nf 1 2 3"), Kind::Obj);
    }

    #[test]
    fn renders_a_mesh() {
        let img = render(&tetrahedron(), Kind::Stl, 64).unwrap();
        assert_eq!((img.width, img.height), (64, 64));
        let px = |x: u32, y: u32| &img.bgra[((y * 64 + x) * 4) as usize..][..4];
        assert_eq!(px(0, 0), [0, 0, 0, 0]); // transparent background
        let opaque = img.bgra.as_chunks::<4>().0.iter().filter(|p| p[3] == 255);
        assert!(opaque.count() > 64 * 64 / 10);
        assert!(render(b"garbage", Kind::Obj, 64).is_err());
    }

    #[test]
    fn uses_the_3mf_preview() {
        let red = [255, 0, 0, 255].repeat(8 * 4);
        let data = zip(&[
            ("3D/3dmodel.model", b"<model/>"),
            ("Metadata/thumbnail.png", &png(8, 4, &red)),
        ]);
        let img = render(&data, Kind::ThreeMf, 4).unwrap();
        assert_eq!((img.width, img.height), (4, 2));
        assert_eq!(&img.bgra[..4], [0, 0, 255, 255]);
        // Not a PNG: the mesh is drawn instead (here there is none).
        let data = zip(&[("Metadata/thumbnail.png", b"\xFF\xD8 jpeg")]);
        assert!(render(&data, Kind::ThreeMf, 4).is_err());
    }
}
