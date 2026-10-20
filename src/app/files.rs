//! File commands: recycle bin, F1..F9 quick folders, clipboard, Explorer,
//! external editor, wallpaper. The work runs on the file thread
//! ([`crate::fileops`]); results come back as WM_APP_FILEOP.

use std::path::{Path, PathBuf};

use anyhow::Result;

use super::{App, WM_APP_FILEOP, post};
use crate::cache;
use crate::fileops::{FileOps, Job, Outcome};
use crate::i18n::tr;
use crate::library;
use crate::orientation::Orientation;
use crate::settings::{FolderAction, QuickFolder};
use crate::t;
use crate::win::{clipboard, dialogs};

/// Dialog button ids.
const ID_COPY: i32 = 1;
const ID_MOVE: i32 = 2;
const ID_DELETE: i32 = 3;

fn folder_name(p: &Path) -> String {
    p.file_name().map_or_else(
        || p.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

impl App {
    pub(super) fn start_fileops(&mut self) {
        let hwnd = self.hwnd.0 as isize;
        self.fileops = Some(FileOps::new(hwnd, move || post(hwnd, WM_APP_FILEOP)));
    }

    fn submit_file_job(&mut self, job: Job) {
        match &self.fileops {
            Some(f) => f.submit(job),
            None => tracing::warn!("file thread not running"),
        }
    }

    /// The user's unsaved rotation of the current photo.
    fn current_edit(&self) -> Orientation {
        self.edits
            .get(&cache::key(&self.current))
            .copied()
            .unwrap_or(Orientation::IDENTITY)
    }

    /// `true` (and a notice) while a rotation of the current photo is being
    /// written: the file must not be moved or deleted under the writer.
    fn busy_saving(&mut self) -> bool {
        if self.saving.contains(&cache::key(&self.current)) {
            self.notice(tr("Подождите: фото сохраняется"));
            return true;
        }
        false
    }

    /// Del: the current photo to the recycle bin (its unsaved rotation is
    /// dropped).
    pub(super) fn delete_current(&mut self) -> Result<()> {
        if self.files.is_empty() || self.busy_saving() {
            return self.render();
        }
        if self.settings.confirm_delete {
            let (choice, never) = dialogs::ask(
                self.hwnd,
                tr("Удалить в корзину?"),
                &Self::file_name(&self.current),
                &[(ID_DELETE, tr("Удалить"))],
                Some(tr("Больше не спрашивать")),
            );
            if choice != Some(ID_DELETE) {
                return Ok(());
            }
            if never {
                self.settings.confirm_delete = false;
                self.settings.save();
            }
        }
        self.edits.remove(&cache::key(&self.current));
        self.submit_file_job(Job::Delete {
            path: self.current.clone(),
            confirm: false,
        });
        Ok(())
    }

    /// F`n`: copy / move to quick folder `n` (`swap`: the other action);
    /// unassigned keys ask for a folder first.
    pub(super) fn quick_folder(&mut self, n: u8, swap: bool) -> Result<()> {
        let Some(qf) = self
            .settings
            .quick_folders
            .iter()
            .find(|q| q.key == n)
            .cloned()
        else {
            return self.assign_folder(n);
        };
        if self.files.is_empty() || self.busy_saving() {
            return self.render();
        }
        let action = match (qf.action, swap) {
            (FolderAction::Copy, false) | (FolderAction::Move, true) => FolderAction::Copy,
            _ => FolderAction::Move,
        };
        // The photo leaves (or is copied) as shown: write a pending
        // rotation first and send the file once that is done.
        let job = |path: PathBuf| match action {
            FolderAction::Copy => Job::Copy { path, to: qf.path },
            FolderAction::Move => Job::Move { path, to: qf.path },
        };
        if self.dirty() {
            self.after_save = Some(job(self.current.clone()));
            self.save_current()?;
        } else {
            self.submit_file_job(job(self.current.clone()));
        }
        Ok(())
    }

    /// Shift+F`n`: choose the folder for F`n` and what it does.
    pub(super) fn assign_folder(&mut self, n: u8) -> Result<()> {
        let start = self
            .settings
            .quick_folders
            .iter()
            .find(|q| q.key == n)
            .map(|q| q.path.clone())
            .or_else(|| self.current.parent().map(Path::to_path_buf));
        let Some(path) = dialogs::open(
            Some(self.hwnd),
            &t!("Папка для F{}", n),
            true,
            None,
            start.as_deref(),
        ) else {
            return Ok(());
        };
        let (choice, _) = dialogs::ask(
            self.hwnd,
            &format!("F{n} → {}", folder_name(&path)),
            tr("Что делать с фото по этой клавише? С Ctrl — наоборот."),
            &[(ID_COPY, tr("Копировать")), (ID_MOVE, tr("Перемещать"))],
            None,
        );
        let Some(id) = choice else {
            return Ok(());
        };
        let action = if id == ID_MOVE {
            FolderAction::Move
        } else {
            FolderAction::Copy
        };
        self.settings.quick_folders.retain(|q| q.key != n);
        self.settings.quick_folders.push(QuickFolder {
            key: n,
            path: path.clone(),
            action,
        });
        self.settings.quick_folders.sort_by_key(|q| q.key);
        self.settings.save();
        self.notice(&format!("F{n}: {}", folder_name(&path)));
        self.render()
    }

    /// Ctrl+C: the picture as shown.
    pub(super) fn copy_image(&mut self) -> Result<()> {
        if self.files.is_empty() {
            return Ok(());
        }
        self.submit_file_job(Job::CopyImage {
            path: self.current.clone(),
            edit: self.current_edit(),
        });
        self.notice(tr("Копирование…"));
        self.render()
    }

    /// Ctrl+Shift+C: the file's path.
    pub(super) fn copy_path(&mut self) -> Result<()> {
        let text = self.current.display().to_string();
        match clipboard::set_text(self.hwnd, &text) {
            Ok(()) => self.notice(tr("Путь скопирован")),
            Err(e) => {
                tracing::warn!("clipboard: {e:#}");
                self.notice(tr("Не удалось скопировать"));
            }
        }
        self.render()
    }

    pub(super) fn show_in_explorer(&mut self) {
        self.submit_file_job(Job::ShowInExplorer {
            path: self.current.clone(),
        });
    }

    pub(super) fn open_in_editor(&mut self) {
        self.submit_file_job(Job::OpenInEditor {
            path: self.current.clone(),
            editor: self.settings.editor.clone(),
        });
    }

    pub(super) fn set_wallpaper(&mut self) -> Result<()> {
        self.submit_file_job(Job::Wallpaper {
            path: self.current.clone(),
            edit: self.current_edit(),
        });
        self.notice(tr("Установка обоев…"));
        self.render()
    }

    /// A pending copy / move waiting for its rotation to be written.
    pub(super) fn run_after_save(&mut self) {
        if self.saving.is_empty()
            && let Some(job) = self.after_save.take()
        {
            self.submit_file_job(job);
        }
    }

    pub(super) fn on_fileop(&mut self) -> Result<()> {
        while let Some(out) = self.fileops.as_ref().and_then(FileOps::try_recv) {
            match out {
                Outcome::Deleted { path } => {
                    self.notice(tr("Удалено в корзину"));
                    self.remove_file(&path)?;
                }
                Outcome::Moved { path, to } => {
                    self.notice(&t!("Перемещено в «{}»", folder_name(&to)));
                    self.remove_file(&path)?;
                }
                Outcome::Copied { to } => {
                    self.notice(&t!("Скопировано в «{}»", folder_name(&to)));
                }
                Outcome::ImageCopied => self.notice(tr("Изображение скопировано")),
                Outcome::WallpaperSet => self.notice(tr("Обои установлены")),
                Outcome::Cancelled | Outcome::Done => {}
                Outcome::Failed { what, error } => {
                    tracing::warn!("{what}: {error}");
                    self.notice(&format!("{what}: {error}"));
                }
            }
        }
        self.render()
    }

    /// Closes the viewer (Esc, the close button, last file removed).
    pub(super) fn close_window(&self) {
        // SAFETY: posting to our own window.
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                Some(self.hwnd),
                windows::Win32::UI::WindowsAndMessaging::WM_CLOSE,
                windows::Win32::Foundation::WPARAM(0),
                windows::Win32::Foundation::LPARAM(0),
            );
        }
    }

    /// A file left the folder (deleted / moved): drop it from the list
    /// and show its neighbour.
    fn remove_file(&mut self, path: &Path) -> Result<()> {
        let key = cache::key(path);
        self.cache.remove(path);
        self.textures.remove(&key);
        self.reloading.remove(&key);
        self.edits.remove(&key);
        self.compare_key_removed(&key);
        let Some(i) = self.files.iter().position(|p| library::same_path(p, path)) else {
            return Ok(());
        };
        let gone = self.files.remove(i);
        self.all_files.retain(|p| *p != gone);
        if self.files.is_empty() {
            self.close_window();
            return Ok(());
        }
        let was_current = i == self.index;
        if i < self.index || self.index >= self.files.len() {
            self.index = self.index.saturating_sub(1).min(self.files.len() - 1);
        }
        self.strip_pos = self.index as f64;
        if was_current {
            self.current = self.files[self.index].clone();
            self.angle = 0.0;
            self.update_title();
            if self.show_current()? {
                self.render()?;
            }
        }
        self.request_thumbs();
        self.schedule();
        Ok(())
    }
}

