//! Fullscreen ↔ windowed modes and the window geometry messages.
//!
//! Transitions are animated inside the fullscreen-sized window (the image
//! glides between its two positions while the backdrop fades); the actual
//! window resize happens at the end of fullscreen → windowed, or at the start
//! of windowed → fullscreen, aligned to a DWM frame.

use std::time::Instant;

use anyhow::Result;
use windows::Win32::Foundation::{LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_MENU};
use windows::Win32::UI::WindowsAndMessaging::{
    HTBOTTOM, HTBOTTOMLEFT, HTBOTTOMRIGHT, HTCAPTION, HTCLIENT, HTLEFT, HTRIGHT, HTTOP, HTTOPLEFT,
    HTTOPRIGHT, SC_MAXIMIZE, WM_DISPLAYCHANGE, WM_DPICHANGED, WM_NCHITTEST, WM_NCLBUTTONDBLCLK,
    WM_SIZE, WM_SYSCOMMAND,
};

use super::{Anim, AnimKind, App, MODE_ANIM, Mode};

/// Smallest windowed-mode window (longer side), in 96-DPI pixels.
const MIN_WINDOW_DIP: f64 = 160.0;
/// Width of the dark frame around the photo in windowed mode, 96-DPI pixels.
pub(super) const FRAME_DIP: f64 = 6.0;
use crate::ui::caption;
use crate::view::{self, Rect, View};
use crate::win::monitor::Monitor;
use crate::win::window;

fn to_rect(r: RECT) -> Rect {
    Rect {
        x0: f64::from(r.left),
        y0: f64::from(r.top),
        x1: f64::from(r.right),
        y1: f64::from(r.bottom),
    }
}

fn to_win32(r: Rect) -> RECT {
    RECT {
        left: r.x0.round() as i32,
        top: r.y0.round() as i32,
        right: r.x1.round() as i32,
        bottom: r.y1.round() as i32,
    }
}

/// Signed screen coordinates packed in an LPARAM.
pub(super) fn lparam_point(lparam: LPARAM) -> (i32, i32) {
    (
        i32::from((lparam.0 & 0xffff) as u16 as i16),
        i32::from(((lparam.0 >> 16) & 0xffff) as u16 as i16),
    )
}

impl App {
    pub(super) fn toggle_mode(&mut self) -> Result<()> {
        if self.home() {
            return Ok(());
        }
        match self.mode {
            Mode::Fullscreen => self.go_windowed(),
            Mode::Windowed => self.go_fullscreen(),
        }
    }

    /// Fullscreen → windowed: the window becomes the photo's size, centred
    /// where the photo currently is.
    pub(super) fn go_windowed(&mut self) -> Result<()> {
        if self.mode == Mode::Windowed {
            return Ok(());
        }
        self.finish_anim()?;
        if self.mode == Mode::Windowed {
            return Ok(()); // the animation that just ended entered it
        }
        let Some(display) = self.display() else {
            return Ok(()); // nothing to put in a window
        };
        let mon = self.monitor;
        let work = to_rect(mon.work);
        let size = window_size(display, &mon, self.window_frac());
        let now = self.view.image_rect(display, self.viewport());
        let (ox, oy) = (f64::from(mon.rect.left), f64::from(mon.rect.top));
        let center = now.offset(ox, oy).center();
        let target = view::place_within(size, center, work);
        match self.anim_len(MODE_ANIM) {
            Some(duration) => {
                self.anim = Some(Anim {
                    kind: AnimKind::Mode {
                        from: now,
                        to: photo_area(target.offset(-ox, -oy), &mon),
                        dim: (self.dim, 0.0),
                        opacity: (1.0, 1.0),
                        enter_windowed: Some(target),
                        end_view: None,
                    },
                    start: Instant::now(),
                    duration,
                    frames: 0,
                });
                Ok(())
            }
            None => self.enter_windowed(target),
        }
    }

