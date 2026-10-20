//! Registration for the current user only (HKCU, no administrator rights):
//! the COM class, and the class as the thumbnail handler of each model
//! extension. Another program's handler found there is saved and put back
//! on unregistration; unregistration removes only what was written here.

use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, WIN32_ERROR};
use windows::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_SZ, RRF_RT_REG_SZ, RegCloseKey, RegDeleteKeyValueW,
    RegDeleteKeyW, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW, RegSetKeyValueW,
};
use windows::Win32::UI::Shell::{SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify};
use windows::core::{GUID, HSTRING, Result};

/// The thumbnail handler's class. The installer checks the same string.
pub const CLSID: GUID = GUID::from_u128(0xbe34661c_1fc5_4be3_a5da_4de11dd6bd54);
const CLSID_STR: &str = "{BE34661C-1FC5-4BE3-A5DA-4DE11DD6BD54}";
/// IThumbnailProvider's shell extension slot.
const THUMBNAIL_HANDLER: &str = "{e357fccd-a995-4576-b01f-234630154e96}";
const EXTENSIONS: [&str; 3] = [".stl", ".obj", ".3mf"];
/// The handlers we replaced, by extension.
const BACKUP: &str = r"Software\Lightbox\ThumbnailBackup";

fn class_key() -> String {
    format!(r"Software\Classes\CLSID\{CLSID_STR}")
}

fn handler_key(ext: &str) -> String {
    format!(r"Software\Classes\{ext}\ShellEx\{THUMBNAIL_HANDLER}")
}

fn ours(value: &str) -> bool {
    value.eq_ignore_ascii_case(CLSID_STR)
}

/// Registers `dll` as the thumbnail handler of STL, OBJ and 3MF files.
pub fn register(dll: &str) -> Result<()> {
    let class = class_key();
    let server = format!(r"{class}\InprocServer32");
    set(&class, "", "Lightbox 3D model thumbnails")?;
    set(&server, "", dll)?;
    set(&server, "ThreadingModel", "Apartment")?;
    for ext in EXTENSIONS {
        let key = handler_key(ext);
        match get(&key, "") {
            Some(old) if ours(&old) => {}
            // Another program's: the latest one is put back later.
            Some(old) => set(BACKUP, ext, &old)?,
            None => delete_value(BACKUP, ext),
        }
        set(&key, "", CLSID_STR)?;
    }
    notify();
    Ok(())
}

/// Undoes [`register`]: our handler entries go, replaced ones come back.
pub fn unregister() -> Result<()> {
    for ext in EXTENSIONS {
        let key = handler_key(ext);
        if get(&key, "").is_some_and(|v| ours(&v)) {
            match get(BACKUP, ext) {
                Some(old) => set(&key, "", &old)?,
                None => {
                    delete_value(&key, "");
                    delete_if_empty(&key);
                    delete_if_empty(&format!(r"Software\Classes\{ext}\ShellEx"));
                    delete_if_empty(&format!(r"Software\Classes\{ext}"));
                }
            }
        }
        delete_value(BACKUP, ext);
    }
    delete_if_empty(BACKUP);
    delete_if_empty(r"Software\Lightbox");
    // SAFETY: deletes our own class key.
    let e = unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, &HSTRING::from(class_key())) };
    if e != ERROR_FILE_NOT_FOUND {
        e.ok()?;
    }
    notify();
    Ok(())
}

/// Tells Explorer that handlers changed.
fn notify() {
    // SAFETY: a notification without items.
    unsafe { SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None) };
}

fn set(key: &str, name: &str, value: &str) -> Result<()> {
    let data: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: NUL-terminated strings and a buffer of the stated size.
    unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            &HSTRING::from(key),
            &HSTRING::from(name),
            REG_SZ.0,
            Some(data.as_ptr().cast()),
            (data.len() * 2) as u32,
        )
    }
    .ok()
}

fn get(key: &str, name: &str) -> Option<String> {
    let mut buf = [0u16; 1024];
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

fn delete_value(key: &str, name: &str) {
    // SAFETY: plain registry call; a missing value is fine.
    unsafe {
        let _ = RegDeleteKeyValueW(HKEY_CURRENT_USER, &HSTRING::from(key), &HSTRING::from(name));
    }
}

/// Deletes `key` if it has neither values nor subkeys.
fn delete_if_empty(key: &str) {
    let key = HSTRING::from(key);
    let mut h = HKEY::default();
    let (mut subkeys, mut values) = (1u32, 1u32);
    // SAFETY: the handle is closed here; counts are plain out-parameters.
    unsafe {
        if RegOpenKeyExW(HKEY_CURRENT_USER, &key, None, KEY_READ, &mut h) != WIN32_ERROR(0) {
            return;
        }
        let e = RegQueryInfoKeyW(
            h,
            None,
            None,
            None,
            Some(&mut subkeys),
            None,
            None,
            Some(&mut values),
            None,
            None,
            None,
            None,
        );
        let _ = RegCloseKey(h);
        if e.is_ok() && subkeys == 0 && values == 0 {
            let _ = RegDeleteKeyW(HKEY_CURRENT_USER, &key);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clsid_string_matches() {
        assert_eq!(format!("{{{CLSID:?}}}"), CLSID_STR);
    }
}
