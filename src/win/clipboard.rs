//! Text to the clipboard.

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{HANDLE, HWND};
use windows::Win32::System::DataExchange::{
    CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
};
use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
use windows::Win32::System::Ole::CF_UNICODETEXT;

/// Puts `text` on the clipboard (replacing its contents).
pub fn set_text(hwnd: HWND, text: &str) -> Result<()> {
    let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
    let bytes = wide.len() * 2;
    // SAFETY: the clipboard is opened and closed here; the global memory
    // block is filled while locked and handed to the system on success.
    unsafe {
        OpenClipboard(Some(hwnd)).context("OpenClipboard")?;
        let result = (|| -> Result<()> {
            EmptyClipboard()?;
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes)?;
            let ptr = GlobalLock(mem).cast::<u16>();
            if ptr.is_null() {
                bail!("GlobalLock failed");
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
            let _ = GlobalUnlock(mem);
            SetClipboardData(u32::from(CF_UNICODETEXT.0), Some(HANDLE(mem.0)))?;
            Ok(())
        })();
        let _ = CloseClipboard();
        result
    }
}
