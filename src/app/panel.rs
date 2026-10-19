//! The bottom panel and filmstrip: visibility, what they show, rotation.

use std::time::Instant;

use anyhow::Result;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::{App, Mode, PANEL_HOLD_MS, PANEL_TIMER, WM_APP_THUMB, post};
use crate::cache;
use crate::gfx::Icon;
use crate::gfx::{ButtonView, CardView, CellView, PanelView, SegmentView, TextStyle, TextView};
use crate::keymap::{self, Command, Key, Keymap};
use crate::orientation::Orientation;
use crate::settings::AlphaBg;
use crate::ui::caption::CaptionButton;
use crate::ui::info::InfoButton;
use crate::ui::settings_card;
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
    /// Title bar / corner button.
    Caption(CaptionButton),
    /// Item of the transparency-background menu.
    Menu(AlphaBg),
    /// The straightening dial's ruler and its "0°" button.
    DialRuler,
    DialReset,
    /// Button of the info panel.
    Info(InfoButton),
    /// Button of the start screen.
    Home(crate::ui::home::HomeButton),
}

/// Thumbnails requested beyond the visible cells, each side.
const THUMB_LOOKAHEAD: usize = 6;

impl App {
    pub(super) fn panel_layout(&self) -> Layout {
        let windowed = self.mode == Mode::Windowed;
        toolbar::layout(
            self.viewport(),
            self.ui_scale(),
            windowed,
            self.strip_height(),
            windowed,
            !self.preview(),
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
        if self.pressed.is_some() || self.menu_open || (inside && self.over_panel(p)) {
            self.show_panel();
        } else {
            self.hide_panel();
        }
    }

    /// Height of the panel (bar + strip) from the bottom edge, pixels.
    pub(super) fn panel_height(&self) -> f64 {
        let l = self.panel_layout();
        self.viewport().1 - l.strip.map_or(l.bar.y0, |s| s.y0)
    }

    /// `true` if `p` is over any visible control: the panel or strip, a
    /// caption button, the dial or the pop-up menu.
    pub(super) fn over_panel(&self, p: (f64, f64)) -> bool {
        self.menu_hit(p).is_some()
            || self.caption_hit(p).is_some()
            || self.dial_hit(p).is_some()
            || self.info_hit(p).is_some()
            || (self.home() && crate::ui::home::hit(self.viewport(), self.ui_scale(), p).is_some())
            || (self.panel_target > 0.5 && self.panel_layout().contains(p))
    }

    /// Control (button, filmstrip cell, caption button, menu item, dial)
    /// under `p`.
    pub(super) fn panel_hit(&self, p: (f64, f64)) -> Option<Hot> {
        if let Some(item) = self.menu_hit(p) {
            return item.map(Hot::Menu);
        }
        if let Some(b) = self.caption_hit(p) {
            return Some(Hot::Caption(b));
        }
        if self.home() {
            return crate::ui::home::hit(self.viewport(), self.ui_scale(), p).map(Hot::Home);
        }
        if let Some(h) = self.dial_hit(p) {
            return Some(h);
        }
        if let Some(b) = self.info_hit(p) {
            return b.map(Hot::Info);
        }
        if self.panel_target < 0.5 {
            return None;
        }
        let l = self.panel_layout();
        if let Some(a) = l.hit(p) {
            return Some(Hot::Button(a));
        }
        let strip = l.strip.filter(|s| s.contains(p))?;
        toolbar::strip_cells(strip, self.ui_scale(), self.files.len(), self.strip_pos)
            .into_iter()
            .find(|(_, r)| r.contains(p))
            .map(|(i, _)| Hot::Cell(i))
    }

    pub(super) fn set_hot(&mut self, hot: Option<Hot>) -> Result<()> {
        if hot != self.hot {
            self.hot = hot;
            self.arm_tooltip();
            self.render()?;
        }
        Ok(())
    }

    /// Builds what the overlay draws; `None` when the panel is hidden.
    pub(super) fn panel_view(&self) -> Option<PanelView> {
        if self.panel_alpha <= 0.0 || self.home() {
            return None;
        }
        let l = self.panel_layout();
        let buttons = l
            .buttons
            .iter()
            .map(|b| ButtonView {
                rect: b.rect,
                // The transparency button shows the chosen background.
                icon: match (b.action, self.settings.alpha_bg.gray()) {
                    (Action::Checker, Some(g)) => Icon::Swatch(g),
                    _ => b.icon,
                },
                round: b.round,
                hot: self.hot == Some(Hot::Button(b.action)),
                pressed: self.pressed == Some(Hot::Button(b.action)),
                active: match b.action {
                    Action::Checker => self.settings.checker,
                    Action::Smoothing => self.settings.smoothing,
                    Action::Filmstrip => self.settings.filmstrip,
                    Action::Straighten => self.dial_open,
                    Action::Grid => self.settings.pixel_grid,
                    Action::Info => self.info_state.open,
                    Action::Histogram => self.hist_state.open,
                    Action::Slideshow => self.slideshow,
                    Action::Compare => self.compare.is_some(),
                    _ => false,
                },
                enabled: match b.action {
                    Action::Save => self.dirty(),
                    Action::Reset => self.edits.contains_key(&cache::key(&self.current)),
                    _ => true,
                },
            })
            .collect();

        let mut texts = Vec::new();
        let mut swatch = None;
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
            // The pixel under the cursor replaces the details while hovering.
            let detail = Rect { y0: mid + 2.0, ..r };
            match &self.probe {
                Some(pr) => {
                    let sw = 11.0 * self.ui_scale();
                    let cy = (detail.y0 + detail.y1) / 2.0 - 2.0;
                    swatch = Some((
                        Rect {
                            x0: detail.x0,
                            y0: (cy - sw / 2.0).round(),
                            x1: detail.x0 + sw,
                            y1: (cy + sw / 2.0).round(),
                        },
                        pr.rgba,
                    ));
                    texts.push(TextView {
                        rect: Rect {
                            x0: detail.x0 + sw + 6.0 * self.ui_scale(),
                            ..detail
                        },
                        text: pr.text(),
                        style: TextStyle::Detail,
                    });
                }
                None => texts.push(TextView {
                    rect: detail,
                    text: self.details(),
                    style: TextStyle::Detail,
                }),
            }
        }

        let cells = l.strip.map_or_else(Vec::new, |strip| {
            toolbar::strip_cells(strip, self.ui_scale(), self.files.len(), self.strip_pos)
                .into_iter()
                .map(|(i, rect)| CellView {
                    rect,
                    thumb: self.thumbs.as_ref().and_then(|t| t.get(&self.files[i])),
                    current: i == self.index,
                    selected: self.selected.contains(&cache::key(&self.files[i])),
                    turn: self
                        .edits
                        .get(&cache::key(&self.files[i]))
                        .copied()
                        .unwrap_or(Orientation::IDENTITY),
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
            swatch,
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
        if let Some(f) = self.frame_label() {
            parts.push(f);
        }
        if let Some(m) = self.model_details() {
            parts.push(m);
        }
        parts.join("  ·  ")
    }

    /// Rotates / mirrors the current photo on screen (saved to the file in M2).
    pub(super) fn rotate(&mut self, op: Orientation) -> Result<()> {
        let turn = match op {
            Orientation::ROTATE_CW => 45.0,
            Orientation::ROTATE_CCW => -45.0,
            _ => 0.0,
        };
        if self.model_mode() {
            self.turn_model(turn)?;
            return Ok(());
        }
        if !self.selected.is_empty() {
            return self.rotate_selection(op);
        }
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
        if let Some(v) = self.fit_view() {
            self.view = v;
            self.fitted = true;
        }
        self.render()
    }

    pub(super) fn toggle_filmstrip(&mut self) -> Result<()> {
        self.settings.filmstrip = !self.settings.filmstrip;
        self.settings.save();
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
        if !self.settings.filmstrip || self.mode == Mode::Windowed || self.files.is_empty() {
            return;
        }
        let Some(strip) = self.panel_layout().strip else {
            return;
        };
        let visible =
            toolbar::strip_cells(strip, self.ui_scale(), self.files.len(), self.index as f64).len();
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
        if got && self.panel_alpha > 0.0 && self.settings.filmstrip {
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

impl App {
    fn card_layout(&self) -> settings_card::CardLayout {
        settings_card::layout(
            self.viewport(),
            self.ui_scale(),
            self.card_tab,
            self.card_scroll,
        )
    }

    pub(super) fn card_view(&self) -> Option<CardView> {
        if !self.card_open {
            return None;
        }
        let l = self.card_layout();
        let mut labels = Vec::new();
        let mut segments = Vec::new();
        for row in &l.rows {
            labels.push((row.label, row.text.to_string()));
            let n = row.choices.len();
            for (i, (c, text, rect)) in row.choices.iter().enumerate() {
                segments.push(SegmentView {
                    rect: *rect,
                    text: (*text).to_string(),
                    selected: settings_card::is_selected(&self.settings, *c),
                    hot: self.card_hot == Some(settings_card::Hit::Choice(*c)),
                    first: i == 0,
                    last: i + 1 == n,
                });
            }
        }
        // Keys page: the action names and a chip per key slot.
        for r in &l.keys {
            labels.push((r.label, r.cmd.label().to_string()));
            let keys = self.keymap.keys(r.cmd);
            for (k, rect) in r.chips.iter().enumerate() {
                let capturing = self.card_capture == Some((r.cmd, k));
                let text = if capturing {
                    "Нажмите…".to_string()
                } else {
                    keys.get(k).map_or_else(String::new, |key| key.label())
                };
                segments.push(SegmentView {
                    rect: *rect,
                    text,
                    selected: capturing,
                    hot: self.card_hot == Some(settings_card::Hit::KeySlot(r.cmd, k)),
                    first: true,
                    last: true,
                });
            }
        }
        let keys_page = self.card_tab == settings_card::Tab::Keys;
        let footer = if keys_page {
            "Щелчок — назначить клавишу, правый щелчок — убрать".to_string()
        } else {
            settings_card::version_text()
        };
        Some(CardView {
            rect: l.card,
            title: (l.title, "Настройки".to_string()),
            close: (l.close, self.card_hot == Some(settings_card::Hit::Close)),
            tabs: l
                .tabs
                .iter()
                .map(|(t, rect)| SegmentView {
                    rect: *rect,
                    text: t.label().to_string(),
                    selected: *t == self.card_tab,
                    hot: self.card_hot == Some(settings_card::Hit::Tab(*t)),
                    first: false,
                    last: false,
                })
                .collect(),
            labels,
            footer: (l.footer, footer),
            segments,
            clip: keys_page.then_some(l.list),
            button: l.reset.map(|r| {
                (
                    r,
                    "Сбросить все".to_string(),
                    self.card_hot == Some(settings_card::Hit::ResetKeys),
                )
            }),
        })
    }

    pub(super) fn open_settings(&mut self) -> Result<()> {
        self.card_open = true;
        self.card_hot = None;
        self.card_capture = None;
        self.render()
    }

    pub(super) fn close_settings(&mut self) -> Result<()> {
        self.card_open = false;
        self.card_capture = None;
        self.render()
    }

    /// Input while the card is open: it takes all mouse and keyboard input.
    pub(super) fn card_input(&mut self, msg: u32, wparam: usize, p: (f64, f64)) -> Result<bool> {
        use settings_card::Hit;
        use windows::Win32::UI::Input::KeyboardAndMouse::{
            GetKeyState, VK_CONTROL, VK_ESCAPE, VK_LWIN, VK_MENU, VK_RWIN, VK_SHIFT,
        };
        use windows::Win32::UI::WindowsAndMessaging::{
            WM_KEYDOWN, WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
            WM_MOUSEWHEEL, WM_RBUTTONDOWN, WM_RBUTTONUP, WM_SYSKEYDOWN,
        };
        let l = self.card_layout();
        let down = |vk: u16| {
            // SAFETY: no preconditions.
            unsafe { GetKeyState(i32::from(vk)) < 0 }
        };
        match msg {
            // Waiting for the new key of a shortcut.
            WM_KEYDOWN | WM_SYSKEYDOWN if self.card_capture.is_some() => {
                let vk = wparam as u16;
                let modifier = [VK_SHIFT, VK_CONTROL, VK_MENU, VK_LWIN, VK_RWIN]
                    .iter()
                    .any(|m| m.0 == vk);
                if modifier {
                    return Ok(true);
                }
                if let Some((cmd, slot)) = self.card_capture.take()
                    && vk != VK_ESCAPE.0
                {
                    let key = Key {
                        vk,
                        ctrl: down(VK_CONTROL.0),
                        shift: down(VK_SHIFT.0),
                        alt: down(VK_MENU.0),
                    };
                    self.set_shortcut(cmd, slot, Some(key));
                }
                self.render()?;
            }
            WM_MOUSEMOVE => {
                let hot = l.hit(p).filter(|h| *h != Hit::Card);
                if hot != self.card_hot {
                    self.card_hot = hot;
                    self.render()?;
                }
            }
            WM_MOUSEWHEEL if self.card_tab == settings_card::Tab::Keys => {
                let delta = f64::from((wparam >> 16) as u16 as i16);
                let step = 3.0 * 30.0 * self.ui_scale();
                self.card_scroll =
                    (self.card_scroll - delta / 120.0 * step).clamp(0.0, l.max_scroll);
                self.card_hot = None;
                self.render()?;
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK | WM_MOUSEWHEEL | WM_RBUTTONDOWN => {}
            WM_RBUTTONUP => {
                if let Some(Hit::KeySlot(cmd, slot)) = l.hit(p) {
                    self.card_capture = None;
                    self.set_shortcut(cmd, slot, None);
                    self.render()?;
                }
            }
            WM_LBUTTONUP => match l.hit(p) {
                None | Some(Hit::Close) => self.close_settings()?,
                Some(Hit::Tab(t)) => {
                    self.card_tab = t;
                    self.card_scroll = 0.0;
                    self.card_capture = None;
                    self.render()?;
                }
                Some(Hit::Choice(c)) => self.choose(c)?,
                Some(Hit::KeySlot(cmd, slot)) => {
                    self.card_capture = Some((cmd, slot));
                    self.render()?;
                }
                Some(Hit::ResetKeys) => {
                    self.settings.keys.clear();
                    self.settings.save();
                    self.keymap = Keymap::new(&self.settings.keys);
                    self.card_capture = None;
                    self.notice("Клавиши — как по умолчанию");
                    self.render()?;
                }
                Some(Hit::Card) => {}
            },
            WM_KEYDOWN if wparam == usize::from(VK_ESCAPE.0) => self.close_settings()?,
            WM_KEYDOWN | WM_SYSKEYDOWN => {}
            _ => return Ok(false),
        }
        Ok(true)
    }

    /// Reassigns (or clears) one key slot of an action and saves; a key
    /// taken from another action is reported.
    fn set_shortcut(&mut self, cmd: Command, slot: usize, key: Option<Key>) {
        let taken = key.and_then(|k| {
            Command::ALL
                .into_iter()
                .find(|c| *c != cmd && self.keymap.keys(*c).contains(&k))
        });
        keymap::assign(&mut self.settings.keys, cmd, slot, key);
        self.settings.save();
        self.keymap = Keymap::new(&self.settings.keys);
        if let (Some(k), Some(other)) = (key, taken) {
            self.notice(&format!("{} снята с «{}»", k.label(), other.label()));
        }
    }

    /// Applies a choice from the card, with its immediate effects.
    fn choose(&mut self, c: settings_card::Choice) -> Result<()> {
        use settings_card::Choice;
        settings_card::apply(&mut self.settings, c);
        self.settings.save();
        match c {
            Choice::Thumbs(_) | Choice::Filmstrip(_) => {
                if self.fitted
                    && let Some(v) = self.fit_view()
                {
                    self.view = v;
                }
                self.request_thumbs();
            }
            Choice::Dim(_) if self.mode == Mode::Fullscreen && self.anim.is_none() => {
                self.dim = self.settings.dim();
            }
            // Starting with Windows only makes sense in the tray, and the
            // tray without it is still useful: they go off together only.
            Choice::Resident(on) => {
                if !on && self.settings.autostart {
                    self.settings.autostart = false;
                    self.set_autostart(false);
                    self.settings.save();
                }
                self.apply_resident();
            }
            Choice::CheckUpdates => self.check_updates(true),
            Choice::Cache(mb) => self.cache.set_budget(mb as usize * (1 << 20)),
            Choice::Preload(_) => self.schedule(),
            Choice::Autostart(on) => {
                self.set_autostart(on);
                if on && !self.settings.resident {
                    self.settings.resident = true;
                    self.settings.save();
                    self.apply_resident();
                }
            }
            // Space in Explorer needs the viewer waiting in the tray, also
            // after a restart: turning it on turns those on too.
            Choice::QuickLook(on) => {
                if on {
                    self.settings.resident = true;
                    if !self.settings.autostart {
                        self.settings.autostart = true;
                        self.set_autostart(true);
                    }
                    self.settings.save();
                    self.apply_resident();
                    self.notice("Пробел в проводнике работает, пока Lightbox в трее");
                } else {
                    self.apply_quick_look();
                }
            }
            Choice::Editor(true) => {
                let start = self
                    .settings
                    .editor
                    .as_deref()
                    .and_then(std::path::Path::parent);
                if let Some(exe) = crate::win::dialogs::open(
                    Some(self.hwnd),
                    "Программа для редактирования",
                    false,
                    Some(("Программы", "*.exe")),
                    start,
                ) {
                    self.settings.editor = Some(exe);
                    self.settings.save();
                }
            }
            _ => {}
        }
        self.render()
    }
}
