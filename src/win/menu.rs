//! Native pop-up menus (context menu, tray menu), in the dark theme where
//! Windows offers it.

use std::sync::Once;

use windows::Win32::Foundation::{HWND, POINT};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{
    AppendMenuW, CreatePopupMenu, DestroyMenu, HMENU, MENU_ITEM_FLAGS, MF_GRAYED, MF_POPUP,
    MF_SEPARATOR, MF_STRING, SetForegroundWindow, TPM_RETURNCMD, TPM_RIGHTBUTTON, TrackPopupMenuEx,
};
use windows::core::{HSTRING, PCSTR, w};

pub enum Item {
    /// Command id (non-zero), label ("Text\tShortcut"), enabled.
    Command(u32, String, bool),
    Separator,
    Submenu(String, Vec<Item>),
}

/// Lets menus follow the system dark mode (undocumented uxtheme exports
/// used by Explorer itself; silently skipped where missing).
fn allow_dark_menus() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: uxtheme ordinals 135 (SetPreferredAppMode, Windows 10 1903+)
        // and 136 (FlushMenuThemes) take an int / nothing; a missing export
        // yields None and nothing is called.
        unsafe {
            let Ok(ux) = LoadLibraryW(w!("uxtheme.dll")) else {
                return;
            };
            if let Some(f) = GetProcAddress(ux, PCSTR(135 as *const u8)) {
                let set: extern "system" fn(i32) -> i32 = std::mem::transmute(f);
                set(2); // ForceDark: the viewer is dark itself
            }
            if let Some(f) = GetProcAddress(ux, PCSTR(136 as *const u8)) {
                let flush: extern "system" fn() = std::mem::transmute(f);
                flush();
            }
        }
    });
}

fn build(items: &[Item]) -> Option<HMENU> {
    // SAFETY: plain menu construction; strings live for the calls.
    unsafe {
        let menu = CreatePopupMenu().ok()?;
        for item in items {
            match item {
                Item::Command(id, label, enabled) => {
                    let flags = if *enabled {
                        MF_STRING
                    } else {
                        MF_STRING | MF_GRAYED
                    };
                    let _ = AppendMenuW(menu, flags, *id as usize, &HSTRING::from(label.as_str()));
                }
                Item::Separator => {
                    let _ = AppendMenuW(menu, MF_SEPARATOR, 0, None);
                }
                Item::Submenu(label, sub) => {
                    if let Some(s) = build(sub) {
                        let _ = AppendMenuW(
                            menu,
                            MENU_ITEM_FLAGS(MF_POPUP.0 | MF_STRING.0),
                            s.0 as usize,
                            &HSTRING::from(label.as_str()),
                        );
                    }
                }
            }
        }
        Some(menu)
    }
}

/// Shows `items` at screen point `at`; returns the chosen command id.
pub fn popup(hwnd: HWND, items: &[Item], at: POINT) -> Option<u32> {
    allow_dark_menus();
    let menu = build(items)?;
    // SAFETY: own window and menu; the menu (with its submenus) is destroyed
    // after the modal tracking loop returns.
    unsafe {
        // Required so that the menu closes when clicking elsewhere.
        let _ = SetForegroundWindow(hwnd);
        let id = TrackPopupMenuEx(
            menu,
            (TPM_RETURNCMD | TPM_RIGHTBUTTON).0,
            at.x,
            at.y,
            hwnd,
            None,
        );
        let _ = DestroyMenu(menu);
        (id.0 != 0).then_some(id.0 as u32)
    }
}
