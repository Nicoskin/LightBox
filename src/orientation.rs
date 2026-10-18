//! Image orientation: the 8 combinations of 90° rotations and a mirror,
//! numbered as in EXIF (1 = as stored).

/// EXIF-style orientation, always in `1..=8`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Orientation(u8);

impl Default for Orientation {
    fn default() -> Self {
        Self(1)
    }
}

impl Orientation {
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
}
