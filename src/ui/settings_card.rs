//! The settings card: pages (tabs) of rows of segmented choices drawn over
//! the viewer, with the version in the footer. Pure layout / hit-testing /
//! applying; drawing is in the overlay.

use crate::i18n::tr;
use crate::keymap::{Command, SLOTS};
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
    JpegQuality(u8),
    /// Cache size, megabytes.
    Cache(u32),
    /// Photos decoded ahead.
    Preload(u8),
    MapTiles(bool),
    Language(crate::i18n::Language),
}

/// Settings pages.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    View,
    Interface,
    Files,
    System,
    /// Keyboard shortcuts (a list of its own, see `KeyRowLayout`).
    Keys,
}

impl Tab {
    pub const ALL: [Tab; 5] = [
        Tab::View,
        Tab::Interface,
        Tab::Files,
        Tab::System,
        Tab::Keys,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tab::View => tr("Просмотр"),
            Tab::Interface => tr("Интерфейс"),
            Tab::Files => tr("Файлы"),
            Tab::System => tr("Система"),
            Tab::Keys => tr("Клавиши"),
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
    let mut rows = all_rows(tab);
    // The portable version stays out of the tray and Windows start-up.
    if crate::paths::portable() {
        rows.retain(|r| {
            !r.choices.iter().any(|(c, _)| {
                matches!(
                    c,
                    Choice::Resident(_) | Choice::QuickLook(_) | Choice::Autostart(_)
                )
            })
        });
    }
    rows
}

fn all_rows(tab: Tab) -> Vec<Row> {
    use Choice::*;
    match tab {
        Tab::View => vec![
            row(
                tr("Открывать фото"),
                vec![
                    (Open(OpenMode::Fullscreen), tr("Во весь экран")),
                    (Open(OpenMode::Windowed), tr("В окне")),
                ],
            ),
            row(
                tr("Колесо мыши"),
                vec![
                    (Wheel(crate::settings::Wheel::Zoom), tr("Масштаб")),
                    (Wheel(crate::settings::Wheel::Browse), tr("Листание")),
                ],
            ),
            row(
                tr("Затемнение фона"),
                vec![(Dim(60), "60 %"), (Dim(80), "80 %"), (Dim(92), "92 %")],
            ),
            row(
                tr("Карты по GPS"),
                vec![
                    (MapTiles(true), tr("Загружать")),
                    (MapTiles(false), tr("Нет")),
                ],
            ),
            row(
                tr("Слайд-шоу (S, F5)"),
                vec![
                    (Slideshow(2), tr("2 с")),
                    (Slideshow(4), tr("4 с")),
                    (Slideshow(8), tr("8 с")),
                ],
            ),
        ],
        Tab::Interface => vec![
            row(
                tr("Язык"),
                vec![
                    (Language(crate::i18n::Language::Auto), tr("Как в Windows")),
                    (Language(crate::i18n::Language::Ru), "Русский"),
                    (Language(crate::i18n::Language::En), "English"),
                ],
            ),
            row(
                tr("Лента миниатюр"),
                vec![
                    (Filmstrip(true), tr("Показывать")),
                    (Filmstrip(false), tr("Скрыть")),
                ],
            ),
            row(
                tr("Размер миниатюр"),
                vec![
                    (Thumbs(ThumbSize::Tiny), tr("Крошечные")),
                    (Thumbs(ThumbSize::Small), tr("Мелкие")),
                    (Thumbs(ThumbSize::Medium), tr("Средние")),
                    (Thumbs(ThumbSize::Large), tr("Крупные")),
                ],
            ),
            row(
                tr("Анимации интерфейса"),
                vec![
                    (Animations(true), tr("Вкл")),
                    (Animations(false), tr("Выкл")),
                ],
            ),
            row(
                tr("Смена фото"),
                vec![
                    (TurnAnimation(true), tr("Плавно")),
                    (TurnAnimation(false), tr("Сразу")),
                ],
            ),
        ],
        Tab::Files => vec![
            row(
                tr("Сохранять поворот"),
                vec![
                    (Save(SaveMode::Auto), tr("Сразу")),
                    (Save(SaveMode::Ask), tr("Спрашивать")),
                    (Save(SaveMode::Manual), tr("Вручную")),
                ],
            ),
            row(
                tr("Дата изменения файла"),
                vec![
                    (KeepDates(true), tr("Сохранять")),
                    (KeepDates(false), tr("Обновлять")),
                ],
            ),
            row(
                tr("JPEG не кратный 16 px"),
                vec![
                    (Imperfect(None), tr("Спрашивать")),
                    (Imperfect(Some(ImperfectJpeg::Trim)), tr("Обрезать")),
                    (Imperfect(Some(ImperfectJpeg::ExifOnly)), tr("Только тег")),
                ],
            ),
            row(
                tr("Подтверждать удаление"),
                vec![
                    (ConfirmDelete(true), tr("Да")),
                    (ConfirmDelete(false), tr("Нет")),
                ],
            ),
            row(
                tr("Редактор (E)"),
                vec![
                    (Editor(false), tr("Системный")),
                    (Editor(true), tr("Выбрать…")),
                ],
            ),
            row(
                tr("Качество JPEG"),
                vec![
                    (JpegQuality(85), "85"),
                    (JpegQuality(92), "92"),
                    (JpegQuality(97), "97"),
                ],
            ),
        ],
        Tab::System => vec![
            row(
                tr("После закрытия"),
                vec![
                    (Resident(false), tr("Выходить")),
                    (Resident(true), tr("Остаться в трее")),
                ],
            ),
            row(
                tr("Пробел в проводнике"),
                vec![
                    (QuickLook(true), tr("Просмотр")),
                    (QuickLook(false), tr("Выкл")),
                ],
            ),
            row(
                tr("Запуск с Windows"),
                vec![
                    (Autostart(true), tr("В трее")),
                    (Autostart(false), tr("Нет")),
                ],
            ),
            row(
                tr("Обновления"),
                vec![
                    (AutoUpdate(true), tr("Раз в день")),
                    (AutoUpdate(false), tr("Вручную")),
                    (CheckUpdates, tr("Проверить")),
                ],
            ),
            row(
                tr("Кэш изображений"),
                vec![
                    (Cache(512), tr("0,5 ГБ")),
                    (Cache(1536), tr("1,5 ГБ")),
                    (Cache(4096), tr("4 ГБ")),
                ],
            ),
            row(
                tr("Предзагрузка"),
                vec![
                    (Preload(1), tr("1 фото")),
                    (Preload(3), tr("3 фото")),
                    (Preload(6), tr("6 фото")),
                ],
            ),
        ],
        Tab::Keys => vec![],
    }
}

/// "Lightbox 0.1.0 · сборка 485c787 · 19.10.2026".
pub fn version_text() -> String {
    let build = env!("LIGHTBOX_BUILD");
    let mut s = format!("{} {}", crate::APP_NAME, crate::update::VERSION);
    if !build.is_empty() {
        s.push_str(tr(" · сборка "));
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
        Choice::JpegQuality(v) => s.jpeg_quality == v,
        Choice::Cache(v) => s.cache_mb == v,
        Choice::Preload(v) => s.preload == v,
        Choice::MapTiles(v) => s.map_tiles == v,
        Choice::Language(v) => s.language == v,
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
        Choice::JpegQuality(v) => s.jpeg_quality = v,
        Choice::Cache(v) => s.cache_mb = v,
        Choice::Preload(v) => s.preload = v,
        Choice::MapTiles(v) => s.map_tiles = v,
        Choice::Language(v) => s.language = v,
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
    /// Keys page: the scrolling list, its rows (already scrolled) and the
    /// largest scroll offset.
    pub list: Rect,
    pub keys: Vec<KeyRowLayout>,
    pub max_scroll: f64,
    /// Keys page: "reset all" in the footer.
    pub reset: Option<Rect>,
}

/// One action on the keys page: its name and a chip per key slot.
pub struct KeyRowLayout {
    pub cmd: Command,
    pub label: Rect,
    pub chips: [Rect; SLOTS],
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
/// Keys page: row height, column gap, chip width.
const KEY_ROW_H: f64 = 30.0;
const KEY_COL_GAP: f64 = 20.0;
const CHIP_W: f64 = 66.0;

/// Rows on the longest page: every page gets this height, so the card does
/// not jump when switching pages.
fn max_rows() -> usize {
    Tab::ALL.iter().map(|t| rows(*t).len()).max().unwrap_or(0)
}

/// The card for page `tab`, centred in a `vp` viewport at UI scale `s`
/// (shrunk to fit); `scroll` moves the keys page.
pub fn layout(vp: (f64, f64), s: f64, tab: Tab, scroll: f64) -> CardLayout {
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
    let list = Rect {
        x0: x0 + PAD * s,
        y0: rows_y,
        x1: card.x1 - PAD * s,
        y1: rows_y + ROW_H * max_rows() as f64 * s,
    };
    let (keys, max_scroll, reset) = if tab == Tab::Keys {
        let (k, m) = key_rows(list, s, scroll);
        let bw = 170.0 * s;
        let reset = Rect {
            x0: footer.x1 - bw,
            y0: footer.y0 + 6.0 * s,
            x1: footer.x1,
            y1: footer.y1 - 8.0 * s,
        };
        (k, m, Some(reset))
    } else {
        (Vec::new(), 0.0, None)
    };
    CardLayout {
        card,
        title,
        close,
        tabs,
        rows: out,
        footer,
        list,
        keys,
        max_scroll,
        reset,
    }
}

/// The actions in two columns (down the left, then the right), moved up
/// by `scroll` (clamped); returns them and the largest scroll.
fn key_rows(list: Rect, s: f64, scroll: f64) -> (Vec<KeyRowLayout>, f64) {
    let per_col = Command::ALL.len().div_ceil(2);
    let row_h = KEY_ROW_H * s;
    let max_scroll = (per_col as f64 * row_h - list.height()).max(0.0);
    let scroll = scroll.clamp(0.0, max_scroll);
    let col_w = (list.width() - KEY_COL_GAP * s) / 2.0;
    let chip_w = CHIP_W * s;
    let rows = Command::ALL
        .iter()
        .enumerate()
        .map(|(i, c)| {
            let (col, r) = (i / per_col, i % per_col);
            let x0 = list.x0 + col as f64 * (col_w + KEY_COL_GAP * s);
            let y0 = (list.y0 + r as f64 * row_h - scroll).round();
            let x1 = x0 + col_w;
            let chip = |k: usize| {
                let right = x1 - (SLOTS - 1 - k) as f64 * (chip_w + 4.0 * s);
                Rect {
                    x0: (right - chip_w).round(),
                    y0: y0 + 3.0 * s,
                    x1: right.round(),
                    y1: y0 + row_h - 3.0 * s,
                }
            };
            KeyRowLayout {
                cmd: *c,
                label: Rect {
                    x0,
                    y0,
                    x1: x1 - SLOTS as f64 * (chip_w + 4.0 * s),
                    y1: y0 + row_h,
                },
                chips: std::array::from_fn(chip),
            }
        })
        .collect();
    (rows, max_scroll)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Close,
    Tab(Tab),
    Choice(Choice),
    /// Keys page: a key slot of an action.
    KeySlot(Command, usize),
    ResetKeys,
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
        if self.reset.is_some_and(|r| r.contains(p)) {
            return Some(Hit::ResetKeys);
        }
        if self.list.contains(p) {
            for r in &self.keys {
                if let Some(k) = r.chips.iter().position(|c| c.contains(p)) {
                    return Some(Hit::KeySlot(r.cmd, k));
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
                let l = layout(vp, 1.5, tab, 0.0);
                assert!(
                    l.card.x0 >= 0.0 && l.card.x1 <= vp.0 && l.card.y1 <= vp.1,
                    "{vp:?}"
                );
                if let Some(row) = l.rows.get(1) {
                    let (c, _, r) = row.choices[1];
                    assert_eq!(l.hit(r.center()), Some(Hit::Choice(c)));
                }
                assert_eq!(l.hit(l.close.center()), Some(Hit::Close));
                assert_eq!(l.hit(l.tabs[2].1.center()), Some(Hit::Tab(Tab::Files)));
                assert_eq!(l.hit((1.0, 1.0)), None);
                // The last row stays above the footer.
                if let Some(last) = l.rows.last() {
                    assert!(last.label.y1 <= l.footer.y0 + 0.5);
                }
            }
        }
        // Every page has the same card.
        let a = layout((1920.0, 1080.0), 1.0, Tab::View, 0.0).card;
        let b = layout((1920.0, 1080.0), 1.0, Tab::Keys, 0.0).card;
        assert_eq!((a.y0, a.y1), (b.y0, b.y1));
    }

    #[test]
    fn keys_page_scrolls_and_hits() {
        let l = layout((1920.0, 1080.0), 1.0, Tab::Keys, 0.0);
        assert_eq!(l.keys.len(), Command::ALL.len());
        assert!(l.max_scroll > 0.0);
        let first = &l.keys[0];
        assert_eq!(
            l.hit(first.chips[1].center()),
            Some(Hit::KeySlot(first.cmd, 1))
        );
        assert_eq!(l.hit(l.reset.unwrap().center()), Some(Hit::ResetKeys));
        // Scrolled to the end: the last action of the left column is in
        // view, the first is out of the list and cannot be hit.
        let end = layout((1920.0, 1080.0), 1.0, Tab::Keys, 1e9);
        let per_col = Command::ALL.len().div_ceil(2);
        let last = &end.keys[per_col - 1];
        assert!(last.label.y1 <= end.list.y1 + 0.5);
        assert_ne!(
            end.hit(end.keys[0].chips[0].center()),
            Some(Hit::KeySlot(Command::ALL[0], 0))
        );
    }
}
