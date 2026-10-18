//! SVG / SVGZ rendered with resvg. The drawing is first rasterised large
//! enough to look sharp when fitted to the screen; when zoomed in further,
//! the app re-renders just the visible region at screen resolution
//! ([`render_region`]), so outlines stay smooth at any zoom.

use std::sync::{Arc, OnceLock};

use resvg::tiny_skia::{Pixmap, Transform};
pub use resvg::usvg::Tree;
use resvg::usvg::{self, fontdb};

use super::{DecodeError, DecodedImage, Raw, RawImage, SizeHint};

/// Longest side a drawing is rendered at, at most.
const MAX_SIDE: f32 = 8192.0;
/// …and at least (for small icons that would otherwise look blocky).
const MIN_SIDE: f32 = 2048.0;

/// System fonts for SVG text, loaded once on first use (~100 ms).
fn fonts() -> Arc<fontdb::Database> {
    static DB: OnceLock<Arc<fontdb::Database>> = OnceLock::new();
    Arc::clone(DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        Arc::new(db)
    }))
}

/// Parses an SVG / SVGZ document.
pub fn parse(data: &[u8]) -> Result<usvg::Tree, DecodeError> {
    let opt = usvg::Options {
        fontdb: fonts(),
        ..Default::default()
    };
    let tree = match usvg::Tree::from_data(data, &opt) {
        Ok(t) => t,
        // Inline SVG copied from web pages often lacks the namespace that
        // browsers do not insist on: add it and try again.
        Err(e) => add_namespace(data)
            .and_then(|fixed| usvg::Tree::from_data(&fixed, &opt).ok())
            .ok_or_else(|| DecodeError::Other(format!("SVG: {e}")))?,
    };
    let size = tree.size();
    if !(size.width() > 0.0 && size.height() > 0.0) {
        return Err(DecodeError::Unsupported);
    }
    Ok(tree)
}

