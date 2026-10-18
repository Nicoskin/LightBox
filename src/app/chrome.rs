//! Window caption buttons, the transparency-background pop-up menu and the
//! straightening dial (free rotation, view only).

use anyhow::Result;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{
    KillTimer, PostMessageW, SW_MINIMIZE, SetTimer, ShowWindow, WM_CLOSE,
};

use super::panel::Hot;
use super::{App, MENU_TIMER, Mode};
use crate::gfx::{CaptionButtonView, CaptionView, DialView, MenuItemView, MenuView};
use crate::settings::AlphaBg;
use crate::ui::caption::{self, CaptionButton, CaptionLayout};
use crate::ui::dial::{self, DialLayout};
use crate::ui::popup::{self, MenuLayout};
use crate::ui::toolbar::Action;
use crate::view::Rect;

/// Hovering the transparency button this long opens its menu.
const MENU_HOVER_MS: u32 = 450;

impl App {
    pub(super) fn caption_layout(&self) -> CaptionLayout {
        caption::layout(
            self.viewport(),
            self.ui_scale(),
            self.mode == Mode::Windowed,
        )
    }

    /// The caption is always there in a window; in fullscreen the close
    /// button comes and goes with the panel.
    fn caption_visible(&self) -> bool {
        self.mode == Mode::Windowed || self.panel_target > 0.5
    }

    pub(super) fn caption_view(&self) -> Option<CaptionView> {
        let windowed = self.mode == Mode::Windowed;
        let opacity = if windowed { 1.0 } else { self.panel_alpha };
        if opacity <= 0.0 {
            return None;
        }
        let l = self.caption_layout();
        let title = l.title.map(|r| {
            let name = self
                .current
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            (r, name)
        });
        Some(CaptionView {
            bar: l.bar,
            title,
            buttons: l
                .buttons
                .iter()
                .map(|(b, rect)| CaptionButtonView {
                    rect: *rect,
                    glyph: match b {
                        CaptionButton::Minimize => '\u{E921}',
                        CaptionButton::Fullscreen => '\u{E740}',
                        CaptionButton::Close => '\u{E8BB}',
                    },
                    close: *b == CaptionButton::Close,
                    hot: self.hot == Some(Hot::Caption(*b)),
                    pressed: self.pressed == Some(Hot::Caption(*b)),
                })
                .collect(),
            round: l.round,
            opacity,
        })
    }

    pub(super) fn caption_hit(&self, p: (f64, f64)) -> Option<CaptionButton> {
        if !self.caption_visible() {
            return None;
        }
        self.caption_layout().hit(p)
    }

    /// `true` over the windowed title bar (outside its buttons this drags
    /// the window).
    pub(super) fn over_title_bar(&self, p: (f64, f64)) -> bool {
        self.mode == Mode::Windowed && self.caption_layout().over_bar(p)
    }

    pub(super) fn caption_action(&mut self, b: CaptionButton) -> Result<()> {
        match b {
            CaptionButton::Minimize => {
                // SAFETY: own window.
                unsafe {
                    let _ = ShowWindow(self.hwnd, SW_MINIMIZE);
                }
                Ok(())
            }
            CaptionButton::Fullscreen => self.toggle_mode(),
            CaptionButton::Close => {
                // SAFETY: posting to our own window.
                unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))? };
                Ok(())
            }
        }
    }
}

// Transparency background menu.
impl App {
    /// The transparency toggle on the panel, if it is laid out.
    fn checker_button(&self) -> Option<Rect> {
        self.panel_layout()
            .buttons
            .iter()
            .find(|b| b.action == Action::Checker)
            .map(|b| b.rect)
    }

    fn menu_layout(&self) -> Option<(MenuLayout, Rect)> {
        if !self.menu_open {
            return None;
        }
        let anchor = self.checker_button()?;
        Some((
            popup::layout(anchor, self.viewport(), self.ui_scale()),
            anchor,
        ))
    }

    pub(super) fn menu_view(&self) -> Option<MenuView> {
        let (l, _) = self.menu_layout()?;
        Some(MenuView {
            rect: l.rect,
            items: l
                .items
                .iter()
                .map(|(bg, rect)| MenuItemView {
                    rect: *rect,
                    label: bg.label().to_string(),
                    swatch: bg.gray(),
                    selected: self.settings.checker && self.settings.alpha_bg == *bg,
                    hot: self.hot == Some(Hot::Menu(*bg)),
                })
                .collect(),
        })
    }

