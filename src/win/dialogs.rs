//! Standard Windows dialogs: task dialogs (questions) and "Save as".

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDCBF_CANCEL_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_POSITION_RELATIVE_TO_WINDOW, TDF_USE_COMMAND_LINKS, TaskDialogIndirect,
};
use windows::Win32::UI::Shell::Common::COMDLG_FILTERSPEC;
use windows::Win32::UI::Shell::{
    FOS_FORCEFILESYSTEM, FOS_PICKFOLDERS, FileOpenDialog, FileSaveDialog, IFileDialogCustomize,
    IFileOpenDialog, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName, SIGDN_FILESYSPATH,
};
use windows::core::{BOOL, HSTRING, Interface as _, PCWSTR, w};

use crate::edit::ExportFormat;

/// Native modal text entry for filtering a folder by file name.
pub fn folder_search(hwnd: HWND, initial: &str) -> Option<String> {
    use crate::i18n::tr;
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::*;

    const EDIT: i32 = 100;
    // DWLP_USER follows the pointer-sized result and dialog procedure.
    const USER: i32 = (2 * std::mem::size_of::<isize>()) as i32;

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, w: WPARAM, l: LPARAM) -> isize {
        // SAFETY: lParam is the caller's live String on WM_INITDIALOG; it
        // remains alive for the modal loop and is only used on this thread.
        unsafe {
            match msg {
                WM_INITDIALOG => {
                    SetWindowLongPtrW(hwnd, WINDOW_LONG_PTR_INDEX(USER), l.0);
                    let text = &*(l.0 as *const String);
                    let _ = SetDlgItemTextW(hwnd, EDIT, &HSTRING::from(text.as_str()));
                    1
                }
                WM_COMMAND if w.0 as u16 == IDOK.0 as u16 => {
                    let mut buf = [0u16; 1024];
                    let n = GetDlgItemTextW(hwnd, EDIT, &mut buf) as usize;
                    let text = GetWindowLongPtrW(hwnd, WINDOW_LONG_PTR_INDEX(USER)) as *mut String;
                    if let Some(text) = text.as_mut() {
                        *text = String::from_utf16_lossy(&buf[..n]);
                    }
                    let _ = EndDialog(hwnd, 1);
                    1
                }
                WM_CLOSE | WM_COMMAND if msg == WM_CLOSE || w.0 as u16 == IDCANCEL.0 as u16 => {
                    let _ = EndDialog(hwnd, 0);
                    1
                }
                _ => 0,
            }
        }
    }

    fn dword(words: &mut Vec<u16>, value: u32) {
        words.extend([value as u16, (value >> 16) as u16]);
    }

    let mut words = Vec::new();
    dword(
        &mut words,
        (WS_POPUP | WS_CAPTION | WS_SYSMENU).0 | (DS_CENTER | DS_MODALFRAME | DS_SETFONT) as u32,
    );
    dword(&mut words, 0);
    words.extend([3, 0, 0, 234, 54, 0, 0]); // header, no menu or class
    words.extend(tr("Поиск по имени…").encode_utf16().chain([0]));
    words.push(9);
    words.extend("Segoe UI".encode_utf16().chain([0]));
    let mut control = |style: u32, rect: [u16; 4], id: u16, class, title: &str| {
        if words.len() % 2 != 0 {
            words.push(0);
        }
        dword(&mut words, (WS_CHILD | WS_VISIBLE | WS_TABSTOP).0 | style);
        dword(&mut words, 0);
        words.extend(rect);
        words.extend([id, 0xffff, class]);
        words.extend(title.encode_utf16().chain([0, 0]));
    };
    control(
        WS_BORDER.0 | ES_AUTOHSCROLL as u32,
        [7, 7, 220, 14],
        EDIT as u16,
        0x81,
        "",
    );
    control(
        BS_DEFPUSHBUTTON as u32,
        [121, 31, 50, 15],
        IDOK.0 as u16,
        0x80,
        "OK",
    );
    control(0, [177, 31, 50, 15], IDCANCEL.0 as u16, 0x80, tr("Отмена"));
    // Both the template and its control records require DWORD alignment.
    if words.len() % 2 != 0 {
        words.push(0);
    }
    let template: Vec<u32> = words
        .as_chunks::<2>()
        .0
        .iter()
        .map(|w| u32::from(w[0]) | (u32::from(w[1]) << 16))
        .collect();
    let mut query = initial.to_owned();
    // SAFETY: the DWORD-aligned dialog template and query live until the
    // modal dialog returns. Controls use the system's predefined classes.
    let accepted = unsafe {
        DialogBoxIndirectParamW(
            None,
            template.as_ptr().cast(),
            Some(hwnd),
            Some(proc),
            LPARAM((&mut query as *mut String) as isize),
        )
    } == 1;
    accepted.then_some(query)
}

/// Command-link task dialog. Returns the chosen button id (`None` when
/// cancelled) and the state of the verification check box.
pub fn ask(
    hwnd: HWND,
    instruction: &str,
    content: &str,
    buttons: &[(i32, &str)],
    verification: Option<&str>,
) -> (Option<i32>, bool) {
    let instruction = HSTRING::from(instruction);
    let content = HSTRING::from(content);
    let labels: Vec<HSTRING> = buttons.iter().map(|(_, l)| HSTRING::from(*l)).collect();
    let items: Vec<TASKDIALOG_BUTTON> = buttons
        .iter()
        .zip(&labels)
        .map(|((id, _), l)| TASKDIALOG_BUTTON {
            nButtonID: *id,
            pszButtonText: PCWSTR(l.as_ptr()),
        })
        .collect();
    let verify = verification.map(HSTRING::from);
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        hwndParent: hwnd,
        dwFlags: TDF_USE_COMMAND_LINKS
            | TDF_ALLOW_DIALOG_CANCELLATION
            | TDF_POSITION_RELATIVE_TO_WINDOW,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: w!("Lightbox"),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: items.len() as u32,
        pButtons: items.as_ptr(),
        nDefaultButton: buttons.first().map_or(0, |b| b.0),
        pszVerificationText: verify
            .as_ref()
            .map_or(PCWSTR::null(), |v| PCWSTR(v.as_ptr())),
        ..Default::default()
    };
    let mut button = 0;
    let mut checked = BOOL(0);
    // SAFETY: every pointer in `config` refers to data alive for the call.
    let ok =
        unsafe { TaskDialogIndirect(&config, Some(&mut button), None, Some(&mut checked)).is_ok() };
    let chosen = (ok && buttons.iter().any(|b| b.0 == button)).then_some(button);
    (chosen, checked.as_bool())
}

