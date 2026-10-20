//! Lightbox installer and updater.
//!
//! Installs for the current user only (no administrator rights) into
//! `%LOCALAPPDATA%\Programs\Lightbox`, with a Start menu shortcut and an
//! entry in "Installed apps", and optionally the Explorer thumbnail handler
//! for 3D models (`lightbox_thumbs.dll`, see shellext/). Run over an
//! existing installation it updates it in place: settings, file
//! associations, the tray mode and the thumbnails choice stay.
//!
//!   lightbox-setup.exe                 interactive install / update
//!   lightbox-setup.exe /update [file]  silent update (started by the
//!                                      viewer), then opens `file` again

#![windows_subsystem = "windows"]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use windows::Win32::Foundation::{CloseHandle, FreeLibrary, HWND, LPARAM, WPARAM};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, IPersistFile,
};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RRF_RT_REG_SZ, RegGetValueW, RegSetKeyValueW,
};
use windows::Win32::System::Threading::{
    OpenProcess, PROCESS_NAME_WIN32, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Controls::{
    TASKDIALOG_BUTTON, TASKDIALOGCONFIG, TDCBF_CANCEL_BUTTON, TDF_ALLOW_DIALOG_CANCELLATION,
    TDF_USE_COMMAND_LINKS, TaskDialogIndirect,
};
use windows::Win32::UI::Shell::{IShellLinkW, ShellExecuteW, ShellLink};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowExW, GetWindowThreadProcessId, MB_ICONERROR, MB_OK, MessageBoxW, PostMessageW,
    RegisterWindowMessageW, SW_SHOWNORMAL,
};
use windows::core::{BOOL, HRESULT, HSTRING, Interface as _, PCSTR, PCWSTR, PWSTR, s, w};

const APP: &str = "Lightbox";
const VERSION: &str = env!("LIGHTBOX_VERSION");
/// The viewer this installer installs.
static PAYLOAD: &[u8] = include_bytes!(env!("LIGHTBOX_EXE"));
/// Explorer thumbnails for 3D models, installed next to the viewer.
static THUMBS: &[u8] = include_bytes!(env!("LIGHTBOX_THUMBS_DLL"));
const THUMBS_DLL: &str = "lightbox_thumbs.dll";
/// The thumbnail handler's class (shellext/src/register.rs).
const THUMBS_CLSID: &str = "{BE34661C-1FC5-4BE3-A5DA-4DE11DD6BD54}";

const ID_ALL: i32 = 101;
const ID_ONLY: i32 = 102;
const ID_UPDATE: i32 = 103;
const ID_THUMBS_ON: i32 = 201;
const ID_THUMBS_OFF: i32 = 202;

/// What the user chose.
struct Choice {
    /// Open the default apps page to make Lightbox the default viewer.
    defaults: bool,
    /// Start with Windows, in the tray.
    tray: bool,
    /// Explorer thumbnails for 3D models.
    thumbs: bool,
}

fn main() {
    let mut update = false;
    let mut open = None;
    for a in std::env::args_os().skip(1) {
        match a.to_str() {
            Some("/update") => update = true,
            _ => open = Some(PathBuf::from(a)),
        }
    }
    if let Err(e) = run(update, open) {
        let text = HSTRING::from(format!(
            "{} {APP}:\n{e}",
            l("Не удалось установить", "Could not install")
        ));
        // SAFETY: plain message box.
        unsafe { MessageBoxW(None, &text, &HSTRING::from(APP), MB_OK | MB_ICONERROR) };
        std::process::exit(1);
    }
}

/// Russian for a Russian Windows, English otherwise (as in the viewer).
fn ru() -> bool {
    // SAFETY: no preconditions.
    let lang = unsafe { windows::Win32::Globalization::GetUserDefaultUILanguage() };
    lang & 0x3ff == 0x19 // LANG_RUSSIAN
}

/// The text in the installer's language.
fn l(ru_text: &'static str, en: &'static str) -> &'static str {
    if ru() { ru_text } else { en }
}

