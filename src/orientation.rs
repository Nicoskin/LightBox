//! Image orientation: the 8 combinations of 90° rotations and a mirror,
//! numbered as in EXIF (1 = as stored).
//!
//! Each orientation is a mapping from display UV to stored (texture) UV —
//! exactly what the shader applies — so composing a user rotation with the
//! EXIF orientation is composing those mappings.

/// EXIF-style orientation, always in `1..=8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Orientation(u8);

impl Default for Orientation {
    fn default() -> Self {
        Self(1)
    }
}

impl Orientation {
    /// As stored.
    pub const IDENTITY: Self = Self(1);
    /// Turn the displayed image 90° clockwise.
    pub const ROTATE_CW: Self = Self(6);
    /// Turn the displayed image 90° counter-clockwise.
    pub const ROTATE_CCW: Self = Self(8);
    /// Mirror the displayed image left ↔ right.
    pub const FLIP_H: Self = Self(2);
    /// Mirror the displayed image top ↔ bottom.
    pub const FLIP_V: Self = Self(4);

    /// From an EXIF tag value; anything out of range means "as stored".
    pub fn from_exif(v: u32) -> Self {
        match v {
            1..=8 => Self(v as u8),
            _ => Self(1),
        }
    }

    pub fn exif(self) -> u8 {
        self.0
    }

    /// `true` for the 90°/270° variants, which swap width and height.
    pub fn swaps_axes(self) -> bool {
        self.0 >= 5
    }

    /// Size of a `w × h` stored image after applying this orientation.
    pub fn apply_size(self, w: u32, h: u32) -> (u32, u32) {
        if self.swaps_axes() { (h, w) } else { (w, h) }
    }

    /// Rectangle `[x0, y0, x1, y1]` in stored pixels → the same area in
    /// display pixels, for an image of `stored` size.
    pub fn stored_rect_to_display(self, r: [f64; 4], stored: (f64, f64)) -> [f64; 4] {
        let (dw, dh) = if self.swaps_axes() {
            (stored.1, stored.0)
        } else {
            stored
        };
        let map = |sx: f64, sy: f64| match self.0 {
            2 => (dw - sx, sy),
            3 => (dw - sx, dh - sy),
            4 => (sx, dh - sy),
            5 => (sy, sx),
            6 => (dw - sy, sx),
            7 => (dw - sy, dh - sx),
            8 => (sy, dh - sx),
            _ => (sx, sy),
        };
        let (a, b) = map(r[0], r[1]);
        let (c, d) = map(r[2], r[3]);
        [a.min(c), b.min(d), a.max(c), b.max(d)]
    }

