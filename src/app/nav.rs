//! Folder navigation, decode scheduling and showing the current file.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::channel;
use std::thread;
use std::time::Instant;

use anyhow::Result;

use super::{Anim, AnimKind, App, TURN_SHIFT, WM_APP_DECODED, WM_APP_LIBRARY, post};
use crate::cache::{self, Slot};
use crate::decode::{DecodedImage, SizeHint};
use crate::gfx::GpuImage;
use crate::library::{self, SortMode};
use crate::loader::{self, Loader, Quality, Request};
use crate::win::window;

/// Prefetch window: decoded ahead in the direction of travel / behind.
const AHEAD: usize = 3;
const BEHIND: usize = 1;

impl App {
    /// Starts the folder scan and the decoder pool. Called once the first
    /// frame is on screen so neither competes with it.
    pub fn start_background(&mut self) {
        let hwnd = self.hwnd.0 as isize;
        let (mw, mh) = self.monitor.size();
        let hint = SizeHint {
            box_width: mw,
            box_height: mh,
        };
        self.loader = Some(Loader::new(loader::default_threads(), hint, move || {
            post(hwnd, WM_APP_DECODED);
        }));

        self.rescan_folder();
        self.start_thumbs();
        self.start_writer();
        // The preview may have finished before the window could be notified.
        if let Err(e) = self.on_decoded() {
            tracing::error!("{e:#}");
        }
        self.show_panel();
        self.update_title();
        self.schedule();
    }

    /// Lists the current folder on a background thread (WM_APP_LIBRARY).
    pub(super) fn rescan_folder(&mut self) {
        let hwnd = self.hwnd.0 as isize;
        let current = self.current.clone();
        let dir = current
            .parent()
            .map_or_else(|| PathBuf::from("."), Path::to_path_buf);
        let (tx, rx) = channel();
        self.library_rx = Some(rx);
        let spawned = thread::Builder::new()
            .name("library".into())
            .spawn(move || {
                let files = library::scan(&dir, &current, SortMode::Name);
                if tx.send(files).is_ok() {
                    post(hwnd, WM_APP_LIBRARY);
                }
            });
        if let Err(e) = spawned {
            tracing::error!("cannot start folder scan: {e}");
        }
    }

    pub(super) fn on_library(&mut self) -> Result<()> {
        let Some(files) = self.library_rx.as_ref().and_then(|rx| rx.try_recv().ok()) else {
            return Ok(());
        };
        self.library_rx = None;
        self.index = files
            .iter()
            .position(|p| library::same_path(p, &self.current))
            .unwrap_or(0);
        self.files = files;
        self.strip_pos = self.index as f64;
        if let Some(p) = self.files.get(self.index) {
            self.current = p.clone();
        }
        self.update_title();
        self.request_thumbs();
        self.schedule();
        Ok(())
    }

    pub(super) fn on_decoded(&mut self) -> Result<()> {
        let mut redraw = false;
        // The start-up preview that follows the EXIF thumbnail.
        if let Some(rx) = &self.initial
            && let Ok(super::Startup::Final(result)) = rx.try_recv()
        {
            self.initial = None;
            let current = self.current.clone();
            self.store_result(&current, result.map(Arc::new).map_err(|e| e.to_string()));
            redraw |= self.show_current()?;
        }
        while let Some(done) = self.loader.as_ref().and_then(Loader::try_recv) {
            // No EXIF thumbnail is not an error: the preview is on its way.
            if done.quality == Quality::Thumb && done.result.is_err() {
                continue;
            }
            let key = cache::key(&done.path);
            // A rewritten file came back: its pixels now include the saved
            // rotation, so the remaining edit shrinks accordingly.
            if done.result.is_ok()
                && let Some(saved) = self.reloading.remove(&key)
            {
                self.apply_saved_edit(&key, saved);
            }
            let is_current = cache::key(&done.path) == cache::key(&self.current);
            let ok = done.result.as_ref().ok().cloned();
            self.store_result(&done.path, done.result);
            if is_current {
                redraw |= self.show_current()?;
            } else if let Some(img) = ok {
                self.preupload(&done.path, &img)?;
            }
        }
        if redraw {
            self.render()?;
        }
        self.schedule();
        Ok(())
    }

