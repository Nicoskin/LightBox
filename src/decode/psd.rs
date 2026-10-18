//! Photoshop documents: the flattened composite image stored in the file.

use super::{DecodeError, Raw, RawImage};

pub fn decode(data: &[u8]) -> Result<RawImage, DecodeError> {
    let psd = psd::Psd::from_bytes(data).map_err(|e| DecodeError::Other(format!("PSD: {e}")))?;
    let (w, h) = (psd.width(), psd.height());
    let rgba = psd.rgba();
    if rgba.len() != w as usize * h as usize * 4 {
        return Err(DecodeError::Other("PSD: unexpected composite size".into()));
    }
    Ok(RawImage::new(w, h, Raw::Rgba8(rgba)))
}
