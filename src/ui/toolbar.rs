//! Bottom panel (Picasa-style): layout and hit-testing. Pure geometry — the
//! drawing is done by the Direct2D overlay from the views built here.

use crate::gfx::Icon;
use crate::view::Rect;

/// What a panel button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ZoomOut,
    ZoomIn,
    ActualSize,
    Fit,
    Prev,
    Next,
    RotateCcw,
    RotateCw,
    Checker,
    Smoothing,
    Filmstrip,
    ToggleMode,
    FlipH,
    FlipV,
    Save,
    SaveAs,
    Reset,
    Settings,
}

pub struct Button {
    pub action: Action,
    pub rect: Rect,
    pub icon: Icon,
    /// Larger round button (previous / next).
    pub round: bool,
}

pub struct Layout {
    /// The translucent bar along the bottom edge.
    pub bar: Rect,
    /// Thumbnail strip above the bar (fullscreen, when enabled).
    pub strip: Option<Rect>,
    pub buttons: Vec<Button>,
    /// Zoom percentage between the − and + buttons.
    pub zoom_text: Option<Rect>,
    /// File name / details on the left (fullscreen only).
    pub caption: Option<Rect>,
}

/// Sizes in 96-DPI pixels.
pub const BAR_H: f64 = 52.0;
const BAR_H_COMPACT: f64 = 44.0;
const BTN: f64 = 36.0;
const BTN_ROUND: f64 = 42.0;
const GAP: f64 = 4.0;
const GROUP_GAP: f64 = 14.0;
const ZOOM_TEXT_W: f64 = 64.0;

/// Height reserved at the bottom of the screen in fullscreen mode, so that
/// a fitted photo is not covered by the panel.
pub fn reserved_height(strip_h: Option<f64>) -> f64 {
    BAR_H + strip_h.unwrap_or(0.0)
}

enum Item {
    Button(Action, Icon, bool),
    ZoomText,
    Gap,
}

/// Lays a row of items out from `x` (left to right), vertically centred on `cy`.
fn row(items: &[Item], x: f64, cy: f64, s: f64, out: &mut Layout) -> f64 {
    let mut x = x;
    for item in items {
        match item {
            Item::Button(action, icon, round) => {
                let size = if *round { BTN_ROUND } else { BTN } * s;
                out.buttons.push(Button {
                    action: *action,
                    rect: Rect {
                        x0: x,
                        y0: cy - size / 2.0,
                        x1: x + size,
                        y1: cy + size / 2.0,
                    },
                    icon: *icon,
                    round: *round,
                });
                x += size + GAP * s;
            }
            Item::ZoomText => {
                let w = ZOOM_TEXT_W * s;
                out.zoom_text = Some(Rect {
                    x0: x,
                    y0: cy - BTN * s / 2.0,
                    x1: x + w,
                    y1: cy + BTN * s / 2.0,
                });
                x += w + GAP * s;
            }
            Item::Gap => x += GROUP_GAP * s,
        }
    }
    x
}

fn width(items: &[Item], s: f64) -> f64 {
    items
        .iter()
        .map(|i| match i {
            Item::Button(_, _, true) => BTN_ROUND + GAP,
            Item::Button(..) => BTN + GAP,
            Item::ZoomText => ZOOM_TEXT_W + GAP,
            Item::Gap => GROUP_GAP,
        })
        .sum::<f64>()
        * s
        - GAP * s
}

