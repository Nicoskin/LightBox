//! Star ratings and keywords through the Windows Property System
//! (System.Rating, System.Keywords): Windows writes them into the file's
//! metadata (XMP xmp:Rating / dc:subject and the Microsoft tags), where
//! Explorer, Lightroom and other programs read them. Blocking: call from a
//! worker thread.

use std::path::Path;

use crate::i18n::tr;
use windows::Win32::Foundation::PROPERTYKEY;
use windows::Win32::Storage::EnhancedStorage::{PKEY_Keywords, PKEY_Rating};
use windows::Win32::System::Com::StructuredStorage::{
    InitPropVariantFromStringVector, PROPVARIANT, PropVariantToStringVectorAlloc,
};
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoTaskMemFree};
use windows::Win32::UI::Shell::PropertiesSystem::{
    GETPROPERTYSTOREFLAGS, GPS_DEFAULT, GPS_READWRITE, IPropertyStore,
    SHGetPropertyStoreFromParsingName,
};
use windows::core::{HSTRING, PCWSTR};

/// Stars ↔ the 0–99 scale Windows uses.
pub fn to_percent(stars: u8) -> u32 {
    match stars {
        0 => 0,
        1 => 1,
        2 => 25,
        3 => 50,
        4 => 75,
        _ => 99,
    }
}

pub fn to_stars(percent: u32) -> u8 {
    match percent {
        0 => 0,
        1..=12 => 1,
        13..=37 => 2,
        38..=62 => 3,
        63..=87 => 4,
        _ => 5,
    }
}

fn store(path: &Path, flags: GETPROPERTYSTOREFLAGS) -> windows::core::Result<IPropertyStore> {
    // SAFETY: COM for this worker thread; a plain shell call.
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        SHGetPropertyStoreFromParsingName(&HSTRING::from(path.as_os_str()), None, flags)
    }
}

fn get(path: &Path, key: &PROPERTYKEY) -> Option<PROPVARIANT> {
    let s = store(path, GPS_DEFAULT).ok()?;
    // SAFETY: valid store and key.
    unsafe { s.GetValue(key).ok() }
}

/// The file's rating in stars (0 = none), if its format has one.
pub fn read(path: &Path) -> Option<u8> {
    let v = get(path, &PKEY_Rating)?;
    if v.is_empty() {
        return Some(0);
    }
    u32::try_from(&v).ok().map(to_stars)
}

/// The file's keywords (tags).
pub fn keywords(path: &Path) -> Vec<String> {
    let Some(v) = get(path, &PKEY_Keywords) else {
        return Vec::new();
    };
    if v.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    // SAFETY: the allocated array and its strings are freed here.
    unsafe {
        let mut arr = std::ptr::null_mut();
        let mut n = 0;
        if PropVariantToStringVectorAlloc(&v, &mut arr, &mut n).is_ok() && !arr.is_null() {
            for i in 0..n as usize {
                let s = *arr.add(i);
                out.push(s.to_string().unwrap_or_default());
                CoTaskMemFree(Some(s.0 as *const _));
            }
            CoTaskMemFree(Some(arr as *const _));
        }
    }
    out
}

fn write(
    path: &Path,
    key: &PROPERTYKEY,
    value: &PROPVARIANT,
    keep_date: bool,
) -> Result<(), String> {
    let modified = std::fs::metadata(path).and_then(|m| m.modified()).ok();
    let s = store(path, GPS_READWRITE).map_err(|_| unsupported())?;
    // SAFETY: valid store, key and value.
    unsafe {
        s.SetValue(key, value).map_err(|_| unsupported())?;
        s.Commit().map_err(|e| e.message())?;
    }
    drop(s);
    if keep_date && let Some(t) = modified {
        let _ = std::fs::File::options()
            .write(true)
            .open(path)
            .and_then(|f| f.set_modified(t));
    }
    Ok(())
}

fn unsupported() -> String {
    tr("этот формат не хранит рейтинг и метки").into()
}

/// Sets the rating (0 clears it).
pub fn set(path: &Path, stars: u8, keep_date: bool) -> Result<(), String> {
    write(
        path,
        &PKEY_Rating,
        &PROPVARIANT::from(to_percent(stars)),
        keep_date,
    )
}

/// Replaces the keywords.
pub fn set_keywords(path: &Path, words: &[String], keep_date: bool) -> Result<(), String> {
    let wide: Vec<HSTRING> = words.iter().map(HSTRING::from).collect();
    let ptrs: Vec<PCWSTR> = wide.iter().map(|w| PCWSTR(w.as_ptr())).collect();
    // SAFETY: the strings outlive the call; the value copies them.
    let value = unsafe { InitPropVariantFromStringVector(Some(&ptrs)) }.map_err(|e| e.message())?;
    write(path, &PKEY_Keywords, &value, keep_date)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stars_round_trip() {
        for s in 0..=5 {
            assert_eq!(to_stars(to_percent(s)), s);
        }
    }

    /// Writes a rating and tags into a copy of a JPEG and reads them back.
    #[test]
    fn jpeg_keeps_rating_and_keywords() {
        let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/camera_exif.jpg");
        if !src.exists() {
            return;
        }
        let dir = std::env::temp_dir().join("lightbox-rating-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("rated.jpg");
        std::fs::copy(&src, &p).unwrap();
        set(&p, 4, true).unwrap();
        assert_eq!(read(&p), Some(4));
        set_keywords(&p, &["море".into(), "отпуск".into()], true).unwrap();
        assert_eq!(keywords(&p), ["море", "отпуск"]);
        set(&p, 0, true).unwrap();
        assert_eq!(read(&p), Some(0));
    }
}
