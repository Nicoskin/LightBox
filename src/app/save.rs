//! Writing on-screen rotations to files: autosave, Ctrl+S, "Save as",
//! Ctrl+Z, and keeping the display stable while a saved file reloads.

use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::Result;

use super::{App, WM_APP_SAVED, post};
use crate::cache;
use crate::edit::{Job, Outcome, Writer};
use crate::orientation::Orientation;
use crate::settings::{ImperfectJpeg, SaveMode};
use crate::win::dialogs;

const ASK_SAVE: i32 = 100;
const ASK_DISCARD: i32 = 101;
const CHOICE_TRIM: i32 = 200;
const CHOICE_EXIF: i32 = 201;

impl App {
    pub(super) fn start_writer(&mut self) {
        let hwnd = self.hwnd.0 as isize;
        self.writer = Some(Writer::new(move || post(hwnd, WM_APP_SAVED)));
    }

    pub(super) fn file_name(path: &Path) -> String {
        path.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The file's own (EXIF) orientation, from whatever is decoded.
    fn base_orientation(&self, path: &Path) -> Orientation {
        let key = cache::key(path);
        if let Some((k, t)) = &self.shown
            && *k == key
        {
            return t.orientation;
        }
        if let Some(i) = self.cache.image(path) {
            return i.orientation;
        }
        // Not decoded (rotated from the filmstrip): the JPEG's EXIF tag.
        let mut head = vec![0u8; 128 * 1024];
        let n = std::fs::File::open(path)
            .and_then(|mut f| std::io::Read::read(&mut f, &mut head))
            .unwrap_or(0);
        if head[..n].starts_with(&[0xFF, 0xD8, 0xFF]) {
            crate::metadata::jpeg_orientation(&head[..n])
        } else {
            Orientation::IDENTITY
        }
    }

    /// Queues writing the user's edit of `path` into the file.
    pub(super) fn save_rotation(&mut self, path: &Path) {
        let key = cache::key(path);
        let Some(edit) = self.edits.get(&key).copied() else {
            return;
        };
        if self.saving.contains(&key) {
            return;
        }
        let Some(writer) = &self.writer else {
            return;
        };
        writer.submit(Job::Rotate {
            path: path.to_path_buf(),
            display: self.base_orientation(path).then(edit),
            edit,
            keep_dates: self.settings.keep_file_dates,
            imperfect: self.settings.imperfect_jpeg,
        });
        self.saving.insert(key);
    }

    /// Called before leaving the current photo (next / previous / close):
    /// writes, asks or keeps its rotation according to the setting.
    pub(super) fn leave_current(&mut self) {
        let key = cache::key(&self.current);
        if !self.edits.contains_key(&key) || self.saving.contains(&key) {
            return;
        }
        let path = self.current.clone();
        match self.settings.save_mode {
            SaveMode::Auto => self.save_rotation(&path),
            SaveMode::Manual => {}
            SaveMode::Ask => {
                let (choice, _) = dialogs::ask(
                    self.hwnd,
                    "Сохранить поворот в файл?",
                    &Self::file_name(&path),
                    &[
                        (
                            ASK_SAVE,
                            "Сохранить\nФайл будет перезаписан без потери качества",
                        ),
                        (ASK_DISCARD, "Не сохранять\nВернуть исходную ориентацию"),
                    ],
                    None,
                );
                match choice {
                    Some(ASK_SAVE) => self.save_rotation(&path),
                    Some(ASK_DISCARD) => {
                        self.edits.remove(&key);
                    }
                    _ => {}
                }
            }
        }
    }

    /// Ctrl+S / Save button.
    pub(super) fn save_current(&mut self) -> Result<()> {
        if self.edits.contains_key(&cache::key(&self.current)) {
            let path = self.current.clone();
            self.save_rotation(&path);
            self.notice("Сохранение…");
        } else {
            self.notice("Нет изменений для сохранения");
        }
        self.render()
    }

    /// "Сбросить": back to the file's own orientation.
    pub(super) fn reset_current(&mut self) -> Result<()> {
        if self.edits.remove(&cache::key(&self.current)).is_some()
            && let Some(v) = self.fit_view()
        {
            self.view = v;
            self.fitted = true;
        }
        self.render()
    }

    /// Ctrl+Shift+S.
    pub(super) fn save_as(&mut self) -> Result<()> {
        if self.shown.is_none() {
            return Ok(());
        }
        let folder = self
            .current
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let name = Self::file_name(&self.current);
        let Some((dst, format, quality)) =
            dialogs::save_as(self.hwnd, &folder, &name, self.settings.jpeg_quality)
        else {
            return Ok(());
        };
        if quality != self.settings.jpeg_quality {
            self.settings.jpeg_quality = quality;
            self.settings.save();
        }
        let edit = self
            .edits
            .get(&cache::key(&self.current))
            .copied()
            .unwrap_or(Orientation::IDENTITY);
        if let Some(w) = &self.writer {
            w.submit(Job::Export {
                src: self.current.clone(),
                dst,
                edit,
                format,
                quality,
            });
            self.notice("Сохранение…");
        }
        self.render()
    }

    /// Ctrl+Z: undoes the last rotation written in this session.
    pub(super) fn undo_save(&mut self) -> Result<()> {
        match self.undo.pop() {
            Some(undo) => {
                if let Some(w) = &self.writer {
                    w.submit(Job::Restore { undo });
                }
            }
            None => self.notice("Нечего отменять"),
        }
        self.render()
    }

    /// A file changed on disk: forget everything decoded from it.
    fn invalidate(&mut self, path: &Path) {
        let key = cache::key(path);
        self.cache.remove(path);
        self.textures.remove(&key);
        if let Some(t) = &mut self.thumbs {
            t.forget(path);
        }
    }

    pub(super) fn on_saved(&mut self) -> Result<()> {
        while let Some(outcome) = self.writer.as_ref().and_then(Writer::try_recv) {
            match outcome {
                Outcome::Rotated { path, edit, undo } => {
                    let key = cache::key(&path);
                    self.saving.remove(&key);
                    self.undo.push(undo);
                    self.invalidate(&path);
                    if key == cache::key(&self.current) {
                        // Keep showing the old pixels + edit until the
                        // rewritten file has been decoded (see on_decoded).
                        self.reloading.insert(key, edit);
                    } else {
                        self.apply_saved_edit(&key, edit);
                    }
                    self.notice("Поворот сохранён");
                }
                Outcome::NeedsJpegChoice {
                    path,
                    edit,
                    display,
                } => {
                    let key = cache::key(&path);
                    self.saving.remove(&key);
                    let (choice, remember) = dialogs::ask(
                        self.hwnd,
                        "Поворот без потерь невозможен точно",
                        &format!(
                            "{}: ширина или высота не кратна размеру блока JPEG (16 px).",
                            Self::file_name(&path)
                        ),
                        &[
                            (
                                CHOICE_TRIM,
                                "Обрезать край\nТеряется не больше 15 пикселей, качество не меняется",
                            ),
                            (
                                CHOICE_EXIF,
                                "Записать только тег ориентации\nПиксели не меняются; некоторые программы игнорируют тег",
                            ),
                        ],
                        Some("Запомнить выбор"),
                    );
                    let imperfect = match choice {
                        Some(CHOICE_TRIM) => ImperfectJpeg::Trim,
                        Some(CHOICE_EXIF) => ImperfectJpeg::ExifOnly,
                        _ => continue, // cancelled: the edit stays unsaved
                    };
                    if remember {
                        self.settings.imperfect_jpeg = Some(imperfect);
                        self.settings.save();
                    }
                    if let Some(w) = &self.writer {
                        w.submit(Job::Rotate {
                            path,
                            display,
                            edit,
                            keep_dates: self.settings.keep_file_dates,
                            imperfect: Some(imperfect),
                        });
                        self.saving.insert(key);
                    }
                }
                Outcome::Exported { dst } => {
                    self.notice(&format!("Сохранено: {}", Self::file_name(&dst)));
                    if dst.parent() == self.current.parent() {
                        self.rescan_folder();
                    }
                }
                Outcome::Restored { path } => {
                    let key = cache::key(&path);
                    self.edits.remove(&key);
                    self.reloading.remove(&key);
                    self.invalidate(&path);
                    self.notice("Сохранение отменено");
                }
                Outcome::Failed { path, error } => {
                    self.saving.remove(&cache::key(&path));
                    self.notice(&format!(
                        "Не удалось сохранить {}: {error}",
                        Self::file_name(&path)
                    ));
                }
            }
        }
        self.run_after_save();
        self.schedule();
        self.request_thumbs();
        self.render()
    }

    /// The file now contains `saved`: what remains of the user's edit is
    /// `saved⁻¹ · edit` on top of the new pixels.
    pub(super) fn apply_saved_edit(&mut self, key: &str, saved: Orientation) {
        let current = self
            .edits
            .get(key)
            .copied()
            .unwrap_or(Orientation::IDENTITY);
        let rest = saved.inverse().then(current);
        if rest == Orientation::IDENTITY {
            self.edits.remove(key);
        } else {
            self.edits.insert(key.to_string(), rest);
        }
    }

    /// Short status message in the badge.
    pub(super) fn notice(&mut self, text: &str) {
        self.show_badge();
        self.notice = Some((text.to_string(), Instant::now()));
        // Messages stay a little longer than the zoom level.
        // SAFETY: own window; re-arming the timer resets it.
        unsafe {
            windows::Win32::UI::WindowsAndMessaging::SetTimer(
                Some(self.hwnd),
                super::BADGE_TIMER,
                2500,
                None,
            )
        };
    }

    /// Whether the current photo has an unsaved rotation.
    pub(super) fn dirty(&self) -> bool {
        let key = cache::key(&self.current);
        self.edits.contains_key(&key) && !self.saving.contains(&key)
    }
}