/// Panel layout for a `vp`-sized viewport at UI scale `s`. `compact` is the
/// windowed-mode variant (no caption, no strip, fewer buttons if narrow);
/// `strip_h` is the filmstrip height (96-DPI pixels) when it is shown.
pub fn layout(
    vp: (f64, f64),
    s: f64,
    compact: bool,
    strip_h: Option<f64>,
    windowed: bool,
) -> Layout {
    use Action::*;
    let bar_h = if compact { BAR_H_COMPACT } else { BAR_H } * s;
    let bar = Rect {
        x0: 0.0,
        y0: vp.1 - bar_h,
        x1: vp.0,
        y1: vp.1,
    };
    let strip = strip_h.filter(|_| !compact).map(|h| Rect {
        x0: 0.0,
        y0: bar.y0 - h * s,
        x1: vp.0,
        y1: bar.y0,
    });
    let mut out = Layout {
        bar,
        strip,
        buttons: Vec::new(),
        zoom_text: None,
        caption: None,
    };
    let cy = (bar.y0 + bar.y1) / 2.0;
    let pad = 12.0 * s;
    let mode_icon = Icon::Glyph(if windowed { '\u{E740}' } else { '\u{E73F}' });

    let center = [
        Item::Button(RotateCcw, Icon::Mirrored('\u{E7AD}'), false),
        Item::Button(Prev, Icon::Glyph('\u{E76B}'), true),
        Item::Button(Next, Icon::Glyph('\u{E76C}'), true),
        Item::Button(RotateCw, Icon::Glyph('\u{E7AD}'), false),
    ];
    let zoom = [
        Item::Button(ZoomOut, Icon::Glyph('\u{E71F}'), false),
        Item::ZoomText,
        Item::Button(ZoomIn, Icon::Glyph('\u{E8A3}'), false),
    ];

    if compact {
        // Windowed: centre group; zoom on the left and the mode toggle on
        // the right when there is room.
        let cw = width(&center, s);
        let roomy = vp.0 >= cw + 2.0 * (width(&zoom, s) + 2.0 * pad);
        let centre_items: &[Item] = if vp.0 >= cw + 2.0 * pad {
            &center
        } else {
            &center[1..3]
        };
        let w = width(centre_items, s);
        row(centre_items, (vp.0 - w) / 2.0, cy, s, &mut out);
        if roomy {
            row(&zoom, pad, cy, s, &mut out);
            let right = [Item::Button(ToggleMode, mode_icon, false)];
            row(&right, vp.0 - pad - width(&right, s), cy, s, &mut out);
        }
        return out;
    }

    let cw = width(&center, s);
    row(&center, (vp.0 - cw) / 2.0, cy, s, &mut out);
    let right = [
        Item::Button(ZoomOut, Icon::Glyph('\u{E71F}'), false),
        Item::ZoomText,
        Item::Button(ZoomIn, Icon::Glyph('\u{E8A3}'), false),
        Item::Button(ActualSize, Icon::Text("1:1"), false),
        Item::Button(Fit, Icon::Glyph('\u{E9A6}'), false),
        Item::Gap,
        Item::Button(Checker, Icon::Checker, false),
        Item::Button(Smoothing, Icon::Smoothing, false),
        Item::Gap,
        Item::Button(Filmstrip, Icon::Filmstrip, false),
        Item::Button(ToggleMode, mode_icon, false),
        Item::Button(Settings, Icon::Glyph('\u{E713}'), false),
    ];
    let rw = width(&right, s);
    let right_x = vp.0 - pad - rw;
    // Drop the right group on narrow screens rather than overlap.
    if right_x > (vp.0 + cw) / 2.0 + GROUP_GAP * s {
        row(&right, right_x, cy, s, &mut out);
    }
    // Editing tools to the left of the centre group.
    let edit = [
        Item::Button(FlipH, Icon::Text("\u{21C6}"), false),
        Item::Button(FlipV, Icon::Text("\u{21C5}"), false),
        Item::Gap,
        Item::Button(Save, Icon::Glyph('\u{E74E}'), false),
        Item::Button(SaveAs, Icon::Glyph('\u{E792}'), false),
        Item::Button(Reset, Icon::Glyph('\u{E7A7}'), false),
    ];
    let ew = width(&edit, s);
    let mut caption_right = (vp.0 - cw) / 2.0 - GROUP_GAP * s;
    if caption_right - ew > pad {
        row(&edit, caption_right - ew, cy, s, &mut out);
        caption_right -= ew + GROUP_GAP * s;
    }
    if caption_right > pad + 80.0 * s {
        out.caption = Some(Rect {
            x0: pad + 4.0 * s,
            y0: bar.y0,
            x1: caption_right,
            y1: bar.y1,
        });
    }
    out
}

impl Layout {
    pub fn hit(&self, p: (f64, f64)) -> Option<Action> {
        self.buttons
            .iter()
            .find(|b| b.rect.contains(p))
            .map(|b| b.action)
    }

