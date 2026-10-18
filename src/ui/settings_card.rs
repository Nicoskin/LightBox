//! The settings card: rows of segmented choices drawn over the viewer.
//! Pure layout / hit-testing / applying; drawing is in the overlay.

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
    Dim(u8),
}

pub struct Row {
    pub label: &'static str,
    pub choices: Vec<(Choice, &'static str)>,
}

pub fn rows() -> Vec<Row> {
    use Choice::*;
    vec![
        Row {
            label: "Открывать фото",
            choices: vec![
                (Open(OpenMode::Fullscreen), "Во весь экран"),
                (Open(OpenMode::Windowed), "В окне"),
            ],
        },
        Row {
            label: "Лента миниатюр",
            choices: vec![
                (Filmstrip(true), "Показывать"),
                (Filmstrip(false), "Скрыть"),
            ],
        },
        Row {
            label: "Размер миниатюр",
            choices: vec![
                (Thumbs(ThumbSize::Small), "Мелкие"),
                (Thumbs(ThumbSize::Medium), "Средние"),
                (Thumbs(ThumbSize::Large), "Крупные"),
            ],
        },
        Row {
            label: "Колесо мыши",
            choices: vec![
                (Wheel(crate::settings::Wheel::Zoom), "Масштаб"),
                (Wheel(crate::settings::Wheel::Browse), "Листание"),
            ],
        },
        Row {
            label: "Сохранять поворот",
            choices: vec![
                (Save(SaveMode::Auto), "Сразу"),
                (Save(SaveMode::Ask), "Спрашивать"),
                (Save(SaveMode::Manual), "Вручную"),
            ],
        },
        Row {
            label: "Дата изменения файла",
            choices: vec![
                (KeepDates(true), "Сохранять"),
                (KeepDates(false), "Обновлять"),
            ],
        },
        Row {
            label: "JPEG не кратный 16 px",
            choices: vec![
                (Imperfect(None), "Спрашивать"),
                (Imperfect(Some(ImperfectJpeg::Trim)), "Обрезать"),
                (Imperfect(Some(ImperfectJpeg::ExifOnly)), "Только тег"),
            ],
        },
        Row {
            label: "Анимации",
            choices: vec![(Animations(true), "Вкл"), (Animations(false), "Выкл")],
        },
        Row {
            label: "Затемнение фона",
            choices: vec![(Dim(60), "60 %"), (Dim(80), "80 %"), (Dim(92), "92 %")],
        },
    ]
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
        Choice::Dim(v) => s.dim_percent == v,
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
        Choice::Dim(v) => s.dim_percent = v,
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
    pub rows: Vec<RowLayout>,
}

/// Sizes in 96-DPI pixels.
const CARD_W: f64 = 640.0;
const TITLE_H: f64 = 56.0;
const ROW_H: f64 = 46.0;
const LABEL_W: f64 = 220.0;
const PAD: f64 = 24.0;

/// The card centred in a `vp` viewport at UI scale `s` (shrunk to fit).
pub fn layout(vp: (f64, f64), s: f64) -> CardLayout {
    let rows = rows();
    let natural_h = TITLE_H + ROW_H * rows.len() as f64 + PAD;
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
    let mut out = Vec::with_capacity(rows.len());
    for (i, row) in rows.into_iter().enumerate() {
        let row_label = row.label;
        let ry = y0 + (TITLE_H + ROW_H * i as f64) * s;
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
    CardLayout {
        card,
        title,
        close,
        rows: out,
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Hit {
    Close,
    Choice(Choice),
    /// Inside the card but on nothing clickable.
    Card,
}

impl CardLayout {
    pub fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        if self.close.contains(p) {
            return Some(Hit::Close);
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

    #[test]
    fn every_choice_applies_and_reads_back() {
        let mut s = Settings::default();
        for row in rows() {
            for (c, _) in row.choices {
                apply(&mut s, c);
                assert!(is_selected(&s, c), "{c:?}");
            }
        }
    }

    #[test]
    fn defaults_are_offered() {
        // Each row has exactly one choice selected with default settings.
        let s = Settings::default();
        for row in rows() {
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
            let l = layout(vp, 1.5);
            assert!(
                l.card.x0 >= 0.0 && l.card.x1 <= vp.0 && l.card.y1 <= vp.1,
                "{vp:?}"
            );
            let (c, _, r) = l.rows[1].choices[1];
            assert_eq!(l.hit(r.center()), Some(Hit::Choice(c)));
            assert_eq!(l.hit(l.close.center()), Some(Hit::Close));
            assert_eq!(l.hit((1.0, 1.0)), None);
        }
    }
}