    /// Final step of fullscreen → windowed: restyle and shrink the window
    /// to `r` (screen coordinates).
    pub(super) fn enter_windowed(&mut self, r: Rect) -> Result<()> {
        window::sync_to_compositor();
        self.mode = Mode::Windowed;
        self.dim = 0.0;
        window::set_mode(self.hwnd, true, to_win32(r))?;
        window::set_topmost(self.hwnd, self.on_top);
        self.gpu
            .resize(r.width().round() as u32, r.height().round() as u32)?;
        if let Some(v) = self.fit_view() {
            self.view = v;
        }
        self.fitted = true;
        self.mode_switched_at = Instant::now();
        self.schedule();
        self.render()
    }

    /// Windowed → fullscreen on the monitor the window is on.
    pub(super) fn go_fullscreen(&mut self) -> Result<()> {
        if self.mode == Mode::Fullscreen {
            return Ok(());
        }
        self.finish_anim()?;
        if self.mode == Mode::Fullscreen {
            return Ok(());
        }
        let wr = window::rect(self.hwnd);
        let mon = Monitor::of_window(self.hwnd);
        let from = self.image_rect().map(|r| {
            r.offset(
                f64::from(wr.left - mon.rect.left),
                f64::from(wr.top - mon.rect.top),
            )
        });

        window::sync_to_compositor();
        self.mode = Mode::Fullscreen;
        self.monitor = mon;
        window::set_mode(self.hwnd, false, mon.rect)?;
        window::set_topmost(self.hwnd, false);
        let (w, h) = mon.size();
        self.gpu.resize(w, h)?;
        self.request_thumbs();
        self.mode_switched_at = Instant::now();
        self.fitted = true;

        let target = self.fit_view();
        match (from, target, self.anim_len(MODE_ANIM)) {
            (Some(from), Some(target), Some(duration)) => {
                let display = self.display().unwrap_or((1.0, 1.0));
                let vp = self.viewport();
                // First frame: the photo exactly where the window was.
                self.view = View::placed(display, from, vp);
                self.dim = 0.0;
                self.anim = Some(Anim {
                    kind: AnimKind::Mode {
                        from,
                        to: target.image_rect(display, vp),
                        dim: (0.0, self.settings.dim()),
                        opacity: (1.0, 1.0),
                        enter_windowed: None,
                        end_view: Some(target),
                    },
                    start: Instant::now(),
                    duration,
                    frames: 0,
                });
            }
            (_, target, _) => {
                if let Some(v) = target {
                    self.view = v;
                }
                self.dim = self.settings.dim();
            }
        }
        self.render()
    }

    /// Start-up in windowed mode (setting): the window takes the photo's
    /// size, centred in the work area, before the first frame is drawn.
    pub(super) fn start_windowed(&mut self) -> Result<()> {
        let Some(display) = self.display() else {
            return Ok(());
        };
        let mon = self.monitor;
        let work = to_rect(mon.work);
        let target = view::place_within(
            window_size(display, &mon, self.window_frac()),
            work.center(),
            work,
        );
        self.mode = Mode::Windowed;
        self.dim = 0.0;
        window::set_mode(self.hwnd, true, to_win32(target))?;
        window::set_topmost(self.hwnd, self.on_top);
        self.gpu.resize(
            target.width().round() as u32,
            target.height().round() as u32,
        )?;
        if let Some(v) = self.fit_view() {
            self.view = v;
        }
        self.fitted = true;
        Ok(())
    }

    /// Fullscreen: re-cover the monitor (resolution or DPI changed).
    fn cover_monitor(&mut self) -> Result<()> {
        self.monitor = Monitor::of_window(self.hwnd);
        if self.mode == Mode::Fullscreen {
            window::set_rect(self.hwnd, self.monitor.rect)?;
            let (w, h) = self.monitor.size();
            self.on_size(w, h)?;
        }
        Ok(())
    }

