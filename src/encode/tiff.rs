//! In-place edits of EXIF data (a TIFF structure) after a rotation:
//! Orientation tag, pixel dimensions and the embedded thumbnail.
//!
//! The TIFF block is edited without re-serialising it, so unknown tags,
//! maker notes and their offsets stay intact.

const ORIENTATION: u16 = 0x0112;
const EXIF_IFD: u16 = 0x8769;
const PIXEL_X: u16 = 0xA002;
const PIXEL_Y: u16 = 0xA003;
const THUMB_OFFSET: u16 = 0x0201;
const THUMB_LENGTH: u16 = 0x0202;
const SHORT: u16 = 3;
const LONG: u16 = 4;

struct Tiff<'a> {
    d: &'a mut Vec<u8>,
    le: bool,
}

impl<'a> Tiff<'a> {
    fn new(d: &'a mut Vec<u8>) -> Option<Self> {
        let le = match d.get(0..4)? {
            b"II*\0" => true,
            b"MM\0*" => false,
            _ => return None,
        };
        Some(Self { d, le })
    }

    fn u16(&self, at: usize) -> Option<u16> {
        let b: [u8; 2] = self.d.get(at..at + 2)?.try_into().ok()?;
        Some(if self.le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        })
    }

    fn u32(&self, at: usize) -> Option<u32> {
        let b: [u8; 4] = self.d.get(at..at + 4)?.try_into().ok()?;
        Some(if self.le {
            u32::from_le_bytes(b)
        } else {
            u32::from_be_bytes(b)
        })
    }

    fn put_u16(&mut self, at: usize, v: u16) -> Option<()> {
        let b = if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        self.d.get_mut(at..at + 2)?.copy_from_slice(&b);
        Some(())
    }

    fn put_u32(&mut self, at: usize, v: u32) -> Option<()> {
        let b = if self.le {
            v.to_le_bytes()
        } else {
            v.to_be_bytes()
        };
        self.d.get_mut(at..at + 4)?.copy_from_slice(&b);
        Some(())
    }

    fn ifd0(&self) -> Option<usize> {
        self.u32(4).map(|v| v as usize)
    }

    /// Byte offset of the 12-byte entry for `tag` in the IFD at `ifd`.
    fn entry(&self, ifd: usize, tag: u16) -> Option<usize> {
        let n = self.u16(ifd)? as usize;
        (0..n)
            .map(|i| ifd + 2 + i * 12)
            .find(|&e| self.u16(e) == Some(tag))
    }

    fn next_ifd(&self, ifd: usize) -> Option<usize> {
        let n = self.u16(ifd)? as usize;
        let next = self.u32(ifd + 2 + n * 12)? as usize;
        (next != 0).then_some(next)
    }

    /// Inline SHORT or LONG value of an entry.
    fn value(&self, e: usize) -> Option<u32> {
        match self.u16(e + 2)? {
            SHORT => self.u16(e + 8).map(u32::from),
            LONG => self.u32(e + 8),
            _ => None,
        }
    }

    fn set_value(&mut self, e: usize, v: u32) -> Option<()> {
        match self.u16(e + 2)? {
            SHORT => self.put_u16(e + 8, u16::try_from(v).ok()?),
            LONG => self.put_u32(e + 8, v),
            _ => None,
        }
    }
}

