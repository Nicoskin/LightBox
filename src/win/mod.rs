//! Win32 windowing: thin safe wrappers around the raw API.

pub mod clipboard;
pub mod dialogs;
pub mod menu;
pub mod monitor;
pub mod registry;
pub mod watch;
pub mod window;

/// Opens a URL or a file with its default program.
pub fn shell_open(target: &str) {
    use windows::Win32::UI::Shell::ShellExecuteW;
    use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
    use windows::core::{HSTRING, PCWSTR};
    // SAFETY: NUL-terminated strings alive for the call.
    unsafe {
        ShellExecuteW(
            None,
            &HSTRING::from("open"),
            &HSTRING::from(target),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}
