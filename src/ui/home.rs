//! Start screen (launched without a file): "Open file…" and "Settings"
//! under a centred hint. Pure layout and hit-testing.

use crate::i18n::tr;
use crate::view::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HomeButton {
    Open,
    Settings,
}

impl HomeButton {
    pub fn label(self) -> &'static str {
        match self {
            HomeButton::Open => tr("Открыть файл…"),
            HomeButton::Settings => tr("Настройки"),
        }
    }
}

/// The hint drawn as the centred message above the buttons.
pub const MESSAGE: &str = "Lightbox\nОткройте фото или 3D-модель — или перетащите файл в это окно";

/// Sizes in 96-DPI pixels.
const BUTTON_W: f64 = 180.0;
const BUTTON_H: f64 = 40.0;
const GAP: f64 = 12.0;
/// Top of the buttons below the centre (under the message).
const TOP: f64 = 66.0;

/// The buttons for a `vp` viewport at UI scale `s`.
pub fn layout(vp: (f64, f64), s: f64) -> Vec<(HomeButton, Rect)> {
    let (w, h, gap) = (BUTTON_W * s, BUTTON_H * s, GAP * s);
    let x0 = ((vp.0 - 2.0 * w - gap) / 2.0).round();
    let y0 = (vp.1 / 2.0 + TOP * s).round();
    [HomeButton::Open, HomeButton::Settings]
        .into_iter()
        .enumerate()
        .map(|(i, b)| {
            let x = x0 + (w + gap) * i as f64;
            (
                b,
                Rect {
                    x0: x,
                    y0,
                    x1: (x + w).round(),
                    y1: y0 + h.round(),
                },
            )
        })
        .collect()
}

pub fn hit(vp: (f64, f64), s: f64, p: (f64, f64)) -> Option<HomeButton> {
    layout(vp, s)
        .into_iter()
        .find(|(_, r)| r.contains(p))
        .map(|(b, _)| b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buttons_are_centred_and_hit() {
        let vp = (900.0, 600.0);
        let l = layout(vp, 1.25);
        let mid = (l[0].1.x0 + l[1].1.x1) / 2.0;
        assert!((mid - vp.0 / 2.0).abs() <= 1.0, "{mid}");
        assert_eq!(hit(vp, 1.25, l[1].1.center()), Some(HomeButton::Settings));
        assert_eq!(hit(vp, 1.25, (5.0, 5.0)), None);
    }
}