fn run(update: bool, open: Option<PathBuf>) -> Result<(), String> {
    let dir = install_dir().ok_or(l("нет папки LOCALAPPDATA", "no LOCALAPPDATA folder"))?;
    let exe = dir.join("lightbox.exe");
    let installed = reg_get(&uninstall_key(), "DisplayVersion");

    let had_thumbs = thumbs_registered();

    // What to do.
    let Choice {
        defaults,
        tray,
        thumbs,
    } = if update {
        Choice {
            defaults: false,
            tray: false,
            thumbs: had_thumbs,
        }
    } else {
        // Suggested on: new installations and those from before the option
        // (no DLL yet); otherwise as it was.
        let suggest = had_thumbs || !dir.join(THUMBS_DLL).exists();
        match ask(installed.as_deref(), suggest) {
            None => return Ok(()),
            Some(choice) => choice,
        }
    };

    // Close running copies (a viewer in the tray included) and replace the
    // program and the thumbnail handler (which Explorer may have loaded).
    let was_running = quit_instances(&exe, Duration::from_secs(4));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    cleanup_old(&dir);
    replace(&exe, PAYLOAD)?;
    let dll = dir.join(THUMBS_DLL);
    replace(&dll, THUMBS)?;

    shortcut(&exe).map_err(|e| format!("{}: {e}", l("ярлык", "shortcut")))?;
    uninstall_entry(&dir, &exe)?;
    // Registered again on updates too: refreshes the DLL's path.
    let thumbs_error = if thumbs {
        call_dll(&dll, s!("DllRegisterServer")).err()
    } else if had_thumbs {
        call_dll(&dll, s!("DllUnregisterServer")).err()
    } else {
        None
    };
    // File types for "Open with" and Default Apps (also refreshes the path).
    let _ = Command::new(&exe).arg("--register").status();
    if tray {
        let _ = Command::new(&exe).arg("--enable-tray").status();
    }
    if defaults {
        // Windows lets only the user choose defaults: open its page for
        // this program, where one click makes it the default for all.
        let uri = HSTRING::from(format!("ms-settings:defaultapps?registeredAppUser={APP}"));
        // SAFETY: plain shell execute of a settings URI.
        unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                &uri,
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            );
        }
    }

    if update {
        // Back to where the user was.
        if was_running || tray_enabled() {
            let _ = Command::new(&exe).arg("--background").spawn();
        }
        if let Some(file) = open {
            let _ = Command::new(&exe).arg(file).spawn();
        }
    } else {
        done(installed.is_some(), thumbs && !had_thumbs, thumbs_error);
    }
    Ok(())
}

