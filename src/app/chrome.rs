//! Window caption buttons, the pop-up menus of the panel and the
//! straightening dial (free rotation, view only).

use anyhow::Result;
use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, SW_MINIMIZE, ShowWindow, WM_CLOSE};

use super::panel::Hot;
use super::{App, Mode};
use crate::gfx::{CaptionButtonView, CaptionView, DialView, MenuItemView, MenuView, SegmentView};
use crate::i18n::tr;
use crate::keymap::Command;
use crate::settings::AlphaBg;
use crate::ui::caption::{self, CaptionButton, CaptionLayout, CaptionStyle};
use crate::ui::dial::{self, DialLayout};
use crate::ui::home::{self, HomeButton};
use crate::ui::popup::{self, MenuItem, MenuLayout};
use crate::ui::toolbar::Action;
use crate::view::Rect;
use crate::win::window;

impl App {
    pub(super) fn caption_layout(&self) -> CaptionLayout {
        let style = match (self.mode, self.quicklook, self.pinned) {
            (Mode::Fullscreen, ..) => CaptionStyle::Fullscreen,
            (_, _, true) => CaptionStyle::Pinned,
            (_, true, _) => CaptionStyle::Preview,
            _ => CaptionStyle::Window,
        };
        caption::layout(self.viewport(), self.ui_scale(), style)
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
                .unwrap_or_else(|| crate::APP_NAME.to_string());
            // The colour readout follows the name while hovering the picture.
            let name = match &self.probe {
                Some(p) => format!("{name}      {}", p.text()),
                None => name,
            };
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
                        CaptionButton::Pin => '\u{E718}',
                        CaptionButton::OnTop if self.on_top => '\u{E841}',
                        CaptionButton::OnTop => '\u{E718}',
                        CaptionButton::OpenFull => '\u{E8A7}',
                    },
                    close: *b == CaptionButton::Close,
                    hot: self.hot == Some(Hot::Caption(*b)),
                    pressed: self.pressed == Some(Hot::Caption(*b)),
                    active: *b == CaptionButton::OnTop && self.on_top,
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
            CaptionButton::Pin => self.pin_preview(),
            CaptionButton::OnTop => {
                self.on_top = !self.on_top;
                window::set_topmost(self.hwnd, self.on_top && self.mode == Mode::Windowed);
                self.render()
            }
            CaptionButton::OpenFull => self.open_full(),
            CaptionButton::Close => {
                // SAFETY: posting to our own window.
                unsafe { PostMessageW(Some(self.hwnd), WM_CLOSE, WPARAM(0), LPARAM(0))? };
                Ok(())
            }
        }
    }
}

// Pop-up menus: editing and "more".
impl App {
    /// The panel button a menu opens from, if it is laid out.
    fn menu_button(&self, a: Action) -> Option<Rect> {
        self.panel_layout()
            .buttons
            .iter()
            .find(|b| b.action == a)
            .map(|b| b.rect)
    }

    fn menu_layout(&self) -> Option<(MenuLayout, Rect)> {
        let a = self.menu?;
        let anchor = self.menu_button(a)?;
        Some((
            popup::layout(anchor, self.viewport(), self.ui_scale(), &popup::items(a)),
            anchor,
        ))
    }

    /// The command behind a menu item: its name and its first key.
    fn menu_command(a: Action) -> Option<Command> {
        Some(match a {
            Action::Crop => Command::Crop,
            Action::Adjust => Command::Adjust,
            Action::FlipH => Command::FlipH,
            Action::FlipV => Command::FlipV,
            Action::SaveAs => Command::SaveAs,
            Action::Grid => Command::Grid,
            Action::Histogram => Command::Histogram,
            Action::Compare => Command::Compare,
            Action::Slideshow => Command::Slideshow,
            Action::Checker => Command::Checker,
            _ => return None,
        })
    }

