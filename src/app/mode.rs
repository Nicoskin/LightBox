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
    WM_SIZE, WM_SIZING, WM_SYSCOMMAND, WMSZ_BOTTOM, WMSZ_BOTTOMLEFT, WMSZ_LEFT, WMSZ_RIGHT,
    WMSZ_TOP, WMSZ_TOPLEFT, WMSZ_TOPRIGHT,
};

use super::{Anim, AnimKind, App, DIM, MODE_ANIM, Mode};

/// Smallest windowed-mode window (longer side), in 96-DPI pixels.
const MIN_WINDOW_DIP: f64 = 160.0;
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
        let Some(display) = self.shown_image().map(|i| i.display) else {
            return Ok(()); // nothing to put in a window
        };
        let mon = self.monitor;
        let work = to_rect(mon.work);
        let size = view::windowed_size(
            display,
            (work.width(), work.height()),
            mon.px(MIN_WINDOW_DIP),
        );
        let now = self.view.image_rect(display, self.viewport());
        let (ox, oy) = (f64::from(mon.rect.left), f64::from(mon.rect.top));
        let center = now.offset(ox, oy).center();
        let target = view::place_within(size, center, work);
        match MODE_ANIM {
            Some(duration) => {
                self.anim = Some(Anim {
                    kind: AnimKind::Mode {
                        from: now,
                        to: target.offset(-ox, -oy),
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
        let (w, h) = mon.size();
        self.gpu.resize(w, h)?;
        self.mode_switched_at = Instant::now();
        self.fitted = true;

        let target = self.fit_view();
        match (from, target, MODE_ANIM) {
            (Some(from), Some(target), Some(duration)) => {
                let display = self.shown_image().map_or((1.0, 1.0), |i| i.display);
                let vp = self.viewport();
                // First frame: the photo exactly where the window was.
                self.view = View::placed(display, from, vp);
                self.dim = 0.0;
                self.anim = Some(Anim {
                    kind: AnimKind::Mode {
                        from,
                        to: target.image_rect(display, vp),
                        dim: (0.0, DIM),
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
                self.dim = DIM;
            }
        }
        self.render()
    }

    /// Windowed mode, new photo: resize the window to it, keeping the centre.
    pub(super) fn fit_window_to_image(&mut self) -> Result<()> {
        let Some(display) = self.shown_image().map(|i| i.display) else {
            return Ok(());
        };
        let mon = Monitor::of_window(self.hwnd);
        self.monitor = mon;
        let work = to_rect(mon.work);
        let size = view::windowed_size(
            display,
            (work.width(), work.height()),
            mon.px(MIN_WINDOW_DIP),
        );
        let center = to_rect(window::rect(self.hwnd)).center();
        let target = view::place_within(size, center, work);
        window::sync_to_compositor();
        window::set_rect(self.hwnd, to_win32(target))?;
        self.gpu.resize(
            target.width().round() as u32,
            target.height().round() as u32,
        )
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
        } else if let Some(img) = self.shown_image() {
            self.view = self.view.clamped(img.display, self.viewport());
        }
        self.render()
    }

    /// `true` when dragging the photo should move the window (windowed
    /// mode, not zoomed in beyond "fill") — or Alt is held.
    fn drag_moves_window(&self) -> bool {
        // SAFETY: no preconditions.
        let alt = unsafe { GetKeyState(i32::from(VK_MENU.0)) } < 0;
        alt || self
            .fit_view()
            .is_none_or(|fit| self.view.scale <= fit.scale * 1.0001)
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
            _ if self.drag_moves_window() => HTCAPTION,
            _ => HTCLIENT,
        };
        LRESULT(code as isize)
    }

    /// Keeps the photo's aspect ratio while the user resizes the window.
    fn on_sizing(&self, edge: u32, r: &mut RECT) {
        let Some((dw, dh)) = self.shown_image().map(|i| i.display) else {
            return;
        };
        let aspect = dw / dh;
        let w = f64::from(r.right - r.left);
        let h = f64::from(r.bottom - r.top);
        let h_from_w = (w / aspect).round() as i32;
        let w_from_h = (h * aspect).round() as i32;
        match edge {
            WMSZ_LEFT | WMSZ_RIGHT => r.bottom = r.top + h_from_w,
            WMSZ_TOP | WMSZ_BOTTOM => r.right = r.left + w_from_h,
            _ => {
                // Corners: follow whichever side grew proportionally more.
                let by_height = f64::from(w_from_h) > w;
                let left = matches!(edge, WMSZ_TOPLEFT | WMSZ_BOTTOMLEFT);
                let top = matches!(edge, WMSZ_TOPLEFT | WMSZ_TOPRIGHT);
                if by_height {
                    if left {
                        r.left = r.right - w_from_h;
                    } else {
                        r.right = r.left + w_from_h;
                    }
                } else if top {
                    r.top = r.bottom - h_from_w;
                } else {
                    r.bottom = r.top + h_from_w;
                }
            }
        }
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
            WM_SIZING if windowed => {
                // SAFETY: for WM_SIZING, lParam points to the drag RECT.
                let r = unsafe { &mut *(lparam.0 as *mut RECT) };
                self.on_sizing(wparam.0 as u32, r);
                Some(LRESULT(1))
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