/// Writes `bytes` to `path`. A running exe or a loaded DLL can be renamed
/// but not overwritten: the old file moves aside and is deleted now if it
/// is free, else on the next installation.
fn replace(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if path.exists() {
        let mut old = path.as_os_str().to_owned();
        old.push(format!(".old-{}", std::process::id()));
        std::fs::rename(path, &old).map_err(|e| format!("{}: {e}", path.display()))?;
        let _ = std::fs::remove_file(&old);
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

/// `true` if the thumbnail handler is registered for this user.
fn thumbs_registered() -> bool {
    reg_get(
        &format!(r"Software\Classes\CLSID\{THUMBS_CLSID}\InprocServer32"),
        "",
    )
    .is_some()
}

/// Calls the thumbnail handler's `DllRegisterServer` /
/// `DllUnregisterServer` (registration lives in the DLL: shellext/).
fn call_dll(dll: &Path, export: PCSTR) -> Result<(), String> {
    // SAFETY: our own DLL; the export has the documented COM signature
    // `HRESULT __stdcall fn(void)`, and the library is freed after the call.
    unsafe {
        let h = LoadLibraryW(&HSTRING::from(dll.as_os_str())).map_err(|e| e.message())?;
        let r = match GetProcAddress(h, export) {
            Some(f) => {
                let f: unsafe extern "system" fn() -> HRESULT = std::mem::transmute(f);
                f().ok().map_err(|e| e.message())
            }
            None => Err(format!(
                "{} {}",
                l("нет функции", "no function"),
                export.display()
            )),
        };
        let _ = FreeLibrary(h);
        r
    }
}

fn install_dir() -> Option<PathBuf> {
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join("Programs").join(APP))
}

fn uninstall_key() -> String {
    format!(r"Software\Microsoft\Windows\CurrentVersion\Uninstall\{APP}")
}

/// `true` if the viewer starts with Windows (its Run entry exists).
fn tray_enabled() -> bool {
    reg_get(r"Software\Microsoft\Windows\CurrentVersion\Run", APP).is_some()
}

/// Install / update dialog; `thumbs` is the thumbnails option's default.
fn ask(installed: Option<&str>, thumbs: bool) -> Option<Choice> {
    let (instruction, content, buttons): (String, String, Vec<(i32, &str)>) = match installed {
        Some(old) => (
            format!("{} {APP} {old} → {VERSION}", l("Обновить", "Update")),
            l(
                "Настройки, ассоциации файлов и режим в трее сохранятся.",
                "Settings, file associations and the tray mode are kept.",
            )
            .into(),
            vec![(ID_UPDATE, l("Обновить", "Update"))],
        ),
        None => (
            format!("{} {APP} {VERSION}", l("Установить", "Install")),
            l(
                "Быстрый просмотрщик изображений. Устанавливается только для вас, \
                 права администратора не нужны.",
                "A fast image viewer. Installed for you only; no administrator \
                 rights needed.",
            )
            .into(),
            vec![
                (
                    ID_ALL,
                    l(
                        "Установить и открывать им все изображения\n\
                         Windows покажет страницу «Приложения по умолчанию» — \
                         подтвердите выбор одним нажатием",
                        "Install and open all images with it\n\
                         Windows shows the Default apps page — confirm the \
                         choice with one click",
                    ),
                ),
                (
                    ID_ONLY,
                    l(
                        "Только установить\nПрограмма появится в «Открыть с помощью»",
                        "Install only\nIt appears in “Open with”",
                    ),
                ),
            ],
        ),
    };
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
    let verify = HSTRING::from(l(
        "Запускать при входе в Windows (в трее: мгновенное открытие и просмотр по пробелу в проводнике)",
        "Start when Windows starts (in the tray: instant opening and Space preview in Explorer)",
    ));
    let title = HSTRING::from(l("Установка Lightbox", "Lightbox Setup"));
    let thumbs_on = HSTRING::from(l(
        "Показывать миниатюры 3D-моделей в Проводнике (STL, OBJ, 3MF)",
        "Show 3D model thumbnails in Explorer (STL, OBJ, 3MF)",
    ));
    let thumbs_off = HSTRING::from(l(
        "Не показывать миниатюры 3D-моделей",
        "Don't show 3D model thumbnails",
    ));
    let radios = [
        TASKDIALOG_BUTTON {
            nButtonID: ID_THUMBS_ON,
            pszButtonText: PCWSTR(thumbs_on.as_ptr()),
        },
        TASKDIALOG_BUTTON {
            nButtonID: ID_THUMBS_OFF,
            pszButtonText: PCWSTR(thumbs_off.as_ptr()),
        },
    ];
    let config = TASKDIALOGCONFIG {
        cbSize: std::mem::size_of::<TASKDIALOGCONFIG>() as u32,
        dwFlags: TDF_USE_COMMAND_LINKS | TDF_ALLOW_DIALOG_CANCELLATION,
        dwCommonButtons: TDCBF_CANCEL_BUTTON,
        pszWindowTitle: PCWSTR(title.as_ptr()),
        pszMainInstruction: PCWSTR(instruction.as_ptr()),
        pszContent: PCWSTR(content.as_ptr()),
        cButtons: items.len() as u32,
        pButtons: items.as_ptr(),
        nDefaultButton: buttons[0].0,
        cRadioButtons: radios.len() as u32,
        pRadioButtons: radios.as_ptr(),
        nDefaultRadioButton: if thumbs { ID_THUMBS_ON } else { ID_THUMBS_OFF },
        pszVerificationText: if installed.is_some() {
            PCWSTR::null()
        } else {
            PCWSTR(verify.as_ptr())
        },
        ..Default::default()
    };
    let mut button = 0;
    let mut radio = 0;
    let mut checked = BOOL(0);
    // SAFETY: every pointer in `config` refers to data alive for the call.
    unsafe {
        TaskDialogIndirect(
            &config,
            Some(&mut button),
            Some(&mut radio),
            Some(&mut checked),
        )
    }
    .ok()?;
    let (defaults, tray) = match button {
        ID_ALL => (true, checked.as_bool()),
        ID_ONLY => (false, checked.as_bool()),
        ID_UPDATE => (false, false),
        _ => return None,
    };
    Some(Choice {
        defaults,
        tray,
        thumbs: radio == ID_THUMBS_ON,
    })
}

/// `thumbs_on`: the thumbnails were just turned on; `thumbs_error`: they
/// could not be turned on or off.
fn done(updated: bool, thumbs_on: bool, thumbs_error: Option<String>) {
    let mut text = match (updated, ru()) {
        (true, true) => format!("{APP} обновлён до версии {VERSION}."),
        (true, false) => format!("{APP} has been updated to {VERSION}."),
        (false, true) => format!(
            "{APP} {VERSION} установлен.\n\nОн есть в меню «Пуск» и в «Открыть с помощью». \
             Удалить его можно в «Установленных приложениях»."
        ),
        (false, false) => format!(
            "{APP} {VERSION} is installed.\n\nIt is in the Start menu and in “Open with”. \
             You can remove it in Installed apps."
        ),
    };
    if let Some(e) = thumbs_error {
        text.push_str(&format!(
            "\n\n{}: {e}",
            l(
                "Миниатюры 3D-моделей не настроены",
                "3D model thumbnails were not set up"
            )
        ));
    } else if thumbs_on {
        // Explorer runs thumbnail handlers in a separate host, which sees a
        // newly registered per-user class only after signing in again.
        text.push_str(l(
            "\n\nМиниатюры 3D-моделей появятся в Проводнике после повторного входа в Windows. \
             Если он показывает старые, очистите эскизы: «Очистка диска» → «Эскизы».",
            "\n\n3D model thumbnails appear in Explorer after signing in to Windows again. \
             If it shows old ones, clear them: Disk Cleanup → Thumbnails.",
        ));
    }
    // SAFETY: plain message box.
    unsafe { MessageBoxW(None, &HSTRING::from(text), &HSTRING::from(APP), MB_OK) };
}

/// Old copies moved aside by earlier updates.
fn cleanup_old(dir: &Path) {
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if name.starts_with("lightbox.exe.old")
                || name.starts_with(&format!("{THUMBS_DLL}.old"))
            {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.to_string_lossy()
        .eq_ignore_ascii_case(&b.to_string_lossy())
}

/// Asks viewers started from `exe` to quit; waits up to `wait`. Returns
/// `true` if any was running.
fn quit_instances(exe: &Path, wait: Duration) -> bool {
    // SAFETY: registering a message name has no preconditions.
    let msg = unsafe { RegisterWindowMessageW(w!("Lightbox.Quit")) };
    let deadline = Instant::now() + wait;
    let mut any = false;
    loop {
        let mut found = false;
        let mut h = None;
        // SAFETY: plain window enumeration by class name.
        while let Ok(win) = unsafe { FindWindowExW(None, h, w!("LightboxViewer"), None) } {
            h = Some(win);
            if !process_path(win).is_some_and(|p| same_file(&p, exe)) {
                continue;
            }
            found = true;
            any = true;
            // SAFETY: posting a registered message.
            unsafe {
                let _ = PostMessageW(Some(win), msg, WPARAM(0), LPARAM(0));
            }
        }
        if !found || Instant::now() >= deadline {
            // Let the last one finish exiting.
            std::thread::sleep(Duration::from_millis(if any { 300 } else { 0 }));
            return any;
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

fn process_path(hwnd: HWND) -> Option<PathBuf> {
    let mut pid = 0;
    // SAFETY: plain queries; the handle is closed here.
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

/// Start menu shortcut.
fn shortcut(exe: &Path) -> windows::core::Result<()> {
    let Some(appdata) = std::env::var_os("APPDATA") else {
        return Ok(());
    };
    let link = PathBuf::from(appdata)
        .join(r"Microsoft\Windows\Start Menu\Programs")
        .join(format!("{APP}.lnk"));
    // SAFETY: COM on this thread; strings live for the calls.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        let sl: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        sl.SetPath(&HSTRING::from(exe.as_os_str()))?;
        sl.SetDescription(&HSTRING::from(l(
            "Быстрый просмотрщик изображений",
            "Fast image viewer",
        )))?;
        sl.SetIconLocation(&HSTRING::from(exe.as_os_str()), 0)?;
        let pf: IPersistFile = sl.cast()?;
        pf.Save(&HSTRING::from(link.as_os_str()), true)?;
    }
    Ok(())
}

/// The entry in Settings → Installed apps.
fn uninstall_entry(dir: &Path, exe: &Path) -> Result<(), String> {
    let key = uninstall_key();
    let exe_s = exe.display().to_string();
    for (name, value) in [
        ("DisplayName", APP.to_string()),
        ("DisplayVersion", VERSION.to_string()),
        ("Publisher", APP.to_string()),
        ("DisplayIcon", format!("\"{exe_s}\",0")),
        ("InstallLocation", dir.display().to_string()),
        ("UninstallString", format!("\"{exe_s}\" --uninstall")),
    ] {
        reg_set_sz(&key, name, &value)?;
    }
    reg_set_dword(&key, "NoModify", 1)?;
    reg_set_dword(&key, "NoRepair", 1)?;
    let size = PAYLOAD.len() + THUMBS.len();
    reg_set_dword(&key, "EstimatedSize", (size / 1024) as u32)?;
    Ok(())
}

fn reg_set_sz(key: &str, name: &str, value: &str) -> Result<(), String> {
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
    e.ok().map_err(|e| format!("{key}: {e}"))
}

fn reg_set_dword(key: &str, name: &str, value: u32) -> Result<(), String> {
    // SAFETY: a 4-byte value.
    let e = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            REG_DWORD.0,
            Some(std::ptr::from_ref(&value).cast()),
            4,
        )
    };
    e.ok().map_err(|e| format!("{key}: {e}"))
}

fn reg_get(key: &str, name: &str) -> Option<String> {
    let mut buf = [0u16; 512];
    let mut size = (buf.len() * 2) as u32;
    // SAFETY: the buffer and its size are valid for the call.
    let e = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    e.is_ok().then(|| {
        let n = (size as usize / 2).saturating_sub(1);
        String::from_utf16_lossy(&buf[..n])
    })
}
