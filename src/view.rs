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
    pub fn center(&self) -> (f64, f64) {
        ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0)
    }
    pub fn contains(&self, p: (f64, f64)) -> bool {
        p.0 >= self.x0 && p.0 < self.x1 && p.1 >= self.y0 && p.1 < self.y1
    }
    pub fn offset(&self, dx: f64, dy: f64) -> Self {
        Self {
            x0: self.x0 + dx,
            y0: self.y0 + dy,
            x1: self.x1 + dx,
            y1: self.y1 + dy,
        }
    }
    pub fn lerp(&self, to: &Rect, t: f64) -> Self {
        let l = |a: f64, b: f64| a + (b - a) * t;
        Self {
            x0: l(self.x0, to.x0),
            y0: l(self.y0, to.y0),
            x1: l(self.x1, to.x1),
            y1: l(self.y1, to.y1),
        }
    }
}

/// Zoom limits: 1 % … 50 000 %.
pub const MIN_SCALE: f64 = 0.01;
pub const MAX_SCALE: f64 = 500.0;

/// Scale that fits `img` into `area`, never enlarging past 100 %.
pub fn fit_scale(img_w: f64, img_h: f64, area_w: f64, area_h: f64) -> f64 {
    fill_scale(img_w, img_h, area_w, area_h).min(1.0)
}

/// Scale that fits `img` into `area`, enlarging if needed.
pub fn fill_scale(img_w: f64, img_h: f64, area_w: f64, area_h: f64) -> f64 {
    if img_w <= 0.0 || img_h <= 0.0 || area_w <= 0.0 || area_h <= 0.0 {
        return 1.0;
    }
    (area_w / img_w).min(area_h / img_h)
}

/// Size of the window in windowed mode for an image of `img` pixels on a
/// monitor work area of `work`: 100 % if it fits, otherwise fitted into
/// 90 % of the work area, keeping the aspect ratio. Tiny images are
/// enlarged so that the longer side is at least `min` (a usable window).
pub fn windowed_size(img: (f64, f64), work: (f64, f64), min: f64) -> (f64, f64) {
    let mut s = if img.0 <= work.0 && img.1 <= work.1 {
        1.0
    } else {
        fill_scale(img.0, img.1, work.0 * 0.9, work.1 * 0.9)
    };
    if img.0.max(img.1) * s < min {
        s = min / img.0.max(img.1);
    }
    ((img.0 * s).round().max(1.0), (img.1 * s).round().max(1.0))
}

