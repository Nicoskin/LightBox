//! Tooltips: resting the pointer on a control shows what it does and its
//! keyboard shortcut.

use anyhow::Result;
use windows::Win32::UI::WindowsAndMessaging::{KillTimer, SetTimer};

use super::panel::Hot;
use super::{App, Mode, TOOLTIP_TIMER};
use crate::i18n::tr;
use crate::ui::caption::CaptionButton;
use crate::ui::info::InfoButton;
use crate::ui::toolbar::{self, Action};
use crate::view::Rect;

/// How long the pointer rests before the tooltip appears.
const DELAY_MS: u32 = 550;

impl App {
    /// The control under the pointer changed: hide the tooltip and start
    /// waiting for the next one.
    pub(super) fn arm_tooltip(&mut self) {
        self.tooltip = None;
        if self.hot.is_some() {
            // SAFETY: own window; re-arming a timer id resets it.
            unsafe { SetTimer(Some(self.hwnd), TOOLTIP_TIMER, DELAY_MS, None) };
        }
    }

    pub(super) fn tooltip_timer(&mut self) -> Result<()> {
        // SAFETY: own window and timer.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TOOLTIP_TIMER);
        }
        if self.hot.is_some() && self.pressed.is_none() && !self.menu_open {
            self.tooltip = self.hot;
            self.render()?;
        }
        Ok(())
    }

    /// The tooltip to draw: the control's rectangle and the text.
    pub(super) fn tooltip_view(&self) -> Option<(Rect, String)> {
        let h = self.tooltip.filter(|t| Some(*t) == self.hot)?;
        Some((self.hot_rect(h)?, self.tooltip_text(h)?))
    }

    fn hot_rect(&self, h: Hot) -> Option<Rect> {
        match h {
            Hot::Button(a) => self
                .panel_layout()
                .buttons
                .iter()
                .find(|b| b.action == a)
                .map(|b| b.rect),
            Hot::Cell(i) => {
                let strip = self.panel_layout().strip?;
                toolbar::strip_cells(strip, self.ui_scale(), self.files.len(), self.strip_pos)
                    .into_iter()
                    .find(|(k, _)| *k == i)
                    .map(|(_, r)| r)
            }
            Hot::Caption(b) => self
                .caption_layout()
                .buttons
                .iter()
                .find(|(c, _)| *c == b)
                .map(|(_, r)| *r),
            Hot::DialReset => self.dial_view().map(|d| d.reset),
            Hot::DialRuler | Hot::Menu(_) | Hot::Info(_) | Hot::Home(_) => None,
        }
    }

    fn tooltip_text(&self, h: Hot) -> Option<String> {
        let text = match h {
            Hot::Button(a) => match a {
                Action::ZoomOut => tr("Уменьшить (−)"),
                Action::ZoomIn => tr("Увеличить (+)"),
                Action::ToggleZoom => tr("Вписать ↔ 100 % (щелчок; клавиши 0, 1, Z)"),
                Action::Prev => tr("Предыдущее фото (←)"),
                Action::Next => tr("Следующее фото (→)"),
                Action::RotateCcw => tr("Повернуть влево (L)"),
                Action::RotateCw => tr("Повернуть вправо (R)"),
                Action::Checker => tr("Фон прозрачности (T) · правый клик — выбор фона"),
                Action::Smoothing => tr("Сглаживание (N)"),
                Action::Grid => tr("Сетка пикселей при увеличении от 800 % (G)"),
                Action::Filmstrip => tr("Лента миниатюр (F)"),
                Action::ToggleMode => match self.mode {
                    Mode::Fullscreen => tr("В окне (F11)"),
                    Mode::Windowed => tr("Во весь экран (F11)"),
                },
                Action::FlipH => tr("Отразить по горизонтали (M)"),
                Action::FlipV => tr("Отразить по вертикали (Shift+M)"),
                Action::Straighten => tr("Повернуть на любой угол ( [ и ] )"),
                Action::Crop => tr("Обрезать (X)"),
                Action::Adjust => tr("Коррекция: экспозиция, контраст, цвет (A)"),
                Action::Wireframe => tr("Каркас: рёбра треугольников (W)"),
                Action::Section => tr("Разрез по высоте (P) · ↑ и ↓ двигают"),
                Action::Measure => tr("Линейка: две точки на модели (D)"),
                Action::LayFlat => tr("Положить гранью на стол: щёлкните грань (B)"),
                Action::Tags => tr("Метки (Ctrl+K)"),
                Action::Batch => tr("Переименовать или конвертировать выбранные (Ctrl+B)"),
                Action::Info => tr("Информация о файле (I)"),
                Action::Histogram => tr("Гистограмма (H)"),
                Action::Slideshow => tr("Слайд-шоу (S, F5)"),
                Action::Compare => tr("Сравнить с соседним фото (C) · V — вид"),
                Action::Save => tr("Сохранить поворот в файл (Ctrl+S)"),
                Action::SaveAs => tr("Сохранить как… (Ctrl+Shift+S)"),
                Action::Reset => tr("Отменить поворот"),
                Action::Settings => tr("Настройки"),
            }
            .to_string(),
            Hot::Cell(i) => self
                .files
                .get(i)?
                .file_name()?
                .to_string_lossy()
                .into_owned(),
            Hot::Caption(b) => match b {
                CaptionButton::Minimize => tr("Свернуть"),
                CaptionButton::Fullscreen => tr("Во весь экран (F11)"),
                CaptionButton::Close => tr("Закрыть (Esc)"),
                CaptionButton::Pin => tr("Закрепить поверх всех окон (в отдельном окне)"),
                CaptionButton::OnTop if self.on_top => tr("Открепить: обычное окно"),
                CaptionButton::OnTop => tr("Закрепить поверх всех окон"),
                CaptionButton::OpenFull => tr("Открыть в Lightbox (с папкой)"),
            }
            .to_string(),
            Hot::DialReset => tr("Сбросить угол (двойной клик по шкале)").to_string(),
            Hot::DialRuler | Hot::Menu(_) | Hot::Home(_) => return None,
            Hot::Info(b) => match b {
                InfoButton::Prompt => tr("Скопировать промпт"),
                InfoButton::Workflow => tr("Скопировать workflow (JSON)"),
                InfoButton::All => tr("Скопировать все параметры"),
                InfoButton::Map => tr("Место съёмки на OpenStreetMap"),
            }
            .to_string(),
        };
        Some(text)
    }
}
