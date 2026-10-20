//! `--document <file>`: Space preview of files Lightbox does not draw
//! itself (DOCX, XLSX, PPTX, RTF…): a plain window hosting the preview
//! handler Explorer's preview pane uses for that type. It runs in its own
//! process, so a misbehaving handler cannot take the viewer down. Esc or
//! Space closes it.

use std::path::Path;

use anyhow::{Context as _, Result, bail};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{COLOR_WINDOW, HBRUSH};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CLSCTX_LOCAL_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance,
    CoInitializeEx, STGM_READ,
};
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_ESCAPE, VK_SPACE};
use windows::Win32::UI::Shell::PropertiesSystem::{IInitializeWithFile, IInitializeWithStream};
use windows::Win32::UI::Shell::{
    ASSOCF_INIT_DEFAULTTOSTAR, ASSOCSTR_SHELLEXTENSION, AssocQueryStringW, IInitializeWithItem,
    IPreviewHandler, IShellItem, SHCreateItemFromParsingName, SHCreateStreamOnFileEx,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DispatchMessageW,
    GWLP_USERDATA, GetClientRect, GetMessageW, GetWindowLongPtrW, IDC_ARROW, LoadCursorW, MSG,
    PostQuitMessage, RegisterClassExW, SW_SHOW, SetForegroundWindow, SetWindowLongPtrW, ShowWindow,
    TranslateMessage, WINDOW_EX_STYLE, WM_CLOSE, WM_DESTROY, WM_KEYDOWN, WM_SETFOCUS, WM_SIZE,
    WNDCLASSEXW, WS_CLIPCHILDREN, WS_OVERLAPPEDWINDOW,
};
use windows::core::{GUID, HSTRING, Interface as _, PWSTR, w};

use crate::i18n::tr;
use crate::win::monitor::Monitor;

/// The interface id of preview handlers, as registered under ShellEx.
const PREVIEW_HANDLER: &str = "{8895b1c6-b41f-4c1c-a562-0d564250836f}";

/// The preview handler registered for `path`'s type, if any.
pub fn handler_for(path: &Path) -> Option<GUID> {
    let ext = format!(".{}", path.extension()?.to_str()?);
    let mut buf = [0u16; 64];
    let mut len = buf.len() as u32;
    // SAFETY: valid strings and an output buffer of `len` characters.
    unsafe {
        AssocQueryStringW(
            ASSOCF_INIT_DEFAULTTOSTAR,
            ASSOCSTR_SHELLEXTENSION,
            &HSTRING::from(ext),
            &HSTRING::from(PREVIEW_HANDLER),
            Some(PWSTR(buf.as_mut_ptr())),
            &mut len,
        )
        .ok()
        .ok()?;
    }
    let s = String::from_utf16_lossy(&buf[..(len as usize).saturating_sub(1)]);
    GUID::try_from(s.trim_matches(|c| c == '{' || c == '}')).ok()
}

/// Window state reached from the window procedure.
struct State {
    handler: IPreviewHandler,
}

pub fn run(path: &Path) -> Result<()> {
    // SAFETY: COM for this (UI) thread; preview handlers expect an STA.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
    }
    let clsid = handler_for(path).context(tr("нет программы для предпросмотра этого типа"))?;
    let path = std::path::absolute(path)?;
    let wide = HSTRING::from(path.as_os_str());

    // The handler: out of process when it allows it (Office does), so it
    // runs as it does for Explorer; in process otherwise.
    // SAFETY: plain COM activation of a registered class.
    let handler: IPreviewHandler = unsafe {
        CoCreateInstance(&clsid, None, CLSCTX_LOCAL_SERVER)
            .or_else(|_| CoCreateInstance(&clsid, None, CLSCTX_INPROC_SERVER))
            .context(tr("обработчик предпросмотра не запустился"))?
    };
    // SAFETY: the handler reads the file through one of these.
    unsafe {
        if let Ok(i) = handler.cast::<IInitializeWithFile>() {
            i.Initialize(&wide, STGM_READ.0)?;
        } else if let Ok(i) = handler.cast::<IInitializeWithStream>() {
            let stream = SHCreateStreamOnFileEx(&wide, STGM_READ.0, 0, false, None)?;
            i.Initialize(&stream, STGM_READ.0)?;
        } else if let Ok(i) = handler.cast::<IInitializeWithItem>() {
            let item: IShellItem = SHCreateItemFromParsingName(&wide, None)?;
            i.Initialize(&item, STGM_READ.0)?;
        } else {
            bail!(tr("обработчик предпросмотра не принимает файлы"));
        }
    }

    let hwnd = create_window(&path)?;
    let state = Box::new(State {
        handler: handler.clone(),
    });
    // SAFETY: the box lives until WM_DESTROY takes it back.
    unsafe {
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, Box::into_raw(state) as isize);
        let mut rc = RECT::default();
        GetClientRect(hwnd, &mut rc)?;
        handler.SetWindow(hwnd, &rc)?;
        handler.DoPreview()?;
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = handler.SetFocus();

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            // Esc / Space close, whichever child window has the focus.
            if msg.message == WM_KEYDOWN
                && (msg.wParam.0 == usize::from(VK_ESCAPE.0)
                    || msg.wParam.0 == usize::from(VK_SPACE.0))
            {
                let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                    Some(hwnd),
                    WM_CLOSE,
                    WPARAM(0),
                    LPARAM(0),
                );
                continue;
            }
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
    Ok(())
}

fn create_window(path: &Path) -> Result<HWND> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    // A page-like window: 80 % of the work area's height, A4 proportions.
    let mon = Monitor::under_cursor();
    let work = mon.work;
    let (ww, wh) = (work.right - work.left, work.bottom - work.top);
    let h = (wh as f64 * 0.8) as i32;
    let w = ((h as f64 * 0.78) as i32).min((ww as f64 * 0.8) as i32);
    // SAFETY: plain window class registration and creation.
    unsafe {
        let instance = windows::Win32::System::LibraryLoader::GetModuleHandleW(None)?;
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            hbrBackground: HBRUSH((COLOR_WINDOW.0 + 1) as *mut _),
            hIcon: crate::win::window::small_icon(),
            lpszClassName: w!("LightboxDocument"),
            ..Default::default()
        };
        RegisterClassExW(&class);
        let hwnd = CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("LightboxDocument"),
            &HSTRING::from(format!("{name} — Lightbox")),
            WS_OVERLAPPEDWINDOW | WS_CLIPCHILDREN,
            if w > 0 {
                work.left + (ww - w) / 2
            } else {
                CW_USEDEFAULT
            },
            work.top + (wh - h) / 2,
            w.max(400),
            h.max(300),
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&dark).cast(),
            4,
        );
        Ok(hwnd)
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA holds our `State` from creation to WM_DESTROY.
    unsafe {
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        match msg {
            WM_SIZE if !state.is_null() => {
                let mut rc = RECT::default();
                let _ = GetClientRect(hwnd, &mut rc);
                let _ = (*state).handler.SetRect(&rc);
                LRESULT(0)
            }
            WM_SETFOCUS if !state.is_null() => {
                let _ = (*state).handler.SetFocus();
                LRESULT(0)
            }
            WM_DESTROY => {
                if !state.is_null() {
                    SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
                    let s = Box::from_raw(state);
                    let _ = s.handler.Unload();
                }
                PostQuitMessage(0);
                LRESULT(0)
            }
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
