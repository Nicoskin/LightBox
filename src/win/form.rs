//! A small native modal form: rows of label + field (text, number, choice,
//! folder with a "…" button) and OK / Cancel. Used where the viewer needs
//! typed input (tags, batch rename and convert). Tab moves between fields,
//! Enter is OK, Esc is Cancel.

use std::path::PathBuf;

use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Dwm::{DWMWA_USE_IMMERSIVE_DARK_MODE, DwmSetWindowAttribute};
use windows::Win32::Graphics::Gdi::{
    COLOR_BTNFACE, CreateFontW, DEFAULT_CHARSET, DeleteObject, FW_NORMAL, HBRUSH,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::Input::KeyboardAndMouse::EnableWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    BS_DEFPUSHBUTTON, BS_PUSHBUTTON, CB_ADDSTRING, CB_GETCURSEL, CB_SETCURSEL, CBS_DROPDOWNLIST,
    CS_HREDRAW, CS_VREDRAW, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW,
    ES_AUTOHSCROLL, ES_NUMBER, GWLP_USERDATA, GetMessageW, GetWindowLongPtrW, GetWindowRect,
    GetWindowTextLengthW, GetWindowTextW, HMENU, IDC_ARROW, IsDialogMessageW, LoadCursorW, MSG,
    RegisterClassExW, SW_SHOW, SendMessageW, SetForegroundWindow, SetWindowLongPtrW,
    SetWindowTextW, ShowWindow, TranslateMessage, WINDOW_EX_STYLE, WINDOW_STYLE, WM_CLOSE,
    WM_COMMAND, WM_DESTROY, WM_SETFONT, WNDCLASSEXW, WS_BORDER, WS_CAPTION, WS_CHILD,
    WS_EX_CONTROLPARENT, WS_EX_DLGMODALFRAME, WS_POPUP, WS_SYSMENU, WS_TABSTOP, WS_VISIBLE,
    WS_VSCROLL,
};
use windows::core::{HSTRING, PCWSTR, w};

/// One row of the form.
pub enum Field {
    Text {
        label: String,
        value: String,
    },
    /// Digits only.
    Number {
        label: String,
        value: u32,
    },
    Choice {
        label: String,
        options: Vec<String>,
        selected: usize,
    },
    Folder {
        label: String,
        value: PathBuf,
    },
}

impl Field {
    fn label(&self) -> &str {
        match self {
            Field::Text { label, .. }
            | Field::Number { label, .. }
            | Field::Choice { label, .. }
            | Field::Folder { label, .. } => label,
        }
    }
}

const ID_OK: usize = 1;
const ID_CANCEL: usize = 2;
/// Field controls get ids from here; their "…" buttons add `BROWSE`.
const ID_FIELD: usize = 100;
const BROWSE: usize = 50;

struct State {
    controls: Vec<HWND>,
    folders: Vec<Option<usize>>,
    /// `Some(true)`: OK, `Some(false)`: cancelled.
    result: Option<bool>,
}

