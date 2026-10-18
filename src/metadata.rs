//! Metadata parsing (EXIF; later PNG text chunks for AI generation info).

use std::io::Cursor;

use exif::{In, Reader, Tag};

use crate::orientation::Orientation;

/// EXIF orientation of a JPEG file; "as stored" if absent or unreadable.
pub fn jpeg_orientation(data: &[u8]) -> Orientation {
    let Ok(exif) = Reader::new().read_from_container(&mut Cursor::new(data)) else {
        return Orientation::default();
    };
    exif.get_field(Tag::Orientation, In::PRIMARY)
        .and_then(|f| f.value.get_uint(0))
        .map_or_else(Orientation::default, Orientation::from_exif)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minimal JPEG prefix: SOI + APP1 "Exif" with a little-endian TIFF
    /// header and one IFD entry (Orientation = `o`).
    pub fn jpeg_with_orientation(o: u16) -> Vec<u8> {
        let mut tiff = Vec::new();
        tiff.extend_from_slice(b"II*\0");
        tiff.extend_from_slice(&8u32.to_le_bytes()); // IFD0 offset
        tiff.extend_from_slice(&1u16.to_le_bytes()); // 1 entry
        tiff.extend_from_slice(&0x0112u16.to_le_bytes()); // Orientation
        tiff.extend_from_slice(&3u16.to_le_bytes()); // SHORT
        tiff.extend_from_slice(&1u32.to_le_bytes()); // count
        tiff.extend_from_slice(&o.to_le_bytes());
        tiff.extend_from_slice(&[0, 0]);
        tiff.extend_from_slice(&0u32.to_le_bytes()); // no next IFD

        let mut app1 = b"Exif\0\0".to_vec();
        app1.extend_from_slice(&tiff);
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
        jpeg.extend_from_slice(&((app1.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(&app1);
        jpeg.extend_from_slice(&[0xFF, 0xD9]);
        jpeg
    }

    #[test]
    fn reads_orientation() {
        for o in 1..=8 {
            assert_eq!(jpeg_orientation(&jpeg_with_orientation(o)).exif(), o as u8);
        }
    }

    #[test]
    fn missing_or_broken_exif_is_default() {
        assert_eq!(
            jpeg_orientation(&[0xFF, 0xD8, 0xFF, 0xD9]),
            Orientation::default()
        );
        assert_eq!(jpeg_orientation(b"garbage"), Orientation::default());
    }
}
