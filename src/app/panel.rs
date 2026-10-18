//! The bottom panel and filmstrip: visibility, what they show, rotation.

use std::time::Instant;

use anyhow::Result;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::{App, Mode, PANEL_HOLD_MS, PANEL_TIMER, WM_APP_THUMB, post};
use crate::cache;
use crate::gfx::{ButtonView, CellView, PanelView, TextStyle, TextView};
use crate::orientation::Orientation;
use crate::ui::thumbs::Thumbnails;
use crate::ui::toolbar::{self, Action, Layout};
use crate::view::Rect;
use crate::win::window;

/// What the mouse is over / pressing on the panel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hot {
    Button(Action),
    /// Filmstrip cell: file index.
    Cell(usize),
}

/// Thumbnails requested beyond the visible cells, each side.
const THUMB_LOOKAHEAD: usize = 6;

impl App {
    fn panel_layout(&self) -> Layout {
        let windowed = self.mode == Mode::Windowed;
        toolbar::layout(
            self.viewport(),
            self.ui_scale(),
            windowed,
            self.filmstrip,
            windowed,
        )
    }

    /// Shows the panel (fading in) and restarts its auto-hide timer.
    pub(super) fn show_panel(&mut self) {
        if self.panel_target < 1.0 {
            self.panel_target = 1.0;
            self.panel_tick = Instant::now();
        }
        // SAFETY: own window; re-arming a timer id resets it.
        unsafe { SetTimer(Some(self.hwnd), PANEL_TIMER, PANEL_HOLD_MS, None) };
    }

    fn hide_panel(&mut self) {
        if self.panel_target > 0.0 {
            self.panel_target = 0.0;
            self.panel_tick = Instant::now();
            self.hot = None;
        }
    }

