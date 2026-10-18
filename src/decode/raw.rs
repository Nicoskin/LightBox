//! Camera RAW files: shown through the full-size JPEG preview every camera
//! embeds (developing the raw data itself is out of scope). Works for any
//! container — TIFF-based (CR2, NEF, ARW, DNG, ORF, RW2, PEF…), RAF, CR3 —
//! by scanning for embedded JPEG streams and taking the largest one that
//! decodes. The WIC Raw Image Extension is the fallback.

use super::{DecodeError, RawImage, SizeHint, jpeg};
use crate::orientation::Orientation;

/// Offsets and sizes of JPEG streams that have a readable header, largest
/// first.
fn embedded_jpegs(data: &[u8]) -> Vec<(usize, usize, usize)> {
    let mut found = Vec::new();
    let mut i = 0;
    while i + 4 < data.len() {
        // SOI followed by a marker: DQT, DHT, APPn or SOFn.
        if data[i] == 0xFF
            && data[i + 1] == 0xD8
            && data[i + 2] == 0xFF
            && matches!(data[i + 3], 0xDB | 0xC4 | 0xE0..=0xEF | 0xC0..=0xC3)
        {
            if let Ok(h) = turbojpeg::read_header(&data[i..]) {
                found.push((i, h.width, h.height));
            }
            i += 4;
        } else {
            i += 1;
        }
    }
    found.sort_by_key(|&(_, w, h)| std::cmp::Reverse(w * h));
    found
}

/// Orientation of TIFF-based RAW files (IFD0 Orientation tag).
fn tiff_orientation(data: &[u8]) -> Orientation {
    if !(data.starts_with(b"II") || data.starts_with(b"MM")) {
        return Orientation::IDENTITY;
    }
    exif::Reader::new()
        .read_raw(data.to_vec())
        .ok()
        .and_then(|e| {
            e.get_field(exif::Tag::Orientation, exif::In::PRIMARY)
                .and_then(|f| f.value.get_uint(0))
        })
        .map_or(Orientation::IDENTITY, Orientation::from_exif)
}

pub fn decode(data: &[u8], hint: Option<SizeHint>) -> Result<RawImage, DecodeError> {
    for (offset, _, _) in embedded_jpegs(data).into_iter().take(3) {
        if let Ok(mut img) = jpeg::decode(&data[offset..], hint) {
            // The embedded JPEG often has no EXIF of its own: use the RAW's.
            if img.orientation == Orientation::IDENTITY {
                img.orientation = tiff_orientation(data);
            }
            return Ok(img);
        }
    }
    Err(DecodeError::Unsupported)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_largest_embedded_jpeg() {
        let small = crate::encode::encode_jpeg(&vec![90u8; 16 * 8 * 4], 16, 8, 80).unwrap();
        let big = crate::encode::encode_jpeg(&vec![200u8; 64 * 32 * 4], 64, 32, 80).unwrap();
        let mut raw = b"II*\0\x08\0\0\0junk-before".to_vec();
        raw.extend_from_slice(&small);
        raw.extend_from_slice(b"sensor data \xFF\xD8 not a jpeg");
        raw.extend_from_slice(&big);
        let found = embedded_jpegs(&raw);
        assert_eq!((found[0].1, found[0].2), (64, 32));
        let img = decode(&raw, None).unwrap();
        assert_eq!((img.width, img.height), (64, 32));
    }
}
