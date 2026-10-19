//! The settings card: pages (tabs) of rows of segmented choices drawn over
//! the viewer, with the version in the footer. Pure layout / hit-testing /
//! applying; drawing is in the overlay.

use crate::settings::{ImperfectJpeg, OpenMode, SaveMode, Settings, ThumbSize, Wheel};
use crate::view::Rect;

/// One selectable value.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Choice {
    Open(OpenMode),
    Thumbs(ThumbSize),
    Filmstrip(bool),
    Wheel(Wheel),
    Save(SaveMode),
    KeepDates(bool),
    Imperfect(Option<ImperfectJpeg>),
    Animations(bool),
    TurnAnimation(bool),
    Dim(u8),
    Slideshow(u8),
    Resident(bool),
    QuickLook(bool),
    Autostart(bool),
    AutoUpdate(bool),
    /// Action: check for updates now (never shown as selected).
    CheckUpdates,
    ConfirmDelete(bool),
    /// `true`: choose an editor (the app opens a dialog); `false`: system.
    Editor(bool),
}

/// Settings pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    View,
    Interface,
    Files,
    System,
}

impl Tab {
    pub const ALL: [Tab; 4] = [Tab::View, Tab::Interface, Tab::Files, Tab::System];

    pub fn label(self) -> &'static str {
        match self {
            Tab::View => "Просмотр",
            Tab::Interface => "Интерфейс",
            Tab::Files => "Файлы",
            Tab::System => "Система",
        }
    }
}

pub struct Row {
    pub label: &'static str,
    pub choices: Vec<(Choice, &'static str)>,
}

fn row(label: &'static str, choices: Vec<(Choice, &'static str)>) -> Row {
    Row { label, choices }
}

/// The rows of one page.
pub fn rows(tab: Tab) -> Vec<Row> {
    use Choice::*;
    match tab {
        Tab::View => vec![
            row(
                "Открывать фото",
                vec![
                    (Open(OpenMode::Fullscreen), "Во весь экран"),
                    (Open(OpenMode::Windowed), "В окне"),
                ],
            ),
            row(
                "Колесо мыши",
                vec![
                    (Wheel(crate::settings::Wheel::Zoom), "Масштаб"),
                    (Wheel(crate::settings::Wheel::Browse), "Листание"),
                ],
            ),
            row(
                "Затемнение фона",
                vec![(Dim(60), "60 %"), (Dim(80), "80 %"), (Dim(92), "92 %")],
            ),
            row(
                "Слайд-шоу (S, F5)",
                vec![
                    (Slideshow(2), "2 с"),
                    (Slideshow(4), "4 с"),
                    (Slideshow(8), "8 с"),
                ],
            ),
        ],
        Tab::Interface => vec![
            row(
                "Лента миниатюр",
                vec![
                    (Filmstrip(true), "Показывать"),
                    (Filmstrip(false), "Скрыть"),
                ],
            ),
            row(
                "Размер миниатюр",
                vec![
                    (Thumbs(ThumbSize::Tiny), "Крошечные"),
                    (Thumbs(ThumbSize::Small), "Мелкие"),
                    (Thumbs(ThumbSize::Medium), "Средние"),
                    (Thumbs(ThumbSize::Large), "Крупные"),
                ],
            ),
            row(
                "Анимации интерфейса",
                vec![(Animations(true), "Вкл"), (Animations(false), "Выкл")],
            ),
            row(
                "Смена фото",
                vec![
                    (TurnAnimation(true), "Плавно"),
                    (TurnAnimation(false), "Сразу"),
                ],
            ),
        ],
        Tab::Files => vec![
            row(
                "Сохранять поворот",
                vec![
                    (Save(SaveMode::Auto), "Сразу"),
                    (Save(SaveMode::Ask), "Спрашивать"),
                    (Save(SaveMode::Manual), "Вручную"),
                ],
            ),
            row(
                "Дата изменения файла",
                vec![
                    (KeepDates(true), "Сохранять"),
                    (KeepDates(false), "Обновлять"),
                ],
            ),
            row(
                "JPEG не кратный 16 px",
                vec![
                    (Imperfect(None), "Спрашивать"),
                    (Imperfect(Some(ImperfectJpeg::Trim)), "Обрезать"),
                    (Imperfect(Some(ImperfectJpeg::ExifOnly)), "Только тег"),
                ],
            ),
            row(
                "Подтверждать удаление",
                vec![(ConfirmDelete(true), "Да"), (ConfirmDelete(false), "Нет")],
            ),
            row(
                "Редактор (E)",
                vec![(Editor(false), "Системный"), (Editor(true), "Выбрать…")],
            ),
        ],
        Tab::System => vec![
            row(
                "После закрытия",
                vec![
                    (Resident(false), "Выходить"),
                    (Resident(true), "Остаться в трее"),
                ],
            ),
            row(
                "Пробел в проводнике",
                vec![(QuickLook(true), "Просмотр"), (QuickLook(false), "Выкл")],
            ),
            row(
                "Запуск с Windows",
                vec![(Autostart(true), "В трее"), (Autostart(false), "Нет")],
            ),
            row(
                "Обновления",
                vec![
                    (AutoUpdate(true), "Раз в день"),
                    (AutoUpdate(false), "Вручную"),
                    (CheckUpdates, "Проверить"),
                ],
            ),
        ],
    }
}

