//! The objects card of a 3D project (O): the objects with their colours,
//! a click hides or shows one, the swatch opens a palette, the slider sets
//! how see-through it is; buttons for contrasting colours and for showing
//! all. Pure layout and hit-testing.

use crate::view::Rect;

/// Sizes in 96-DPI pixels.
const W: f64 = 420.0;
/// The opacity slider at the right of a row, and its value.
const OPACITY_W: f64 = 90.0;
const OPACITY_TEXT_W: f64 = 44.0;
/// Palette cells.
const CELL: f64 = 28.0;
const PALETTE_COLS: usize = 6;
const PAD: f64 = 12.0;
const TITLE_H: f64 = 30.0;
const BTN_H: f64 = 30.0;
const ROW: f64 = 30.0;
const MARGIN: f64 = 16.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Contrast,
    ShowAll,
    Row(usize),
    /// The colour swatch of a row: opens the palette.
    Swatch(usize),
    /// The opacity slider of a row.
    Opacity(usize),
    /// A palette colour, or back to the file's colour.
    Paint(usize),
    PaintReset,
    /// The card itself (not a control).
    Card,
}

pub struct Layout {
    pub card: Rect,
    pub title: Rect,
    pub contrast: Rect,
    pub show_all: Rect,
    /// The scrolling area of the rows.
    pub list: Rect,
    /// Visible rows: object index and rectangle (may stick out of `list`).
    pub rows: Vec<(usize, Rect)>,
    /// Largest scroll offset, pixels.
    pub max_scroll: f64,
    /// The open palette: its card, the colour cells, "as in the file".
    pub palette: Option<(Rect, Vec<Rect>, Rect)>,
}

/// Colours offered for an object (sRGB).
pub const PALETTE: [[f32; 4]; 12] = [
    [0.91, 0.26, 0.21, 1.0],
    [0.98, 0.55, 0.15, 1.0],
    [0.98, 0.84, 0.20, 1.0],
    [0.36, 0.78, 0.30, 1.0],
    [0.15, 0.70, 0.66, 1.0],
    [0.25, 0.55, 0.95, 1.0],
    [0.45, 0.33, 0.85, 1.0],
    [0.90, 0.40, 0.70, 1.0],
    [0.96, 0.96, 0.96, 1.0],
    [0.62, 0.64, 0.68, 1.0],
    [0.32, 0.33, 0.36, 1.0],
    [0.08, 0.08, 0.09, 1.0],
];

/// The swatch of a row (clickable a little wider than drawn).
pub fn swatch_rect(row: Rect, s: f64) -> Rect {
    Rect {
        x1: row.x0 + 34.0 * s,
        ..row
    }
}

/// The opacity slider of a row, and the text of its value left of it.
pub fn opacity_rects(row: Rect, s: f64) -> (Rect, Rect) {
    let slider = Rect {
        x0: row.x1 - (OPACITY_W + 8.0) * s,
        x1: row.x1 - 8.0 * s,
        ..row
    };
    let text = Rect {
        x0: slider.x0 - OPACITY_TEXT_W * s,
        x1: slider.x0 - 4.0 * s,
        ..row
    };
    (slider, text)
}

/// The opacity at `x` on a row's slider, 0.1 … 1.
pub fn opacity_at(slider: Rect, x: f64) -> f32 {
    let inset = slider.height() * 0.3;
    let f = (x - slider.x0 - inset) / (slider.width() - 2.0 * inset).max(1.0);
    (f.clamp(0.0, 1.0) as f32).max(0.1)
}

/// The card on the left, below `top` and above `bottom` (screen pixels),
/// for `count` objects scrolled by `scroll` pixels.
pub fn layout(
    s: f64,
    top: f64,
    bottom: f64,
    count: usize,
    scroll: f64,
    palette_for: Option<usize>,
) -> Layout {
    let x0 = MARGIN * s;
    let y0 = top + MARGIN * s;
    let head = (PAD + TITLE_H + BTN_H + 8.0) * s;
    let natural = head + ROW * s * count as f64 + PAD * s;
    let h = natural.min((bottom - MARGIN * s - y0).max(head + ROW * s * 2.0));
    let card = Rect {
        x0,
        y0,
        x1: x0 + W * s,
        y1: y0 + h,
    };
    let inner = (x0 + PAD * s, card.x1 - PAD * s);
    let title = Rect {
        x0: inner.0,
        y0: y0 + PAD * s,
        x1: inner.1,
        y1: y0 + (PAD + TITLE_H) * s,
    };
    let by = title.y1;
    let half = (inner.1 - inner.0 - 8.0 * s) / 2.0;
    let contrast = Rect {
        x0: inner.0,
        y0: by,
        x1: inner.0 + half,
        y1: by + BTN_H * s,
    };
    let show_all = Rect {
        x0: contrast.x1 + 8.0 * s,
        x1: inner.1,
        ..contrast
    };
    let list = Rect {
        x0: x0 + 4.0 * s,
        y0: contrast.y1 + 8.0 * s,
        x1: card.x1 - 4.0 * s,
        y1: card.y1 - PAD * s,
    };
    let max_scroll = (ROW * s * count as f64 - list.height()).max(0.0);
    let scroll = scroll.clamp(0.0, max_scroll);
    let rows = (0..count)
        .map(|i| {
            let y = list.y0 + ROW * s * i as f64 - scroll;
            (
                i,
                Rect {
                    x0: list.x0,
                    y0: y,
                    x1: list.x1,
                    y1: y + ROW * s,
                },
            )
        })
        .filter(|(_, r): &(usize, Rect)| r.y1 > list.y0 && r.y0 < list.y1)
        .collect::<Vec<_>>();
    // The palette under its row (above it near the bottom).
    let palette = palette_for
        .and_then(|i| rows.iter().find(|(k, _)| *k == i))
        .map(|&(_, row)| {
            let pad = 8.0 * s;
            let rows_n = PALETTE.len().div_ceil(PALETTE_COLS) as f64;
            let w = CELL * s * PALETTE_COLS as f64 + 2.0 * pad;
            let h = CELL * s * rows_n + 30.0 * s + 3.0 * pad;
            let x0 = row.x0 + 10.0 * s;
            let y0 = if row.y1 + h <= bottom - MARGIN * s {
                row.y1
            } else {
                row.y0 - h
            };
            let pcard = Rect {
                x0,
                y0,
                x1: x0 + w,
                y1: y0 + h,
            };
            let cells = (0..PALETTE.len())
                .map(|k| {
                    let (c, r) = ((k % PALETTE_COLS) as f64, (k / PALETTE_COLS) as f64);
                    let cx = x0 + pad + c * CELL * s;
                    let cy = y0 + pad + r * CELL * s;
                    Rect {
                        x0: cx + 2.0 * s,
                        y0: cy + 2.0 * s,
                        x1: cx + (CELL - 2.0) * s,
                        y1: cy + (CELL - 2.0) * s,
                    }
                })
                .collect();
            let reset = Rect {
                x0: x0 + pad,
                y0: pcard.y1 - pad - 30.0 * s,
                x1: pcard.x1 - pad,
                y1: pcard.y1 - pad,
            };
            (pcard, cells, reset)
        });
    Layout {
        card,
        title,
        contrast,
        show_all,
        list,
        rows,
        max_scroll,
        palette,
    }
}