    /// `true` if `p` is over the bar or the strip.
    pub fn contains(&self, p: (f64, f64)) -> bool {
        self.bar.contains(p) || self.strip.is_some_and(|r| r.contains(p))
    }
}

/// Thumbnail cells of the strip: the current file centred, neighbours to
/// both sides while they fit. Returns `(file index, cell rect)`.
pub fn strip_cells(strip: Rect, s: f64, count: usize, current: usize) -> Vec<(usize, Rect)> {
    if count == 0 {
        return Vec::new();
    }
    let h = strip.height() - 14.0 * s;
    let w = (h * 1.34).round();
    let step = w + 6.0 * s;
    let (cx, cy) = strip.center();
    let half = ((strip.width() / 2.0 - w / 2.0) / step).floor().max(0.0) as isize;
    let mut out = Vec::new();
    for k in -half..=half {
        let i = current as isize + k;
        if i < 0 || i >= count as isize {
            continue;
        }
        let x = cx + k as f64 * step;
        out.push((
            i as usize,
            Rect {
                x0: x - w / 2.0,
                y0: cy - h / 2.0,
                x1: x + w / 2.0,
                y1: cy + h / 2.0,
            },
        ));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_overlaps(l: &Layout) {
        for (i, a) in l.buttons.iter().enumerate() {
            assert!(
                a.rect.x0 >= l.bar.x0 && a.rect.x1 <= l.bar.x1,
                "{:?}",
                a.action
            );
            assert!(
                a.rect.y0 >= l.bar.y0 && a.rect.y1 <= l.bar.y1,
                "{:?}",
                a.action
            );
            for b in &l.buttons[i + 1..] {
                let overlap = a.rect.x0 < b.rect.x1
                    && b.rect.x0 < a.rect.x1
                    && a.rect.y0 < b.rect.y1
                    && b.rect.y0 < a.rect.y1;
                assert!(!overlap, "{:?} overlaps {:?}", a.action, b.action);
            }
        }
    }

    #[test]
    fn fullscreen_layout() {
        let l = layout((2560.0, 1440.0), 1.0, false, Some(78.0), false);
        no_overlaps(&l);
        assert_eq!(l.buttons.len(), 18);
        assert!(l.caption.is_some() && l.zoom_text.is_some());
        let strip = l.strip.unwrap();
        assert_eq!(strip.y1, l.bar.y0);
        // Previous / next sit around the centre.
        let prev = l.buttons.iter().find(|b| b.action == Action::Prev).unwrap();
        let next = l.buttons.iter().find(|b| b.action == Action::Next).unwrap();
        assert!(prev.rect.x1 <= 1280.0 && next.rect.x0 >= 1280.0);
        assert_eq!(l.hit(prev.rect.center()), Some(Action::Prev));
        assert_eq!(l.hit((5.0, 5.0)), None);
        assert!(l.contains((10.0, 1430.0)));
        assert!(!l.contains((10.0, 100.0)));
    }

    #[test]
    fn compact_layouts_fit() {
        for w in [160.0, 300.0, 500.0, 1400.0] {
            let l = layout((w, 400.0), 1.0, true, Some(78.0), true);
            no_overlaps(&l);
            assert!(l.strip.is_none());
            assert!(l.buttons.iter().any(|b| b.action == Action::Next), "w={w}");
        }
        // Narrow screens drop the right group instead of overlapping.
        no_overlaps(&layout((700.0, 400.0), 1.5, false, None, false));
    }

    #[test]
    fn strip_centres_current() {
        let strip = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 1000.0,
            y1: 78.0,
        };
        let cells = strip_cells(strip, 1.0, 100, 50);
        let cur = cells.iter().find(|(i, _)| *i == 50).unwrap().1;
        assert!((cur.center().0 - 500.0).abs() < 1e-9);
        assert!(cells.iter().all(|(_, r)| r.x0 >= 0.0 && r.x1 <= 1000.0));
        // At the start of the folder nothing is drawn to the left.
        let cells = strip_cells(strip, 1.0, 100, 0);
        assert_eq!(cells[0].0, 0);
        assert!(strip_cells(strip, 1.0, 0, 0).is_empty());
    }
}