/// Sets the Orientation tag of the TIFF block `d` to `code`. If the tag is
/// missing, a copy of IFD0 with the tag added is appended and the header is
/// pointed at it (the old IFD0 becomes unreferenced; all offsets stay valid).
pub fn set_orientation(d: &mut Vec<u8>, code: u16) -> bool {
    let Some(mut t) = Tiff::new(d) else {
        return false;
    };
    let Some(ifd0) = t.ifd0() else {
        return false;
    };
    if let Some(e) = t.entry(ifd0, ORIENTATION) {
        return t.put_u16(e + 8, code).is_some();
    }
    // Append IFD0 + Orientation, keeping entries sorted by tag.
    let Some(n) = t.u16(ifd0).map(usize::from) else {
        return false;
    };
    let Some(next) = t.u32(ifd0 + 2 + n * 12) else {
        return false;
    };
    let entries: Vec<[u8; 12]> = (0..n)
        .filter_map(|i| {
            let at = ifd0 + 2 + i * 12;
            t.d.get(at..at + 12).and_then(|s| s.try_into().ok())
        })
        .collect();
    let le = t.le;
    let w16 = |v: u16| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let w32 = |v: u32| if le { v.to_le_bytes() } else { v.to_be_bytes() };
    let mut new_entry = [0u8; 12];
    new_entry[0..2].copy_from_slice(&w16(ORIENTATION));
    new_entry[2..4].copy_from_slice(&w16(SHORT));
    new_entry[4..8].copy_from_slice(&w32(1));
    new_entry[8..10].copy_from_slice(&w16(code));
    let tag_of = |e: &[u8; 12]| {
        let b = [e[0], e[1]];
        if le {
            u16::from_le_bytes(b)
        } else {
            u16::from_be_bytes(b)
        }
    };
    let pos = entries
        .iter()
        .position(|e| tag_of(e) > ORIENTATION)
        .unwrap_or(entries.len());

    if t.d.len() % 2 == 1 {
        t.d.push(0); // IFDs start on a word boundary
    }
    let Ok(at) = u32::try_from(t.d.len()) else {
        return false;
    };
    let mut ifd = Vec::with_capacity(2 + (n + 1) * 12 + 4);
    ifd.extend_from_slice(&w16((n + 1) as u16));
    for (i, e) in entries.iter().enumerate() {
        if i == pos {
            ifd.extend_from_slice(&new_entry);
        }
        ifd.extend_from_slice(e);
    }
    if pos == entries.len() {
        ifd.extend_from_slice(&new_entry);
    }
    ifd.extend_from_slice(&w32(next));
    t.d.extend_from_slice(&ifd);
    t.put_u32(4, at).is_some()
}

/// After the pixels were rotated losslessly: Orientation = 1, pixel
/// dimensions swapped for 90° turns, and the embedded thumbnail passed
/// through `rotate_thumb` (replaced when the result still fits).
pub fn normalize(
    d: &mut Vec<u8>,
    swap_dims: bool,
    rotate_thumb: impl Fn(&[u8]) -> Option<Vec<u8>>,
) {
    set_orientation(d, 1);
    let Some(mut t) = Tiff::new(d) else {
        return;
    };
    let Some(ifd0) = t.ifd0() else {
        return;
    };
    if swap_dims
        && let Some(exif) = t
            .entry(ifd0, EXIF_IFD)
            .and_then(|e| t.u32(e + 8))
            .map(|v| v as usize)
        && let (Some(ex), Some(ey)) = (t.entry(exif, PIXEL_X), t.entry(exif, PIXEL_Y))
        && let (Some(x), Some(y)) = (t.value(ex), t.value(ey))
    {
        let _ = t.set_value(ex, y);
        let _ = t.set_value(ey, x);
    }

    // IFD1: the thumbnail.
    let Some(ifd1) = t.next_ifd(ifd0) else {
        return;
    };
    let (Some(eo), Some(el)) = (t.entry(ifd1, THUMB_OFFSET), t.entry(ifd1, THUMB_LENGTH)) else {
        return;
    };
    let (Some(off), Some(len)) = (t.value(eo), t.value(el)) else {
        return;
    };
    let (off, len) = (off as usize, len as usize);
    let Some(old) = t.d.get(off..off + len).map(<[u8]>::to_vec) else {
        return;
    };
    let Some(new) = rotate_thumb(&old) else {
        return;
    };
    let at_end = off + len == t.d.len();
    if at_end {
        t.d.truncate(off);
        t.d.extend_from_slice(&new);
    } else if new.len() <= len {
        t.d[off..off + new.len()].copy_from_slice(&new);
    } else {
        return; // does not fit; keep the old thumbnail
    }
    let _ = t.set_value(el, new.len() as u32);
}