/// "Lightbox 0.1.0 · сборка 485c787 от 19.10.2026".
pub fn version_text() -> String {
    let build = env!("LIGHTBOX_BUILD");
    let mut s = format!("{} {}", crate::APP_NAME, crate::update::VERSION);
    if !build.is_empty() {
        s.push_str(" · сборка ");
        s.push_str(build);
    }
    s
}

pub fn is_selected(s: &Settings, c: Choice) -> bool {
    match c {
        Choice::Open(v) => s.open_mode == v,
        Choice::Thumbs(v) => s.thumb_size == v,
        Choice::Filmstrip(v) => s.filmstrip == v,
        Choice::Wheel(v) => s.wheel == v,
        Choice::Save(v) => s.save_mode == v,
        Choice::KeepDates(v) => s.keep_file_dates == v,
        Choice::Imperfect(v) => s.imperfect_jpeg == v,
        Choice::Animations(v) => s.animations == v,
        Choice::TurnAnimation(v) => s.turn_animation == v,
        Choice::Dim(v) => s.dim_percent == v,
        Choice::Slideshow(v) => s.slideshow_secs == v,
        Choice::Resident(v) => s.resident == v,
        // Only works while the viewer waits in the tray.
        Choice::QuickLook(v) => (s.quick_look && s.resident) == v,
        Choice::Autostart(v) => s.autostart == v,
        Choice::AutoUpdate(v) => s.check_updates == v,
        Choice::CheckUpdates => false,
        Choice::ConfirmDelete(v) => s.confirm_delete == v,
        Choice::Editor(v) => s.editor.is_some() == v,
    }
}

pub fn apply(s: &mut Settings, c: Choice) {
    match c {
        Choice::Open(v) => s.open_mode = v,
        Choice::Thumbs(v) => s.thumb_size = v,
        Choice::Filmstrip(v) => s.filmstrip = v,
        Choice::Wheel(v) => s.wheel = v,
        Choice::Save(v) => s.save_mode = v,
        Choice::KeepDates(v) => s.keep_file_dates = v,
        Choice::Imperfect(v) => s.imperfect_jpeg = v,
        Choice::Animations(v) => s.animations = v,
        Choice::TurnAnimation(v) => s.turn_animation = v,
        Choice::Dim(v) => s.dim_percent = v,
        Choice::Slideshow(v) => s.slideshow_secs = v,
        Choice::Resident(v) => s.resident = v,
        Choice::QuickLook(v) => {
            s.quick_look = v;
            s.resident |= v;
        }
        Choice::Autostart(v) => s.autostart = v,
        Choice::AutoUpdate(v) => s.check_updates = v,
        Choice::CheckUpdates => {}
        Choice::ConfirmDelete(v) => s.confirm_delete = v,
        Choice::Editor(false) => s.editor = None,
        // Chosen in a file dialog by the app.
        Choice::Editor(true) => {}
    }
}

