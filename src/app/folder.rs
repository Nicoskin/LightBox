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
use crate::i18n::tr;
use crate::library::SortMode;
use crate::win::menu::Item;
use crate::win::watch::Watcher;
use crate::{cache, decode, library};

/// Changes are collected this long before the folder is read again.
const SETTLE_MS: u32 = 250;
const SORTS: [SortMode; 5] = [
    SortMode::Explorer,
    SortMode::Name,
    SortMode::Modified,
    SortMode::Rating,
    SortMode::Type,
];
const SORT: u32 = 400;
const REVERSE: u32 = 405;
const SEARCH: u32 = 406;
const RESET: u32 = 407;
const RATING: u32 = 410;
const TYPE: u32 = 420;

impl App {
    pub(super) fn search_folder(&mut self) -> Result<()> {
        if self.preview() || self.is_doc() || self.current.as_os_str().is_empty() {
            return Ok(());
        }
        self.folder_menu_action(SEARCH)
    }

    fn folder_types(&self) -> Vec<String> {
        let mut types: Vec<_> = self
            .all_files
            .iter()
            .map(|p| library::file_type(p))
            .filter(|s| !s.is_empty())
            .collect();
        types.sort_unstable();
        types.dedup();
        types
    }

    pub(super) fn folder_menu(&self) -> Item {
        let option = |id, label: &str, active, enabled| {
            Item::Command(
                id,
                if active {
                    format!("✓ {label}")
                } else {
                    label.to_owned()
                },
                enabled,
            )
        };
        let labels = [
            "Как в Проводнике",
            "По имени",
            "По дате изменения",
            "По рейтингу",
            "По типу файла",
        ];
        let mut sorts: Vec<_> = SORTS
            .iter()
            .zip(labels)
            .enumerate()
            .map(|(i, (mode, label))| {
                option(
                    SORT + i as u32,
                    tr(label),
                    self.settings.folder_sort == *mode,
                    true,
                )
            })
            .collect();
        sorts.push(Item::Separator);
        sorts.push(option(
            REVERSE,
            tr("Обратный порядок"),
            self.settings.folder_reverse && self.settings.folder_sort != SortMode::Explorer,
            self.settings.folder_sort != SortMode::Explorer,
        ));
        let mut types = vec![option(
            TYPE,
            tr("Все типы"),
            self.folder_extension.is_empty(),
            true,
        )];
        for (i, ext) in self.folder_types().iter().enumerate() {
            types.push(option(
                TYPE + 1 + i as u32,
                &format!("*.{ext}"),
                self.folder_extension == *ext,
                true,
            ));
        }
        let ratings = (0..=5)
            .map(|stars| {
                let label = if stars == 0 {
                    tr("Любой рейтинг").to_owned()
                } else {
                    crate::t!("фильтр: от {}", super::stars::stars_text(stars))
                };
                option(
                    RATING + u32::from(stars),
                    &label,
                    self.rating_filter == stars,
                    true,
                )
            })
            .collect();
        Item::Submenu(
            tr("Папка").into(),
            vec![
                Item::Submenu(tr("Сортировка").into(), sorts),
                Item::Submenu(tr("Тип файла").into(), types),
                Item::Submenu(tr("Рейтинг").into(), ratings),
                option(
                    SEARCH,
                    tr("Поиск по имени…"),
                    !self.folder_query.is_empty(),
                    true,
                ),
                option(
                    RESET,
                    tr("Сбросить фильтры"),
                    false,
                    self.filter_label().is_some(),
                ),
            ],
        )
    }

    pub(super) fn folder_menu_action(&mut self, id: u32) -> Result<()> {
        match id {
            SORT..REVERSE => {
                self.settings.folder_sort = SORTS[(id - SORT) as usize];
                self.settings.folder_reverse = false;
                self.settings.save();
                self.rescan_folder();
                return Ok(());
            }
            REVERSE => {
                self.settings.folder_reverse = !self.settings.folder_reverse;
                self.settings.save();
            }
            SEARCH => {
                let Some(query) = crate::win::dialogs::folder_search(self.hwnd, &self.folder_query)
                else {
                    return Ok(());
                };
                self.folder_query = query.trim().to_lowercase();
            }
            RESET => {
                self.folder_query.clear();
                self.folder_extension.clear();
                self.rating_filter = 0;
            }
            RATING..TYPE => self.rating_filter = (id - RATING).min(5) as u8,
            TYPE => self.folder_extension.clear(),
            id if id > TYPE => {
                if let Some(ext) = self.folder_types().get((id - TYPE - 1) as usize) {
                    self.folder_extension = ext.clone();
                }
            }
            _ => return Ok(()),
        }
        self.apply_filter();
        if matches!(id, SEARCH | TYPE..) && !self.matches_folder_filter(&self.current) {
            if let Some(i) = self
                .files
                .iter()
                .position(|p| self.matches_folder_filter(p))
            {
                return self.go_to(i);
            }
            self.notice(tr("Файлы не найдены"));
        }
        self.render()
    }

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
            // `shown` retains the old texture until its replacement arrives.
            self.invalidate(p);
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
                    self.notice(tr("В папке нет изображений"));
                    return self.render();
                }
            }
        } else {
            path
        };
        if !decode::is_supported(&target) {
            self.notice(tr("Этот файл не изображение и не 3D-модель"));
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
