//! Where the installer puts the program, and `--uninstall` (run from
//! "Installed apps"): closes running copies, removes the file types, the
//! shortcut, the Explorer thumbnail handler, the uninstall entry and the
//! program folder.

use std::os::windows::process::CommandExt;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::{CloseHandle, FreeLibrary, HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{HKEY_CURRENT_USER, RegDeleteTreeW};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowThreadProcessId, MB_ICONINFORMATION, MB_OK, MessageBoxW, PostMessageW,
};
use windows::core::{HRESULT, HSTRING, PWSTR, s, w};

use crate::APP_NAME;
use crate::win::{dialogs, registry};

/// `%LOCALAPPDATA%\Programs\Lightbox`.
pub fn install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join(APP_NAME))
}

/// `…\Start Menu\Programs\Lightbox.lnk`.
pub fn start_menu_link() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|d| {
        PathBuf::from(d)
            .join(r"Microsoft\Windows\Start Menu\Programs")
            .join(format!("{APP_NAME}.lnk"))
    })
}

/// The "Installed apps" entry.
pub fn uninstall_key() -> String {
    format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{APP_NAME}")
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Image path of the process owning `hwnd`.
fn process_path(hwnd: HWND) -> Option<PathBuf> {
    let mut pid = 0;
    // SAFETY: plain queries; the process handle is closed here.
    unsafe {
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid).ok()?;
        let mut buf = [0u16; 1024];
        let mut len = buf.len() as u32;
        let ok =
            QueryFullProcessImageNameW(h, PROCESS_NAME_WIN32, PWSTR(buf.as_mut_ptr()), &mut len);
        let _ = CloseHandle(h);
        ok.ok()?;
        Some(PathBuf::from(String::from_utf16_lossy(
            &buf[..len as usize],
        )))
    }
}

/// Asks every running viewer started from `exe` (except this process) to
/// quit, and waits up to `wait` for them to go.
pub fn quit_instances(exe: &Path, wait: Duration) {
    let msg = crate::app::quit_message();
    let me = std::process::id();
    let deadline = Instant::now() + wait;
    loop {
        let mut found = false;
        let mut h = None;
        // SAFETY: plain window enumeration by class name.
        while let Ok(w) = unsafe { FindWindowExW(None, h, w!("LightboxViewer"), None) } {
            h = Some(w);
            let mut pid = 0;
            // SAFETY: valid out-pointer.
            unsafe { GetWindowThreadProcessId(w, Some(&mut pid)) };
            if pid == me || !process_path(w).is_some_and(|p| same_file(&p, exe)) {
                continue;
            }
            found = true;
            // SAFETY: posting a registered message to another window.
            unsafe {
                let _ = PostMessageW(Some(w), msg, WPARAM(0), LPARAM(0));
            }
        }
        if !found || Instant::now() >= deadline {
            return;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Removes the Explorer thumbnail handler the installer put next to `exe`
/// (shellext/), which also brings back the handlers it replaced.
fn unregister_thumbnails(exe: &Path) {
    let Some(dll) = exe.parent().map(|d| d.join("lightbox_thumbs.dll")) else {
        return;
    };
    if !dll.exists() {
        return;
    }
    // SAFETY: our own DLL; `DllUnregisterServer` is `HRESULT __stdcall
    // fn(void)`, and the library is freed after the call.
    unsafe {
        let Ok(h) = LoadLibraryW(&HSTRING::from(dll.as_os_str())) else {
            return;
        };
        if let Some(f) = GetProcAddress(h, s!("DllUnregisterServer")) {
            let f: unsafe extern "system" fn() -> HRESULT = std::mem::transmute(f);
            let _ = f();
        }
        let _ = FreeLibrary(h);
    }
}

/// `--uninstall`.
pub fn uninstall() -> Result<()> {
    let exe = std::env::current_exe()?;
    let (choice, remove_settings) = dialogs::ask(
        HWND::default(),
        &format!("Удалить {APP_NAME}?"),
        "Программа, её ярлык и ассоциации файлов будут удалены.",
        &[(1, "Удалить")],
        Some("Удалить и настройки"),
    );
    if choice != Some(1) {
        return Ok(());
    }
    quit_instances(&exe, Duration::from_secs(3));
    unregister_thumbnails(&exe);
    registry::unregister(&exe)?;
    registry::set_autostart(&exe, false)?;
    if let Some(link) = start_menu_link() {
        let _ = std::fs::remove_file(link);
    }
    // SAFETY: deletes our own uninstall entry.
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(uninstall_key()));
    }
    if remove_settings && let Some(appdata) = std::env::var_os("APPDATA") {
        let _ = std::fs::remove_dir_all(PathBuf::from(appdata).join(APP_NAME));
    }
    // The program folder (only the real installation, never a build
    // folder): removed by a helper once this process has exited.
    if let (Some(dir), Some(parent)) = (install_dir(), exe.parent())
        && same_file(&dir, parent)
    {
        let script = format!(
            "ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"",
            dir.display()
        );
        let _ = std::process::Command::new("cmd")
            .args(["/c", &script])
            .creation_flags(0x0800_0000) // CREATE_NO_WINDOW
            .spawn();
    }
    // SAFETY: plain message box.
    unsafe {
        MessageBoxW(
            None,
            &HSTRING::from(format!("{APP_NAME} удалён.")),
            &HSTRING::from(APP_NAME),
            MB_OK | MB_ICONINFORMATION,
        );
    }
    Ok(())
}
