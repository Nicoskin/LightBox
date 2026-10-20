//! Where Lightbox keeps its files: settings in `%APPDATA%\Lightbox`, caches
//! in `%LOCALAPPDATA%\Lightbox`. The portable version (`portable.txt` next to
//! the exe) keeps both in a `data` folder beside the exe, and stays out of
//! the tray, the registry and Windows start-up.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The marker file of the portable version.
pub const PORTABLE_MARKER: &str = "portable.txt";

fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()?
        .parent()
        .map(Path::to_path_buf)
}

/// `true` in the portable version.
pub fn portable() -> bool {
    static PORTABLE: OnceLock<bool> = OnceLock::new();
    *PORTABLE.get_or_init(|| exe_dir().is_some_and(|d| d.join(PORTABLE_MARKER).is_file()))
}

/// The portable version's data folder.
fn portable_dir() -> Option<PathBuf> {
    exe_dir().map(|d| d.join("data"))
}

/// Settings (roaming).
pub fn config_dir() -> Option<PathBuf> {
    if portable() {
        return portable_dir();
    }
    std::env::var_os("APPDATA").map(|d| PathBuf::from(d).join(crate::APP_NAME))
}

/// Caches and generated files (local).
pub fn local_dir() -> Option<PathBuf> {
    if portable() {
        return portable_dir();
    }
    std::env::var_os("LOCALAPPDATA").map(|d| PathBuf::from(d).join(crate::APP_NAME))
}