/// Context menu command ids.
const CMD_COPY_IMAGE: u32 = 1;
const CMD_COPY_PATH: u32 = 2;
const CMD_EXPLORER: u32 = 3;
const CMD_EDITOR: u32 = 4;
const CMD_WALLPAPER: u32 = 5;
const CMD_DELETE: u32 = 6;
const CMD_INFO: u32 = 7;
const CMD_COPY_TO: u32 = 100;
const CMD_MOVE_TO: u32 = 200;
const CMD_ASSIGN: u32 = 300;

impl App {
    /// Right click on the picture.
    pub(super) fn context_menu(&mut self) -> Result<()> {
        use crate::win::menu::{self, Item};
        let has_file = !self.files.is_empty() && self.shown.is_some();
        let cmd = |id, label: &str| Item::Command(id, label.to_string(), has_file);
        let folders = |base: u32| -> Vec<Item> {
            let mut v: Vec<Item> = (1..=9u8)
                .filter_map(|n| {
                    let q = self.settings.quick_folders.iter().find(|q| q.key == n)?;
                    Some(Item::Command(
                        base + u32::from(n),
                        format!("{}\tF{n}", folder_name(&q.path)),
                        has_file,
                    ))
                })
                .collect();
            if !v.is_empty() {
                v.push(Item::Separator);
            }
            v.push(Item::Command(
                CMD_ASSIGN,
                tr("Назначить папку…\tShift+F1..F9").into(),
                true,
            ));
            v
        };
        let items = vec![
            cmd(CMD_COPY_IMAGE, tr("Копировать изображение\tCtrl+C")),
            cmd(CMD_COPY_PATH, tr("Копировать путь\tCtrl+Shift+C")),
            Item::Separator,
            Item::Submenu(tr("Копировать в папку").into(), folders(CMD_COPY_TO)),
            Item::Submenu(tr("Переместить в папку").into(), folders(CMD_MOVE_TO)),
            Item::Separator,
            cmd(CMD_EXPLORER, tr("Показать в проводнике")),
            cmd(CMD_EDITOR, tr("Открыть в редакторе\tE")),
            cmd(CMD_WALLPAPER, tr("Сделать обоями рабочего стола")),
            Item::Separator,
            cmd(CMD_INFO, tr("Информация\tI")),
            Item::Separator,
            cmd(CMD_DELETE, tr("Удалить в корзину\tDel")),
        ];
        let at = crate::win::window::cursor_pos();
        let Some(id) = menu::popup(self.hwnd, &items, at) else {
            return Ok(());
        };
        match id {
            CMD_COPY_IMAGE => self.copy_image(),
            CMD_COPY_PATH => self.copy_path(),
            CMD_EXPLORER => {
                self.show_in_explorer();
                Ok(())
            }
            CMD_EDITOR => {
                self.open_in_editor();
                Ok(())
            }
            CMD_WALLPAPER => self.set_wallpaper(),
            CMD_DELETE => self.delete_current(),
            CMD_INFO => self.toggle_info(),
            CMD_ASSIGN => {
                // The first free key, else F1.
                let n = (1..=9u8)
                    .filter(|n| *n != 5) // F5 is the slideshow
                    .find(|n| !self.settings.quick_folders.iter().any(|q| q.key == *n))
                    .unwrap_or(1);
                self.assign_folder(n)
            }
            id if id > CMD_MOVE_TO => self.send_to(id - CMD_MOVE_TO, FolderAction::Move),
            id if id > CMD_COPY_TO => self.send_to(id - CMD_COPY_TO, FolderAction::Copy),
            _ => Ok(()),
        }
    }

    /// Copy / move to quick folder `n` with an explicit action.
    fn send_to(&mut self, n: u32, action: FolderAction) -> Result<()> {
        let n = n as u8;
        let stored = self
            .settings
            .quick_folders
            .iter()
            .find(|q| q.key == n)
            .map(|q| q.action);
        self.quick_folder(n, stored.is_some_and(|s| s != action))
    }
}