/// The document with `xmlns="http://www.w3.org/2000/svg"` (and the xlink
/// namespace if used) added to its root element; `None` if it has one or
/// is not plain text.
fn add_namespace(data: &[u8]) -> Option<Vec<u8>> {
    let text = std::str::from_utf8(data).ok()?;
    if text.contains("http://www.w3.org/2000/svg") {
        return None;
    }
    let at = text.match_indices("<svg").map(|(i, _)| i + 4).find(|&i| {
        text[i..]
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || c == '>' || c == '/')
    })?;
    let mut s = String::with_capacity(text.len() + 96);
    s.push_str(&text[..at]);
    s.push_str(r#" xmlns="http://www.w3.org/2000/svg""#);
    if text.contains("xlink:") && !text.contains("xmlns:xlink") {
        s.push_str(r#" xmlns:xlink="http://www.w3.org/1999/xlink""#);
    }
    s.push_str(&text[at..]);
    Some(s.into_bytes())
}

pub fn decode(data: &[u8], hint: Option<SizeHint>) -> Result<RawImage, DecodeError> {
    let tree = parse(data)?;
    let size = tree.size();
    let (sw, sh) = (size.width(), size.height());
    // At least the screen box (or MIN_SIDE), at most MAX_SIDE.
    let target = hint.map_or(MIN_SIDE, |h| h.box_width.max(h.box_height) as f32);
    let longest = sw.max(sh);
    let k = (target.max(MIN_SIDE).max(longest) / longest).min(MAX_SIDE / longest);
    let (w, h) = (
        (sw * k).round().max(1.0) as u32,
        (sh * k).round().max(1.0) as u32,
    );
    render(&tree, Transform::from_scale(k, k), w, h)
}

/// Renders `region` (x0, y0, x1, y1 in pixels of a `raster`-sized base
/// rendering of the drawing) into an `out`-sized image.
pub fn render_region(
    tree: &usvg::Tree,
    raster: (u32, u32),
    region: [f64; 4],
    out: (u32, u32),
) -> Result<DecodedImage, DecodeError> {
    // Raster pixels per SVG unit, then output pixels per raster pixel.
    let base = f64::from(raster.0) / f64::from(tree.size().width());
    let k = f64::from(out.0) / (region[2] - region[0]);
    let t = Transform::from_row(
        (base * k) as f32,
        0.0,
        0.0,
        (base * k) as f32,
        (-region[0] * k) as f32,
        (-region[1] * k) as f32,
    );
    super::finish(render(tree, t, out.0, out.1)?, true)
}

/// Renders with `transform` into a `w` × `h` straight-alpha RGBA8 image.
fn render(
    tree: &usvg::Tree,
    transform: Transform,
    w: u32,
    h: u32,
) -> Result<RawImage, DecodeError> {
    let mut pixmap = Pixmap::new(w, h).ok_or(DecodeError::TooLarge(w, h))?;
    resvg::render(tree, transform, &mut pixmap.as_mut());
    // tiny-skia is premultiplied (in sRGB space): back to straight alpha.
    let mut rgba = pixmap.take();
    for p in rgba.as_chunks_mut::<4>().0 {
        let a = u32::from(p[3]);
        if a != 0 && a != 255 {
            for c in &mut p[..3] {
                *c = ((u32::from(*c) * 255 + a / 2) / a).min(255) as u8;
            }
        }
    }
    Ok(RawImage::new(w, h, Raw::Rgba8(rgba)))
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_a_circle() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
            <circle cx="25" cy="25" r="20" fill="#ff0000"/></svg>"##;
        let img = decode(svg, None).unwrap();
        // 100×50 scaled up to the minimum side.
        assert_eq!((img.width, img.height), (2048, 1024));
        let Raw::Rgba8(px) = img.data else { panic!() };
        let at = |x: u32, y: u32| &px[((y * img.width + x) * 4) as usize..][..4];
        assert_eq!(at(512, 512), &[255, 0, 0, 255]); // inside the circle
        assert_eq!(at(1500, 512)[3], 0); // transparent outside
    }

    #[test]
    fn svg_without_namespace() {
        // As copied from a web page: no xmlns, size from the viewBox only.
        let svg = br##"<svg viewBox="0 0 25.4 25.4" aria-hidden="true"><path fill="#111111" d="M0 12.7C0 5.7 5.7 0 12.7 0S25.4 5.7 25.4 12.7 19.7 25.4 12.7 25.4 0 19.7 0 12.7Z"></path></svg>"##;
        let tree = parse(svg).unwrap();
        assert!((tree.size().width() - 25.4).abs() < 1e-3);
        let img = decode(svg, None).unwrap();
        assert_eq!((img.width, img.height), (2048, 2048));
        assert!(add_namespace(br#"<svg xmlns="http://www.w3.org/2000/svg"/>"#).is_none());
    }

    #[test]
    fn region_renders_sharp_edges() {
        let svg = br##"<svg xmlns="http://www.w3.org/2000/svg" width="100" height="50">
            <circle cx="25" cy="25" r="20" fill="#ff0000"/></svg>"##;
        let tree = parse(svg).unwrap();
        // 10 × 10 raster pixels around the circle's left edge (x = 5 units
        // = 102.4 px in a 2048 px raster), rendered 100× larger.
        let img = render_region(
            &tree,
            (2048, 1024),
            [97.0, 507.0, 107.0, 517.0],
            (1000, 1000),
        )
        .unwrap();
        assert_eq!((img.width, img.height), (1000, 1000));
        let crate::decode::Pixels::Srgb8(px) = &img.pixels else {
            panic!()
        };
        let alpha = |x: u32| px[((500 * 1000 + x) * 4 + 3) as usize];
        // Edge at 102.4 px → 540 px in the output; inside opaque, outside clear.
        assert_eq!(alpha(10), 0);
        assert_eq!(alpha(990), 255);
        // Anti-aliasing spans about one output pixel, not 100.
        let soft = (0..1000).filter(|&x| (1..255).contains(&alpha(x))).count();
        assert!(soft <= 3, "{soft}");
    }
}