    pub(super) fn menu_view(&self) -> Option<MenuView> {
        let (l, _) = self.menu_layout()?;
        let item_view = |item: MenuItem, rect: Rect| {
            let hot = self.hot == Some(Hot::Menu(item));
            let base = MenuItemView {
                rect,
                label: String::new(),
                hint: String::new(),
                swatch: None,
                separator: false,
                selected: false,
                hot,
            };
            match item {
                MenuItem::Separator => MenuItemView {
                    separator: true,
                    hot: false,
                    ..base
                },
                MenuItem::Background(bg) => MenuItemView {
                    label: bg.label().to_string(),
                    swatch: Some(bg.gray()),
                    selected: self.settings.checker && self.settings.alpha_bg == bg,
                    ..base
                },
                MenuItem::Do(a) => {
                    let (label, hint) = match Self::menu_command(a) {
                        Some(c) => (
                            c.label().to_string(),
                            self.keymap
                                .keys(c)
                                .first()
                                .map(|k| k.label())
                                .unwrap_or_default(),
                        ),
                        None => (tr("Повернуть на любой угол").to_string(), "[ ]".into()),
                    };
                    MenuItemView {
                        label,
                        hint,
                        selected: match a {
                            Action::Crop => self.crop.is_some(),
                            Action::Adjust => self.tone.is_some(),
                            Action::Straighten => self.dial_open,
                            Action::Grid => self.settings.pixel_grid,
                            Action::Histogram => self.hist_state.open,
                            Action::Compare => self.compare.is_some(),
                            Action::Slideshow => self.slideshow,
                            Action::Checker => self.settings.checker,
                            _ => false,
                        },
                        ..base
                    }
                }
            }
        };
        Some(MenuView {
            rect: l.rect,
            items: l
                .items
                .iter()
                .map(|(item, rect)| item_view(*item, *rect))
                .collect(),
        })
    }

    /// `true` over the button of the open menu (its click closes it).
    pub(super) fn over_menu_button(&self, p: (f64, f64)) -> bool {
        self.menu
            .and_then(|a| self.menu_button(a))
            .is_some_and(|r| r.contains(p))
    }

    pub(super) fn menu_hit(&self, p: (f64, f64)) -> Option<Option<MenuItem>> {
        let (l, _) = self.menu_layout()?;
        l.rect.contains(p).then(|| l.hit(p))
    }

    /// A click on a menu button: opens its menu, or closes it.
    pub(super) fn toggle_menu(&mut self, a: Action) -> Result<()> {
        if self.menu == Some(a) || self.menu_button(a).is_none() {
            return self.close_menu();
        }
        self.menu = Some(a);
        self.show_panel();
        self.render()
    }

    pub(super) fn close_menu(&mut self) -> Result<()> {
        if self.menu.take().is_some() {
            self.render()?;
        }
        Ok(())
    }

    /// A menu item was clicked: the menu closes and the item runs.
    pub(super) fn menu_item(&mut self, item: MenuItem) -> Result<()> {
        self.close_menu()?;
        match item {
            MenuItem::Do(a) => self.run_action(a),
            MenuItem::Background(bg) => self.choose_background(bg),
            MenuItem::Separator => Ok(()),
        }
    }

    fn choose_background(&mut self, bg: AlphaBg) -> Result<()> {
        self.settings.alpha_bg = bg;
        self.settings.checker = true;
        self.settings.save();
        self.render()
    }

    /// Hover tracking: closes the menu once the pointer has left it.
    pub(super) fn menu_hover(&mut self, p: (f64, f64)) -> Result<()> {
        if let Some((l, anchor)) = self.menu_layout()
            && !l.keeps_open(anchor, p, self.ui_scale())
        {
            self.close_menu()?;
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

// Start screen (no file).
impl App {
    pub(super) fn home_view(&self) -> Vec<SegmentView> {
        if !self.home() || self.card_open {
            return Vec::new();
        }
        home::layout(self.viewport(), self.ui_scale())
            .into_iter()
            .map(|(b, rect)| SegmentView {
                rect,
                text: b.label().to_string(),
                selected: b == HomeButton::Open,
                hot: self.hot == Some(Hot::Home(b)),
                first: true,
                last: true,
            })
            .collect()
    }
}
