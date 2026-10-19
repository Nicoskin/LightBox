//! The folder as it changes (files added, removed, renamed, rewritten by
//! other programs) and drag & drop in both directions.

use std::path::{Path, PathBuf};

use anyhow::Result;
use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_LINK};
use windows::Win32::UI::Shell::{
    BHID_DataObject, DragFinish, DragQueryFileW, HDROP, IShellItem, SHCreateItemFromParsingName,
    SHDoDragDrop,
};
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};
use windows::core::HSTRING;

use super::{App, WATCH_TIMER, WM_APP_WATCH, post};
use crate::win::watch::Watcher;
use crate::{cache, decode, library};

/// Changes are collected this long before the folder is read again.
const SETTLE_MS: u32 = 250;

impl App {
    /// Watches the folder of the current file (again, if it changed).
    pub(super) fn watch_folder(&mut self) {
        let Some(dir) = self.current.parent().map(Path::to_path_buf) else {
            return;
        };
        if dir.as_os_str().is_empty() || self.watcher.as_ref().is_some_and(|w| w.dir == dir) {
            return;
        }
        let hwnd = self.hwnd.0 as isize;
        self.watcher = Watcher::new(&dir, move || post(hwnd, WM_APP_WATCH));
    }

    /// Something changed: wait for the burst to settle (a copy of many
    /// files, an editor saving through a temporary file).
    pub(super) fn on_watch(&mut self) {
        while let Some(batch) = self.watcher.as_ref().and_then(|w| w.rx.try_recv().ok()) {
            self.changed.extend(batch);
        }
        // SAFETY: own window; re-arming resets the delay.
        unsafe { SetTimer(Some(self.hwnd), WATCH_TIMER, SETTLE_MS, None) };
    }

    pub(super) fn watch_timer(&mut self) -> Result<()> {
        // SAFETY: own window and timer.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), WATCH_TIMER);
        }
        let changed = std::mem::take(&mut self.changed);
        if changed.is_empty() {
            return Ok(());
        }
        let current = cache::key(&self.current);
        for p in &changed {
            // Rewritten or replaced files are decoded again.
            let key = cache::key(p);
            self.cache.remove(p);
            if key != current {
                self.textures.remove(&key);
            }
        }
        if changed.iter().any(|p| cache::key(p) == current) && self.current.exists() {
            // Keep showing the old pixels until the new ones are decoded.
            self.schedule();
        }
        self.rescan_folder();
        Ok(())
    }

    /// Files dropped on the window: the first one opens (a folder: its
    /// first image).
    pub(super) fn on_drop(&mut self, drop: HDROP) -> Result<()> {
        let mut first = None;
        // SAFETY: the HDROP comes with WM_DROPFILES and is released here.
        unsafe {
            let mut buf = [0u16; 32 * 1024];
            if DragQueryFileW(drop, 0, Some(&mut buf)) > 0 {
                let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
                first = Some(PathBuf::from(String::from_utf16_lossy(&buf[..n])));
            }
            DragFinish(drop);
        }
        let Some(path) = first else {
            return Ok(());
        };
        let target = if path.is_dir() {
            match library::scan(&path, &path, library::SortMode::Name)
                .into_iter()
                .find(|p| p.is_file())
            {
                Some(p) => p,
                None => {
                    self.notice("В папке нет изображений");
                    return self.render();
                }
            }
        } else {
            path
        };
        if !decode::is_supported(&target) {
            self.notice("Этот файл не изображение и не 3D-модель");
            return self.render();
        }
        self.quicklook = false;
        self.open_external(&target)
    }

    /// Drags the current photo out of the window (to Explorer, a chat, an
    /// editor); returns once it is dropped or cancelled.
    pub(super) fn drag_out(&mut self) -> Result<()> {
        // SAFETY: COM / OLE calls on the UI thread (OLE initialised at
        // start-up); the data object is the shell's own for the file.
        unsafe {
            let item: IShellItem =
                SHCreateItemFromParsingName(&HSTRING::from(self.current.as_os_str()), None)?;
            let data: windows::Win32::System::Com::IDataObject =
                item.BindToHandler(None, &BHID_DataObject)?;
            let _ = SHDoDragDrop(
                Some(self.hwnd),
                &data,
                None,
                DROPEFFECT_COPY | DROPEFFECT_LINK,
            );
        }
        Ok(())
    }
}
