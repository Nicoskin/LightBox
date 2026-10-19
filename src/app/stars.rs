//! Star ratings in the viewer: Ctrl+1…5 rate the current photo, Ctrl+0
//! clears; ratings of the whole folder are read in the background and shown
//! on the panel and the filmstrip; Ctrl+F filters the folder by rating.

use std::path::PathBuf;
use std::sync::mpsc::Sender;
use std::thread;

use anyhow::Result;

use super::{App, WM_APP_RATING, post};
use crate::{cache, rating};

/// Ratings read or written: file key, stars or an error, written by us.
pub(super) type RatingMsg = (String, Result<u8, String>, bool);

/// The filter steps Ctrl+F goes through (minimum stars, 0 = all).
const FILTERS: [u8; 5] = [0, 1, 3, 4, 5];

fn spawn_reader(files: Vec<PathBuf>, tx: Sender<RatingMsg>, hwnd: isize) {
    let spawned = thread::Builder::new()
        .name("ratings".into())
        .spawn(move || {
            for (i, p) in files.iter().enumerate() {
                if let Some(s) = rating::read(p)
                    && tx.send((cache::key(p), Ok(s), false)).is_err()
                {
                    return;
                }
                if i % 32 == 31 {
                    post(hwnd, WM_APP_RATING);
                }
            }
            post(hwnd, WM_APP_RATING);
        });
    if let Err(e) = spawned {
        tracing::warn!("cannot start the rating reader: {e}");
    }
}

impl App {
    /// Reads the ratings not known yet (after a folder scan).
    pub(super) fn read_ratings(&mut self) {
        let todo: Vec<PathBuf> = self
            .all_files
            .iter()
            .filter(|p| !crate::model::is_model(p) && !self.ratings.contains_key(&cache::key(p)))
            .cloned()
            .collect();
        if !todo.is_empty() {
            spawn_reader(todo, self.stars_tx.clone(), self.hwnd.0 as isize);
        }
    }

    /// Ctrl+0…5: rates the current photo (0 clears).
    pub(super) fn rate(&mut self, stars: u8) -> Result<()> {
        if self.current.as_os_str().is_empty() || self.model_mode() || self.is_doc() {
            return Ok(());
        }
        let path = self.current.clone();
        let tx = self.stars_tx.clone();
        let hwnd = self.hwnd.0 as isize;
        let keep = self.settings.keep_file_dates;
        let spawned = thread::Builder::new().name("rate".into()).spawn(move || {
            let r = rating::set(&path, stars, keep).map(|()| stars);
            let _ = tx.send((cache::key(&path), r, true));
            post(hwnd, WM_APP_RATING);
        });
        if let Err(e) = spawned {
            tracing::warn!("cannot start the rating writer: {e}");
        }
        Ok(())
    }

    pub(super) fn on_rating(&mut self) -> Result<()> {
        let mut redraw = false;
        while let Ok((key, r, ours)) = self.stars_rx.try_recv() {
            match r {
                Ok(s) => {
                    self.ratings.insert(key.clone(), s);
                    if ours {
                        let text = if s == 0 {
                            "Рейтинг снят".to_string()
                        } else {
                            stars_text(s)
                        };
                        self.notice(&text);
                    }
                }
                Err(e) if ours => self.notice(&format!("Рейтинг не записан: {e}")),
                Err(_) => {}
            }
            redraw = true;
        }
        if redraw {
            if self.rating_filter > 0 {
                self.apply_filter();
            }
            self.render()?;
        }
        Ok(())
    }

    /// The current photo's stars, if known.
    pub(super) fn stars(&self) -> Option<u8> {
        self.ratings.get(&cache::key(&self.current)).copied()
    }

    /// Stars of a filmstrip file.
    pub(super) fn stars_of(&self, i: usize) -> u8 {
        self.files
            .get(i)
            .and_then(|p| self.ratings.get(&cache::key(p)))
            .copied()
            .unwrap_or(0)
    }

    /// Ctrl+F: all → ★ from 1 → 3 → 4 → 5 → all.
    pub(super) fn cycle_rating_filter(&mut self) -> Result<()> {
        if self.preview() || self.all_files.len() < 2 {
            return Ok(());
        }
        let i = FILTERS
            .iter()
            .position(|f| *f == self.rating_filter)
            .unwrap_or(0);
        self.rating_filter = FILTERS[(i + 1) % FILTERS.len()];
        self.apply_filter();
        let text = match self.rating_filter {
            0 => "Показаны все фото".to_string(),
            n => format!(
                "Только с рейтингом от {} · {} фото",
                stars_text(n),
                self.files.len()
            ),
        };
        self.notice(&text);
        self.render()
    }

    /// The folder narrowed to the rating filter (the current photo stays).
    pub(super) fn apply_filter(&mut self) {
        let min = self.rating_filter;
        let current = cache::key(&self.current);
        let files: Vec<PathBuf> = self
            .all_files
            .iter()
            .filter(|p| {
                let k = cache::key(p);
                min == 0 || k == current || self.ratings.get(&k).copied().unwrap_or(0) >= min
            })
            .cloned()
            .collect();
        if files.is_empty() || files == self.files {
            return;
        }
        self.index = files
            .iter()
            .position(|p| cache::key(p) == current)
            .unwrap_or(0);
        self.files = files;
        self.strip_pos = self.index as f64;
        self.update_title();
        self.request_thumbs();
        self.schedule();
    }

    /// "фильтр: от ★★★" while filtering.
    pub(super) fn filter_label(&self) -> Option<String> {
        (self.rating_filter > 0).then(|| format!("фильтр: от {}", stars_text(self.rating_filter)))
    }
}

/// "★★★☆☆".
pub(super) fn stars_text(s: u8) -> String {
    let s = usize::from(s.min(5));
    "★".repeat(s) + &"☆".repeat(5 - s)
}
