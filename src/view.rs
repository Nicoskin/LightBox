//! View geometry in f64: fitting, zoom and the visible-quad computation.
//!
//! All math happens on the CPU in f64; the GPU only receives the already
//! clipped on-screen rectangle and its UV range. This keeps the image stable
//! (no jitter) even at 50 000 % zoom, where f32 screen coordinates of the
//! full image would be enormous.

/// Screen-space rectangle in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Rect {
    pub x0: f64,
    pub y0: f64,
    pub x1: f64,
    pub y1: f64,
}

impl Rect {
    pub fn width(&self) -> f64 {
        self.x1 - self.x0
    }
    pub fn height(&self) -> f64 {
        self.y1 - self.y0
    }
}

/// Scale that fits `img` into `area`, never enlarging past 100 %.
pub fn fit_scale(img_w: f64, img_h: f64, area_w: f64, area_h: f64) -> f64 {
    if img_w <= 0.0 || img_h <= 0.0 || area_w <= 0.0 || area_h <= 0.0 {
        return 1.0;
    }
    (area_w / img_w).min(area_h / img_h).min(1.0)
}

/// Current view: zoom factor and the image point shown at the viewport centre.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct View {
    /// Screen pixels per image pixel.
    pub scale: f64,
    /// Image-space point (display orientation) at the viewport centre.
    pub center: (f64, f64),
}

impl View {
    /// View that fits the image into the viewport with a margin on each side.
    pub fn fit(img: (f64, f64), viewport: (f64, f64), margin: f64) -> Self {
        let s = fit_scale(
            img.0,
            img.1,
            viewport.0 - 2.0 * margin,
            viewport.1 - 2.0 * margin,
        );
        Self {
            scale: s,
            center: (img.0 / 2.0, img.1 / 2.0),
        }
    }

    /// The whole image's rectangle on screen (may extend past the viewport).
    pub fn image_rect(&self, img: (f64, f64), viewport: (f64, f64)) -> Rect {
        // Snap the origin to whole pixels when the image is not magnified so
        // that 100 % views map texels 1:1 onto pixels.
        let mut x0 = viewport.0 / 2.0 - self.center.0 * self.scale;
        let mut y0 = viewport.1 / 2.0 - self.center.1 * self.scale;
        if self.scale <= 1.0 {
            x0 = x0.round();
            y0 = y0.round();
        }
        Rect {
            x0,
            y0,
            x1: x0 + img.0 * self.scale,
            y1: y0 + img.1 * self.scale,
        }
    }
}

/// Visible part of the image: on-screen rectangle and matching UV range
/// (display orientation, 0..1), ready for the shader.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Quad {
    pub dst: [f32; 4],
    pub uv: [f32; 4],
}

/// Clips the image rectangle to the viewport. `None` if nothing is visible.
pub fn visible_quad(image: Rect, viewport: (f64, f64)) -> Option<Quad> {
    let x0 = image.x0.max(0.0);
    let y0 = image.y0.max(0.0);
    let x1 = image.x1.min(viewport.0);
    let y1 = image.y1.min(viewport.1);
    if x1 <= x0 || y1 <= y0 {
        return None;
    }
    let (w, h) = (image.width(), image.height());
    let u = |x: f64| ((x - image.x0) / w) as f32;
    let v = |y: f64| ((y - image.y0) / h) as f32;
    Some(Quad {
        dst: [x0 as f32, y0 as f32, x1 as f32, y1 as f32],
        uv: [u(x0), v(y0), u(x1), v(y1)],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fit_never_upscales() {
        assert_eq!(fit_scale(100.0, 50.0, 1920.0, 1080.0), 1.0);
        assert_eq!(fit_scale(6000.0, 4000.0, 1920.0, 1080.0), 0.27);
        assert_eq!(fit_scale(4000.0, 6000.0, 1920.0, 1080.0), 0.18);
    }

    #[test]
    fn fit_is_centered() {
        let v = View::fit((6000.0, 4000.0), (1920.0, 1080.0), 0.0);
        let r = v.image_rect((6000.0, 4000.0), (1920.0, 1080.0));
        assert_eq!(r.y0, 0.0);
        assert_eq!(r.y1, 1080.0);
        assert_eq!(r.x0, 150.0);
        assert_eq!(r.x1, 1770.0);
    }

    #[test]
    fn fully_visible_quad_has_full_uv() {
        let r = Rect {
            x0: 10.0,
            y0: 20.0,
            x1: 110.0,
            y1: 70.0,
        };
        let q = visible_quad(r, (200.0, 100.0)).unwrap();
        assert_eq!(q.dst, [10.0, 20.0, 110.0, 70.0]);
        assert_eq!(q.uv, [0.0, 0.0, 1.0, 1.0]);
    }

    #[test]
    fn clipped_quad_uv_matches() {
        let r = Rect {
            x0: -100.0,
            y0: 0.0,
            x1: 100.0,
            y1: 100.0,
        };
        let q = visible_quad(r, (50.0, 100.0)).unwrap();
        assert_eq!(q.dst, [0.0, 0.0, 50.0, 100.0]);
        assert_eq!(q.uv, [0.5, 0.0, 0.75, 1.0]);
    }

    #[test]
    fn offscreen_is_none() {
        let r = Rect {
            x0: 300.0,
            y0: 0.0,
            x1: 400.0,
            y1: 100.0,
        };
        assert!(visible_quad(r, (200.0, 100.0)).is_none());
    }

    #[test]
    fn huge_zoom_stays_precise() {
        // 50 000 % on a 30 000 px wide image: the image is 15 M px wide on
        // screen, yet the visible UV window must still be exact.
        let img = (30_000.0, 20_000.0);
        let v = View {
            scale: 500.0,
            center: (12_345.5, 6_789.25),
        };
        let vp = (1000.0, 1000.0);
        let q = visible_quad(v.image_rect(img, vp), vp).unwrap();
        assert_eq!(q.dst, [0.0, 0.0, 1000.0, 1000.0]);
        let u0 = f64::from(q.uv[0]) * img.0;
        let u1 = f64::from(q.uv[2]) * img.0;
        assert!((u0 - (12_345.5 - 1.0)).abs() < 0.01, "{u0}");
        assert!((u1 - (12_345.5 + 1.0)).abs() < 0.01, "{u1}");
    }
}