    fn on_size(&mut self, width: u32, height: u32) -> Result<()> {
        if width == 0 || height == 0 {
            return Ok(()); // minimised
        }
        self.gpu.resize(width, height)?;
        if self.fitted {
            if let Some(v) = self.fit_view() {
                self.view = v;
            }
        } else {
            self.view = self.clamp(self.view);
        }
        self.update_detail();
        self.render()
    }

    /// `true` when dragging the photo should move the window (windowed
    /// mode, not zoomed in beyond "fill") — or Alt is held.
    fn drag_moves_window(&self) -> bool {
        // SAFETY: no preconditions.
        let alt = unsafe { GetKeyState(i32::from(VK_MENU.0)) } < 0;
        alt || (!self.model_mode()
            && self
                .fit_view()
                .is_none_or(|fit| self.view.scale <= fit.scale * 1.0001))
    }

    fn hit_test(&self, lparam: LPARAM) -> LRESULT {
        let (sx, sy) = lparam_point(lparam);
        let wr = window::rect(self.hwnd);
        let (x, y) = (f64::from(sx - wr.left), f64::from(sy - wr.top));
        let (w, h) = self.viewport();
        let b = (6.0 * self.ui_scale()).max(4.0);
        let (l, r, t, btm) = (x < b, x >= w - b, y < b, y >= h - b);
        let code = match (l, r, t, btm) {
            (true, _, true, _) => HTTOPLEFT,
            (_, true, true, _) => HTTOPRIGHT,
            (true, _, _, true) => HTBOTTOMLEFT,
            (_, true, _, true) => HTBOTTOMRIGHT,
            (true, ..) => HTLEFT,
            (_, true, ..) => HTRIGHT,
            (_, _, true, _) => HTTOP,
            (.., true) => HTBOTTOM,
            _ if self.over_panel((x, y)) => HTCLIENT,
            // The settings card takes every click (outside it: close).
            _ if self.card_open => HTCLIENT,
            _ if self.over_title_bar((x, y)) => HTCAPTION,
            _ if self.drag_moves_window() => HTCAPTION,
            _ => HTCLIENT,
        };
        LRESULT(code as isize)
    }

    /// Window-geometry messages. `None` = not handled here.
    pub(super) fn handle_window(
        &mut self,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<Option<LRESULT>> {
        let windowed = self.mode == Mode::Windowed;
        Ok(match msg {
            WM_NCHITTEST if windowed => Some(self.hit_test(lparam)),
            WM_NCLBUTTONDBLCLK if windowed => {
                self.go_fullscreen()?;
                Some(LRESULT(0))
            }
            WM_SIZE => {
                let (w, h) = (lparam.0 & 0xffff, (lparam.0 >> 16) & 0xffff);
                self.on_size(w as u32, h as u32)?;
                Some(LRESULT(0))
            }
            // Maximise (Win+Up, snap) means "fullscreen" for a photo window.
            WM_SYSCOMMAND if (wparam.0 as u32 & 0xfff0) == SC_MAXIMIZE => {
                self.go_fullscreen()?;
                Some(LRESULT(0))
            }
            WM_DPICHANGED | WM_DISPLAYCHANGE => {
                // Windowed: keep the size (photo pixels stay 1:1) and ignore
                // the suggested rectangle; fullscreen: re-cover the monitor.
                self.cover_monitor()?;
                self.render()?;
                Some(LRESULT(0))
            }
            _ => None,
        })
    }
}

/// Windowed-mode window size for a photo of `display` pixels: the photo at
/// 100 % (or fitted into 90 % of the work area) plus the title bar and the
/// frame. Chosen when the window opens; browsing keeps it.
fn window_size(display: (f64, f64), mon: &Monitor, frac: f64) -> (f64, f64) {
    let f = mon.px(FRAME_DIP);
    let t = mon.px(caption::TITLE_H).round();
    let work = to_rect(mon.work);
    let photo = view::windowed_size(
        display,
        (work.width() - 2.0 * f, work.height() - f - t),
        frac,
        mon.px(MIN_WINDOW_DIP),
    );
    (photo.0 + 2.0 * f, photo.1 + f + t)
}

/// Where the photo goes inside a window occupying `window`: below the
/// title bar, inside the frame.
pub(super) fn photo_area(window: Rect, mon: &Monitor) -> Rect {
    let f = mon.px(FRAME_DIP);
    Rect {
        x0: window.x0 + f,
        y0: window.y0 + mon.px(caption::TITLE_H).round(),
        x1: window.x1 - f,
        y1: window.y1 - f,
    }
}

impl App {
    /// Largest share of the work area a new window takes: previews (Space
    /// in Explorer, pinned) stay smaller than the viewer window.
    pub(super) fn window_frac(&self) -> f64 {
        if self.preview() { 0.8 } else { 0.9 }
    }

