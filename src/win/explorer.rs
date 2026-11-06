//! Reads the order of an existing Explorer folder view without changing it.

use std::path::{Path, PathBuf};
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoTaskMemFree,
    CoUninitialize, IServiceProvider,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IFolderView, IPersistFolder2, IShellBrowser, IShellItemArray, IShellWindows, IWebBrowserApp,
    SHGetNameFromIDList, SID_STopLevelBrowser, SIGDN_FILESYSPATH, SVGIO_ALLVIEW,
    SVGIO_FLAG_VIEWORDER, ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::FindWindowExW;
use windows::core::{Interface as _, PWSTR, w};

pub fn folder_order(dir: &Path) -> Option<Vec<PathBuf>> {
    // SAFETY: this runs on the scan worker. COM interfaces and shell-allocated
    // paths are released on that thread before its apartment is torn down.
    unsafe {
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok().ok()?;
        let order = read_order(dir);
        CoUninitialize();
        order
    }
}

unsafe fn take_path(p: PWSTR) -> Option<PathBuf> {
    // SAFETY: a NUL-terminated shell string, freed once after copying it.
    unsafe {
        let path = p.to_string().ok().map(PathBuf::from);
        CoTaskMemFree(Some(p.0.cast()));
        path
    }
}

unsafe fn read_order(dir: &Path) -> Option<Vec<PathBuf>> {
    use windows::Win32::Foundation::HWND;
    // SAFETY: COM initialized by folder_order; each PIDL is freed after use.
    unsafe {
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).ok()?;
        let mut best = None;
        for i in 0..windows.Count().ok()? {
            let Ok(window) = windows.Item(&VARIANT::from(i)) else {
                continue;
            };
            let Some((browser, view)) = (|| {
                let sp = window.cast::<IServiceProvider>().ok()?;
                let browser = sp
                    .QueryService::<IShellBrowser>(&SID_STopLevelBrowser)
                    .ok()?;
                let view = browser
                    .QueryActiveShellView()
                    .ok()?
                    .cast::<IFolderView>()
                    .ok()?;
                let folder = view.GetFolder::<IPersistFolder2>().ok()?;
                let pidl = folder.GetCurFolder().ok()?;
                let path = SHGetNameFromIDList(pidl, SIGDN_FILESYSPATH)
                    .ok()
                    .and_then(|p| take_path(p));
                CoTaskMemFree(Some(pidl.cast()));
                crate::library::same_path(&path?, dir).then_some((browser, view))
            })() else {
                continue;
            };
            let active_tab = window
                .cast::<IWebBrowserApp>()
                .ok()
                .and_then(|w| w.HWND().ok())
                .and_then(|h| {
                    FindWindowExW(
                        Some(HWND(h.0 as *mut _)),
                        None,
                        w!("ShellTabWindowClass"),
                        None,
                    )
                    .ok()
                });
            let active = active_tab.is_none_or(|tab| browser.GetWindow().ok() == Some(tab));
            if best.is_some() && !active {
                continue;
            }
            let Ok(items) = view.Items::<IShellItemArray>(SVGIO_ALLVIEW | SVGIO_FLAG_VIEWORDER)
            else {
                continue;
            };
            let Ok(count) = items.GetCount() else {
                continue;
            };
            let paths = (0..count)
                .filter_map(|i| {
                    let item = items.GetItemAt(i).ok()?;
                    take_path(item.GetDisplayName(SIGDN_FILESYSPATH).ok()?)
                })
                .collect();
            if active {
                return Some(paths);
            }
            best = Some(paths);
        }
        best
    }
}
