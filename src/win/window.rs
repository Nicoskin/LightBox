use anyhow::{Context as _, Result};
use windows::Win32::Foundation::{HWND, POINT, RECT};
use windows::Win32::Graphics::Dwm::{
    DWMWA_BORDER_COLOR, DWMWA_COLOR_NONE, DWMWA_USE_IMMERSIVE_DARK_MODE,
    DWMWA_WINDOW_CORNER_PREFERENCE, DWMWCP_DONOTROUND, DWMWCP_ROUND, DWMWINDOWATTRIBUTE,
    DwmExtendFrameIntoClientArea, DwmFlush, DwmSetWindowAttribute,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::Controls::MARGINS;
use windows::Win32::UI::WindowsAndMessaging::{
    CS_DBLCLKS, CreateWindowExW, DispatchMessageW, GWL_STYLE, GetCursorPos, GetWindowRect,
    IDC_ARROW, LoadCursorW, MSG, PM_REMOVE, PeekMessageW, RegisterClassExW, SW_SHOWNA,
    SWP_FRAMECHANGED, SWP_NOACTIVATE, SWP_NOZORDER, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowPos, SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_STYLE, WM_QUIT, WNDCLASSEXW,
    WNDPROC, WS_CAPTION, WS_EX_APPWINDOW, WS_EX_NOREDIRECTIONBITMAP, WS_MINIMIZEBOX, WS_POPUP,
    WS_SYSMENU, WS_THICKFRAME, WS_VISIBLE, WaitMessage,
};
use windows::core::{BOOL, HSTRING, PCWSTR, w};

const CLASS_NAME: PCWSTR = w!("LightboxViewer");

/// Fullscreen: a bare popup covering the monitor (so the shell treats it as
/// fullscreen and drops the taskbar behind it).
const STYLE_FULLSCREEN: WINDOW_STYLE = WS_POPUP;
/// Windowed: caption + thick frame give the DWM shadow, snapping, resizing
/// and minimising; the visible frame is removed in WM_NCCALCSIZE.
const STYLE_WINDOWED: WINDOW_STYLE =
    WINDOW_STYLE(WS_POPUP.0 | WS_CAPTION.0 | WS_THICKFRAME.0 | WS_SYSMENU.0 | WS_MINIMIZEBOX.0);

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

        CreateWindowExW(
            WS_EX_NOREDIRECTIONBITMAP | WS_EX_APPWINDOW,
            CLASS_NAME,
            &HSTRING::from(title),
            STYLE_FULLSCREEN,
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

/// Shows the window without activating it (cheap; see [`activate`]).
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

pub fn set_title(hwnd: HWND, title: &str) {
    // SAFETY: own window; HSTRING is NUL-terminated.
    unsafe {
        let _ = SetWindowTextW(hwnd, &HSTRING::from(title));
    }
}

/// Window rectangle in screen coordinates.
pub fn rect(hwnd: HWND) -> RECT {
    let mut r = RECT::default();
    // SAFETY: valid out-pointer; on failure an empty rect is returned.
    unsafe {
        let _ = GetWindowRect(hwnd, &mut r);
    }
    r
}

pub fn cursor_pos() -> POINT {
    let mut p = POINT::default();
    // SAFETY: valid out-pointer.
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    p
}

/// Sets a DWM window attribute; failures (older Windows) are ignored.
fn dwm_attr<T>(hwnd: HWND, attr: DWMWINDOWATTRIBUTE, value: T) {
    // SAFETY: own window; the value outlives the call and its size matches.
    unsafe {
        let _ = DwmSetWindowAttribute(
            hwnd,
            attr,
            std::ptr::from_ref(&value).cast(),
            std::mem::size_of::<T>() as u32,
        );
    }
}

/// Windowed-mode frame: Windows 11 rounded corners, a thin neutral border
/// and the dark-mode shadow; fullscreen: square corners, no border.
fn set_frame(hwnd: HWND, windowed: bool) {
    let (corners, border) = if windowed {
        // #3A3A3A: visible on light and dark wallpapers, quiet next to photos.
        (DWMWCP_ROUND, 0x003A_3A3A_u32)
    } else {
        (DWMWCP_DONOTROUND, DWMWA_COLOR_NONE)
    };
    dwm_attr(hwnd, DWMWA_USE_IMMERSIVE_DARK_MODE, BOOL::from(true));
    dwm_attr(hwnd, DWMWA_WINDOW_CORNER_PREFERENCE, corners);
    dwm_attr(hwnd, DWMWA_BORDER_COLOR, border);
    // A one-pixel "frame" extended into the client area makes DWM draw the
    // drop shadow of a window without a visible system frame.
    let m = i32::from(windowed);
    let margins = MARGINS {
        cxLeftWidth: m,
        cxRightWidth: m,
        cyTopHeight: m,
        cyBottomHeight: m,
    };
    // SAFETY: own window; the struct outlives the call.
    unsafe {
        let _ = DwmExtendFrameIntoClientArea(hwnd, &margins);
    }
}

/// Switches between the fullscreen and windowed styles and moves the window
/// to `r` in one step.
pub fn set_mode(hwnd: HWND, windowed: bool, r: RECT) -> Result<()> {
    let style = if windowed {
        STYLE_WINDOWED
    } else {
        STYLE_FULLSCREEN
    };
    // SAFETY: own window.
    unsafe {
        SetWindowLongPtrW(hwnd, GWL_STYLE, (style.0 | WS_VISIBLE.0) as isize);
    }
    set_frame(hwnd, windowed);
    // SAFETY: own window.
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_FRAMECHANGED | SWP_NOZORDER | SWP_NOACTIVATE,
        )?;
    }
    Ok(())
}

/// Moves / resizes the window without changing its style.
pub fn set_rect(hwnd: HWND, r: RECT) -> Result<()> {
    // SAFETY: own window.
    unsafe {
        SetWindowPos(
            hwnd,
            None,
            r.left,
            r.top,
            r.right - r.left,
            r.bottom - r.top,
            SWP_NOZORDER | SWP_NOACTIVATE,
        )?;
    }
    Ok(())
}

/// Waits for the next DWM composition. Called right before a geometry
/// change so that the window move and the matching new frame land in the
/// same composed frame (no one-frame glitch).
pub fn sync_to_compositor() {
    // SAFETY: no preconditions.
    unsafe {
        let _ = DwmFlush();
    }
}

/// Runs the message loop until `WM_QUIT`; returns its exit code.
///
/// While `frame` reports an animation in progress, it is called once per
/// loop iteration after all pending messages have been handled (so input is
/// never starved); its Present paces the loop to the display. Otherwise the
/// thread sleeps in `WaitMessage`.
pub fn run_message_loop(mut frame: impl FnMut() -> bool) -> i32 {
    let mut msg = MSG::default();
    let mut animating = false;
    loop {
        // SAFETY: standard message pump on the thread that owns the window.
        unsafe {
            while PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                if msg.message == WM_QUIT {
                    return msg.wParam.0 as i32;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
                animating = true; // re-check after any message
            }
        }
        if animating {
            animating = frame();
        }
        if !animating {
            // SAFETY: blocks until a message arrives.
            unsafe {
                let _ = WaitMessage();
            }
            animating = true;
        }
    }
}
