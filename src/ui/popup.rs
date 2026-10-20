//! Pop-up menus above panel buttons: the editing menu (crop, adjust, flip,
//! any angle, save as) and "more" (grid, histogram, compare, slideshow and
//! what shows through transparent areas). Pure layout and hit-testing.

use crate::settings::AlphaBg;
use crate::ui::toolbar::Action;
use crate::view::Rect;

/// Sizes in 96-DPI pixels.
const WIDTH: f64 = 340.0;
const ITEM_H: f64 = 34.0;
const SEP_H: f64 = 9.0;
const PAD: f64 = 6.0;
const GAP: f64 = 8.0;

/// One line of a menu.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MenuItem {
    /// Does what the panel button with this action would.
    Do(Action),
    /// Transparent areas show this.
    Background(AlphaBg),
    Separator,
}

/// The items of the menu opened by `button` (`Action::EditMenu` or
/// `Action::More`).
pub fn items(button: Action) -> Vec<MenuItem> {
    use Action::*;
    use MenuItem::{Background, Do, Separator};
    match button {
        EditMenu => vec![
            Do(Crop),
            Do(Adjust),
            Do(Straighten),
            Separator,
            Do(FlipH),
            Do(FlipV),
            Separator,
            Do(SaveAs),
        ],
        _ => {
            let mut v = vec![
                Do(Grid),
                Do(Histogram),
                Do(Compare),
                Do(Slideshow),
                Separator,
                Do(Checker),
            ];
            v.extend(AlphaBg::ALL.iter().map(|b| Background(*b)));
            v
        }
    }
}

pub struct MenuLayout {
    pub rect: Rect,
    pub items: Vec<(MenuItem, Rect)>,
}

/// The menu above `anchor` (the button), kept inside the viewport.
pub fn layout(anchor: Rect, vp: (f64, f64), s: f64, items: &[MenuItem]) -> MenuLayout {
    let height = |i: &MenuItem| {
        if *i == MenuItem::Separator {
            SEP_H
        } else {
            ITEM_H
        }
    };
    let content: f64 = items.iter().map(height).sum();
    let (w, h) = (WIDTH * s, (content + 2.0 * PAD) * s);
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
    let mut y = y0 + PAD * s;
    let items = items
        .iter()
        .map(|item| {
            let h = height(item) * s;
            let r = Rect {
                x0: x0 + PAD * s,
                y0: y,
                x1: rect.x1 - PAD * s,
                y1: y + h,
            };
            y += h;
            (*item, r)
        })
        .collect();
    MenuLayout { rect, items }
}

impl MenuLayout {
    pub fn hit(&self, p: (f64, f64)) -> Option<MenuItem> {
        self.items
            .iter()
            .find(|(i, r)| *i != MenuItem::Separator && r.contains(p))
            .map(|(i, _)| *i)
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
        let more = items(Action::More);
        let l = layout(button, (2560.0, 1440.0), 1.0, &more);
        assert!(l.rect.y1 <= button.y0 && l.rect.x1 <= 2560.0);
        assert_eq!(l.items.len(), more.len());
        let grey = l
            .items
            .iter()
            .find(|(i, _)| *i == MenuItem::Background(AlphaBg::Gray))
            .unwrap()
            .1;
        assert_eq!(
            l.hit(grey.center()),
            Some(MenuItem::Background(AlphaBg::Gray))
        );
        // Separators do nothing.
        let sep = l
            .items
            .iter()
            .find(|(i, _)| *i == MenuItem::Separator)
            .unwrap()
            .1;
        assert_eq!(l.hit(sep.center()), None);
        assert!(l.keeps_open(button, button.center(), 1.0));
        assert!(!l.keeps_open(button, (100.0, 100.0), 1.0));
        // Near the top edge it opens downwards.
        let top = Rect {
            y0: 5.0,
            y1: 41.0,
            ..button
        };
        let edit = items(Action::EditMenu);
        assert!(layout(top, (2560.0, 1440.0), 1.0, &edit).rect.y0 > 41.0);
    }
}