    pub(super) fn store_result(&mut self, path: &Path, result: Result<Arc<DecodedImage>, String>) {
        let slot = match result {
            Ok(img) => Slot::Image(img),
            Err(e) => Slot::Failed(e.into()),
        };
        let keep = self.keep_list();
        let keep: Vec<&Path> = keep.iter().map(PathBuf::as_path).collect();
        self.cache.insert(path, slot, &keep);
    }

    /// Uploads a decoded neighbour so that stepping to it is instant.
    fn preupload(&mut self, path: &Path, img: &DecodedImage) -> Result<()> {
        let key = cache::key(path);
        if self.textures.contains(&key) || !self.neighbours().iter().any(|p| cache::key(p) == key) {
            return Ok(());
        }
        let tex = Rc::new(GpuImage::upload(&self.gpu, img)?);
        self.textures.insert(key, tex);
        Ok(())
    }

    /// Steps `delta` files forward/back (clamped to the ends of the folder).
    pub(super) fn step(&mut self, delta: isize) -> Result<()> {
        if self.files.is_empty() {
            return Ok(());
        }
        let last = self.files.len() as isize - 1;
        let to = (self.index as isize + delta).clamp(0, last) as usize;
        self.go_to(to)
    }

    pub(super) fn go_to(&mut self, index: usize) -> Result<()> {
        if index == self.index || index >= self.files.len() {
            return Ok(());
        }
        // Write / ask about a rotation of the photo being left.
        self.leave_current();
        let t0 = std::time::Instant::now();
        self.finish_anim()?;
        self.direction = if index > self.index { 1 } else { -1 };
        self.index = index;
        // The free rotation belongs to the photo being left.
        self.angle = 0.0;
        self.current = self.files[index].clone();
        self.update_title();
        let shown = self.show_current()?;
        if shown {
            self.render()?;
        }
        self.request_thumbs();
        tracing::debug!(
            "navigate to {}: {:.2} ms ({})",
            self.current.display(),
            t0.elapsed().as_secs_f64() * 1000.0,
            if shown { "shown" } else { "decoding" }
        );
        self.schedule();
        Ok(())
    }

    /// Puts the current file on screen if it is available (GPU texture or
    /// decoded image); otherwise keeps showing what is there while it
    /// decodes. Returns `true` if a redraw is needed.
    pub(super) fn show_current(&mut self) -> Result<bool> {
        let key = cache::key(&self.current);
        let img = match self.cache.get(&self.current) {
            Some(Slot::Image(img)) => Some(Arc::clone(img)),
            Some(Slot::Failed(e)) => {
                let e = e.to_string();
                let changed = self.error.as_deref() != Some(&e) || self.shown.is_some();
                self.error = Some(e);
                self.shown = None;
                return Ok(changed);
            }
            None => None,
        };
        let tex = match (self.textures.get(&key), img) {
            (Some(t), Some(img)) if t.full || !img.is_full() => t,
            (_, Some(img)) => {
                let t = Rc::new(GpuImage::upload(&self.gpu, &img)?);
                self.textures.insert(key.clone(), Rc::clone(&t));
                t
            }
            (Some(t), None) => t,
            (None, None) => return Ok(false), // still decoding
        };
        self.error = None;

        let same_file = self.shown.as_ref().is_some_and(|(k, _)| *k == key);
        if same_file {
            // Quality upgrade (preview → full): keep the view as it is.
            let changed = self
                .shown
                .as_ref()
                .is_some_and(|(_, t)| !Rc::ptr_eq(t, &tex));
            let before = self.display();
            self.shown = Some((key, tex));
            // The file was rewritten or restored with other dimensions
            // (undo of a rotation): fit again.
            if self.display() != before
                && let Some(v) = self.fit_view()
            {
                self.view = v;
                self.fitted = true;
            }
            return Ok(changed);
        }
        // Settle any running animation on the old image first.
        self.finish_anim()?;
        let old_view = self.view;
        let previous = self.shown.replace((key, tex));
        if let Some(v) = self.fit_view() {
            self.view = v;
            self.fitted = true;
        }
        self.opacity = 1.0;
        self.badge = false;
        // Page turn, unless the user is skimming with key auto-repeat.
        if let (Some(duration), Some((old_key, old)), false) =
            (self.turn_len(), previous, self.fast_nav)
        {
            let old = Some((old_key, old, old_view));
            let shift = self.viewport().0 * TURN_SHIFT * self.direction as f64;
            self.anim = Some(Anim {
                kind: AnimKind::Turn { old, shift },
                start: Instant::now(),
                duration,
                frames: 0,
            });
        }
        Ok(true)
    }