pub struct RowLayout {
    pub label: Rect,
    pub text: &'static str,
    pub choices: Vec<(Choice, &'static str, Rect)>,
}

pub struct CardLayout {
    pub card: Rect,
    pub title: Rect,
    pub close: Rect,
    /// Page switcher under the title.
    pub tabs: Vec<(Tab, Rect)>,
    pub rows: Vec<RowLayout>,
    /// Version line at the bottom.
    pub footer: Rect,
}

/// Sizes in 96-DPI pixels.
const CARD_W: f64 = 640.0;
const TITLE_H: f64 = 56.0;
const TABS_H: f64 = 36.0;
/// Space between the page switcher and the first row.
const TABS_GAP: f64 = 14.0;
const ROW_H: f64 = 46.0;
const LABEL_W: f64 = 220.0;
const PAD: f64 = 24.0;
const FOOTER_H: f64 = 40.0;

/// Rows on the longest page: every page gets this height, so the card does
/// not jump when switching pages.
fn max_rows() -> usize {
    Tab::ALL.iter().map(|t| rows(*t).len()).max().unwrap_or(0)
}

/// The card for page `tab`, centred in a `vp` viewport at UI scale `s`
/// (shrunk to fit).
pub fn layout(vp: (f64, f64), s: f64, tab: Tab) -> CardLayout {
    let page = rows(tab);
    let natural_h = TITLE_H + TABS_H + TABS_GAP + ROW_H * max_rows() as f64 + FOOTER_H;
    // Scale down on small windows so that the card always fits.
    let s = s
        .min((vp.0 - 16.0) / CARD_W)
        .min((vp.1 - 16.0) / natural_h)
        .max(0.3);
    let (w, h) = (CARD_W * s, natural_h * s);
    let x0 = ((vp.0 - w) / 2.0).round();
    let y0 = ((vp.1 - h) / 2.0).round();
    let card = Rect {
        x0,
        y0,
        x1: x0 + w,
        y1: y0 + h,
    };
    let title = Rect {
        x0: x0 + PAD * s,
        y0,
        x1: card.x1 - 60.0 * s,
        y1: y0 + TITLE_H * s,
    };
    let c = 32.0 * s;
    let close = Rect {
        x0: card.x1 - 16.0 * s - c,
        y0: y0 + (TITLE_H * s - c) / 2.0,
        x1: card.x1 - 16.0 * s,
        y1: y0 + (TITLE_H * s + c) / 2.0,
    };
    let (tx0, tx1) = (x0 + PAD * s, card.x1 - PAD * s);
    let tw = (tx1 - tx0) / Tab::ALL.len() as f64;
    let ty = y0 + TITLE_H * s;
    let tabs = Tab::ALL
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let a = tx0 + tw * i as f64;
            (
                *t,
                Rect {
                    x0: a.round(),
                    y0: ty,
                    x1: (a + tw).round(),
                    y1: ty + TABS_H * s,
                },
            )
        })
        .collect();
    let rows_y = ty + (TABS_H + TABS_GAP) * s;
    let mut out = Vec::with_capacity(page.len());
    for (i, row) in page.into_iter().enumerate() {
        let row_label = row.label;
        let ry = rows_y + ROW_H * i as f64 * s;
        let label = Rect {
            x0: x0 + PAD * s,
            y0: ry,
            x1: x0 + (PAD + LABEL_W) * s,
            y1: ry + ROW_H * s,
        };
        let seg_x0 = label.x1;
        let seg_x1 = card.x1 - PAD * s;
        let n = row.choices.len() as f64;
        let seg_w = (seg_x1 - seg_x0) / n;
        let (sy0, sy1) = (ry + 7.0 * s, ry + (ROW_H - 7.0) * s);
        let choices = row
            .choices
            .into_iter()
            .enumerate()
            .map(|(k, (c, text))| {
                let a = seg_x0 + seg_w * k as f64;
                (
                    c,
                    text,
                    Rect {
                        x0: a.round(),
                        y0: sy0,
                        x1: (a + seg_w).round(),
                        y1: sy1,
                    },
                )
            })
            .collect();
        out.push(RowLayout {
            label,
            text: row_label,
            choices,
        });
    }
    let footer = Rect {
        x0: x0 + PAD * s,
        y0: card.y1 - FOOTER_H * s,
        x1: card.x1 - PAD * s,
        y1: card.y1,
    };
    CardLayout {
        card,
        title,
        close,
        tabs,
        rows: out,
        footer,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Close,
    Tab(Tab),
    Choice(Choice),
    /// Inside the card but on nothing clickable.
    Card,
}

impl CardLayout {
    pub fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        if self.close.contains(p) {
            return Some(Hit::Close);
        }
        if let Some((t, _)) = self.tabs.iter().find(|(_, r)| r.contains(p)) {
            return Some(Hit::Tab(*t));
        }
        for r in &self.rows {
            for (c, _, rect) in &r.choices {
                if rect.contains(p) {
                    return Some(Hit::Choice(*c));
                }
            }
        }
        self.card.contains(p).then_some(Hit::Card)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn every_row() -> impl Iterator<Item = Row> {
        Tab::ALL.into_iter().flat_map(rows)
    }

    #[test]
    fn every_choice_applies_and_reads_back() {
        let mut s = Settings::default();
        for row in every_row() {
            for (c, _) in row.choices {
                if c == Choice::Editor(true) || c == Choice::CheckUpdates {
                    continue; // actions, not settings
                }
                apply(&mut s, c);
                assert!(is_selected(&s, c), "{c:?}");
            }
        }
    }

    #[test]
    fn defaults_are_offered() {
        // Each row has exactly one choice selected with default settings.
        let s = Settings::default();
        for row in every_row() {
            let n = row
                .choices
                .iter()
                .filter(|(c, _)| is_selected(&s, *c))
                .count();
            assert_eq!(n, 1, "{}", row.label);
        }
    }

    #[test]
    fn layout_fits_and_hits() {
        for vp in [(2560.0, 1440.0), (800.0, 500.0)] {
            for tab in Tab::ALL {
                let l = layout(vp, 1.5, tab);
                assert!(
                    l.card.x0 >= 0.0 && l.card.x1 <= vp.0 && l.card.y1 <= vp.1,
                    "{vp:?}"
                );
                let (c, _, r) = l.rows[1].choices[1];
                assert_eq!(l.hit(r.center()), Some(Hit::Choice(c)));
                assert_eq!(l.hit(l.close.center()), Some(Hit::Close));
                assert_eq!(l.hit(l.tabs[2].1.center()), Some(Hit::Tab(Tab::Files)));
                assert_eq!(l.hit((1.0, 1.0)), None);
                // The last row stays above the footer.
                assert!(l.rows.last().unwrap().label.y1 <= l.footer.y0 + 0.5);
            }
        }
        // Every page has the same card.
        let a = layout((1920.0, 1080.0), 1.0, Tab::View).card;
        let b = layout((1920.0, 1080.0), 1.0, Tab::Files).card;
        assert_eq!((a.y0, a.y1), (b.y0, b.y1));
    }
}
