//! The crop tool (X): a frame over the photo with corner and edge handles,
//! aspect ratios, Enter to apply, Esc to cancel. Applying writes through
//! the same job as rotations (lossless where the format allows, Ctrl+Z
//! restores the file).

use anyhow::Result;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    ReleaseCapture, SetCapture, VK_ESCAPE, VK_RETURN,
};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

use super::App;
use crate::cache;
use crate::edit::Job;
use crate::gfx::{CropView, SegmentView};
use crate::orientation::Orientation;
use crate::ui::crop::{self, Aspect, Grip};
use crate::view::Rect;

pub(super) struct CropState {
    key: String,
    /// x0, y0, x1, y1 in pixels of the photo as shown.
    frame: [f64; 4],
    aspect: Aspect,
    /// Grip, frame and pointer when the drag began.
    drag: Option<(Grip, [f64; 4], (f64, f64))>,
    /// The bar control under the pointer.
    hot: Option<BarHit>,
}

#[derive(Clone, Copy, PartialEq)]
enum BarHit {
    Aspect(usize),
    Apply,
    Cancel,
}

impl App {
    /// X: starts cropping the current photo (or cancels).
    pub(super) fn toggle_crop(&mut self) -> Result<()> {
        if self.crop.take().is_some() {
            return self.render();
        }
        if self.shown.is_none() || self.model_mode() || self.is_doc() || self.preview() {
            return Ok(());
        }
        if self.compare.is_some() {
            self.toggle_compare()?;
        }
        // The frame is axis-aligned: drop a free rotation.
        if self.angle != 0.0 {
            self.set_angle(0.0)?;
        }
        if let Some(v) = self.fit_view() {
            self.view = v;
            self.fitted = true;
        }
        let Some((w, h)) = self.display() else {
            return Ok(());
        };
        self.crop = Some(CropState {
            key: cache::key(&self.current),
            frame: [0.0, 0.0, w, h],
            aspect: Aspect::Free,
            drag: None,
            hot: None,
        });
        self.notice("Обрезка: тяните за углы и края · Enter — применить · Esc — отмена");
        self.render()
    }

    /// The photo on screen and its size as shown, while cropping.
    fn crop_geometry(&self) -> Option<(Rect, (f64, f64))> {
        let c = self.crop.as_ref()?;
        if c.key != cache::key(&self.current) {
            return None;
        }
        let img = self.display()?;
        Some((self.view.image_rect(img, self.image_vp()), img))
    }

    fn crop_bar(&self) -> crop::BarLayout {
        let bottom = self.viewport().1 - self.panel_height();
        crop::bar(self.viewport(), self.ui_scale(), bottom)
    }

    fn bar_hit(&self, p: (f64, f64)) -> Option<BarHit> {
        let b = self.crop_bar();
        if b.apply.contains(p) {
            return Some(BarHit::Apply);
        }
        if b.cancel.contains(p) {
            return Some(BarHit::Cancel);
        }
        b.aspects
            .iter()
            .position(|(_, _, r)| r.contains(p))
            .map(BarHit::Aspect)
    }

