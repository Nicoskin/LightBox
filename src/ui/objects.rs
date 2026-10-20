//! The objects card of a 3D project (O): the objects with their colours,
//! a click hides or shows one; buttons for contrasting colours and for
//! showing all. Pure layout and hit-testing.

use crate::view::Rect;

/// Sizes in 96-DPI pixels.
const W: f64 = 300.0;
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
}

/// The card on the left, below `top` and above `bottom` (screen pixels),
/// for `count` objects scrolled by `scroll` pixels.
pub fn layout(s: f64, top: f64, bottom: f64, count: usize, scroll: f64) -> Layout {
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
        .filter(|(_, r)| r.y1 > list.y0 && r.y0 < list.y1)
        .collect();
    Layout {
        card,
        title,
        contrast,
        show_all,
        list,
        rows,
        max_scroll,
    }
}

impl Layout {
    pub fn hit(&self, p: (f64, f64)) -> Option<Hit> {
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
            && let Some((i, _)) = self.rows.iter().find(|(_, r)| r.contains(p))
        {
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
        let l = layout(1.0, 0.0, 600.0, 40, 0.0);
        assert!(l.card.y1 <= 600.0 && l.max_scroll > 0.0);
        let (i, r) = l.rows[0];
        assert_eq!((i, l.hit(r.center())), (0, Some(Hit::Row(0))));
        assert_eq!(l.hit(l.contrast.center()), Some(Hit::Contrast));
        assert_eq!(l.hit((l.card.x1 + 5.0, 100.0)), None);
        let down = layout(1.0, 0.0, 600.0, 40, 1e9);
        assert_eq!(down.rows.last().unwrap().0, 39);
        // Three objects: the card fits them exactly.
        let small = layout(1.0, 0.0, 900.0, 3, 0.0);
        assert_eq!(small.max_scroll, 0.0);
        assert_eq!(small.rows.len(), 3);
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
