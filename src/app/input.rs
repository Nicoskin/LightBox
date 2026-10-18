//! Keyboard and mouse.

use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, ReleaseCapture, SetCapture, VK_ADD, VK_CONTROL, VK_END,
    VK_ESCAPE, VK_F11, VK_HOME, VK_LEFT, VK_NEXT, VK_NUMPAD0, VK_NUMPAD1, VK_OEM_MINUS,
    VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SUBTRACT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WHEEL_DELTA, WM_CAPTURECHANGED, WM_CLOSE, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_XBUTTONUP, XBUTTON1,
};

use super::mode::lparam_point;
use super::{Anim, AnimKind, App, Drag, Mode, ZOOM_ANIM};
use crate::view::View;
use crate::win::window;

/// Zoom factor per wheel notch / key press.
const ZOOM_STEP: f64 = 1.25;
/// A press-release that moved less than this (pixels) is a click, not a drag.
const CLICK_SLOP: f64 = 4.0;
/// Wheel: zooms at the cursor; with Ctrl it browses (configurable in M6).
const WHEEL_ZOOMS: bool = true;

fn key_down(vk: u16) -> bool {
    // SAFETY: no preconditions.
    unsafe { GetKeyState(i32::from(vk)) < 0 }
}

/// Client coordinates packed in an LPARAM (mouse messages).
fn client_point(lparam: LPARAM) -> (f64, f64) {
    let (x, y) = lparam_point(lparam);
    (f64::from(x), f64::from(y))
}