/// Places a `size` rectangle centred on `center`, shifted to stay inside
/// `bounds` (if it is larger than `bounds`, its top-left corner wins).
pub fn place_within(size: (f64, f64), center: (f64, f64), bounds: Rect) -> Rect {
    let clamp = |c: f64, len: f64, lo: f64, hi: f64| {
        let start = (c - len / 2.0).round();
        start.min(hi - len).max(lo)
    };
    let x0 = clamp(center.0, size.0, bounds.x0, bounds.x1);
    let y0 = clamp(center.1, size.1, bounds.y0, bounds.y1);
    Rect {
        x0,
        y0,
        x1: x0 + size.0,
        y1: y0 + size.1,
    }
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
    #[cfg(test)]
    /// View that fits the image into the viewport with a margin on each side,
    /// enlarging small images only with `upscale`.
    pub fn fit(img: (f64, f64), viewport: (f64, f64), margin: f64, upscale: bool) -> Self {
        let f = if upscale { fill_scale } else { fit_scale };
        let s = f(
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

    /// View showing the whole image exactly inside screen rectangle `r`.
    pub fn placed(img: (f64, f64), r: Rect, viewport: (f64, f64)) -> Self {
        let scale = r.width() / img.0;
        Self {
            scale,
            center: (
                (viewport.0 / 2.0 - r.x0) / scale,
                (viewport.1 / 2.0 - r.y0) / scale,
            ),
        }
    }

    /// Image point under screen position `p`.
    pub fn image_point(&self, p: (f64, f64), viewport: (f64, f64)) -> (f64, f64) {
        (
            self.center.0 + (p.0 - viewport.0 / 2.0) / self.scale,
            self.center.1 + (p.1 - viewport.1 / 2.0) / self.scale,
        )
    }

    /// Zooms to `scale` keeping the image point under `anchor` (screen) fixed.
    pub fn zoom_at(&self, scale: f64, anchor: (f64, f64), viewport: (f64, f64)) -> Self {
        let scale = scale.clamp(MIN_SCALE, MAX_SCALE);
        let p = self.image_point(anchor, viewport);
        Self {
            scale,
            center: (
                p.0 - (anchor.0 - viewport.0 / 2.0) / scale,
                p.1 - (anchor.1 - viewport.1 / 2.0) / scale,
            ),
        }
    }

    /// Moves the image by `(dx, dy)` screen pixels.
    pub fn pan(&self, dx: f64, dy: f64) -> Self {
        Self {
            scale: self.scale,
            center: (
                self.center.0 - dx / self.scale,
                self.center.1 - dy / self.scale,
            ),
        }
    }

    #[cfg(test)]
    /// Keeps the image on screen: an axis that fits is centred, an axis
    /// larger than the viewport may not reveal empty space at its edges.
    pub fn clamped(&self, img: (f64, f64), viewport: (f64, f64)) -> Self {
        let full = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: viewport.0,
            y1: viewport.1,
        };
        self.clamped_in(img, full, viewport)
    }

    /// Like [`Self::clamped`], but an axis that fits inside `area` (the part
    /// of the screen not covered by the panel) is centred in `area`.
    pub fn clamped_in(&self, img: (f64, f64), area: Rect, viewport: (f64, f64)) -> Self {
        let s = self.scale;
        let axis = |c: f64, len: f64, vp: f64, a0: f64, a1: f64| {
            let half = vp / 2.0 / s;
            let size = len * s;
            if size <= a1 - a0 {
                len / 2.0 + (vp / 2.0 - (a0 + a1) / 2.0) / s
            } else if size <= vp {
                // Larger than the area but fits the screen: stay on screen.
                c.clamp(len - half, half)
            } else {
                c.clamp(half, len - half)
            }
        };
        Self {
            scale: s,
            center: (
                axis(self.center.0, img.0, viewport.0, area.x0, area.x1),
                axis(self.center.1, img.1, viewport.1, area.y0, area.y1),
            ),
        }
    }

    /// Fits the image into `area` (centred there), enlarging only with `upscale`.
    pub fn fit_in(img: (f64, f64), area: Rect, viewport: (f64, f64), upscale: bool) -> Self {
        let f = if upscale { fill_scale } else { fit_scale };
        let scale = f(img.0, img.1, area.width(), area.height());
        let (ax, ay) = area.center();
        Self {
            scale,
            center: (
                img.0 / 2.0 + (viewport.0 / 2.0 - ax) / scale,
                img.1 / 2.0 + (viewport.1 / 2.0 - ay) / scale,
            ),
        }
    }

    /// Animation step between two views; `t` in 0..1. The scale moves
    /// geometrically (perceptually even zoom). With an `anchor`, the image
    /// point under it stays put, as with [`Self::zoom_at`].
    pub fn interpolate(
        &self,
        to: &View,
        t: f64,
        anchor: Option<(f64, f64)>,
        viewport: (f64, f64),
    ) -> View {
        let scale = self.scale * (to.scale / self.scale).powf(t);
        let lin = |a: f64, b: f64| a + (b - a) * t;
        match anchor {
            Some(a) => {
                let anchored = |v: &View, s: f64| v.zoom_at(s, a, viewport).center;
                let c = anchored(self, scale);
                // Blend in the correction made by clamping the target.
                let end = anchored(self, to.scale);
                View {
                    scale,
                    center: (
                        c.0 + (to.center.0 - end.0) * t,
                        c.1 + (to.center.1 - end.1) * t,
                    ),
                }
            }
            None => View {
                scale,
                center: (
                    lin(self.center.0, to.center.0),
                    lin(self.center.1, to.center.1),
                ),
            },
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
        assert_eq!(fill_scale(100.0, 50.0, 1920.0, 1080.0), 19.2);
        assert_eq!(fit_scale(6000.0, 4000.0, 1920.0, 1080.0), 0.27);
        assert_eq!(fit_scale(4000.0, 6000.0, 1920.0, 1080.0), 0.18);
    }

    #[test]
    fn zoom_keeps_anchor_fixed() {
        let img = (4000.0, 3000.0);
        let vp = (1000.0, 800.0);
        let v = View::fit(img, vp, 0.0, false);
        let anchor = (123.0, 456.0);
        let p = v.image_point(anchor, vp);
        let z = v.zoom_at(v.scale * 7.3, anchor, vp);
        let q = z.image_point(anchor, vp);
        assert!((p.0 - q.0).abs() < 1e-9 && (p.1 - q.1).abs() < 1e-9);
        assert_eq!(v.zoom_at(1e9, anchor, vp).scale, MAX_SCALE);
        assert_eq!(v.zoom_at(0.0, anchor, vp).scale, MIN_SCALE);
    }

    #[test]
    fn interpolation_endpoints_and_anchor() {
        let img = (4000.0, 3000.0);
        let vp = (1000.0, 800.0);
        let a = View::fit(img, vp, 0.0, false);
        let anchor = (300.0, 200.0);
        let b = a.zoom_at(2.0, anchor, vp);
        let p = a.image_point(anchor, vp);
        for t in [0.0, 0.3, 0.7, 1.0] {
            let v = a.interpolate(&b, t, Some(anchor), vp);
            let q = v.image_point(anchor, vp);
            assert!(
                (p.0 - q.0).abs() < 1e-6 && (p.1 - q.1).abs() < 1e-6,
                "t={t}"
            );
        }
        let end = a.interpolate(&b, 1.0, None, vp);
        assert!((end.scale - b.scale).abs() < 1e-12);
        assert!((end.center.0 - b.center.0).abs() < 1e-9);
    }

    #[test]
    fn clamp_centres_small_and_limits_large() {
        let vp = (1000.0, 800.0);
        let small = View {
            scale: 0.1,
            center: (0.0, 0.0),
        }
        .clamped((4000.0, 3000.0), vp);
        assert_eq!(small.center, (2000.0, 1500.0));
        let big = View {
            scale: 1.0,
            center: (-500.0, 99_999.0),
        }
        .clamped((4000.0, 3000.0), vp);
        assert_eq!(big.center, (500.0, 2600.0));
    }

    #[test]
    fn fit_in_area_and_clamp_agree() {
        let vp = (2000.0, 1000.0);
        let area = Rect {
            x0: 20.0,
            y0: 20.0,
            x1: 1980.0,
            y1: 850.0,
        };
        let img = (4000.0, 3000.0);
        let v = View::fit_in(img, area, vp, false);
        let r = v.image_rect(img, vp);
        assert!((r.center().1 - 435.0).abs() < 1.0, "{r:?}");
        assert!(r.y0 >= 19.0 && r.y1 <= 851.0);
        // Clamping a fitted view leaves it unchanged.
        let c = v.clamped_in(img, area, vp);
        assert!((c.center.0 - v.center.0).abs() < 1e-9 && (c.center.1 - v.center.1).abs() < 1e-9);
        // Bigger than the area but smaller than the screen: no panic, on screen.
        let mid = View {
            scale: 0.3,
            center: (0.0, 0.0),
        }
        .clamped_in(img, area, vp);
        let r = mid.image_rect(img, vp);
        assert!(r.y0 >= -1e-9 && r.y1 <= 1000.0 + 1e-9, "{r:?}");
    }

    #[test]
    fn pan_moves_image_with_cursor() {
        let v = View {
            scale: 2.0,
            center: (100.0, 100.0),
        };
        let vp = (400.0, 400.0);
        let before = v.image_rect((1000.0, 1000.0), vp);
        let after = v.pan(10.0, -4.0).image_rect((1000.0, 1000.0), vp);
        assert_eq!(after.x0 - before.x0, 10.0);
        assert_eq!(after.y0 - before.y0, -4.0);
    }

    #[test]
    fn placed_maps_to_rect() {
        let img = (600.0, 400.0);
        let vp = (1920.0, 1080.0);
        let r = Rect {
            x0: 100.0,
            y0: 50.0,
            x1: 400.0,
            y1: 250.0,
        };
        let got = View::placed(img, r, vp).image_rect(img, vp);
        assert!((got.x0 - r.x0).abs() < 1e-9 && (got.y1 - r.y1).abs() < 1e-9);
    }

    #[test]
    fn windowed_size_rules() {
        let work = (2560.0, 1400.0);
        assert_eq!(windowed_size((800.0, 600.0), work, 160.0), (800.0, 600.0));
        // Too tall: fit into 90 % of the work area (1260 px high).
        assert_eq!(
            windowed_size((4000.0, 6000.0), work, 160.0),
            (840.0, 1260.0)
        );
        assert_eq!(
            windowed_size((6000.0, 4000.0), work, 160.0),
            (1890.0, 1260.0)
        );
        assert_eq!(windowed_size((16.0, 8.0), work, 160.0), (160.0, 80.0));
    }

    #[test]
    fn place_within_bounds() {
        let b = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 1000.0,
            y1: 800.0,
        };
        let r = place_within((200.0, 100.0), (500.0, 400.0), b);
        assert_eq!((r.x0, r.y0), (400.0, 350.0));
        let r = place_within((200.0, 100.0), (980.0, 10.0), b);
        assert_eq!((r.x0, r.y0, r.x1, r.y1), (800.0, 0.0, 1000.0, 100.0));
    }

    #[test]
    fn fit_is_centered() {
        let v = View::fit((6000.0, 4000.0), (1920.0, 1080.0), 0.0, false);
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
