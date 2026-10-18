//! The straightening dial: a horizontal ruler of degree ticks that is dragged
//! sideways to turn the picture on screen (view only, never saved).

use crate::view::Rect;

/// Sizes in 96-DPI pixels.
const WIDTH: f64 = 440.0;
const HEIGHT: f64 = 58.0;
const GAP: f64 = 12.0;
const RESET_W: f64 = 44.0;
/// Ruler spacing: screen pixels (96 DPI) per degree.
pub const PX_PER_DEG: f64 = 8.0;
/// Turning is limited to ±180°.
pub const MAX_DEG: f64 = 180.0;

pub struct DialLayout {
    pub rect: Rect,
    /// The draggable ruler (left part).
    pub ruler: Rect,
    /// "0°" button on the right.
    pub reset: Rect,
}

/// The dial centred horizontally, its bottom `GAP` above `bottom`.
pub fn layout(vp: (f64, f64), s: f64, bottom: f64) -> DialLayout {
    let w = (WIDTH * s).min(vp.0 - 2.0 * GAP * s).max(120.0 * s);
    let h = HEIGHT * s;
    let x0 = ((vp.0 - w) / 2.0).round();
    let y1 = (bottom - GAP * s).round();
    let rect = Rect {
        x0,
        y0: y1 - h,
        x1: x0 + w,
        y1,
    };
    let reset = Rect {
        x0: rect.x1 - RESET_W * s,
        ..rect
    };
    let ruler = Rect {
        x1: reset.x0,
        ..rect
    };
    DialLayout { rect, ruler, reset }
}

/// Angle after dragging the ruler by `dx` screen pixels from `start`:
/// ticks follow the pointer, so dragging left turns clockwise. `fine`
/// (Shift) moves ten times slower. Snaps to 0° near it.
pub fn dragged(start: f64, dx: f64, s: f64, fine: bool) -> f64 {
    let per = PX_PER_DEG * s * if fine { 10.0 } else { 1.0 };
    let a = start - dx / per;
    let a = if fine {
        (a * 100.0).round() / 100.0
    } else {
        (a * 10.0).round() / 10.0
    };
    let a = if !fine && a.abs() < 0.5 { 0.0 } else { a };
    a.clamp(-MAX_DEG, MAX_DEG)
}

/// "+12.0°" / "−3.5°" / "0°".
pub fn label(deg: f64) -> String {
    if deg == 0.0 {
        "0°".into()
    } else if deg > 0.0 {
        format!("+{deg:.1}°")
    } else {
        format!("\u{2212}{:.1}°", -deg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn drag_turns_and_snaps() {
        assert_eq!(dragged(0.0, -96.0, 1.0, false), 12.0);
        assert_eq!(dragged(12.0, 96.0, 1.0, false), 0.0);
        assert_eq!(dragged(0.0, -3.0, 1.0, false), 0.0); // snapped
        assert_eq!(dragged(0.0, -80.0, 1.0, true), 1.0);
        assert_eq!(dragged(0.0, -1e6, 1.0, false), MAX_DEG);
        assert_eq!(label(12.0), "+12.0°");
        assert_eq!(label(-3.5), "\u{2212}3.5°");
    }

    #[test]
    fn dial_layout_fits() {
        let l = layout((300.0, 600.0), 1.0, 500.0);
        assert!(l.rect.x0 >= 0.0 && l.rect.x1 <= 300.0 && l.rect.y1 <= 500.0);
        assert!(l.ruler.x1 <= l.reset.x0);
    }
}
