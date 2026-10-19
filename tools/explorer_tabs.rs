//! Diagnostic: lists open Explorer windows, their tab windows and what
//! each shell view reports (window, visibility, selection). Read-only.
//!
//!   cargo run --release --example explorer_tabs

use windows::Win32::Foundation::HWND;
use windows::Win32::System::Com::{
    CLSCTX_ALL, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IServiceProvider,
};
use windows::Win32::System::Variant::VARIANT;
use windows::Win32::UI::Shell::{
    IFolderView, IShellBrowser, IShellItemArray, IShellWindows, IWebBrowserApp,
    SID_STopLevelBrowser, SIGDN_FILESYSPATH, SVGIO_SELECTION, ShellWindows,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetClassNameW, GetWindowTextW, IsWindowVisible,
};
use windows::core::{Interface as _, w};

fn class(h: HWND) -> String {
    let mut b = [0u16; 64];
    let n = unsafe { GetClassNameW(h, &mut b) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

fn title(h: HWND) -> String {
    let mut b = [0u16; 128];
    let n = unsafe { GetWindowTextW(h, &mut b) };
    String::from_utf16_lossy(&b[..n.max(0) as usize])
}

fn main() {
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // Top-level Explorer windows and their tab children in z-order.
        let mut top = None;
        while let Ok(win) = FindWindowExW(None, top, w!("CabinetWClass"), None) {
            top = Some(win);
            println!("window {:?} '{}'", win.0, title(win));
            let mut tab = None;
            while let Ok(t) = FindWindowExW(Some(win), tab, w!("ShellTabWindowClass"), None) {
                tab = Some(t);
                println!(
                    "  tab {:?} visible={} '{}'",
                    t.0,
                    IsWindowVisible(t).as_bool(),
                    title(t)
                );
            }
        }
        let windows: IShellWindows = CoCreateInstance(&ShellWindows, None, CLSCTX_ALL).unwrap();
        let n = windows.Count().unwrap();
        println!("IShellWindows: {n}");
        for i in 0..n {
            let Ok(d) = windows.Item(&VARIANT::from(i)) else {
                continue;
            };
            let Ok(app) = d.cast::<IWebBrowserApp>() else {
                continue;
            };
            let hwnd = app.HWND().map(|h| h.0).unwrap_or(0);
            let url = app.LocationURL().map(|s| s.to_string()).unwrap_or_default();
            print!("  [{i}] top={hwnd:#x} url={url}");
            let Ok(sp) = d.cast::<IServiceProvider>() else {
                println!(" (no service provider)");
                continue;
            };
            let Ok(browser) = sp.QueryService::<IShellBrowser>(&SID_STopLevelBrowser) else {
                println!(" (no browser)");
                continue;
            };
            let bw = browser.GetWindow().map(|h| h.0).ok();
            print!(" browser={bw:?}");
            if let Some(b) = bw {
                print!(" ({})", class(HWND(b)));
            }
            let Ok(view) = browser.QueryActiveShellView() else {
                println!(" (no view)");
                continue;
            };
            if let Ok(vw) = view.GetWindow() {
                print!(" view={:?} visible={}", vw.0, IsWindowVisible(vw).as_bool());
            }
            let sel = view
                .cast::<IFolderView>()
                .and_then(|fv| fv.Items::<IShellItemArray>(SVGIO_SELECTION))
                .and_then(|items| {
                    let n = items.GetCount()?;
                    let first = if n > 0 {
                        let p = items.GetItemAt(0)?.GetDisplayName(SIGDN_FILESYSPATH)?;
                        p.to_string().unwrap_or_default()
                    } else {
                        String::new()
                    };
                    Ok((n, first))
                });
            println!(" selection={sel:?}");
        }
    }
}