/// JPEG qualities offered in "Save as".
const QUALITIES: [u8; 7] = [100, 95, 92, 90, 85, 80, 70];
const QUALITY_COMBO: u32 = 1;

/// "Save as" dialog: file path, format and JPEG quality.
pub fn save_as(
    hwnd: HWND,
    folder: &Path,
    name: &str,
    quality: u8,
) -> Option<(PathBuf, ExportFormat, u8)> {
    // SAFETY: COM calls on the UI thread; strings outlive the calls; the
    // returned path is freed with CoTaskMemFree.
    unsafe {
        // Harmless if COM is already initialised on this thread.
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileSaveDialog =
            CoCreateInstance(&FileSaveDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let types = [
            COMDLG_FILTERSPEC {
                pszName: w!("JPEG"),
                pszSpec: w!("*.jpg;*.jpeg"),
            },
            COMDLG_FILTERSPEC {
                pszName: w!("PNG"),
                pszSpec: w!("*.png"),
            },
        ];
        dlg.SetFileTypes(&types).ok()?;
        let png = name.to_ascii_lowercase().ends_with(".png");
        dlg.SetFileTypeIndex(if png { 2 } else { 1 }).ok()?;
        dlg.SetDefaultExtension(if png { w!("png") } else { w!("jpg") })
            .ok()?;
        let stem = Path::new(name)
            .file_stem()
            .map(|s| crate::t!("{} (копия)", s.to_string_lossy()))
            .unwrap_or_default();
        let _ = dlg.SetFileName(&HSTRING::from(stem));
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
            &HSTRING::from(folder.as_os_str()),
            None,
        ) {
            let _ = dlg.SetFolder(&item);
        }
        if let Ok(c) = dlg.cast::<IFileDialogCustomize>() {
            let _ = c.StartVisualGroup(100, &HSTRING::from(crate::i18n::tr("Качество JPEG:")));
            let _ = c.AddComboBox(QUALITY_COMBO);
            for (i, q) in QUALITIES.iter().enumerate() {
                let _ = c.AddControlItem(QUALITY_COMBO, i as u32, &HSTRING::from(q.to_string()));
            }
            let sel = QUALITIES
                .iter()
                .position(|q| *q <= quality)
                .unwrap_or(QUALITIES.len() - 1);
            let _ = c.SetSelectedControlItem(QUALITY_COMBO, sel as u32);
            let _ = c.EndVisualGroup();
        }
        dlg.Show(Some(hwnd)).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = PathBuf::from(p.to_string().ok()?);
        CoTaskMemFree(Some(p.0 as *const _));
        let format = match dlg.GetFileTypeIndex().ok()? {
            2 => ExportFormat::Png,
            _ => ExportFormat::Jpeg,
        };
        let q = dlg
            .cast::<IFileDialogCustomize>()
            .ok()
            .and_then(|c| c.GetSelectedControlItem(QUALITY_COMBO).ok())
            .and_then(|i| QUALITIES.get(i as usize).copied())
            .unwrap_or(quality);
        Some((path, format, q))
    }
}

/// "Open" dialog. With `folders`, picks a folder; otherwise a file, with
/// `filter` (name, "*.a;*.b") offered. `title` is the dialog caption.
pub fn open(
    hwnd: Option<HWND>,
    title: &str,
    folders: bool,
    filter: Option<(&str, &str)>,
    start: Option<&Path>,
) -> Option<PathBuf> {
    // SAFETY: COM calls on the calling thread; strings outlive the calls;
    // the returned path is freed with CoTaskMemFree.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let dlg: IFileOpenDialog =
            CoCreateInstance(&FileOpenDialog, None, CLSCTX_INPROC_SERVER).ok()?;
        let _ = dlg.SetTitle(&HSTRING::from(title));
        if folders {
            let opts = dlg.GetOptions().ok()?;
            dlg.SetOptions(opts | FOS_PICKFOLDERS | FOS_FORCEFILESYSTEM)
                .ok()?;
        }
        let (name, spec) = filter.map_or((HSTRING::new(), HSTRING::new()), |(n, s)| {
            (HSTRING::from(n), HSTRING::from(s))
        });
        if filter.is_some() {
            let types = [COMDLG_FILTERSPEC {
                pszName: PCWSTR(name.as_ptr()),
                pszSpec: PCWSTR(spec.as_ptr()),
            }];
            let _ = dlg.SetFileTypes(&types);
        }
        if let Some(folder) = start
            && let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
                &HSTRING::from(folder.as_os_str()),
                None,
            )
        {
            let _ = dlg.SetFolder(&item);
        }
        dlg.Show(hwnd).ok()?;
        let item = dlg.GetResult().ok()?;
        let p = item.GetDisplayName(SIGDN_FILESYSPATH).ok()?;
        let path = PathBuf::from(p.to_string().ok()?);
        CoTaskMemFree(Some(p.0 as *const _));
        Some(path)
    }
}
