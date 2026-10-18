//! SVG / SVGZ rasterised with resvg. Vector art is rendered large enough to
//! stay sharp when fitted to the screen and moderately zoomed.

use std::sync::{Arc, OnceLock};

use resvg::tiny_skia::{Pixmap, Transform};
use resvg::usvg::{self, fontdb};

use super::{DecodeError, Raw, RawImage, SizeHint};

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

pub fn decode(data: &[u8], hint: Option<SizeHint>) -> Result<RawImage, DecodeError> {
    let opt = usvg::Options {
        fontdb: fonts(),
        ..Default::default()
    };
    let tree =
        usvg::Tree::from_data(data, &opt).map_err(|e| DecodeError::Other(format!("SVG: {e}")))?;
    let size = tree.size();
    let (sw, sh) = (size.width(), size.height());
    if !(sw > 0.0 && sh > 0.0) {
        return Err(DecodeError::Unsupported);
    }
    // At least the screen box (or MIN_SIDE), at most MAX_SIDE.
    let target = hint.map_or(MIN_SIDE, |h| h.box_width.max(h.box_height) as f32);
    let longest = sw.max(sh);
    let k = (target.max(MIN_SIDE).max(longest) / longest).min(MAX_SIDE / longest);
    let (w, h) = (
        (sw * k).round().max(1.0) as u32,
        (sh * k).round().max(1.0) as u32,
    );
    let mut pixmap = Pixmap::new(w, h).ok_or(DecodeError::TooLarge(w, h))?;
    resvg::render(&tree, Transform::from_scale(k, k), &mut pixmap.as_mut());
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
}
