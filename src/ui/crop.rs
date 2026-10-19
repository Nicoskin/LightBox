//! Crop tool geometry: the frame in image pixels, its handles on screen,
//! dragging with an optional aspect ratio, and the bar of choices. Pure
//! math; the app maps between screen and image.

use crate::view::Rect;

/// Aspect ratio choices (width : height, in the frame's orientation).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Aspect {
    Free,
    /// The photo's own ratio.
    Original,
    Ratio(f64, f64),
}

impl Aspect {
    pub const ALL: [(Aspect, &'static str); 6] = [
        (Aspect::Free, "Свободно"),
        (Aspect::Original, "Как у фото"),
        (Aspect::Ratio(1.0, 1.0), "1:1"),
        (Aspect::Ratio(3.0, 2.0), "3:2"),
        (Aspect::Ratio(4.0, 3.0), "4:3"),
        (Aspect::Ratio(16.0, 9.0), "16:9"),
    ];

    /// Width / height for a frame that is `landscape` (or portrait), on an
    /// image of size `img`.
    fn ratio(self, img: (f64, f64), landscape: bool) -> Option<f64> {
        let r = match self {
            Aspect::Free => return None,
            Aspect::Original => img.0 / img.1,
            Aspect::Ratio(w, h) => w / h,
        };
        // Keep the frame's orientation: 3:2 on a portrait frame is 2:3.
        Some(if landscape == (r >= 1.0) { r } else { 1.0 / r })
    }
}

/// What the pointer grabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Grip {
    /// Edges as flags: left, top, right, bottom.
    Edge {
        l: bool,
        t: bool,
        r: bool,
        b: bool,
    },
    Move,
}

/// Smallest frame side, image pixels.
const MIN: f64 = 8.0;

/// The frame (x0, y0, x1, y1 in image pixels) on screen, where the image
/// spans `screen`.
pub fn to_screen(frame: [f64; 4], img: (f64, f64), screen: Rect) -> Rect {
    let k = screen.width() / img.0;
    Rect {
        x0: screen.x0 + frame[0] * k,
        y0: screen.y0 + frame[1] * k,
        x1: screen.x0 + frame[2] * k,
        y1: screen.y0 + frame[3] * k,
    }
}

/// What is under `p`: a corner or edge (within `grab` pixels), or inside.
pub fn grip_at(f: Rect, p: (f64, f64), grab: f64) -> Option<Grip> {
    let near = |a: f64, b: f64| (a - b).abs() <= grab;
    let inside_x = p.0 >= f.x0 - grab && p.0 <= f.x1 + grab;
    let inside_y = p.1 >= f.y0 - grab && p.1 <= f.y1 + grab;
    if !(inside_x && inside_y) {
        return None;
    }
    let (l, r) = (near(p.0, f.x0), near(p.0, f.x1));
    let (t, b) = (near(p.1, f.y0), near(p.1, f.y1));
    if l || r || t || b {
        return Some(Grip::Edge { l, t, r, b });
    }
    f.contains(p).then_some(Grip::Move)
}

/// The frame after dragging `grip` from `start` by `d` image pixels, kept
/// inside the image and to the aspect ratio.
pub fn drag(
    start: [f64; 4],
    grip: Grip,
    d: (f64, f64),
    img: (f64, f64),
    aspect: Aspect,
) -> [f64; 4] {
    let [mut x0, mut y0, mut x1, mut y1] = start;
    match grip {
        Grip::Move => {
            let (w, h) = (x1 - x0, y1 - y0);
            x0 = (x0 + d.0).clamp(0.0, img.0 - w);
            y0 = (y0 + d.1).clamp(0.0, img.1 - h);
            [x0, y0, x0 + w, y0 + h]
        }
        Grip::Edge { l, t, r, b } => {
            if l {
                x0 = (x0 + d.0).clamp(0.0, x1 - MIN);
            }
            if r {
                x1 = (x1 + d.0).clamp(x0 + MIN, img.0);
            }
            if t {
                y0 = (y0 + d.1).clamp(0.0, y1 - MIN);
            }
            if b {
                y1 = (y1 + d.1).clamp(y0 + MIN, img.1);
            }
            let landscape = (start[2] - start[0]) >= (start[3] - start[1]);
            let Some(ratio) = aspect.ratio(img, landscape) else {
                return [x0, y0, x1, y1];
            };
            // Height follows width, except when only a horizontal edge moved.
            let by_height = (t || b) && !(l || r);
            let (mut w, mut h) = (x1 - x0, y1 - y0);
            if by_height {
                w = h * ratio;
            } else {
                h = w / ratio;
            }
            // Room left in the directions the frame may grow.
            let room_w = if l {
                x1
            } else if r {
                img.0 - x0
            } else {
                img.0
            };
            let room_h = if t {
                y1
            } else if b {
                img.1 - y0
            } else {
                img.1
            };
            let k = (room_w / w).min(room_h / h).min(1.0);
            (w, h) = (w * k, h * k);
            // Anchor at the opposite side; a side edge stays centred across.
            if l {
                x0 = x1 - w;
            } else if r {
                x1 = x0 + w;
            } else {
                let c = (x0 + x1) / 2.0;
                x0 = (c - w / 2.0).clamp(0.0, img.0 - w);
                x1 = x0 + w;
            }
            if t {
                y0 = y1 - h;
            } else if b {
                y1 = y0 + h;
            } else {
                let c = (y0 + y1) / 2.0;
                y0 = (c - h / 2.0).clamp(0.0, img.1 - h);
                y1 = y0 + h;
            }
            [x0, y0, x1, y1]
        }
    }
}

