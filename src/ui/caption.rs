//! Window caption: the title bar with minimise / fullscreen / close buttons
//! in windowed mode, and a close button in the top-right corner in
//! fullscreen. Pure layout and hit-testing.

use crate::view::Rect;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionButton {
    Minimize,
    Fullscreen,
    Close,
    /// Preview: keep this photo in its own window.
    Pin,
    /// Preview: open in the viewer (with the folder).
    OpenFull,
}

/// Which buttons the caption has.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CaptionStyle {
    /// Round close button in the corner.
    Fullscreen,
    /// Viewer window: minimise, fullscreen, close.
    Window,
    /// Space-in-Explorer preview: pin, open in the viewer, close.
    Preview,
    /// Pinned preview: minimise, open in the viewer, close.
    Pinned,
}

/// Title bar height in windowed mode, 96-DPI pixels.
pub const TITLE_H: f64 = 32.0;
/// Caption button width (as in standard Windows title bars).
const BUTTON_W: f64 = 46.0;
/// Fullscreen close button: round, this far from the corner.
const CORNER_SIZE: f64 = 40.0;
const CORNER_GAP: f64 = 10.0;

pub struct CaptionLayout {
    /// Windowed title bar.
    pub bar: Option<Rect>,
    /// File name in the title bar.
    pub title: Option<Rect>,
    pub buttons: Vec<(CaptionButton, Rect)>,
    /// Fullscreen style: a single round button over the picture.
    pub round: bool,
}

/// Caption for a `vp` viewport at UI scale `s`.
pub fn layout(vp: (f64, f64), s: f64, style: CaptionStyle) -> CaptionLayout {
    if style == CaptionStyle::Fullscreen {
        let size = (CORNER_SIZE * s).round();
        let gap = (CORNER_GAP * s).round();
        return CaptionLayout {
            bar: None,
            title: None,
            buttons: vec![(
                CaptionButton::Close,
                Rect {
                    x0: vp.0 - gap - size,
                    y0: gap,
                    x1: vp.0 - gap,
                    y1: gap + size,
                },
            )],
            round: true,
        };
    }
    let h = (TITLE_H * s).round();
    let w = (BUTTON_W * s).round();
    let bar = Rect {
        x0: 0.0,
        y0: 0.0,
        x1: vp.0,
        y1: h,
    };
    let mut buttons = Vec::with_capacity(3);
    let mut x1 = vp.0;
    // Right to left.
    let kinds = match style {
        CaptionStyle::Preview => [
            CaptionButton::Close,
            CaptionButton::OpenFull,
            CaptionButton::Pin,
        ],
        CaptionStyle::Pinned => [
            CaptionButton::Close,
            CaptionButton::OpenFull,
            CaptionButton::Minimize,
        ],
        _ => [
            CaptionButton::Close,
            CaptionButton::Fullscreen,
            CaptionButton::Minimize,
        ],
    };
    for b in kinds {
        buttons.push((
            b,
            Rect {
                x0: x1 - w,
                y0: 0.0,
                x1,
                y1: h,
            },
        ));
        x1 -= w;
    }
    let pad = 12.0 * s;
    let title = (x1 - pad > 2.0 * pad + 40.0 * s).then_some(Rect {
        x0: pad,
        y0: 0.0,
        x1: x1 - pad,
        y1: h,
    });
    CaptionLayout {
        bar: Some(bar),
        title,
        buttons,
        round: false,
    }
}

impl CaptionLayout {
    pub fn hit(&self, p: (f64, f64)) -> Option<CaptionButton> {
        self.buttons
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(b, _)| *b)
    }

    /// `true` over the title bar (buttons included).
    pub fn over_bar(&self, p: (f64, f64)) -> bool {
        self.bar.is_some_and(|b| b.contains(p))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windowed_buttons_sit_right() {
        let l = layout((800.0, 600.0), 1.5, CaptionStyle::Window);
        assert_eq!(l.buttons.len(), 3);
        assert_eq!(l.hit((799.0, 10.0)), Some(CaptionButton::Close));
        assert_eq!(
            l.hit((800.0 - 69.0 - 30.0, 10.0)),
            Some(CaptionButton::Fullscreen)
        );
        assert_eq!(l.hit((10.0, 10.0)), None);
        assert!(l.over_bar((10.0, 10.0)) && !l.over_bar((10.0, 100.0)));
        // Too narrow for a title: the buttons still fit.
        assert!(
            layout((150.0, 100.0), 1.0, CaptionStyle::Window)
                .title
                .is_none()
        );
    }

    #[test]
    fn fullscreen_close_in_the_corner() {
        let l = layout((2560.0, 1440.0), 1.0, CaptionStyle::Fullscreen);
        assert_eq!(l.hit((2535.0, 30.0)), Some(CaptionButton::Close));
        assert!(l.bar.is_none() && l.round);
    }
}
