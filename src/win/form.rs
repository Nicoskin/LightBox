//! A small native modal prompt for a number (label, field, OK / Cancel),
//! where the viewer wants typed input. Enter is OK, Esc is Cancel.

use crate::i18n::tr;
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET, DeleteObject, FW_NORMAL, HBRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::{EnableWindow, SetFocus};
use windows::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW,
    DestroyWindow, DispatchMessageW, ES_AUTOHSCROLL, ES_NUMBER, GWLP_USERDATA, GetMessageW,
    GetWindowLongPtrW, GetWindowRect, GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW,
    IsDialogMessageW, LoadCursorW, MSG, RegisterClassExW, SW_SHOW, SendMessageW,
    SetForegroundWindow, SetWindowLongPtrW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_CLOSE, WM_COMMAND, WM_DESTROY, WM_SETFONT, WNDCLASSEXW, WS_BORDER, WS_CAPTION,
    WS_CHILD, WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_POPUP, WS_SYSMENU, WS_TABSTOP,
    WS_VISIBLE,
};
use windows::core::{HSTRING, PCWSTR, w};

const ID_OK: usize = 1;
const ID_CANCEL: usize = 2;
const ID_FIELD: usize = 100;
/// Edit control: select a range (all of it, to type over).
const EM_SETSEL: u32 = 0x00B1;

/// `Some(true)`: OK, `Some(false)`: cancelled.
struct State {
    result: Option<bool>,
}

/// Asks for a number over `owner`; `None` when cancelled.
pub fn ask_number(owner: HWND, title: &str, label: &str, value: u32) -> Option<u32> {
    // SAFETY: plain window creation and a modal loop on this thread; the
    // state box lives until the window is destroyed below.
    unsafe {
        let dpi = GetDpiForWindow(owner).max(96);
        let px = |v: i32| v * dpi as i32 / 96;
        let instance = GetModuleHandleW(None).unwrap_or_default();
        let class = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            style: CS_HREDRAW | CS_VREDRAW,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            hbrBackground: HBRUSH((COLOR_BTNFACE.0 + 1) as *mut _),
            lpszClassName: w!("LightboxForm"),
            ..Default::default()
        };
        RegisterClassExW(&class);

        let (label_w, field_w, row_h, pad) = (px(230), px(110), px(32), px(16));
        let client_w = pad * 3 + label_w + field_w;
        let client_h = pad * 2 + row_h + px(48);
        let mut owner_rc = RECT::default();
        let _ = GetWindowRect(owner, &mut owner_rc);
        let (w, h) = (client_w + px(16), client_h + px(40));
        let x = owner_rc.left + (owner_rc.right - owner_rc.left - w) / 2;
        let y = owner_rc.top + (owner_rc.bottom - owner_rc.top - h) / 2;
        let Ok(hwnd) = CreateWindowExW(
            WS_EX_DLGMODALFRAME | WS_EX_CONTROLPARENT,
            w!("LightboxForm"),
            &HSTRING::from(title),
            WS_POPUP | WS_CAPTION | WS_SYSMENU,
            x,
            y,
            w,
            h,
            Some(owner),
            None,
            Some(instance.into()),
            None,
        ) else {
            return None;
        };
        let dark: i32 = 1;
        let _ = DwmSetWindowAttribute(
            hwnd,
            DWMWA_USE_IMMERSIVE_DARK_MODE,
            std::ptr::from_ref(&dark).cast(),
            4,
        );
        let font = CreateFontW(
            -px(15),
            0,
            0,
            0,
            FW_NORMAL.0 as i32,
            0,
            0,
            0,
            DEFAULT_CHARSET,
            Default::default(),
            Default::default(),
            Default::default(),
            0,
            w!("Segoe UI"),
        );
        let child = |class: PCWSTR, text: &str, style: u32, rc: (i32, i32, i32, i32), id: usize| {
            let c = CreateWindowExW(
                WINDOW_EX_STYLE(0),
                class,
                &HSTRING::from(text),
                WINDOW_STYLE(WS_CHILD.0 | WS_VISIBLE.0 | style),
                rc.0,
                rc.1,
                rc.2,
                rc.3,
                Some(hwnd),
                Some(HMENU(id as *mut _)),
                Some(instance.into()),
                None,
            )
            .unwrap_or_default();
            SendMessageW(
                c,
                WM_SETFONT,
                Some(WPARAM(font.0 as usize)),
                Some(LPARAM(1)),
            );
            c
        };
        child(
            w!("STATIC"),
            label,
            0,
            (pad, pad + px(6), label_w, row_h),
            0,
        );
        let edit = child(
            w!("EDIT"),
            &value.to_string(),
            WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32 | ES_NUMBER as u32,
            (pad * 2 + label_w, pad + px(2), field_w, px(26)),
            ID_FIELD,
        );
        let by = client_h - pad - px(30);
        let bw = px(120);
        child(
            w!("BUTTON"),
            "OK",
            WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
            (client_w - pad * 2 - bw * 2, by, bw, px(30)),
            ID_OK,
        );
        child(
            w!("BUTTON"),
            tr("Отмена"),
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            (client_w - pad - bw, by, bw, px(30)),
            ID_CANCEL,
        );

        let state = Box::into_raw(Box::new(State { result: None }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        let _ = SetFocus(Some(edit));
        SendMessageW(edit, EM_SETSEL, Some(WPARAM(0)), Some(LPARAM(-1)));

        let mut msg = MSG::default();
        while (*state).result.is_none() && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let typed = ((*state).result == Some(true)).then(|| {
            let n = GetWindowTextLengthW(edit).max(0) as usize;
            let mut buf = vec![0u16; n + 1];
            let got = GetWindowTextW(edit, &mut buf).max(0) as usize;
            String::from_utf16_lossy(&buf[..got])
                .trim()
                .parse()
                .unwrap_or(value)
        });
        let _ = EnableWindow(owner, true);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let _ = DestroyWindow(hwnd);
        drop(Box::from_raw(state));
        let _ = DeleteObject(font.into());
        let _ = SetForegroundWindow(owner);
        typed
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA holds the `State` while the prompt is shown.
    unsafe {
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        match msg {
            WM_COMMAND if !state.is_null() => {
                match wparam.0 & 0xFFFF {
                    ID_OK => (*state).result = Some(true),
                    ID_CANCEL => (*state).result = Some(false),
                    _ => {}
                }
                LRESULT(0)
            }
            WM_CLOSE if !state.is_null() => {
                (*state).result = Some(false);
                LRESULT(0)
            }
            WM_DESTROY => LRESULT(0),
            _ => DefWindowProcW(hwnd, msg, wparam, lparam),
        }
    }
}
