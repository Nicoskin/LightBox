use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{HWND, RECT};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DispatchMessageW, GetMessageW, IDC_ARROW, LoadCursorW, MSG,
    RegisterClassExW, SW_SHOWNA, SetForegroundWindow, ShowWindow, TranslateMessage, WNDCLASSEXW,
    WNDPROC, WS_EX_APPWINDOW, WS_EX_NOREDIRECTIONBITMAP, WS_POPUP,
};
use windows::core::{PCWSTR, w};

const CLASS_NAME: PCWSTR = w!("LightboxViewer");

/// Creates the (hidden) top-level viewer window covering `rect`.
///
/// `WS_EX_NOREDIRECTIONBITMAP`: the window has no GDI surface at all; its
/// content is exclusively the DirectComposition visual tree, which is what
/// makes real per-pixel transparency over the desktop possible.
pub fn create(rect: RECT, title: &str, wndproc: WNDPROC) -> Result<HWND> {
    // SAFETY: standard class registration / window creation; all strings are
    // NUL-terminated and outlive the calls.
    unsafe {
        let instance = GetModuleHandleW(None).context("GetModuleHandleW")?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_DBLCLKS,
            lpfnWndProc: wndproc,
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            lpszClassName: CLASS_NAME,
            ..Default::default()
        };
        // Registration fails harmlessly if the class already exists.
        RegisterClassExW(&class);

        let title: Vec<u16> = title.encode_utf16().chain(Some(0)).collect();
        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_APPWINDOW,
            CLASS_NAME,
            PCWSTR(title.as_ptr()),
            WS_POPUP,
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .context("CreateWindowExW")
    }
}

pub fn show(hwnd: HWND) {
    // SAFETY: hwnd is a window created by this thread.
    unsafe {
        let _ = ShowWindow(hwnd, SW_SHOWNA);
    }
}

/// Brings the window to the foreground and gives it keyboard focus.
///
/// Kept separate from [`show`] because activation is the expensive part of
/// showing a fullscreen window (~20 ms: focus change, text services), so it
/// is done after the first frame is already on screen.
pub fn activate(hwnd: HWND) {
    // SAFETY: hwnd is a window created by this thread. The process was just
    // launched by the user, so it may take the foreground.
    unsafe {
        let _ = SetForegroundWindow(hwnd);
    }
}

/// Runs the message loop until `WM_QUIT`; returns its exit code.
pub fn run_message_loop() -> i32 {
    let mut msg = MSG::default();
    // SAFETY: standard message pump on the thread that owns the window.
    unsafe {
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    msg.wParam.0 as i32
}
