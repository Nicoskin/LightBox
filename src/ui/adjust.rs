//! The adjustments card (A): four sliders, the straightening note and the
//! buttons. Pure layout, hit-testing and slider math.

use crate::adjust::Adjust;
use crate::view::Rect;

/// Sliders: label, range (−r…r).
pub const SLIDERS: [(&str, f32); 4] = [
    ("Экспозиция", 2.0),
    ("Контраст", 1.0),
    ("Насыщенность", 1.0),
    ("Теплота", 1.0),
];

pub fn get(a: &Adjust, i: usize) -> f32 {
    match i {
        0 => a.exposure,
        1 => a.contrast,
        2 => a.saturation,
        _ => a.warmth,
    }
}

pub fn set(a: &mut Adjust, i: usize, v: f32) {
    let r = SLIDERS[i].1;
    let v = v.clamp(-r, r);
    // A little stickiness at zero.
    let v = if v.abs() < r * 0.03 { 0.0 } else { v };
    match i {
        0 => a.exposure = v,
        1 => a.contrast = v,
        2 => a.saturation = v,
        _ => a.warmth = v,
    }
}

/// "+0,7 EV", "−25".
pub fn value_text(i: usize, v: f32) -> String {
    if i == 0 {
        format!("{v:+.1} EV").replace('.', ",").replace('-', "−")
    } else {
        format!("{:+.0}", v * 100.0).replace('-', "−")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Hit {
    Slider(usize),
    Reset,
    Apply,
    Cancel,
    Card,
}

pub struct Layout {
    pub card: Rect,
    /// Label, track, value per slider.
    pub sliders: Vec<(Rect, Rect, Rect)>,
    /// The straightening note (when a tilt will be applied).
    pub note: Rect,
    pub reset: Rect,
    pub apply: Rect,
    pub cancel: Rect,
}

/// Sizes, 96-DPI pixels.
const W: f64 = 520.0;
const ROW: f64 = 36.0;
const PAD: f64 = 16.0;
const LABEL: f64 = 130.0;
const VALUE: f64 = 74.0;
const BTN_H: f64 = 32.0;

/// The card centred above `bottom` in a `vp` viewport at UI scale `s`.
pub fn layout(vp: (f64, f64), s: f64, bottom: f64) -> Layout {
    let w = (W * s).min(vp.0 - 16.0);
    let h = (PAD * 2.0 + ROW * 4.0 + 26.0 + BTN_H + 8.0) * s;
    let x0 = ((vp.0 - w) / 2.0).round();
    let y0 = (bottom - h - 14.0 * s).round();
    let card = Rect {
        x0,
        y0,
        x1: x0 + w,
        y1: y0 + h,
    };
    let (px0, px1) = (x0 + PAD * s, card.x1 - PAD * s);
    let sliders = (0..4)
        .map(|i| {
            let y = y0 + (PAD + ROW * i as f64) * s;
            let row = |a: f64, b: f64| Rect {
                x0: a,
                y0: y,
                x1: b,
                y1: y + ROW * s,
            };
            (
                row(px0, px0 + LABEL * s),
                row(px0 + LABEL * s, px1 - VALUE * s),
                row(px1 - VALUE * s, px1),
            )
        })
        .collect();
    let ny = y0 + (PAD + ROW * 4.0) * s;
    let note = Rect {
        x0: px0,
        y0: ny,
        x1: px1,
        y1: ny + 26.0 * s,
    };
    let by = card.y1 - (PAD - 4.0 + BTN_H) * s;
    let bw = 120.0 * s;
    let button = |x: f64| Rect {
        x0: x.round(),
        y0: by,
        x1: (x + bw).round(),
        y1: by + BTN_H * s,
    };
    Layout {
        card,
        sliders,
        note,
        reset: button(px0),
        apply: button(px1 - bw * 2.0 - 8.0 * s),
        cancel: button(px1 - bw),
    }
}

impl Layout {
    pub fn hit(&self, p: (f64, f64)) -> Option<Hit> {
        if !self.card.contains(p) {
            return None;
        }
        for (b, h) in [
            (self.reset, Hit::Reset),
            (self.apply, Hit::Apply),
            (self.cancel, Hit::Cancel),
        ] {
            if b.contains(p) {
                return Some(h);
            }
        }
        for (i, (_, track, _)) in self.sliders.iter().enumerate() {
            if track.contains(p) {
                return Some(Hit::Slider(i));
            }
        }
        Some(Hit::Card)
    }

    /// The value of slider `i` for a pointer at `x`.
    pub fn value_at(&self, i: usize, x: f64) -> f32 {
        let t = self.sliders[i].1;
        let pad = 10.0;
        let f = ((x - t.x0 - pad) / (t.width() - 2.0 * pad)).clamp(0.0, 1.0);
        let r = SLIDERS[i].1;
        (f as f32 * 2.0 - 1.0) * r
    }

    /// Where the knob of slider `i` sits for `v`.
    pub fn knob_x(&self, i: usize, v: f32) -> f64 {
        let t = self.sliders[i].1;
        let pad = 10.0;
        let f = f64::from(v / SLIDERS[i].1 + 1.0) / 2.0;
        t.x0 + pad + f * (t.width() - 2.0 * pad)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sliders_map_both_ways() {
        let l = layout((1600.0, 900.0), 1.0, 800.0);
        for (i, (_, range)) in SLIDERS.iter().enumerate() {
            for v in [-1.0f32, -0.5, 0.0, 0.25, 1.0] {
                let v = v * range;
                let back = l.value_at(i, l.knob_x(i, v));
                assert!((back - v).abs() < 1e-4, "{i} {v} {back}");
            }
        }
        let track = l.sliders[2].1;
        assert_eq!(l.hit(track.center()), Some(Hit::Slider(2)));
        assert_eq!(l.hit(l.apply.center()), Some(Hit::Apply));
        assert_eq!(l.hit((1.0, 1.0)), None);
        let mut a = Adjust::default();
        set(&mut a, 0, 0.04);
        assert_eq!(a.exposure, 0.0, "sticks at zero");
        set(&mut a, 3, 5.0);
        assert_eq!(a.warmth, 1.0);
        assert_eq!(value_text(0, -0.5), "−0,5 EV");
    }
}