    /// Auto-hide: hide unless the cursor rests on the panel.
    pub(super) fn panel_timer(&mut self) {
        // SAFETY: own window and timer.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), PANEL_TIMER);
        }
        let c = window::cursor_pos();
        let wr = window::rect(self.hwnd);
        let p = (f64::from(c.x - wr.left), f64::from(c.y - wr.top));
        let inside = c.x >= wr.left && c.x < wr.right && c.y >= wr.top && c.y < wr.bottom;
        if self.pressed.is_some() || (inside && self.panel_layout().contains(p)) {
            self.show_panel();
        } else {
            self.hide_panel();
        }
    }

    /// `true` if `p` is over the (visible) panel or strip.
    pub(super) fn over_panel(&self, p: (f64, f64)) -> bool {
        self.panel_target > 0.5 && self.panel_layout().contains(p)
    }

    /// Button or filmstrip cell under `p`.
    pub(super) fn panel_hit(&self, p: (f64, f64)) -> Option<Hot> {
        if self.panel_target < 0.5 {
            return None;
        }
        let l = self.panel_layout();
        if let Some(a) = l.hit(p) {
            return Some(Hot::Button(a));
        }
        let strip = l.strip.filter(|s| s.contains(p))?;
        toolbar::strip_cells(strip, self.ui_scale(), self.files.len(), self.index)
            .into_iter()
            .find(|(_, r)| r.contains(p))
            .map(|(i, _)| Hot::Cell(i))
    }

    pub(super) fn set_hot(&mut self, hot: Option<Hot>) -> Result<()> {
        if hot != self.hot {
            self.hot = hot;
            self.render()?;
        }
        Ok(())
    }

    /// Builds what the overlay draws; `None` when the panel is hidden.
    pub(super) fn panel_view(&self) -> Option<PanelView> {
        if self.panel_alpha <= 0.0 {
            return None;
        }
        let l = self.panel_layout();
        let buttons = l
            .buttons
            .iter()
            .map(|b| ButtonView {
                rect: b.rect,
                icon: b.icon,
                round: b.round,
                hot: self.hot == Some(Hot::Button(b.action)),
                pressed: self.pressed == Some(Hot::Button(b.action)),
                active: match b.action {
                    Action::Checker => self.checker,
                    Action::Smoothing => self.filter == crate::gfx::Filter::Smooth,
                    Action::Filmstrip => self.filmstrip,
                    _ => false,
                },
            })
            .collect();

        let mut texts = Vec::new();
        if let Some(r) = l.zoom_text {
            texts.push(TextView {
                rect: r,
                text: zoom_label(self.zoom_target().scale),
                style: TextStyle::Value,
            });
        }
        if let Some(r) = l.caption {
            let mid = (r.y0 + r.y1) / 2.0;
            let name = self
                .current
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            texts.push(TextView {
                rect: Rect { y1: mid + 2.0, ..r },
                text: name,
                style: TextStyle::Caption,
            });
            texts.push(TextView {
                rect: Rect { y0: mid + 2.0, ..r },
                text: self.details(),
                style: TextStyle::Detail,
            });
        }

        let cells = l.strip.map_or_else(Vec::new, |strip| {
            toolbar::strip_cells(strip, self.ui_scale(), self.files.len(), self.index)
                .into_iter()
                .map(|(i, rect)| CellView {
                    rect,
                    thumb: self.thumbs.as_ref().and_then(|t| t.get(&self.files[i])),
                    current: i == self.index,
                    hot: self.hot == Some(Hot::Cell(i)),
                })
                .collect()
        });

        Some(PanelView {
            opacity: self.panel_alpha,
            bar: l.bar,
            strip: l.strip,
            buttons,
            texts,
            cells,
        })
    }

    /// "15 из 18 · 6000 × 4000" (+ a note about unsaved rotation).
    fn details(&self) -> String {
        let mut parts = Vec::new();
        if self.files.len() > 1 {
            parts.push(format!("{} из {}", self.index + 1, self.files.len()));
        }
        if let Some((w, h)) = self.display() {
            parts.push(format!("{w:.0} × {h:.0}"));
        }
        if self.edits.contains_key(&cache::key(&self.current)) {
            parts.push("повёрнуто, не сохранено".into());
        }
        parts.join("  ·  ")
    }

    /// Rotates / mirrors the current photo on screen (saved to the file in M2).
    pub(super) fn rotate(&mut self, op: Orientation) -> Result<()> {
        if self.shown.is_none() {
            return Ok(());
        }
        self.finish_anim()?;
        let key = cache::key(&self.current);
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
        if self.mode == Mode::Windowed {
            self.fit_window_to_image()?;
        }
        if let Some(v) = self.fit_view() {
            self.view = v;
            self.fitted = true;
        }
        self.render()
    }

    pub(super) fn toggle_filmstrip(&mut self) -> Result<()> {
        self.filmstrip = !self.filmstrip;
        if self.fitted
            && let Some(v) = self.fit_view()
        {
            self.view = v;
        }
        self.request_thumbs();
        self.show_panel();
        self.render()
    }

    /// Starts the thumbnail thread (after the first frame).
    pub(super) fn start_thumbs(&mut self) {
        let hwnd = self.hwnd.0 as isize;
        let size = (96.0 * self.ui_scale()).round() as u32;
        self.thumbs = Some(Thumbnails::new(size, move || post(hwnd, WM_APP_THUMB)));
        self.request_thumbs();
    }

    /// Requests thumbnails for the visible cells and a few beyond, nearest
    /// to the current file first.
    pub(super) fn request_thumbs(&self) {
        let Some(thumbs) = &self.thumbs else {
            return;
        };
        if !self.filmstrip || self.mode == Mode::Windowed || self.files.is_empty() {
            return;
        }
        let Some(strip) = self.panel_layout().strip else {
            return;
        };
        let visible =
            toolbar::strip_cells(strip, self.ui_scale(), self.files.len(), self.index).len();
        let reach = visible / 2 + THUMB_LOOKAHEAD;
        let n = self.files.len();
        let mut order = vec![self.index];
        for k in 1..=reach {
            if self.index + k < n {
                order.push(self.index + k);
            }
            if k <= self.index {
                order.push(self.index - k);
            }
        }
        let paths: Vec<_> = order.into_iter().map(|i| self.files[i].clone()).collect();
        thumbs.request(&paths);
    }

    pub(super) fn on_thumbs(&mut self) -> Result<()> {
        let got = self.thumbs.as_mut().is_some_and(Thumbnails::receive);
        if got && self.panel_alpha > 0.0 && self.filmstrip {
            self.render()?;
        }
        Ok(())
    }
}

/// "68 %", "1250 %", "4.5 %".
pub fn zoom_label(scale: f64) -> String {
    let pct = scale * 100.0;
    if pct < 10.0 {
        format!("{pct:.1} %")
    } else {
        format!("{pct:.0} %")
    }
}
