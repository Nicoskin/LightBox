//! Keyboard and mouse.

use std::time::{Duration, Instant};

use anyhow::Result;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Input::KeyboardAndMouse::{
    GetDoubleClickTime, GetKeyState, ReleaseCapture, SetCapture, VK_CONTROL, VK_ESCAPE, VK_F1,
    VK_F9, VK_MENU, VK_SHIFT, VK_SPACE,
};
use windows::Win32::UI::WindowsAndMessaging::{
    PostMessageW, WHEEL_DELTA, WM_CAPTURECHANGED, WM_CLOSE, WM_KEYDOWN, WM_KEYUP, WM_LBUTTONDBLCLK,
    WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE, WM_MOUSEWHEEL, WM_NCMOUSEMOVE, WM_RBUTTONUP,
    WM_XBUTTONUP, XBUTTON1,
};

use super::mode::lparam_point;
use super::panel::Hot;
use super::{Anim, AnimKind, App, Drag, Mode, ZOOM_ANIM};
use crate::keymap::{Command, Key};
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
        if self.tone.is_some()
            && msg != WM_MOUSEWHEEL
            && self.tone_input(msg, wparam.0, client_point(lparam))?
        {
            return Ok(Some(LRESULT(0)));
        }
        if self.crop.is_some()
            && msg != WM_MOUSEWHEEL
            && self.crop_input(msg, wparam.0, client_point(lparam))?
        {
            return Ok(Some(LRESULT(0)));
        }
        match msg {
            WM_KEYUP if wparam.0 as u16 == VK_SPACE.0 => {
                self.hold_a(false)?;
                Ok(None)
            }
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
                    self.on_click(p)?;
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
            Hot::Cell(i) => {
                if self.select_cell(i, key_down(VK_CONTROL.0), key_down(VK_SHIFT.0))? {
                    return Ok(());
                }
                self.go_to(i)
            }
            Hot::Caption(b) => self.caption_action(b),
            Hot::Menu(bg) => self.choose_background(bg),
            Hot::DialReset => self.set_angle(0.0),
            Hot::DialRuler => Ok(()),
            Hot::Info(b) => self.info_action(b),
            Hot::Home(crate::ui::home::HomeButton::Open) => self.open_dialog(),
            Hot::Home(crate::ui::home::HomeButton::Settings) => self.open_settings(),
        }
    }

    pub(super) fn run_action(&mut self, a: Action) -> Result<()> {
        let center = self.view_area().center();
        match a {
            Action::ZoomOut => self.zoom_by(1.0 / ZOOM_STEP, center),
            Action::ZoomIn => self.zoom_by(ZOOM_STEP, center),
            Action::ToggleZoom => self.toggle_zoom(center),
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
            Action::Crop => self.toggle_crop(),
            Action::Adjust => self.toggle_tone(),
            Action::Tags => self.edit_tags(),
            Action::Batch => self.batch_dialog(),
            Action::Grid => self.toggle_grid(),
            Action::Info => self.toggle_info(),
            Action::Histogram => self.toggle_histogram(),
            Action::Slideshow => self.toggle_slideshow(),
            Action::Compare => self.toggle_compare(),
        }
    }

    fn on_key(&mut self, vk: u16) -> Result<Option<LRESULT>> {
        let ch = |c: u8| u16::from(c);
        let (ctrl, shift) = (key_down(VK_CONTROL.0), key_down(VK_SHIFT.0));
        let alt = key_down(VK_MENU.0);
        // Keys whose meaning depends on what is open come first.
        match vk {
            // Space closes a QuickLook preview (as in Explorer).
            k if k == VK_SPACE.0 && self.quicklook => {
                self.close_window();
                return Ok(Some(LRESULT(0)));
            }
            // Comparing in one place: hold Space to see A.
            k if k == VK_SPACE.0 && self.hold_a(true)? => return Ok(Some(LRESULT(0))),
            k if k == VK_ESCAPE.0 => {
                self.on_escape()?;
                return Ok(Some(LRESULT(0)));
            }
            // ↑ / ↓ turn document pages.
            0x26 | 0x28 if !ctrl && self.step_page(if vk == 0x26 { -1 } else { 1 })? => {
                return Ok(Some(LRESULT(0)));
            }
            // Shift+1..4: one channel; Shift+0: all channels.
            k if shift && !ctrl && (ch(b'0')..=ch(b'4')).contains(&k) => {
                self.set_channel(u32::from(k - ch(b'0')))?;
                return Ok(Some(LRESULT(0)));
            }
            _ => {}
        }
        let key = Key {
            vk,
            ctrl,
            shift,
            alt,
        };
        if let Some(c) = self.keymap.get(key) {
            self.run_command(c)?;
            return Ok(Some(LRESULT(0)));
        }
        // F1..F9 not bound to anything else: quick folders (Ctrl: the other
        // action, Shift: assign).
        if (VK_F1.0..=VK_F9.0).contains(&vk) {
            let n = (vk - VK_F1.0 + 1) as u8;
            if shift {
                self.assign_folder(n)?;
            } else {
                self.quick_folder(n, ctrl)?;
            }
            return Ok(Some(LRESULT(0)));
        }
        Ok(None)
    }

    /// Esc closes what is open, from the top: selection, comparison,
    /// slideshow, menus and panels, then the window.
    fn on_escape(&mut self) -> Result<()> {
        if !self.selected.is_empty() {
            self.clear_selection()
        } else if self.compare.is_some() {
            self.toggle_compare()
        } else if self.slideshow {
            self.stop_slideshow();
            self.render()
        } else if self.menu_open {
            self.close_menu()
        } else if self.info_state.open {
            self.toggle_info()
        } else if self.dial_open {
            self.toggle_dial()
        } else {
            // SAFETY: posting to our own window.
            unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))? };
            Ok(())
        }
    }

    /// Runs a reassignable action (see `keymap`).
    pub(super) fn run_command(&mut self, c: Command) -> Result<()> {
        let center = self.view_area().center();
        let tilt = |a: f64, step: f64| ((a + step) * 10.0).round() / 10.0;
        match c {
            // In a document the same keys turn pages.
            Command::Prev if self.step_page(-1)? => Ok(()),
            Command::Next if self.step_page(1)? => Ok(()),
            Command::First if self.go_page(0)? => Ok(()),
            Command::Last if self.go_page(i64::from(u32::MAX))? => Ok(()),
            Command::Prev => self.step(-1),
            Command::Next => self.step(1),
            Command::First => self.go_to(0),
            Command::Last => self.go_to(self.files.len().saturating_sub(1)),
            Command::ZoomIn => self.zoom_by(ZOOM_STEP, center),
            Command::ZoomOut => self.zoom_by(1.0 / ZOOM_STEP, center),
            Command::Fit => self.zoom_fit(),
            Command::ActualSize => self.zoom_to(1.0, center),
            Command::ToggleZoom => self.toggle_zoom(center),
            Command::Fullscreen => self.go_fullscreen(),
            Command::ToggleMode => self.toggle_mode(),
            Command::RotateCw => self.rotate(Orientation::ROTATE_CW),
            Command::RotateCcw => self.rotate(Orientation::ROTATE_CCW),
            Command::FlipH => self.rotate(Orientation::FLIP_H),
            Command::FlipV => self.rotate(Orientation::FLIP_V),
            Command::TiltLeft => self.set_angle(tilt(self.angle, -1.0)),
            Command::TiltRight => self.set_angle(tilt(self.angle, 1.0)),
            Command::TiltLeftFine => self.set_angle(tilt(self.angle, -0.1)),
            Command::TiltRightFine => self.set_angle(tilt(self.angle, 0.1)),
            Command::Filmstrip => self.toggle_filmstrip(),
            Command::Checker => self.run_action(Action::Checker),
            Command::Smoothing => self.run_action(Action::Smoothing),
            Command::Grid => self.toggle_grid(),
            Command::Histogram => self.toggle_histogram(),
            Command::Info => self.toggle_info(),
            Command::Slideshow => self.toggle_slideshow(),
            Command::Compare => self.toggle_compare(),
            Command::CompareLayout => self.toggle_compare_layout(),
            Command::PlayPause => self.toggle_playback().map(drop),
            Command::FramePrev => self.step_frame(-1),
            Command::FrameNext => self.step_frame(1),
            Command::Editor => {
                self.open_in_editor();
                Ok(())
            }
            Command::Delete => self.delete_current(),
            Command::Save if !self.selected.is_empty() => self.save_selection(),
            Command::Save => self.save_current(),
            Command::SaveAs => self.save_as(),
            Command::Undo => self.undo_save(),
            Command::CopyImage => self.copy_image(),
            Command::CopyPath => self.copy_path(),
            Command::Rate0 => self.rate(0),
            Command::Rate1 => self.rate(1),
            Command::Rate2 => self.rate(2),
            Command::Rate3 => self.rate(3),
            Command::Rate4 => self.rate(4),
            Command::Rate5 => self.rate(5),
            Command::RatingFilter => self.cycle_rating_filter(),
            Command::Tags => self.edit_tags(),
            Command::Batch => self.batch_dialog(),
            Command::Crop => self.toggle_crop(),
            Command::Adjust => self.toggle_tone(),
        }
    }

    fn on_wheel(&mut self, delta: i32, p: (f64, f64)) -> Result<()> {
        // Over the dial the wheel turns the picture.
        if matches!(self.panel_hit(p), Some(Hot::DialRuler | Hot::DialReset)) {
            let step = if key_down(VK_SHIFT.0) { 0.1 } else { 1.0 };
            let a = self.angle + step * f64::from(delta.signum());
            return self.set_angle((a * 10.0).round() / 10.0);
        }
        // Documents: the wheel turns pages, Ctrl + wheel zooms.
        if self.is_doc() && !key_down(VK_CONTROL.0) && !self.over_panel(p) {
            self.step_page(if delta > 0 { -1 } else { 1 })?;
            return Ok(());
        }
        // Shift + wheel in the difference view: the gain.
        if key_down(VK_SHIFT.0) && self.step_diff_gain(delta > 0)? {
            return Ok(());
        }
        // Over the panel the wheel browses; elsewhere it zooms (Ctrl swaps).
        let zooms = self.settings.wheel == Wheel::Zoom;
        let browse = self.over_panel(p) || zooms == key_down(VK_CONTROL.0);
        if !browse {
            let notches = f64::from(delta) / f64::from(WHEEL_DELTA);
            if self.zoom_model(ZOOM_STEP.powf(notches))? {
                return Ok(());
            }
            let anchor = self.to_image_space(p);
            return self.zoom_by(ZOOM_STEP.powf(notches), anchor);
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
            // Dragging the photo itself out of the window: Ctrl + drag, or
            // any drag of a fitted photo in fullscreen (it cannot pan).
            let start = drag.start;
            let on_image = self.image_rect().is_some_and(|r| r.contains(start));
            let out = key_down(VK_CONTROL.0)
                || (self.mode == Mode::Fullscreen && self.fitted && on_image);
            if out && !self.files.is_empty() && self.shown.is_some() {
                self.drag = None;
                // SAFETY: releasing our own capture before the shell's loop.
                unsafe {
                    let _ = ReleaseCapture();
                }
                return self.drag_out();
            }
        }
        let Some(drag) = &mut self.drag else {
            return Ok(());
        };
        let (dx, dy) = (p.0 - drag.last.0, p.1 - drag.last.1);
        drag.last = p;
        // A model turns (or moves with Shift / Ctrl) instead of panning.
        if self.model_mode() {
            self.drag_model(dx, dy, key_down(VK_SHIFT.0) || key_down(VK_CONTROL.0))?;
            return Ok(());
        }
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
        let ip = self.to_image_space(p);
        if self.mode == Mode::Fullscreen
            && self.compare.is_none()
            && !self.model_mode()
            && !self.over_panel(p)
            && !self.image_rect().is_some_and(|r| r.contains(ip))
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
                let ip = self.to_image_space(p);
                if self.image_rect().is_some_and(|r| r.contains(ip)) {
                    self.toggle_zoom(ip)
                } else {
                    self.on_click(p)
                }
            }
            Mode::Windowed => self.go_fullscreen(),
        }
    }

    pub(super) fn zoom_by(&mut self, factor: f64, anchor: (f64, f64)) -> Result<()> {
        if self.zoom_model(factor)? {
            return Ok(());
        }
        let base = self.zoom_target();
        self.zoom_to_view(
            base.zoom_at(base.scale * factor, anchor, self.image_vp()),
            anchor,
        )
    }

    fn zoom_to(&mut self, scale: f64, anchor: (f64, f64)) -> Result<()> {
        let base = self.zoom_target();
        self.zoom_to_view(base.zoom_at(scale, anchor, self.image_vp()), anchor)
    }

    fn zoom_fit(&mut self) -> Result<()> {
        if self.reset_model()? {
            return Ok(());
        }
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
        if self.reset_model()? {
            return Ok(());
        }
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
