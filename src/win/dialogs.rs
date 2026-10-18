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
    FileSaveDialog, IFileDialogCustomize, IFileSaveDialog, IShellItem, SHCreateItemFromParsingName,
    SIGDN_FILESYSPATH,
};
use windows::core::{BOOL, HSTRING, Interface as _, PCWSTR, w};

use crate::edit::ExportFormat;

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
            .map(|s| format!("{} (копия)", s.to_string_lossy()))
            .unwrap_or_default();
        let _ = dlg.SetFileName(&HSTRING::from(stem));
        if let Ok(item) = SHCreateItemFromParsingName::<_, _, IShellItem>(
            &HSTRING::from(folder.as_os_str()),
            None,
        ) {
            let _ = dlg.SetFolder(&item);
        }
        if let Ok(c) = dlg.cast::<IFileDialogCustomize>() {
            let _ = c.StartVisualGroup(100, w!("Качество JPEG:"));
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