    /// Mouse and keys while cropping; `true` when consumed.
    pub(super) fn crop_input(&mut self, msg: u32, wparam: usize, p: (f64, f64)) -> Result<bool> {
        let Some((screen, img)) = self.crop_geometry() else {
            if self.crop.is_some() {
                // Another photo came up: the frame belonged to the old one.
                self.crop = None;
            }
            return Ok(false);
        };
        let grab = 8.0 * self.ui_scale();
        match msg {
            WM_KEYDOWN if wparam == usize::from(VK_RETURN.0) => {
                self.apply_crop()?;
                Ok(true)
            }
            WM_KEYDOWN if wparam == usize::from(VK_ESCAPE.0) => {
                self.crop = None;
                self.render()?;
                Ok(true)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                if self.bar_hit(p).is_some() {
                    return Ok(true);
                }
                if self.over_panel(p) {
                    return Ok(false);
                }
                let Some(c) = self.crop.as_mut() else {
                    return Ok(false);
                };
                let f = crop::to_screen(c.frame, img, screen);
                if let Some(g) = crop::grip_at(f, p, grab) {
                    c.drag = Some((g, c.frame, p));
                    // SAFETY: own window.
                    unsafe { SetCapture(self.hwnd) };
                }
                Ok(true)
            }
            WM_MOUSEMOVE => {
                let hot = self.bar_hit(p);
                let Some(c) = self.crop.as_mut() else {
                    return Ok(false);
                };
                let mut redraw = hot != c.hot;
                c.hot = hot;
                if let Some((g, start, from)) = c.drag {
                    let k = screen.width() / img.0;
                    let d = ((p.0 - from.0) / k, (p.1 - from.1) / k);
                    c.frame = crop::drag(start, g, d, img, c.aspect);
                    redraw = true;
                }
                if redraw {
                    self.render()?;
                }
                // Over the panel the normal hover still applies.
                Ok(self.crop.as_ref().is_some_and(|c| c.drag.is_some()))
            }
            WM_LBUTTONUP => {
                let was_drag = self.crop.as_mut().and_then(|c| c.drag.take()).is_some();
                if was_drag {
                    // SAFETY: releasing our own capture.
                    unsafe {
                        let _ = ReleaseCapture();
                    }
                    return Ok(true);
                }
                match self.bar_hit(p) {
                    Some(BarHit::Apply) => self.apply_crop()?,
                    Some(BarHit::Cancel) => {
                        self.crop = None;
                        self.render()?;
                    }
                    Some(BarHit::Aspect(i)) => {
                        if let Some(c) = self.crop.as_mut() {
                            c.aspect = Aspect::ALL[i].0;
                            c.frame = crop::fit(img, c.aspect, c.frame);
                        }
                        self.render()?;
                    }
                    None => return Ok(!self.over_panel(p)),
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Writes the crop (with any pending rotation) into the file.
    fn apply_crop(&mut self) -> Result<()> {
        let Some(c) = self.crop.take() else {
            return Ok(());
        };
        let path = self.current.clone();
        let key = cache::key(&path);
        if c.key != key || self.saving.contains(&key) {
            return self.render();
        }
        let Some(img) = self.display() else {
            return self.render();
        };
        let rect = crop::pixels(c.frame);
        if rect == [0, 0, img.0.round() as u32, img.1.round() as u32] {
            return self.render(); // the whole photo: nothing to do
        }
        let Some(writer) = &self.writer else {
            return self.render();
        };
        let edit = self
            .edits
            .get(&key)
            .copied()
            .unwrap_or(Orientation::IDENTITY);
        writer.submit(Job::Rotate {
            path: path.clone(),
            display: self.base_orientation(&path).then(edit),
            edit,
            keep_dates: self.settings.keep_file_dates,
            imperfect: self.settings.imperfect_jpeg,
            crop: Some(rect),
        });
        self.saving.insert(key);
        self.notice("Обрезка сохраняется…");
        self.render()
    }

    pub(super) fn crop_view(&self) -> Option<CropView> {
        let (screen, img) = self.crop_geometry()?;
        let c = self.crop.as_ref()?;
        let frame = crop::to_screen(c.frame, img, screen);
        let bar = self.crop_bar();
        let px = crop::pixels(c.frame);
        let mut segments: Vec<SegmentView> = bar
            .aspects
            .iter()
            .enumerate()
            .map(|(i, (a, label, r))| SegmentView {
                rect: *r,
                text: (*label).to_string(),
                selected: *a == c.aspect,
                hot: c.hot == Some(BarHit::Aspect(i)),
                first: i == 0,
                last: i + 1 == bar.aspects.len(),
            })
            .collect();
        for (r, text, hit) in [
            (bar.apply, "Применить", BarHit::Apply),
            (bar.cancel, "Отмена", BarHit::Cancel),
        ] {
            segments.push(SegmentView {
                rect: r,
                text: text.to_string(),
                selected: hit == BarHit::Apply,
                hot: c.hot == Some(hit),
                first: true,
                last: true,
            });
        }
        Some(CropView {
            image: screen,
            frame,
            size: format!("{} × {}", px[2], px[3]),
            segments,
        })
    }
}