/// Orientation tag value, if present.
#[cfg(test)]
pub fn orientation(d: &mut Vec<u8>) -> Option<u16> {
    let t = Tiff::new(d)?;
    let e = t.entry(t.ifd0()?, ORIENTATION)?;
    t.u16(e + 8)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    /// Little-endian TIFF: IFD0 {Orientation?, ExifIFD}, Exif IFD
    /// {PixelX, PixelY}, IFD1 {thumb offset, length} and thumbnail bytes at
    /// the end.
    pub fn sample(orientation: Option<u16>, thumb: &[u8]) -> Vec<u8> {
        let mut d = b"II*\0".to_vec();
        d.extend_from_slice(&8u32.to_le_bytes());
        let n0: u16 = if orientation.is_some() { 2 } else { 1 };
        let ifd0_len = 2 + 12 * n0 as usize + 4;
        let exif_at = 8 + ifd0_len;
        let exif_len = 2 + 12 * 2 + 4;
        let ifd1_at = exif_at + exif_len;
        let ifd1_len = 2 + 12 * 2 + 4;
        let thumb_at = ifd1_at + ifd1_len;
        let entry = |d: &mut Vec<u8>, tag: u16, ty: u16, v: u32| {
            d.extend_from_slice(&tag.to_le_bytes());
            d.extend_from_slice(&ty.to_le_bytes());
            d.extend_from_slice(&1u32.to_le_bytes());
            d.extend_from_slice(&v.to_le_bytes());
        };
        // IFD0
        d.extend_from_slice(&n0.to_le_bytes());
        if let Some(o) = orientation {
            entry(&mut d, ORIENTATION, SHORT, u32::from(o));
        }
        entry(&mut d, EXIF_IFD, LONG, exif_at as u32);
        d.extend_from_slice(&(ifd1_at as u32).to_le_bytes());
        // Exif IFD
        d.extend_from_slice(&2u16.to_le_bytes());
        entry(&mut d, PIXEL_X, LONG, 6000);
        entry(&mut d, PIXEL_Y, SHORT, 4000);
        d.extend_from_slice(&0u32.to_le_bytes());
        // IFD1
        d.extend_from_slice(&2u16.to_le_bytes());
        entry(&mut d, THUMB_OFFSET, LONG, thumb_at as u32);
        entry(&mut d, THUMB_LENGTH, LONG, thumb.len() as u32);
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(thumb);
        d
    }

    fn exif_dims(d: &mut Vec<u8>) -> (u32, u32) {
        let t = Tiff::new(d).unwrap();
        let ifd0 = t.ifd0().unwrap();
        let exif = t.value(t.entry(ifd0, EXIF_IFD).unwrap()).unwrap() as usize;
        (
            t.value(t.entry(exif, PIXEL_X).unwrap()).unwrap(),
            t.value(t.entry(exif, PIXEL_Y).unwrap()).unwrap(),
        )
    }

    #[test]
    fn normalize_resets_orientation_swaps_dims_and_rotates_thumb() {
        let mut d = sample(Some(6), b"THUMB");
        normalize(&mut d, true, |t| {
            assert_eq!(t, b"THUMB");
            Some(b"ROTATED-THUMB".to_vec())
        });
        assert_eq!(orientation(&mut d), Some(1));
        assert_eq!(exif_dims(&mut d), (4000, 6000));
        assert!(d.ends_with(b"ROTATED-THUMB"));
    }

    #[test]
    fn missing_orientation_is_added() {
        let mut d = sample(None, b"T");
        assert_eq!(orientation(&mut d), None);
        assert!(set_orientation(&mut d, 8));
        assert_eq!(orientation(&mut d), Some(8));
        // Other data still reachable through the new IFD0.
        assert_eq!(exif_dims(&mut d), (6000, 4000));
        assert!(set_orientation(&mut d, 3));
        assert_eq!(orientation(&mut d), Some(3));
    }

    #[test]
    fn garbage_is_ignored() {
        let mut d = b"nope".to_vec();
        assert!(!set_orientation(&mut d, 1));
        normalize(&mut d, true, |_| None);
        assert_eq!(d, b"nope");
    }
}
