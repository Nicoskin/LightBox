//! Keyboard and mouse.

use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, ReleaseCapture, SetCapture, VK_ADD, VK_CONTROL, VK_DELETE,
    VK_END, VK_ESCAPE, VK_F1, VK_F9, VK_F11, VK_HOME, VK_LEFT, VK_NEXT, VK_NUMPAD0, VK_NUMPAD1,
    VK_OEM_4, VK_OEM_6, VK_OEM_MINUS, VK_OEM_PLUS, VK_PRIOR, VK_RETURN, VK_RIGHT, VK_SHIFT,
    VK_SPACE, VK_SUBTRACT,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WHEEL_DELTA, WM_CAPTURECHANGED, WM_CLOSE, WM_KEYDOWN, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCMOUSEMOVE, WM_RBUTTONUP,
    WM_XBUTTONUP, XBUTTON1,
};

use super::mode::lparam_point;
use super::panel::Hot;
use super::{Anim, AnimKind, App, Drag, Mode, ZOOM_ANIM};
use crate::orientation::Orientation;
use crate::settings::Wheel;
use crate::ui::toolbar::Action;
use crate::view::View;
use crate::win::window;

/// Zoom factor per wheel notch / key press.
pub(super) const ZOOM_STEP: f64 = 1.25;
/// A press-release that moved less than this (pixels) is a click, not a drag.
const CLICK_SLOP: f64 = 4.0;

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
    /// Screen coordinates (wheel, non-client messages) → client coordinates.
    fn screen_to_client(&self, lparam: LPARAM) -> (f64, f64) {
        let (sx, sy) = lparam_point(lparam);
        let wr = window::rect(self.hwnd);
        (f64::from(sx - wr.left), f64::from(sy - wr.top))
    }

    pub(super) fn handle_input(
        &mut self,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> Result<Option<LRESULT>> {
        if self.card_open {
            let p = if msg == WM_MOUSEWHEEL {
                self.screen_to_client(lparam)
            } else {
                client_point(lparam)
            };
            if self.card_input(msg, wparam.0, p)? {
                return Ok(Some(LRESULT(0)));
            }
        }
        match msg {
            WM_KEYDOWN => {
                // Bit 30: the key was already down (auto-repeat).
                self.fast_nav = (lparam.0 >> 30) & 1 == 1;
                self.on_key(wparam.0 as u16)
            }
            WM_MOUSEWHEEL => {
                let delta = i32::from((wparam.0 >> 16) as u16 as i16);
                let p = self.screen_to_client(lparam);
                self.fast_nav = false;
                self.on_wheel(delta, p)?;
                Ok(Some(LRESULT(0)))
            }
            WM_LBUTTONDOWN => {
                self.on_button_down(client_point(lparam))?;
                Ok(Some(LRESULT(0)))
            }
            WM_MOUSEMOVE => {
                let p = client_point(lparam);
                self.show_panel();
                if let Some(start) = self.dial_drag {
                    self.drag_dial(start, p.0, key_down(VK_SHIFT.0))?;
                    return Ok(Some(LRESULT(0)));
                }
                if self.drag.is_none() {
                    let hot = self.panel_hit(p);
                    self.set_hot(hot)?;
                    self.menu_hover(p)?;
                    self.update_probe(p)?;
                }
                self.on_drag(p)?;
                Ok(Some(LRESULT(0)))
            }
            WM_NCMOUSEMOVE => {
                // Windowed mode: most of the window is a "caption" for dragging.
                self.show_panel();
                self.set_hot(None)?;
                Ok(None)
            }
            WM_LBUTTONUP => {
                let p = client_point(lparam);
                let drag = self.drag.take();
                let pressed = self.pressed.take();
                self.dial_drag = None;
                // SAFETY: releasing our own capture.
                unsafe {
                    let _ = ReleaseCapture();
                }
                if let Some(h) = pressed {
                    if h != Hot::DialRuler && self.panel_hit(p) == Some(h) {
                        self.activate(h)?;
                    }
                    self.render()?;
                } else if drag.is_some_and(|d| !d.moved) {
                    if key_down(VK_CONTROL.0) {
                        self.copy_color(p)?;
                    } else {
                        self.on_click(p)?;
                    }
                }
                Ok(Some(LRESULT(0)))
            }
            WM_CAPTURECHANGED => {
                self.drag = None;
                self.pressed = None;
                self.dial_drag = None;
                Ok(Some(LRESULT(0)))
            }
            WM_LBUTTONDBLCLK => {
                let p = client_point(lparam);
                if self.panel_hit(p) == Some(Hot::DialRuler) {
                    self.set_angle(0.0)?;
                } else if self.panel_hit(p).is_some() {
                    // Fast clicks on a panel button are separate clicks.
                    self.on_button_down(p)?;
                } else {
                    self.on_double_click(p)?;
                }
                Ok(Some(LRESULT(0)))
            }
            WM_RBUTTONUP => {
                // On the transparency button: choose the background; on the
                // picture: the context menu.
                let p = client_point(lparam);
                if self.panel_hit(p) == Some(Hot::Button(Action::Checker)) {
                    self.open_menu()?;
                } else if !self.over_panel(p) {
                    self.context_menu()?;
                }
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

    fn on_button_down(&mut self, p: (f64, f64)) -> Result<()> {
        // A click outside the open menu only closes it.
        if self.menu_open && self.menu_hit(p).is_none() {
            return self.close_menu();
        }
        if let Some(h) = self.panel_hit(p) {
            self.pressed = Some(h);
            if h == Hot::DialRuler {
                self.dial_drag = Some((p.0, self.angle));
            }
            self.drag = None;
            // SAFETY: own window.
            unsafe { SetCapture(self.hwnd) };
            return self.render();
        }
        if self.over_panel(p) {
            return Ok(()); // the bar itself is not a drag handle
        }
        self.drag = Some(Drag {
            start: p,
            last: p,
            moved: false,
        });
        // SAFETY: own window.
        unsafe { SetCapture(self.hwnd) };
        Ok(())
    }

    /// Runs what a panel button / filmstrip cell does.
    fn activate(&mut self, h: Hot) -> Result<()> {
        self.fast_nav = false;
        match h {
            Hot::Button(a) => self.run_action(a),
            Hot::Cell(i) => self.go_to(i),
            Hot::Caption(b) => self.caption_action(b),
            Hot::Menu(bg) => self.choose_background(bg),
            Hot::DialReset => self.set_angle(0.0),
            Hot::DialRuler => Ok(()),
            Hot::Info(b) => self.info_action(b),
        }
    }

    pub(super) fn run_action(&mut self, a: Action) -> Result<()> {
        let center = self.view_area().center();
        match a {
            Action::ZoomOut => self.zoom_by(1.0 / ZOOM_STEP, center),
            Action::ZoomIn => self.zoom_by(ZOOM_STEP, center),
            Action::ActualSize => self.zoom_to(1.0, center),
            Action::Fit => self.zoom_fit(),
            Action::Prev => self.step(-1),
            Action::Next => self.step(1),
            Action::RotateCcw => self.rotate(Orientation::ROTATE_CCW),
            Action::RotateCw => self.rotate(Orientation::ROTATE_CW),
            Action::Checker => {
                self.settings.checker = !self.settings.checker;
                self.settings.save();
                self.render()
            }
            Action::Smoothing => {
                self.settings.smoothing = !self.settings.smoothing;
                self.settings.save();
                self.render()
            }
            Action::Filmstrip => self.toggle_filmstrip(),
            Action::ToggleMode => self.toggle_mode(),
            Action::FlipH => self.rotate(Orientation::FLIP_H),
            Action::FlipV => self.rotate(Orientation::FLIP_V),
            Action::Save => self.save_current(),
            Action::SaveAs => self.save_as(),
            Action::Reset => self.reset_current(),
            Action::Settings => self.open_settings(),
            Action::Straighten => self.toggle_dial(),
        }
    }

    fn on_key(&mut self, vk: u16) -> Result<Option<LRESULT>> {
        let center = self.view_area().center();
        let ch = |c: u8| u16::from(c);
        let (ctrl, shift) = (key_down(VK_CONTROL.0), key_down(VK_SHIFT.0));
        match vk {
            k if ctrl && k == ch(b'S') => {
                if shift {
                    self.save_as()?;
                } else {
                    self.save_current()?;
                }
            }
            k if ctrl && k == ch(b'Z') => self.undo_save()?,
            k if ctrl && k == ch(b'C') => {
                if shift {
                    self.copy_path()?;
                } else {
                    self.copy_image()?;
                }
            }
            k if k == VK_DELETE.0 => self.delete_current()?,
            // F1..F9: quick folders (Ctrl: the other action, Shift: assign).
            k if (VK_F1.0..=VK_F9.0).contains(&k) => {
                let n = (k - VK_F1.0 + 1) as u8;
                if shift {
                    self.assign_folder(n)?;
                } else {
                    self.quick_folder(n, ctrl)?;
                }
            }
            k if k == ch(b'E') => self.open_in_editor(),
            // Space closes a QuickLook preview (as in Explorer).
            k if k == VK_SPACE.0 && self.quicklook => self.close_window(),
            k if k == VK_ESCAPE.0 && self.menu_open => self.close_menu()?,
            k if k == VK_ESCAPE.0 && self.info_state.open => self.toggle_info()?,
            k if k == VK_ESCAPE.0 && self.dial_open => self.toggle_dial()?,
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
            // Shift+1..4: one channel; Shift+0: all channels.
            k if shift && (ch(b'0')..=ch(b'4')).contains(&k) => {
                self.set_channel(u32::from(k - ch(b'0')))?;
            }
            k if k == ch(b'0') || k == VK_NUMPAD0.0 => self.zoom_fit()?,
            k if k == ch(b'1') || k == VK_NUMPAD1.0 => self.zoom_to(1.0, center)?,
            k if k == ch(b'Z') => self.toggle_zoom(center)?,
            k if k == VK_RETURN.0 => self.go_fullscreen()?,
            k if k == VK_F11.0 => self.toggle_mode()?,
            k if k == ch(b'R') => self.rotate(Orientation::ROTATE_CW)?,
            k if k == ch(b'L') => self.rotate(Orientation::ROTATE_CCW)?,
            k if k == ch(b'M') => self.rotate(if key_down(VK_SHIFT.0) {
                Orientation::FLIP_V
            } else {
                Orientation::FLIP_H
            })?,
            k if k == ch(b'F') => self.toggle_filmstrip()?,
            k if k == ch(b'T') => self.run_action(Action::Checker)?,
            k if k == ch(b'N') => self.run_action(Action::Smoothing)?,
            k if k == ch(b'G') => self.toggle_grid()?,
            k if k == ch(b'H') => self.toggle_histogram()?,
            k if k == ch(b'I') => self.toggle_info()?,
            // [ and ]: turn by 1° (Shift: 0.1°).
            k if k == VK_OEM_4.0 || k == VK_OEM_6.0 => {
                let step = if shift { 0.1 } else { 1.0 };
                let step = if k == VK_OEM_4.0 { -step } else { step };
                self.set_angle(((self.angle + step) * 10.0).round() / 10.0)?;
            }
            _ => return Ok(None),
        }
        Ok(Some(LRESULT(0)))
    }

    fn on_wheel(&mut self, delta: i32, p: (f64, f64)) -> Result<()> {
        // Over the dial the wheel turns the picture.
        if matches!(self.panel_hit(p), Some(Hot::DialRuler | Hot::DialReset)) {
            let step = if key_down(VK_SHIFT.0) { 0.1 } else { 1.0 };
            let a = self.angle + step * f64::from(delta.signum());
            return self.set_angle((a * 10.0).round() / 10.0);
        }
        // Over the panel the wheel browses; elsewhere it zooms (Ctrl swaps).
        let zooms = self.settings.wheel == Wheel::Zoom;
        let browse = self.over_panel(p) || zooms == key_down(VK_CONTROL.0);
        if !browse {
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
        if self.shown.is_none() {
            return Ok(());
        }
        self.finish_anim()?;
        let v = self.clamp(self.view.pan(dx, dy));
        if v != self.view {
            self.view = v;
            self.fitted = false;
            self.render()?;
            self.update_detail();
        }
        Ok(())
    }

    fn on_click(&mut self, p: (f64, f64)) -> Result<()> {
        // Click on the free (dimmed) area → windowed mode.
        if self.mode == Mode::Fullscreen
            && !self.over_panel(p)
            && !self.image_rect().is_some_and(|r| r.contains(p))
        {
            self.go_windowed()?;
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

    pub(super) fn zoom_by(&mut self, factor: f64, anchor: (f64, f64)) -> Result<()> {
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
        if self.shown.is_none() {
            return Ok(());
        }
        let to = self.clamp(to);
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
        match self.anim_len(ZOOM_ANIM) {
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
