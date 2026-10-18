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
    fn exif_6_rotated_back_is_upright() {
        // A photo stored sideways (EXIF 6) turned CCW by the user shows as stored.
        assert_eq!(
            Orientation::from_exif(6).then(Orientation::ROTATE_CCW),
            Orientation::IDENTITY
        );
    }
}