    /// `true` if the view magnifies the shown preview beyond its resolution.
    fn needs_full(&self) -> bool {
        let Some(img) = self.shown_image() else {
            return false;
        };
        if img.full {
            return false;
        }
        let target = match &self.anim {
            Some(super::Anim {
                kind: super::AnimKind::View { to, .. },
                ..
            }) => to.scale,
            _ => self.view.scale,
        };
        // Screen pixels per texel > 1 → the preview is being enlarged.
        target * img.display.0 > img.display_texels().0 * 1.01
    }

    /// Files around the current one, nearest first, in the travel direction.
    fn neighbours(&self) -> Vec<PathBuf> {
        let n = self.files.len() as isize;
        let i = self.index as isize;
        let d = self.direction;
        let mut out = Vec::with_capacity(AHEAD + BEHIND);
        for k in 1..=AHEAD as isize {
            let j = i + d * k;
            if (0..n).contains(&j) {
                out.push(self.files[j as usize].clone());
            }
            if k as usize <= BEHIND {
                let j = i - d * k;
                if (0..n).contains(&j) {
                    out.push(self.files[j as usize].clone());
                }
            }
        }
        out
    }

    fn keep_list(&self) -> Vec<PathBuf> {
        let mut k = self.neighbours();
        k.push(self.current.clone());
        k
    }

    /// Tells the decoder pool what is wanted now, most urgent first:
    /// the current file, its full resolution if zoomed in, then neighbours.
    pub(super) fn schedule(&mut self) {
        let Some(loader) = &self.loader else {
            return;
        };
        let mut wanted = Vec::new();
        let cur = &self.current;
        // Nothing to show yet for this file: its EXIF thumbnail first (not
        // while it is already on screen, e.g. reloading after a save).
        let on_screen = self
            .shown
            .as_ref()
            .is_some_and(|(k, _)| *k == cache::key(cur));
        // (The start-up decoder is still producing the current preview.)
        let starting = self.initial.is_some();
        if self.cache.image(cur).is_none() && !on_screen && !starting {
            wanted.push(Request {
                path: cur.clone(),
                quality: Quality::Thumb,
            });
        }
        if !self.cache.satisfies(cur, false) && !starting {
            wanted.push(Request {
                path: cur.clone(),
                quality: Quality::Preview,
            });
        }
        if self.needs_full() && !self.cache.satisfies(cur, true) {
            wanted.push(Request {
                path: cur.clone(),
                quality: Quality::Full,
            });
        }
        for p in self.neighbours() {
            if !self.cache.satisfies(&p, false) {
                wanted.push(Request {
                    path: p,
                    quality: Quality::Preview,
                });
            }
        }
        loader.request(wanted);
        self.update_detail();
    }

    pub(super) fn update_title(&self) {
        let name = self
            .current
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let title = if self.files.len() > 1 {
            format!(
                "{name} ({}/{}) — Lightbox",
                self.index + 1,
                self.files.len()
            )
        } else {
            format!("{name} — Lightbox")
        };
        window::set_title(self.hwnd, &title);
    }
}
