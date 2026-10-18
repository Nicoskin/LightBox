//! Per-user registration (HKCU, no administrator rights): file types for
//! "Open with" and Default Apps, and start with Windows.

use std::path::Path;

use anyhow::{Result, bail};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_SUCCESS, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_SZ, RegDeleteKeyValueW, RegDeleteTreeW, RegSetKeyValueW,
};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::HSTRING;

use crate::APP_NAME;
use crate::decode::EXTENSIONS;

const CLASSES: &str = r"Software\Classes";
const RUN: &str = r"Software\Microsoft\Windows\CurrentVersion\Run";

fn check(e: WIN32_ERROR, what: &str) -> Result<()> {
    if e == ERROR_SUCCESS || e == ERROR_FILE_NOT_FOUND {
        Ok(())
    } else {
        bail!("{what}: error {}", e.0)
    }
}

/// Sets a string value (`name` empty = the key's default value), creating
/// the key as needed.
fn set(key: &str, name: &str, value: &str) -> Result<()> {
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated strings and a buffer of the stated size.
    let e = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    };
    check(e, key)
}

fn delete_tree(key: &str) -> Result<()> {
    // SAFETY: NUL-terminated key name.
    let e = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(key)) };
    check(e, key)
}

fn delete_value(key: &str, name: &str) -> Result<()> {
    // SAFETY: NUL-terminated names.
    let e =
        unsafe { RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name)) };
    check(e, key)
}

fn prog_id() -> String {
    format!("{APP_NAME}.Image")
}

fn exe_name(exe: &Path) -> String {
    exe.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "lightbox.exe".into())
}

fn capabilities() -> String {
    format!(r"Software\{APP_NAME}\Capabilities")
}

/// Registers `exe` as a viewer for every supported extension: it appears
/// in "Open with" and in Settings → Default apps, where the user can make
/// it the default (Windows does not let programs do that by themselves).
pub fn register(exe: &Path) -> Result<()> {
    let exe_s = exe.display().to_string();
    let open = format!("\"{exe_s}\" \"%1\"");
    let icon = format!("\"{exe_s}\",0");
    let pid = prog_id();
    let pk = format!(r"{CLASSES}\{pid}");
    set(&pk, "", &format!("Изображение ({APP_NAME})"))?;
    set(&format!(r"{pk}\DefaultIcon"), "", &icon)?;
    set(&format!(r"{pk}\shell\open\command"), "", &open)?;

    let app = format!(r"{CLASSES}\Applications\{}", exe_name(exe));
    set(&app, "FriendlyAppName", APP_NAME)?;
    set(&format!(r"{app}\shell\open\command"), "", &open)?;

    let caps = capabilities();
    set(&caps, "ApplicationName", APP_NAME)?;
    set(
        &caps,
        "ApplicationDescription",
        "Быстрый просмотрщик изображений",
    )?;
    set(&caps, "ApplicationIcon", &icon)?;
    for ext in EXTENSIONS {
        let dot = format!(".{ext}");
        set(&format!(r"{CLASSES}\{dot}\OpenWithProgids"), &pid, "")?;
        set(&format!(r"{app}\SupportedTypes"), &dot, "")?;
        set(&format!(r"{caps}\FileAssociations"), &dot, &pid)?;
    }
    set(r"Software\RegisteredApplications", APP_NAME, &caps)?;
    notify_shell();
    Ok(())
}

/// Removes everything [`register`] wrote.
pub fn unregister(exe: &Path) -> Result<()> {
    let pid = prog_id();
    for ext in EXTENSIONS {
        delete_value(&format!(r"{CLASSES}\.{ext}\OpenWithProgids"), &pid)?;
    }
    delete_tree(&format!(r"{CLASSES}\{pid}"))?;
    delete_tree(&format!(r"{CLASSES}\Applications\{}", exe_name(exe)))?;
    delete_tree(&format!(r"Software\{APP_NAME}"))?;
    delete_value(r"Software\RegisteredApplications", APP_NAME)?;
    notify_shell();
    Ok(())
}

/// Start with Windows (in the tray) or not.
pub fn set_autostart(exe: &Path, on: bool) -> Result<()> {
    if on {
        set(
            RUN,
            APP_NAME,
            &format!("\"{}\" --background", exe.display()),
        )
    } else {
        delete_value(RUN, APP_NAME)
    }
}

fn notify_shell() {
    // SAFETY: no item pointers are passed for SHCNE_ASSOCCHANGED.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}
