//! Batch rotation: photos picked in the filmstrip (Ctrl+click, Shift+click
//! for a range) are turned together with R / L / M and saved together.

use std::path::PathBuf;

use anyhow::Result;

use super::App;
use crate::cache;
use crate::orientation::Orientation;
use crate::settings::SaveMode;

impl App {
    /// Filmstrip click with Ctrl (toggle) or Shift (range from the current
    /// photo). Returns `false` for a plain click (navigate instead).
    pub(super) fn select_cell(&mut self, i: usize, ctrl: bool, shift: bool) -> Result<bool> {
        if !ctrl && !shift {
            self.selected.clear();
            return Ok(false);
        }
        if shift {
            let (a, b) = if i < self.index {
                (i, self.index)
            } else {
                (self.index, i)
            };
            for p in &self.files[a..=b.min(self.files.len() - 1)] {
                self.selected.insert(cache::key(p));
            }
        } else if let Some(p) = self.files.get(i) {
            let key = cache::key(p);
            if !self.selected.remove(&key) {
                self.selected.insert(key);
            }
        }
        let n = self.selected.len();
        if n > 0 {
            self.notice(&format!(
                "Выбрано фото: {n} · R / L — повернуть все · Esc — снять выбор"
            ));
        }
        self.render()?;
        Ok(true)
    }

    pub(super) fn clear_selection(&mut self) -> Result<()> {
        self.selected.clear();
        self.render()
    }

    fn selected_paths(&self) -> Vec<PathBuf> {
        self.files
            .iter()
            .filter(|p| self.selected.contains(&cache::key(p)))
            .cloned()
            .collect()
    }

    /// R / L / M with photos selected: all of them.
    pub(super) fn rotate_selection(&mut self, op: Orientation) -> Result<()> {
        let paths = self.selected_paths();
        let current = cache::key(&self.current);
        for p in &paths {
            let key = cache::key(p);
            let e = self
                .edits
                .get(&key)
                .copied()
                .unwrap_or(Orientation::IDENTITY)
                .then(op);
            if e == Orientation::IDENTITY {
                self.edits.remove(&key);
            } else {
                self.edits.insert(key, e);
            }
        }
        if self.selected.contains(&current)
            && let Some(v) = self.fit_view()
        {
            self.view = v;
            self.fitted = true;
        }
        // Saved right away when rotations are saved automatically (the
        // current photo when it is left, as usual).
        if self.settings.save_mode == SaveMode::Auto {
            for p in paths.iter().filter(|p| cache::key(p) != current) {
                self.save_rotation(p);
            }
        }
        self.notice(&format!("Повёрнуто фото: {}", paths.len()));
        self.render()
    }

    /// Ctrl+S with photos selected: write all their rotations.
    pub(super) fn save_selection(&mut self) -> Result<()> {
        let paths = self.selected_paths();
        let mut n = 0;
        for p in &paths {
            if self.edits.contains_key(&cache::key(p)) {
                self.save_rotation(p);
                n += 1;
            }
        }
        self.notice(&if n == 0 {
            "Нет изменений для сохранения".to_string()
        } else {
            format!("Сохранение фото: {n}…")
        });
        self.render()
    }
}