    /// Display point → stored point, both normalised to 0..1 (same table as
    /// `orient()` in image.hlsl).
    pub fn display_to_stored(self, (x, y): (f64, f64)) -> (f64, f64) {
        match self.0 {
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

    /// Display UV → stored UV (same table as `orient()` in image.hlsl).
    fn map(self, (x, y): (i8, i8)) -> (i8, i8) {
        match self.0 {
            2 => (1 - x, y),
            3 => (1 - x, 1 - y),
            4 => (x, 1 - y),
            5 => (y, x),
            6 => (y, 1 - x),
            7 => (1 - y, 1 - x),
            8 => (1 - y, x),
            _ => (x, y),
        }
    }

    /// The operation that undoes this one.
    pub fn inverse(self) -> Orientation {
        (1..=8)
            .map(Orientation)
            .find(|o| self.then(*o) == Orientation::IDENTITY)
            .unwrap_or(Orientation::IDENTITY)
    }

    /// This orientation followed by the display operation `op` (e.g.
    /// [`Self::ROTATE_CW`]): the image as shown now, transformed by `op`.
    pub fn then(self, op: Orientation) -> Orientation {
        // Three corners determine an affine map of the unit square.
        let probe = [(0, 0), (1, 0), (0, 1)];
        let want = probe.map(|p| self.map(op.map(p)));
        (1..=8)
            .map(Orientation)
            .find(|o| probe.map(|p| o.map(p)) == want)
            .unwrap_or(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        assert_eq!(Orientation::from_exif(1).apply_size(6, 4), (6, 4));
        assert_eq!(Orientation::from_exif(3).apply_size(6, 4), (6, 4));
        assert_eq!(Orientation::from_exif(6).apply_size(6, 4), (4, 6));
        assert_eq!(Orientation::from_exif(8).apply_size(6, 4), (4, 6));
        assert_eq!(Orientation::from_exif(0), Orientation::default());
        assert_eq!(Orientation::from_exif(9), Orientation::default());
    }

    #[test]
    fn rotations_compose() {
        let id = Orientation::IDENTITY;
        assert_eq!(id.then(Orientation::ROTATE_CW), Orientation::from_exif(6));
        assert_eq!(id.then(Orientation::ROTATE_CCW), Orientation::from_exif(8));
        for start in 1..=8 {
            let o = Orientation::from_exif(start);
            let mut r = o;
            for _ in 0..4 {
                r = r.then(Orientation::ROTATE_CW);
            }
            assert_eq!(r, o, "4× CW from {start}");
            assert_eq!(
                o.then(Orientation::ROTATE_CW).then(Orientation::ROTATE_CCW),
                o
            );
            assert_eq!(o.then(Orientation::FLIP_H).then(Orientation::FLIP_H), o);
            assert_eq!(o.then(o.inverse()), Orientation::IDENTITY);
            assert_eq!(o.inverse().then(o), Orientation::IDENTITY);
            assert_eq!(o.then(Orientation::FLIP_V).then(Orientation::FLIP_V), o);
            // Two 90° turns equal a 180° turn equal both mirrors.
            assert_eq!(
                o.then(Orientation::ROTATE_CW).then(Orientation::ROTATE_CW),
                o.then(Orientation::FLIP_H).then(Orientation::FLIP_V)
            );
        }
    }

    #[test]
    fn display_points_map_back_to_stored() {
        let stored = (6.0, 4.0);
        for e in 1..=8 {
            let o = Orientation::from_exif(e);
            // The stored pixel (1, 2) and where it is displayed.
            let r = o.stored_rect_to_display([1.0, 2.0, 2.0, 3.0], stored);
            let (dw, dh) = if o.swaps_axes() {
                (4.0, 6.0)
            } else {
                (6.0, 4.0)
            };
            let c = ((r[0] + r[2]) / 2.0 / dw, (r[1] + r[3]) / 2.0 / dh);
            let (u, v) = o.display_to_stored(c);
            assert!(
                (u * 6.0 - 1.5).abs() < 1e-9 && (v * 4.0 - 2.5).abs() < 1e-9,
                "{e}"
            );
        }
    }

    #[test]
    fn stored_rects_map_like_pixels() {
        // The stored top-left pixel of a 4×3 image, displayed with each
        // orientation, lands where orient_pixels puts it.
        for code in 1..=8u32 {
            let o = Orientation::from_exif(code);
            let (px, dw, _) =
                crate::encode::orient_pixels(&(0..12u8).collect::<Vec<_>>(), 4, 3, 1, o);
            let at = px.iter().position(|&v| v == 0).unwrap();
            let (x, y) = ((at % dw) as f64, (at / dw) as f64);
            let r = o.stored_rect_to_display([0.0, 0.0, 1.0, 1.0], (4.0, 3.0));
            assert_eq!(r, [x, y, x + 1.0, y + 1.0], "code {code}");
        }
    }

    #[test]
    fn exif_6_rotated_back_is_upright() {
        // A photo stored sideways (EXIF 6) turned CCW by the user shows as stored.
        assert_eq!(
            Orientation::from_exif(6).then(Orientation::ROTATE_CCW),
            Orientation::IDENTITY
        );
    }
}