impl App {
    pub(super) fn handle_input(
        &mut self,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<Option<LRESULT>> {
        match msg {
            WM_KEYDOWN => {
                // Bit 30: the key was already down (auto-repeat).
                self.fast_nav = (lparam.0 >> 30) & 1 == 1;
                self.on_key(wparam.0 as u16)
            }
            WM_MOUSEWHEEL => {
                let delta = i32::from((wparam.0 >> 16) as u16 as i16);
                // Wheel coordinates are screen-relative.
                let (sx, sy) = lparam_point(lparam);
                let wr = window::rect(self.hwnd);
                let p = (f64::from(sx - wr.left), f64::from(sy - wr.top));
                self.fast_nav = false;
                self.on_wheel(delta, p)?;
                Ok(Some(LRESULT(0)))
            }
            WM_LBUTTONDOWN => {
                let p = client_point(lparam);
                self.drag = Some(Drag {
                    start: p,
                    last: p,
                    moved: false,
                });
                // SAFETY: own window.
                unsafe { SetCapture(self.hwnd) };
                Ok(Some(LRESULT(0)))
            }
            WM_MOUSEMOVE => {
                self.on_drag(client_point(lparam))?;
                Ok(Some(LRESULT(0)))
            }
            WM_LBUTTONUP => {
                let p = client_point(lparam);
                let drag = self.drag.take();
                // SAFETY: releasing our own capture.
                unsafe {
                    let _ = ReleaseCapture();
                }
                if drag.is_some_and(|d| !d.moved) {
                    self.on_click(p)?;
                }
                Ok(Some(LRESULT(0)))
            }
            WM_CAPTURECHANGED => {
                self.drag = None;
                Ok(Some(LRESULT(0)))
            }
            WM_LBUTTONDBLCLK => {
                self.on_double_click(client_point(lparam))?;
                Ok(Some(LRESULT(0)))
            }
            WM_XBUTTONUP => {
                let button = (wparam.0 >> 16) as u16;
                self.fast_nav = false;
                self.step(if button == XBUTTON1 { -1 } else { 1 })?;
                Ok(Some(LRESULT(1)))
            }
            _ => Ok(None),
        }
    }

    fn on_key(&mut self, vk: u16) -> Result<Option<LRESULT>> {
        let center = {
            let (w, h) = self.viewport();
            (w / 2.0, h / 2.0)
        };
        match vk {
            k if k == VK_ESCAPE.0 => {
                // SAFETY: posting to our own window.
                unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))? };
            }
            k if k == VK_RIGHT.0 || k == VK_NEXT.0 => self.step(1)?,
            k if k == VK_LEFT.0 || k == VK_PRIOR.0 => self.step(-1)?,
            k if k == VK_HOME.0 => self.go_to(0)?,
            k if k == VK_END.0 => self.go_to(self.files.len().saturating_sub(1))?,
            k if k == VK_ADD.0 || k == VK_OEM_PLUS.0 => self.zoom_by(ZOOM_STEP, center)?,
            k if k == VK_SUBTRACT.0 || k == VK_OEM_MINUS.0 => {
                self.zoom_by(1.0 / ZOOM_STEP, center)?;
            }
            k if k == u16::from(b'0') || k == VK_NUMPAD0.0 => self.zoom_fit()?,
            k if k == u16::from(b'1') || k == VK_NUMPAD1.0 => self.zoom_to(1.0, center)?,
            k if k == u16::from(b'Z') => self.toggle_zoom(center)?,
            k if k == VK_RETURN.0 => self.go_fullscreen()?,
            k if k == VK_F11.0 => self.toggle_mode()?,
            // T: checkerboard ↔ nothing behind transparent areas.
            k if k == u16::from(b'T') => {
                self.checker = !self.checker;
                self.render()?;
            }
            // N: smoothing on/off (nearest-neighbour when magnifying).
            k if k == u16::from(b'N') => {
                self.filter = match self.filter {
                    crate::gfx::Filter::Smooth => crate::gfx::Filter::Pixelated,
                    _ => crate::gfx::Filter::Smooth,
                };
                self.render()?;
            }
            _ => return Ok(None),
        }
        Ok(Some(LRESULT(0)))
    }

    fn on_wheel(&mut self, delta: i32, p: (f64, f64)) -> Result<()> {
        if WHEEL_ZOOMS != key_down(VK_CONTROL.0) {
            let notches = f64::from(delta) / f64::from(WHEEL_DELTA);
            return self.zoom_by(ZOOM_STEP.powf(notches), p);
        }
        // Browsing: one file per full notch (high-resolution wheels send
        // smaller deltas that accumulate).
        self.wheel_accum += delta;
        let notch = WHEEL_DELTA as i32;
        while self.wheel_accum.abs() >= notch {
            let dir = -self.wheel_accum.signum();
            self.wheel_accum += dir * notch;
            self.step(dir as isize)?;
        }
        Ok(())
    }

    fn on_drag(&mut self, p: (f64, f64)) -> Result<()> {
        let Some(drag) = &mut self.drag else {
            return Ok(());
        };
        if !drag.moved {
            let (dx, dy) = (p.0 - drag.start.0, p.1 - drag.start.1);
            if dx.hypot(dy) < CLICK_SLOP {
                return Ok(());
            }
            drag.moved = true;
        }
        let (dx, dy) = (p.0 - drag.last.0, p.1 - drag.last.1);
        drag.last = p;
        let Some(display) = self.shown_image().map(|i| i.display) else {
            return Ok(());
        };
        self.finish_anim()?;
        let vp = self.viewport();
        let v = self.view.pan(dx, dy).clamped(display, vp);
        if v != self.view {
            self.view = v;
            self.fitted = false;
            self.render()?;
        }
        Ok(())
    }

    fn on_click(&mut self, p: (f64, f64)) -> Result<()> {
        match self.mode {
            // Click on the free (dimmed) area → windowed mode.
            Mode::Fullscreen => {
                if !self.image_rect().is_some_and(|r| r.contains(p)) {
                    self.go_windowed()?;
                }
            }
            Mode::Windowed => {}
        }
        Ok(())
    }

    fn on_double_click(&mut self, p: (f64, f64)) -> Result<()> {
        // The second click of a double click that just switched the mode.
        // SAFETY: no preconditions.
        let dbl = Duration::from_millis(u64::from(unsafe { GetDoubleClickTime() }));
        if self.mode_switched_at.elapsed() < dbl {
            return Ok(());
        }
        match self.mode {
            Mode::Fullscreen => {
                if self.image_rect().is_some_and(|r| r.contains(p)) {
                    self.toggle_zoom(p)
                } else {
                    self.on_click(p)
                }
            }
            Mode::Windowed => self.go_fullscreen(),
        }
    }

    fn zoom_by(&mut self, factor: f64, anchor: (f64, f64)) -> Result<()> {
        let base = self.zoom_target();
        self.zoom_to_view(
            base.zoom_at(base.scale * factor, anchor, self.viewport()),
            anchor,
        )
    }

    fn zoom_to(&mut self, scale: f64, anchor: (f64, f64)) -> Result<()> {
        let base = self.zoom_target();
        self.zoom_to_view(base.zoom_at(scale, anchor, self.viewport()), anchor)
    }

    fn zoom_fit(&mut self) -> Result<()> {
        let Some(fit) = self.fit_view() else {
            return Ok(());
        };
        self.animate_view(fit, None);
        self.fitted = true;
        self.show_badge();
        Ok(())
    }

    /// Z / double click: "fit" ↔ 100 %.
    fn toggle_zoom(&mut self, anchor: (f64, f64)) -> Result<()> {
        let Some(fit) = self.fit_view() else {
            return Ok(());
        };
        let base = self.zoom_target();
        if (base.scale - fit.scale).abs() < 1e-6 && (fit.scale - 1.0).abs() > 1e-6 {
            self.zoom_to(1.0, anchor)
        } else {
            self.zoom_fit()
        }
    }

    fn zoom_to_view(&mut self, to: View, anchor: (f64, f64)) -> Result<()> {
        let Some(display) = self.shown_image().map(|i| i.display) else {
            return Ok(());
        };
        let to = to.clamped(display, self.viewport());
        self.fitted = self.fit_view() == Some(to);
        self.animate_view(to, Some(anchor));
        self.show_badge();
        Ok(())
    }

    fn animate_view(&mut self, to: View, anchor: Option<(f64, f64)>) {
        // A running mode transition or page turn must complete first.
        if self
            .anim
            .as_ref()
            .is_some_and(|a| !matches!(a.kind, AnimKind::View { .. }))
            && let Err(e) = self.finish_anim()
        {
            tracing::error!("{e:#}");
        }
        match ZOOM_ANIM {
            Some(duration) => {
                self.anim = Some(Anim {
                    kind: AnimKind::View {
                        from: self.view,
                        to,
                        anchor,
                    },
                    start: Instant::now(),
                    duration,
                    frames: 0,
                });
            }
            None => {
                self.view = to;
                if let Err(e) = self.render() {
                    tracing::error!("{e:#}");
                }
            }
        }
        // Zooming in past the preview's resolution needs the full image.
        self.schedule();
    }
}