/// Shows the form over `owner` and waits. On OK the fields hold the typed
/// values and `true` is returned.
pub fn show(owner: HWND, title: &str, note: &str, fields: &mut [Field], ok: &str) -> bool {
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

        let (label_w, field_w, row_h, pad) = (px(190), px(330), px(32), px(16));
        let note_h = if note.is_empty() { 0 } else { px(44) };
        let client_w = pad * 3 + label_w + field_w;
        let client_h = pad * 2 + note_h + row_h * fields.len() as i32 + px(48);
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
            return false;
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
        let mut y0 = pad;
        if !note.is_empty() {
            child(
                w!("STATIC"),
                note,
                0,
                (pad, y0, client_w - 2 * pad, note_h),
                0,
            );
            y0 += note_h;
        }
        let mut controls = Vec::with_capacity(fields.len());
        let mut folders = Vec::with_capacity(fields.len());
        let fx = pad * 2 + label_w;
        for (i, f) in fields.iter().enumerate() {
            let y = y0 + row_h * i as i32;
            child(
                w!("STATIC"),
                f.label(),
                0,
                (pad, y + px(6), label_w, row_h),
                0,
            );
            let id = ID_FIELD + i;
            let edit = WS_BORDER.0 | WS_TABSTOP.0 | ES_AUTOHSCROLL as u32;
            let ctl = match f {
                Field::Text { value, .. } => child(
                    w!("EDIT"),
                    value,
                    edit,
                    (fx, y + px(2), field_w, px(26)),
                    id,
                ),
                Field::Number { value, .. } => child(
                    w!("EDIT"),
                    &value.to_string(),
                    edit | ES_NUMBER as u32,
                    (fx, y + px(2), field_w, px(26)),
                    id,
                ),
                Field::Choice {
                    options, selected, ..
                } => {
                    let c = child(
                        w!("COMBOBOX"),
                        "",
                        WS_TABSTOP.0 | WS_VSCROLL.0 | CBS_DROPDOWNLIST as u32,
                        (fx, y + px(2), field_w, px(240)),
                        id,
                    );
                    for o in options {
                        let s = HSTRING::from(o.as_str());
                        SendMessageW(c, CB_ADDSTRING, None, Some(LPARAM(s.as_ptr() as isize)));
                    }
                    SendMessageW(c, CB_SETCURSEL, Some(WPARAM(*selected)), None);
                    c
                }
                Field::Folder { value, .. } => {
                    let bw = px(36);
                    let c = child(
                        w!("EDIT"),
                        &value.to_string_lossy(),
                        edit,
                        (fx, y + px(2), field_w - bw - px(6), px(26)),
                        id,
                    );
                    child(
                        w!("BUTTON"),
                        "…",
                        WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
                        (fx + field_w - bw, y + px(1), bw, px(28)),
                        id + BROWSE,
                    );
                    c
                }
            };
            controls.push(ctl);
            folders.push(matches!(f, Field::Folder { .. }).then_some(i));
        }
        let by = client_h - pad - px(30);
        let bw = px(120);
        child(
            w!("BUTTON"),
            ok,
            WS_TABSTOP.0 | BS_DEFPUSHBUTTON as u32,
            (client_w - pad * 2 - bw * 2, by, bw, px(30)),
            ID_OK,
        );
        child(
            w!("BUTTON"),
            "Отмена",
            WS_TABSTOP.0 | BS_PUSHBUTTON as u32,
            (client_w - pad - bw, by, bw, px(30)),
            ID_CANCEL,
        );

        let state = Box::into_raw(Box::new(State {
            controls: controls.clone(),
            folders,
            result: None,
        }));
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, state as isize);
        let _ = EnableWindow(owner, false);
        let _ = ShowWindow(hwnd, SW_SHOW);
        let _ = SetForegroundWindow(hwnd);
        if let Some(first) = controls.first() {
            let _ = windows::Win32::UI::Input::KeyboardAndMouse::SetFocus(Some(*first));
        }

        let mut msg = MSG::default();
        while (*state).result.is_none() && GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if !IsDialogMessageW(hwnd, &msg).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        let ok = (*state).result == Some(true);
        if ok {
            for (f, c) in fields.iter_mut().zip(&controls) {
                let text = || {
                    let n = GetWindowTextLengthW(*c).max(0) as usize;
                    let mut buf = vec![0u16; n + 1];
                    let got = GetWindowTextW(*c, &mut buf).max(0) as usize;
                    String::from_utf16_lossy(&buf[..got])
                };
                match f {
                    Field::Text { value, .. } => *value = text(),
                    Field::Number { value, .. } => *value = text().parse().unwrap_or(*value),
                    Field::Folder { value, .. } => *value = PathBuf::from(text()),
                    Field::Choice { selected, .. } => {
                        let i = SendMessageW(*c, CB_GETCURSEL, None, None).0;
                        if i >= 0 {
                            *selected = i as usize;
                        }
                    }
                }
            }
        }
        let _ = EnableWindow(owner, true);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, 0);
        let _ = DestroyWindow(hwnd);
        drop(Box::from_raw(state));
        let _ = DeleteObject(font.into());
        let _ = SetForegroundWindow(owner);
        ok
    }
}

extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    // SAFETY: GWLP_USERDATA holds the `State` while the form is shown.
    unsafe {
        let state = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut State;
        match msg {
            WM_COMMAND if !state.is_null() => {
                let id = wparam.0 & 0xFFFF;
                match id {
                    ID_OK => (*state).result = Some(true),
                    ID_CANCEL => (*state).result = Some(false),
                    id if id >= ID_FIELD + BROWSE => {
                        let i = id - ID_FIELD - BROWSE;
                        // Copied out: the folder dialog runs its own loop.
                        let st = &*state;
                        let target = st
                            .folders
                            .get(i)
                            .copied()
                            .flatten()
                            .and_then(|_| st.controls.get(i).copied());
                        if let Some(ctl) = target
                            && let Some(p) =
                                super::dialogs::open(Some(hwnd), "Папка", true, None, None)
                        {
                            let _ = SetWindowTextW(ctl, &HSTRING::from(p.as_os_str()));
                        }
                    }
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