    /// A preview shows a new photo: the window takes its size, keeping
    /// the centre.
    pub(super) fn fit_preview_window(&mut self) -> Result<()> {
        let Some(display) = self.display() else {
            return Ok(());
        };
        let mon = Monitor::of_window(self.hwnd);
        self.monitor = mon;
        let work = to_rect(mon.work);
        let size = window_size(display, &mon, self.window_frac());
        let center = to_rect(window::rect(self.hwnd)).center();
        let target = view::place_within(size, center, work);
        window::sync_to_compositor();
        window::set_rect(self.hwnd, to_win32(target))?;
        self.gpu.resize(
            target.width().round() as u32,
            target.height().round() as u32,
        )
    }

    /// `--pinned`: a standalone preview window at `r` (screen), taking over
    /// from the tray viewer without any animation.
    pub fn start_pinned(&mut self, r: RECT) -> Result<()> {
        self.pinned = true;
        self.on_top = true;
        self.mode = Mode::Windowed;
        self.dim = 0.0;
        window::set_mode(self.hwnd, true, r)?;
        window::set_topmost(self.hwnd, self.on_top);
        self.monitor = Monitor::of_window(self.hwnd);
        self.gpu
            .resize((r.right - r.left) as u32, (r.bottom - r.top) as u32)?;
        if let Some(v) = self.fit_view() {
            self.view = v;
        }
        self.fitted = true;
        Ok(())
    }
}

/// Start screen window size, 96-DPI pixels.
const HOME_SIZE: (f64, f64) = (960.0, 620.0);

impl App {
    /// Shows the start screen (launched without a file, or from the tray):
    /// a mid-sized window with "Open file…" and "Settings".
    pub fn show_home(&mut self) -> Result<()> {
        if !self.hidden {
            // Already showing a photo or the start screen.
            self.bring_to_front();
            return Ok(());
        }
        self.current = std::path::PathBuf::new();
        self.files.clear();
        self.all_files.clear();
        self.index = 0;
        self.shown = None;
        self.model = None;
        self.error = None;
        self.notice = None;
        self.quicklook = false;
        self.pending_show = false;
        self.hidden = false;
        self.monitor = Monitor::under_cursor();
        let s = self.ui_scale();
        let work = to_rect(self.monitor.work);
        let size = (
            (HOME_SIZE.0 * s).min(work.width()),
            (HOME_SIZE.1 * s).min(work.height()),
        );
        let target = view::place_within(size, work.center(), work);
        self.mode = Mode::Windowed;
        self.dim = 0.0;
        window::set_mode(self.hwnd, true, to_win32(target))?;
        window::set_topmost(self.hwnd, self.on_top);
        self.gpu.resize(
            target.width().round() as u32,
            target.height().round() as u32,
        )?;
        self.update_title();
        self.render()?;
        window::show(self.hwnd);
        self.bring_to_front();
        Ok(())
    }
}

impl App {
    /// `--peek`: a Space preview in its own process (the viewer that got
    /// the Space shows another photo): a window sized to the photo, no
    /// folder, closed by Space or Esc.
    pub fn start_peek(&mut self) -> Result<()> {
        self.peek = true;
        self.quicklook = true;
        self.start_windowed()
    }
}