/// The largest frame of `aspect` centred in the image (choosing a ratio).
pub fn fit(img: (f64, f64), aspect: Aspect, current: [f64; 4]) -> [f64; 4] {
    let landscape = (current[2] - current[0]) >= (current[3] - current[1]);
    let Some(r) = aspect.ratio(img, landscape) else {
        return current;
    };
    let (mut w, mut h) = (img.0, img.0 / r);
    if h > img.1 {
        (w, h) = (img.1 * r, img.1);
    }
    let (x0, y0) = ((img.0 - w) / 2.0, (img.1 - h) / 2.0);
    [x0, y0, x0 + w, y0 + h]
}

/// Whole pixels: x, y, width, height.
pub fn pixels(f: [f64; 4]) -> [u32; 4] {
    let x = f[0].round().max(0.0);
    let y = f[1].round().max(0.0);
    [
        x as u32,
        y as u32,
        (f[2].round() - x).max(1.0) as u32,
        (f[3].round() - y).max(1.0) as u32,
    ]
}

/// The bar under the frame: aspect choices, then apply / cancel.
pub struct BarLayout {
    pub aspects: Vec<(Aspect, &'static str, Rect)>,
    pub apply: Rect,
    pub cancel: Rect,
}

/// The bar centred above `bottom` in a `vp` viewport at UI scale `s`.
pub fn bar(vp: (f64, f64), s: f64, bottom: f64) -> BarLayout {
    let (seg, btn, h, gap) = (78.0 * s, 110.0 * s, 34.0 * s, 10.0 * s);
    let total = seg * Aspect::ALL.len() as f64 + gap * 2.0 + btn * 2.0 + gap;
    let x0 = ((vp.0 - total) / 2.0).max(4.0).round();
    let y0 = (bottom - h - 14.0 * s).round();
    let rect = |x: f64, w: f64| Rect {
        x0: x.round(),
        y0,
        x1: (x + w).round(),
        y1: y0 + h,
    };
    let aspects = Aspect::ALL
        .iter()
        .enumerate()
        .map(|(i, (a, label))| (*a, *label, rect(x0 + seg * i as f64, seg)))
        .collect();
    let bx = x0 + seg * Aspect::ALL.len() as f64 + gap * 2.0;
    BarLayout {
        aspects,
        apply: rect(bx, btn),
        cancel: rect(bx + btn + gap, btn),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const IMG: (f64, f64) = (600.0, 400.0);

    #[test]
    fn free_drag_and_clamp() {
        let f = [100.0, 100.0, 300.0, 200.0];
        let corner = Grip::Edge {
            l: false,
            t: false,
            r: true,
            b: true,
        };
        assert_eq!(
            drag(f, corner, (50.0, 30.0), IMG, Aspect::Free),
            [100.0, 100.0, 350.0, 230.0]
        );
        // Never past the image or below the minimum.
        assert_eq!(
            drag(f, corner, (1e4, 1e4), IMG, Aspect::Free),
            [100.0, 100.0, 600.0, 400.0]
        );
        let g = drag(f, corner, (-1e4, -1e4), IMG, Aspect::Free);
        assert_eq!((g[2] - g[0], g[3] - g[1]), (MIN, MIN));
        // Moving keeps the size and stays inside.
        assert_eq!(
            drag(f, Grip::Move, (1e4, -1e4), IMG, Aspect::Free),
            [400.0, 0.0, 600.0, 100.0]
        );
    }

    #[test]
    fn aspect_is_kept() {
        let f = [100.0, 100.0, 300.0, 200.0];
        let right = Grip::Edge {
            l: false,
            t: false,
            r: true,
            b: false,
        };
        let g = drag(f, right, (100.0, 0.0), IMG, Aspect::Ratio(1.0, 1.0));
        assert!(((g[2] - g[0]) - (g[3] - g[1])).abs() < 1e-9, "{g:?}");
        assert!(g[0] >= 0.0 && g[2] <= IMG.0 && g[1] >= 0.0 && g[3] <= IMG.1);
        // 16:9 fitted in a 600 × 400 image: full width.
        let w = fit(IMG, Aspect::Ratio(16.0, 9.0), [0.0, 0.0, 600.0, 400.0]);
        assert!((w[2] - w[0] - 600.0).abs() < 1e-9);
        assert!(((w[2] - w[0]) / (w[3] - w[1]) - 16.0 / 9.0).abs() < 1e-9);
        // On a portrait frame 3:2 becomes 2:3.
        let p = fit(IMG, Aspect::Ratio(3.0, 2.0), [0.0, 0.0, 100.0, 300.0]);
        assert!(((p[2] - p[0]) / (p[3] - p[1]) - 2.0 / 3.0).abs() < 1e-9);
    }

    #[test]
    fn grips() {
        let f = Rect {
            x0: 100.0,
            y0: 100.0,
            x1: 300.0,
            y1: 200.0,
        };
        assert_eq!(
            grip_at(f, (101.0, 199.0), 6.0),
            Some(Grip::Edge {
                l: true,
                t: false,
                r: false,
                b: true
            })
        );
        assert_eq!(grip_at(f, (200.0, 150.0), 6.0), Some(Grip::Move));
        assert_eq!(grip_at(f, (50.0, 150.0), 6.0), None);
        assert_eq!(pixels([10.4, 0.0, 20.6, 5.0]), [10, 0, 11, 5]);
    }
}
