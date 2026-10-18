//! Small pop-up menu above a panel button: what shows through transparent
//! areas (checkerboard, white, grey, black). Pure layout and hit-testing.

use crate::settings::AlphaBg;
use crate::view::Rect;

/// Sizes in 96-DPI pixels.
const WIDTH: f64 = 168.0;
const ITEM_H: f64 = 34.0;
const PAD: f64 = 6.0;
const GAP: f64 = 8.0;

pub struct MenuLayout {
    pub rect: Rect,
    pub items: Vec<(AlphaBg, Rect)>,
}

/// The menu above `anchor` (the button), kept inside the viewport.
pub fn layout(anchor: Rect, vp: (f64, f64), s: f64) -> MenuLayout {
    let n = AlphaBg::ALL.len() as f64;
    let (w, h) = (WIDTH * s, (ITEM_H * n + 2.0 * PAD) * s);
    let x0 = (anchor.center().0 - w / 2.0)
        .min(vp.0 - w - PAD * s)
        .max(PAD * s)
        .round();
    // Above the button; below it if there is no room.
    let above = anchor.y0 - GAP * s - h;
    let y0 = if above >= 0.0 {
        above
    } else {
        anchor.y1 + GAP * s
    }
    .round();
    let rect = Rect {
        x0,
        y0,
        x1: x0 + w,
        y1: y0 + h,
    };
    let items = AlphaBg::ALL
        .iter()
        .enumerate()
        .map(|(i, bg)| {
            let y = y0 + (PAD + ITEM_H * i as f64) * s;
            (
                *bg,
                Rect {
                    x0: x0 + PAD * s,
                    y0: y,
                    x1: rect.x1 - PAD * s,
                    y1: y + ITEM_H * s,
                },
            )
        })
        .collect();
    MenuLayout { rect, items }
}

impl MenuLayout {
    pub fn hit(&self, p: (f64, f64)) -> Option<AlphaBg> {
        self.items
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(b, _)| *b)
    }

    /// `true` while the pointer is close enough to keep the menu open:
    /// over the menu, the button, or the gap between them.
    pub fn keeps_open(&self, anchor: Rect, p: (f64, f64), s: f64) -> bool {
        let m = 16.0 * s;
        let r = self.rect;
        let around = Rect {
            x0: r.x0.min(anchor.x0) - m,
            y0: r.y0.min(anchor.y0) - m,
            x1: r.x1.max(anchor.x1) + m,
            y1: r.y1.max(anchor.y1) + m,
        };
        around.contains(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn menu_above_button_and_on_screen() {
        let button = Rect {
            x0: 2500.0,
            y0: 1400.0,
            x1: 2536.0,
            y1: 1436.0,
        };
        let l = layout(button, (2560.0, 1440.0), 1.0);
        assert!(l.rect.y1 <= button.y0 && l.rect.x1 <= 2560.0);
        assert_eq!(l.items.len(), 4);
        assert_eq!(l.hit(l.items[2].1.center()), Some(AlphaBg::Gray));
        assert!(l.keeps_open(button, button.center(), 1.0));
        assert!(!l.keeps_open(button, (100.0, 100.0), 1.0));
        // Near the top edge it opens downwards.
        let top = Rect {
            y0: 5.0,
            y1: 41.0,
            ..button
        };
        assert!(layout(top, (2560.0, 1440.0), 1.0).rect.y0 > 41.0);
    }
}
