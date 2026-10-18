//! Text and images to the clipboard.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{HANDLE, HGLOBAL, HWND};
use windows::Win32::Graphics::Gdi::{BI_BITFIELDS, BITMAPV5HEADER};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::{CF_DIBV5, CF_HDROP, CF_UNICODETEXT};
use windows::Win32::UI::Shell::DROPFILES;
use windows::core::w;

/// A movable global memory block holding `bytes` (ownership passes to the
/// clipboard once set).
fn global(bytes: &[u8]) -> Result<HGLOBAL> {
    // SAFETY: the block is allocated with the right size, written while
    // locked and unlocked before use.
    unsafe {
        let mem = GlobalAlloc(GMEM_MOVEABLE, bytes.len().max(1))?;
        let ptr = GlobalLock(mem).cast::<u8>();
        if ptr.is_null() {
            bail!("GlobalLock failed");
        }
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), ptr, bytes.len());
        let _ = GlobalUnlock(mem);
        Ok(mem)
    }
}

/// Opens the clipboard, empties it, runs `fill`, closes it.
fn with_clipboard(hwnd: HWND, fill: impl FnOnce() -> Result<()>) -> Result<()> {
    // SAFETY: open / empty / close bracket the calls made by `fill`.
    unsafe {
        OpenClipboard(Some(hwnd)).context("OpenClipboard")?;
        let r = EmptyClipboard()
            .context("EmptyClipboard")
            .and_then(|()| fill());
        let _ = CloseClipboard();
        r
    }
}

fn put(format: u32, bytes: &[u8]) -> Result<()> {
    let mem = global(bytes)?;
    // SAFETY: the clipboard is open (see `with_clipboard`); on success the
    // system owns the memory block.
    unsafe { SetClipboardData(format, Some(HANDLE(mem.0)))? };
    Ok(())
}

fn utf16z(s: &str) -> Vec<u8> {
    s.encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

/// Puts `text` on the clipboard (replacing its contents).
pub fn set_text(hwnd: HWND, text: &str) -> Result<()> {
    with_clipboard(hwnd, || put(u32::from(CF_UNICODETEXT.0), &utf16z(text)))
}

/// Puts an image on the clipboard: a 32-bit DIBv5 with alpha (straight RGBA
/// `px`, `w` × `h`), optionally its PNG encoding (registered "PNG" format,
/// read by browsers and Office) and the file itself (Explorer pastes it).
pub fn set_image(
    hwnd: HWND,
    px: &[u8],
    w: usize,
    h: usize,
    png: Option<&[u8]>,
    file: Option<&Path>,
) -> Result<()> {
    let dib = dibv5(px, w, h);
    with_clipboard(hwnd, || {
        put(u32::from(CF_DIBV5.0), &dib)?;
        if let Some(png) = png {
            // SAFETY: registering a format name has no preconditions.
            let fmt = unsafe { RegisterClipboardFormatW(w!("PNG")) };
            if fmt != 0 {
                put(fmt, png)?;
            }
        }
        if let Some(f) = file {
            put(u32::from(CF_HDROP.0), &hdrop(f))?;
        }
        Ok(())
    })
}

/// BITMAPV5HEADER + bottom-up BGRA rows.
fn dibv5(px: &[u8], w: usize, h: usize) -> Vec<u8> {
    let header = BITMAPV5HEADER {
        bV5Size: std::mem::size_of::<BITMAPV5HEADER>() as u32,
        bV5Width: w as i32,
        bV5Height: h as i32,
        bV5Planes: 1,
        bV5BitCount: 32,
        bV5Compression: BI_BITFIELDS,
        bV5SizeImage: (w * h * 4) as u32,
        bV5RedMask: 0x00FF_0000,
        bV5GreenMask: 0x0000_FF00,
        bV5BlueMask: 0x0000_00FF,
        bV5AlphaMask: 0xFF00_0000,
        bV5CSType: 0x7352_4742, // 'sRGB'
        bV5Intent: 4,           // LCS_GM_IMAGES
        ..Default::default()
    };
    let mut out = Vec::with_capacity(header.bV5Size as usize + w * h * 4);
    // SAFETY: BITMAPV5HEADER is plain old data.
    out.extend_from_slice(unsafe {
        std::slice::from_raw_parts(
            std::ptr::from_ref(&header).cast::<u8>(),
            std::mem::size_of::<BITMAPV5HEADER>(),
        )
    });
    for row in px.chunks_exact(w * 4).rev() {
        for p in row.as_chunks::<4>().0 {
            out.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
        }
    }
    out
}

/// DROPFILES followed by the wide path and a double NUL.
fn hdrop(path: &Path) -> Vec<u8> {
    let df = DROPFILES {
        pFiles: std::mem::size_of::<DROPFILES>() as u32,
        fWide: true.into(),
        ..Default::default()
    };
    // SAFETY: DROPFILES is plain old data.
    let mut out = unsafe {
        std::slice::from_raw_parts(
            std::ptr::from_ref(&df).cast::<u8>(),
            std::mem::size_of::<DROPFILES>(),
        )
    }
    .to_vec();
    out.extend(utf16z(&path.to_string_lossy()));
    out.extend_from_slice(&[0, 0]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dib_is_bottom_up_bgra() {
        // 1 × 2: red on top, blue below.
        let px = [255, 0, 0, 255, 0, 0, 255, 128];
        let d = dibv5(&px, 1, 2);
        let body = &d[std::mem::size_of::<BITMAPV5HEADER>()..];
        assert_eq!(body, &[255, 0, 0, 128, 0, 0, 255, 255]);
    }
}