impl Layout {
    pub fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        if let Some((pcard, cells, reset)) = &self.palette
            && pcard.contains(p)
        {
            if reset.contains(p) {
                return Some(Hit::PaintReset);
            }
            return Some(
                cells
                    .iter()
                    .position(|c| c.contains(p))
                    .map_or(Hit::Card, Hit::Paint),
            );
        }
        if !self.card.contains(p) {
            return None;
        }
        if self.contrast.contains(p) {
            return Some(Hit::Contrast);
        }
        if self.show_all.contains(p) {
            return Some(Hit::ShowAll);
        }
        if self.list.contains(p)
            && let Some((i, r)) = self.rows.iter().find(|(_, r)| r.contains(p))
        {
            let s = r.height() / ROW;
            if swatch_rect(*r, s).contains(p) {
                return Some(Hit::Swatch(*i));
            }
            if opacity_rects(*r, s).0.contains(p) {
                return Some(Hit::Opacity(*i));
            }
            return Some(Hit::Row(*i));
        }
        Some(Hit::Card)
    }
}

/// `n` clearly different colours (sRGB): hues spread by the golden angle.
pub fn contrast_color(i: usize) -> [f32; 4] {
    let h = (i as f32 * 0.618_034 + 0.08).fract() * 6.0;
    let (s, v) = (0.70, 0.95);
    let c = v * s;
    let x = c * (1.0 - (h % 2.0 - 1.0).abs());
    let (r, g, b) = match h as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let m = v - c;
    [r + m, g + m, b + m, 1.0]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rows_scroll_and_hit() {
        // 40 objects do not fit in 600 px: the list scrolls.
        let l = layout(1.0, 0.0, 600.0, 40, 0.0, None);
        assert!(l.card.y1 <= 600.0 && l.max_scroll > 0.0);
        let (i, r) = l.rows[0];
        assert_eq!((i, l.hit(r.center())), (0, Some(Hit::Row(0))));
        assert_eq!(l.hit(l.contrast.center()), Some(Hit::Contrast));
        assert_eq!(l.hit((l.card.x1 + 5.0, 100.0)), None);
        let down = layout(1.0, 0.0, 600.0, 40, 1e9, None);
        assert_eq!(down.rows.last().unwrap().0, 39);
        // Three objects: the card fits them exactly.
        let small = layout(1.0, 0.0, 900.0, 3, 0.0, Some(1));
        assert_eq!(small.max_scroll, 0.0);
        assert_eq!(small.rows.len(), 3);
        // A row: swatch on the left, opacity on the right, name between.
        let (_, row) = small.rows[0];
        let (slider, _) = opacity_rects(row, 1.0);
        assert_eq!(
            small.hit(swatch_rect(row, 1.0).center()),
            Some(Hit::Swatch(0))
        );
        assert_eq!(small.hit(slider.center()), Some(Hit::Opacity(0)));
        assert!((opacity_at(slider, slider.x1) - 1.0).abs() < 1e-6);
        assert!((opacity_at(slider, slider.x0) - 0.1).abs() < 1e-6);
        // The palette of row 1 is above everything else.
        let (pcard, cells, reset) = small.palette.as_ref().unwrap();
        assert!(pcard.y0 >= small.rows[1].1.y1 - 0.5);
        assert_eq!(small.hit(cells[3].center()), Some(Hit::Paint(3)));
        assert_eq!(small.hit(reset.center()), Some(Hit::PaintReset));
    }

    #[test]
    fn colours_differ() {
        let c: Vec<_> = (0..6).map(contrast_color).collect();
        for i in 0..6 {
            for j in i + 1..6 {
                let d: f32 = (0..3).map(|k| (c[i][k] - c[j][k]).abs()).sum();
                assert!(d > 0.2, "{i} {j}: {:?} {:?}", c[i], c[j]);
            }
        }
    }
}