    pub(super) fn menu_hit(&self, p: (f64, f64)) -> Option<Option<AlphaBg>> {
        let (l, _) = self.menu_layout()?;
        l.rect.contains(p).then(|| l.hit(p))
    }

    pub(super) fn open_menu(&mut self) -> Result<()> {
        if self.checker_button().is_none() {
            return Ok(());
        }
        self.menu_open = true;
        self.show_panel();
        self.render()
    }

    pub(super) fn close_menu(&mut self) -> Result<()> {
        if self.menu_open {
            self.menu_open = false;
            self.render()?;
        }
        Ok(())
    }

    pub(super) fn choose_background(&mut self, bg: AlphaBg) -> Result<()> {
        self.settings.alpha_bg = bg;
        self.settings.checker = true;
        self.settings.save();
        self.close_menu()?;
        self.render()
    }

    /// Hover tracking: arms the menu timer over the transparency button and
    /// closes the menu once the pointer has left it.
    pub(super) fn menu_hover(&mut self, p: (f64, f64)) -> Result<()> {
        if self.hot == Some(Hot::Button(Action::Checker)) && !self.menu_open {
            // SAFETY: own window; re-arming resets the timer.
            unsafe { SetTimer(Some(self.hwnd), MENU_TIMER, MENU_HOVER_MS, None) };
        }
        if let Some((l, anchor)) = self.menu_layout()
            && !l.keeps_open(anchor, p, self.ui_scale())
        {
            self.close_menu()?;
        }
        Ok(())
    }

    pub(super) fn menu_timer(&mut self) -> Result<()> {
        // SAFETY: own window and timer.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), MENU_TIMER);
        }
        if self.hot == Some(Hot::Button(Action::Checker)) && !self.menu_open {
            self.open_menu()?;
        }
        Ok(())
    }
}

// Straightening dial.
impl App {
    fn dial_layout(&self) -> Option<DialLayout> {
        if !self.dial_open || self.panel_target < 0.5 {
            return None;
        }
        let bottom = self.viewport().1 - self.panel_height();
        Some(dial::layout(self.viewport(), self.ui_scale(), bottom))
    }

    pub(super) fn dial_view(&self) -> Option<DialView> {
        if !self.dial_open || self.panel_alpha <= 0.0 {
            return None;
        }
        let bottom = self.viewport().1 - self.panel_height();
        let l = dial::layout(self.viewport(), self.ui_scale(), bottom);
        Some(DialView {
            rect: l.rect,
            ruler: l.ruler,
            reset: l.reset,
            angle: self.angle,
            label: dial::label(self.angle),
            px_per_deg: dial::PX_PER_DEG * self.ui_scale(),
            reset_hot: self.hot == Some(Hot::DialReset),
            opacity: self.panel_alpha,
        })
    }

    pub(super) fn dial_hit(&self, p: (f64, f64)) -> Option<Hot> {
        let l = self.dial_layout()?;
        if l.reset.contains(p) {
            Some(Hot::DialReset)
        } else if l.rect.contains(p) {
            Some(Hot::DialRuler)
        } else {
            None
        }
    }

    pub(super) fn toggle_dial(&mut self) -> Result<()> {
        self.dial_open = !self.dial_open;
        self.show_panel();
        self.render()
    }

    /// Turns the picture to `deg` degrees (view only).
    pub(super) fn set_angle(&mut self, deg: f64) -> Result<()> {
        let deg = deg.clamp(-dial::MAX_DEG, dial::MAX_DEG);
        if deg == self.angle {
            return Ok(());
        }
        self.finish_anim()?;
        self.angle = deg;
        let turned = self.view.with_angle(deg);
        self.view = match self.fit_view() {
            Some(fit) if self.fitted => fit,
            _ => self.clamp(turned),
        };
        self.schedule();
        self.render()
    }

    /// Mouse drag on the ruler: `start` is where it began and the angle then.
    pub(super) fn drag_dial(&mut self, start: (f64, f64), x: f64, fine: bool) -> Result<()> {
        let a = dial::dragged(start.1, x - start.0, self.ui_scale(), fine);
        self.set_angle(a)
    }
}
