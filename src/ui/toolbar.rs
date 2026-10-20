//! Bottom panel (Picasa-style): layout and hit-testing. Pure geometry — the
//! drawing is done by the Direct2D overlay from the views built here.

use crate::gfx::Icon;
use crate::view::Rect;

/// What a panel button does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    ZoomOut,
    ZoomIn,
    /// The zoom percentage: fit ↔ 100 %.
    ToggleZoom,
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
    /// Shows / hides the straightening dial.
    Straighten,
    /// The crop tool (X).
    Crop,
    /// Adjustments (A).
    Adjust,
    /// Model tools: wireframe (W), section (P), ruler (D), lay flat (B).
    Wireframe,
    Section,
    Measure,
    LayFlat,
    /// The objects of a project (O).
    Objects,
    /// The editing menu: crop, adjust, flip, any angle, save as.
    EditMenu,
    /// The other view options: grid, histogram, compare, slideshow,
    /// transparency.
    More,
    /// Pixel grid (G), info panel (I), histogram (H).
    Grid,
    Info,
    Histogram,
    /// Slideshow (S, F5).
    Slideshow,
    /// Compare two photos (C).
    Compare,
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

#[derive(Clone, Copy)]
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
                let rect = Rect {
                    x0: x,
                    y0: cy - BTN * s / 2.0,
                    x1: x + w,
                    y1: cy + BTN * s / 2.0,
                };
                out.zoom_text = Some(rect);
                // Clicking the percentage toggles fit ↔ 100 %.
                out.buttons.push(Button {
                    action: Action::ToggleZoom,
                    rect,
                    icon: Icon::Text(""),
                    round: false,
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

/// What the panel shows besides the viewport size.
#[derive(Clone, Copy, Debug, Default)]
pub struct Opts {
    /// The windowed-mode variant: no caption, no strip, fewer buttons.
    pub compact: bool,
    /// Filmstrip height (96-DPI pixels) when it is shown.
    pub strip_h: Option<f64>,
    pub windowed: bool,
    /// Browsing a folder (not a single-file preview).
    pub browse: bool,
    /// A 3D model: its own tools instead of the photo ones.
    pub model: bool,
    /// An unsaved edit: the save and undo buttons show.
    pub can_save: bool,
}

/// Panel layout for a `vp`-sized viewport at UI scale `s`.
pub fn layout(vp: (f64, f64), s: f64, o: Opts) -> Layout {
    use Action::*;
    let bar_h = if o.compact { BAR_H_COMPACT } else { BAR_H } * s;
    let bar = Rect {
        x0: 0.0,
        y0: vp.1 - bar_h,
        x1: vp.0,
        y1: vp.1,
    };
    let strip = o.strip_h.filter(|_| !o.compact).map(|h| Rect {
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
    let mode_icon = Icon::Glyph(if o.windowed { '\u{E740}' } else { '\u{E73F}' });
    let edit_menu = Item::Button(EditMenu, Icon::Glyph('\u{E70F}'), false);
    let more = Item::Button(More, Icon::Glyph('\u{E712}'), false);

    let center = [
        Item::Button(RotateCcw, Icon::Mirrored('\u{E7AD}'), false),
        Item::Button(Prev, Icon::Glyph('\u{E76B}'), true),
        Item::Button(Next, Icon::Glyph('\u{E76C}'), true),
        Item::Button(RotateCw, Icon::Glyph('\u{E7AD}'), false),
    ];
    // A preview (Space in Explorer) shows one photo: no previous / next.
    let still = [
        Item::Button(RotateCcw, Icon::Mirrored('\u{E7AD}'), false),
        Item::Button(RotateCw, Icon::Glyph('\u{E7AD}'), false),
    ];
    let center: &[Item] = if o.browse { &center } else { &still };
    let zoom = [
        Item::Button(ZoomOut, Icon::Glyph('\u{E71F}'), false),
        Item::ZoomText,
        Item::Button(ZoomIn, Icon::Glyph('\u{E8A3}'), false),
    ];

    if o.compact {
        // Windowed: centre group; zoom on the left and the menus and the
        // mode toggle on the right when there is room.
        let cw = width(center, s);
        let roomy = vp.0 >= cw + 2.0 * (width(&zoom, s) + 2.0 * pad);
        let centre_items: &[Item] = if vp.0 >= cw + 2.0 * pad || !o.browse {
            center
        } else {
            &center[1..3]
        };
        let w = width(centre_items, s);
        row(centre_items, (vp.0 - w) / 2.0, cy, s, &mut out);
        if roomy {
            row(&zoom, pad, cy, s, &mut out);
            let mut right = Vec::new();
            if !o.model {
                right.push(edit_menu);
            }
            right.push(more);
            if o.browse {
                right.push(Item::Button(ToggleMode, mode_icon, false));
            }
            row(&right, vp.0 - pad - width(&right, s), cy, s, &mut out);
        }
        return out;
    }

    let cw = width(center, s);
    row(center, (vp.0 - cw) / 2.0, cy, s, &mut out);
    let right = [
        Item::Button(ZoomOut, Icon::Glyph('\u{E71F}'), false),
        Item::ZoomText,
        Item::Button(ZoomIn, Icon::Glyph('\u{E8A3}'), false),
        Item::Gap,
        Item::Button(Smoothing, Icon::Smoothing, false),
        Item::Button(Info, Icon::Glyph('\u{E946}'), false),
        more,
        Item::Gap,
        Item::Button(Filmstrip, Icon::Filmstrip, false),
        Item::Button(ToggleMode, mode_icon, false),
        Item::Button(Settings, Icon::Glyph('\u{E713}'), false),
    ];
    let rw = width(&right, s);
    let right_x = vp.0 - pad - rw;
    let gap = GROUP_GAP * s;
    let (c0, c1) = ((vp.0 - cw) / 2.0, (vp.0 + cw) / 2.0);
    // Editing on the left of the centre group, saving (only with something
    // to save) on its right, so that the middle stays where it is.
    let model_tools = [
        Item::Button(Wireframe, Icon::Grid, false),
        Item::Button(Section, Icon::Glyph('\u{E8C6}'), false),
        Item::Button(Measure, Icon::Glyph('\u{ED5E}'), false),
        Item::Button(LayFlat, Icon::Glyph('\u{E74B}'), false),
        Item::Button(Objects, Icon::Glyph('\u{E8FD}'), false),
    ];
    let photo_tools = [edit_menu];
    let tools: &[Item] = if o.model { &model_tools } else { &photo_tools };
    let save = [
        Item::Button(Save, Icon::Glyph('\u{E74E}'), false),
        Item::Button(Reset, Icon::Glyph('\u{E7A7}'), false),
    ];
    let files: &[Item] = if o.can_save && !o.model { &save } else { &[] };
    let (tw, fw) = (width(tools, s), width(files, s));
    let split = c0 - gap - tw > pad && right_x > c1 + gap + fw + gap;
    let mut caption_right = c0 - gap;
    if split {
        row(files, c1 + gap, cy, s, &mut out);
        row(&right, right_x, cy, s, &mut out);
        row(tools, caption_right - tw, cy, s, &mut out);
        caption_right -= tw + gap;
    } else {
        // Drop the right group on narrow screens rather than overlap.
        if right_x > c1 + gap {
            row(&right, right_x, cy, s, &mut out);
        }
        let mut edit = tools.to_vec();
        if !files.is_empty() {
            edit.push(Item::Gap);
            edit.extend_from_slice(files);
        }
        let ew = width(&edit, s);
        if caption_right - ew > pad {
            row(&edit, caption_right - ew, cy, s, &mut out);
            caption_right -= ew + gap;
        }
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

/// Thumbnail cells of the strip around scroll position `pos` (a file index,
/// fractional while the strip glides): file `pos` is centred and neighbours
/// follow on both sides, including cells cut by the strip edges (the
/// drawing clips them). Returns `(file index, cell rect)`.
pub fn strip_cells(strip: Rect, s: f64, count: usize, pos: f64) -> Vec<(usize, Rect)> {
    if count == 0 {
        return Vec::new();
    }
    let h = strip.height() - 14.0 * s;
    let w = (h * 1.34).round();
    let step = w + 6.0 * s;
    let (cx, cy) = strip.center();
    let reach = (strip.width() / 2.0 + w / 2.0) / step;
    let first = (pos - reach).floor().max(0.0) as usize;
    let last = ((pos + reach).ceil().max(0.0) as usize).min(count - 1);
    let mut out = Vec::new();
    for i in first..=last {
        let x = (cx + (i as f64 - pos) * step).round();
        let r = Rect {
            x0: x - w / 2.0,
            y0: cy - h / 2.0,
            x1: x + w / 2.0,
            y1: cy + h / 2.0,
        };
        if r.x1 > strip.x0 && r.x0 < strip.x1 {
            out.push((i, r));
        }
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

    fn full(model: bool, can_save: bool) -> Opts {
        Opts {
            strip_h: Some(78.0),
            browse: true,
            model,
            can_save,
            ..Default::default()
        }
    }

    #[test]
    fn fullscreen_layout() {
        let l = layout((2560.0, 1440.0), 1.0, full(false, false));
        no_overlaps(&l);
        let has = |l: &Layout, a| l.buttons.iter().any(|b| b.action == a);
        // Editing and the rarer options are in menus; saving shows only
        // with something to save.
        assert!(has(&l, Action::EditMenu) && has(&l, Action::More));
        assert!(has(&l, Action::Info) && has(&l, Action::Smoothing));
        assert!(has(&l, Action::Filmstrip) && has(&l, Action::Settings));
        assert!(!has(&l, Action::Crop) && !has(&l, Action::Save) && !has(&l, Action::Grid));
        let dirty = layout((2560.0, 1440.0), 1.0, full(false, true));
        no_overlaps(&dirty);
        assert!(has(&dirty, Action::Save) && has(&dirty, Action::Reset));
        assert!(l.caption.is_some() && l.zoom_text.is_some());
        // A model: its own tools in place of the photo ones.
        let m = layout((2560.0, 1440.0), 1.0, full(true, true));
        no_overlaps(&m);
        assert!(has(&m, Action::Measure) && has(&m, Action::LayFlat));
        assert!(!has(&m, Action::EditMenu) && !has(&m, Action::Save));
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
            let o = Opts {
                compact: true,
                windowed: true,
                ..full(false, true)
            };
            let l = layout((w, 400.0), 1.0, o);
            no_overlaps(&l);
            assert!(l.strip.is_none());
            assert!(l.buttons.iter().any(|b| b.action == Action::Next), "w={w}");
        }
        // Narrow screens drop the right group instead of overlapping.
        let narrow = Opts {
            strip_h: None,
            ..full(false, true)
        };
        no_overlaps(&layout((700.0, 400.0), 1.5, narrow));
    }

    /// Previous / next sit exactly in the middle at every width and scale,
    /// whatever else fits on the bar.
    #[test]
    fn prev_next_always_centred() {
        for s in [1.0, 1.25, 1.5, 2.0] {
            for w in [320.0, 640.0, 900.0, 1280.0, 1920.0, 2560.0, 3840.0] {
                for compact in [false, true] {
                    let o = Opts {
                        compact,
                        windowed: compact,
                        strip_h: Some(64.0),
                        browse: true,
                        ..Default::default()
                    };
                    let l = layout((w * s, 900.0), s, o);
                    let find = |a| l.buttons.iter().find(|b| b.action == a).map(|b| b.rect);
                    let (Some(p), Some(n)) = (find(Action::Prev), find(Action::Next)) else {
                        panic!("no prev/next at {w}×{s}");
                    };
                    let mid = (p.x0 + n.x1) / 2.0;
                    assert!(
                        (mid - w * s / 2.0).abs() < 0.5,
                        "{w}×{s} compact={compact}: {mid}"
                    );
                }
            }
        }
    }

    #[test]
    fn strip_centres_current() {
        let strip = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 1000.0,
            y1: 78.0,
        };
        let cells = strip_cells(strip, 1.0, 100, 50.0);
        let cur = cells.iter().find(|(i, _)| *i == 50).unwrap().1;
        assert!((cur.center().0 - 500.0).abs() < 1e-9);
        assert!(cells.iter().all(|(_, r)| r.x1 > 0.0 && r.x0 < 1000.0));
        // Half-way between two files the strip is half-way too.
        let mid = strip_cells(strip, 1.0, 100, 50.5);
        let a = mid.iter().find(|(i, _)| *i == 50).unwrap().1.center().0;
        let b = mid.iter().find(|(i, _)| *i == 51).unwrap().1.center().0;
        assert!(((a + b) / 2.0 - 500.0).abs() <= 1.0);
        // At the start of the folder nothing is drawn to the left.
        let cells = strip_cells(strip, 1.0, 100, 0.0);
        assert_eq!(cells[0].0, 0);
        assert!(strip_cells(strip, 1.0, 0, 0.0).is_empty());
    }
}
