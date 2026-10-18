//! Info panel (I) and histogram (H) layouts: pure geometry and hit-testing.

use crate::view::Rect;

/// Sizes in 96-DPI pixels.
const WIDTH: f64 = 390.0;
const PAD: f64 = 16.0;
const TITLE_H: f64 = 40.0;
const ROW_H: f64 = 22.0;
const LABEL_W: f64 = 112.0;
const HEADING_H: f64 = 30.0;
const LINE_H: f64 = 17.0;
const PROMPT_LINES: f64 = 7.0;
const NEGATIVE_LINES: f64 = 3.0;
const BUTTON_H: f64 = 30.0;
const GAP: f64 = 8.0;
/// Histogram overlay.
const HIST_W: f64 = 300.0;
const HIST_H: f64 = 130.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
/// Copy buttons of the AI section.
pub enum InfoButton {
    Prompt,
    Workflow,
    All,
}

impl InfoButton {
    pub fn label(self) -> &'static str {
        match self {
            InfoButton::Prompt => "Копировать промпт",
            InfoButton::Workflow => "Workflow",
            InfoButton::All => "Всё",
        }
    }
}

/// What the AI section needs room for.
#[derive(Clone, Copy, Debug, Default)]
pub struct AiShape {
    pub negative: bool,
    pub params: usize,
    pub workflow: bool,
}

pub struct InfoLayout {
    pub card: Rect,
    pub title: Rect,
    /// Label and value rectangles of the plain rows.
    pub rows: Vec<(Rect, Rect)>,
    /// "Генерация (ComfyUI)".
    pub ai_heading: Option<Rect>,
    /// Label and wrapped-text box.
    pub prompt: Option<(Rect, Rect)>,
    pub negative: Option<(Rect, Rect)>,
    pub params: Vec<(Rect, Rect)>,
    pub buttons: Vec<(InfoButton, Rect)>,
}

/// The panel at the right edge of a `vp` viewport, starting at `top`.
pub fn layout(vp: (f64, f64), s: f64, top: f64, rows: usize, ai: Option<AiShape>) -> InfoLayout {
    let w = (WIDTH * s).min(vp.0 - 2.0 * GAP * s);
    let x1 = vp.0 - 12.0 * s;
    let x0 = x1 - w;
    let (px0, px1) = (x0 + PAD * s, x1 - PAD * s);
    let title = Rect {
        x0: px0,
        y0: top,
        x1: px1,
        y1: top + TITLE_H * s,
    };
    let mut y = title.y1;
    let row = |y: f64, h: f64| {
        (
            Rect {
                x0: px0,
                y0: y,
                x1: px0 + LABEL_W * s,
                y1: y + h,
            },
            Rect {
                x0: px0 + LABEL_W * s,
                y0: y,
                x1: px1,
                y1: y + h,
            },
        )
    };
    let mut out_rows = Vec::with_capacity(rows);
    for _ in 0..rows {
        out_rows.push(row(y, ROW_H * s));
        y += ROW_H * s;
    }
    let mut out = InfoLayout {
        card: Rect {
            x0,
            y0: top,
            x1,
            y1: y,
        },
        title,
        rows: out_rows,
        ai_heading: None,
        prompt: None,
        negative: None,
        params: Vec::new(),
        buttons: Vec::new(),
    };
    if let Some(ai) = ai {
        y += GAP * s;
        out.ai_heading = Some(Rect {
            x0: px0,
            y0: y,
            x1: px1,
            y1: y + HEADING_H * s,
        });
        y += HEADING_H * s;
        let block = |y: f64, lines: f64| {
            let label = Rect {
                x0: px0,
                y0: y,
                x1: px1,
                y1: y + ROW_H * s,
            };
            let text = Rect {
                x0: px0,
                y0: label.y1,
                x1: px1,
                y1: label.y1 + (lines * LINE_H + 8.0) * s,
            };
            (label, text)
        };
        let p = block(y, PROMPT_LINES);
        y = p.1.y1 + GAP * s;
        out.prompt = Some(p);
        if ai.negative {
            let n = block(y, NEGATIVE_LINES);
            y = n.1.y1 + GAP * s;
            out.negative = Some(n);
        }
        for _ in 0..ai.params.min(10) {
            out.params.push(row(y, ROW_H * s));
            y += ROW_H * s;
        }
        y += GAP * s;
        let mut kinds = vec![InfoButton::Prompt];
        if ai.workflow {
            kinds.push(InfoButton::Workflow);
        }
        kinds.push(InfoButton::All);
        // The first (longest) label gets the most room.
        let gap = 6.0 * s;
        let shares: &[f64] = if kinds.len() == 3 {
            &[0.46, 0.30, 0.24]
        } else {
            &[0.62, 0.38]
        };
        let free = px1 - px0 - gap * (kinds.len() - 1) as f64;
        let mut bx = px0;
        for (k, share) in kinds.into_iter().zip(shares) {
            let bw = free * share;
            out.buttons.push((
                k,
                Rect {
                    x0: bx.round(),
                    y0: y,
                    x1: (bx + bw).round(),
                    y1: y + BUTTON_H * s,
                },
            ));
            bx += bw + gap;
        }
        y += BUTTON_H * s;
    }
    out.card.y1 = y + PAD * s;
    out
}

impl InfoLayout {
    pub fn hit(&self, p: (f64, f64)) -> Option<InfoButton> {
        self.buttons
            .iter()
            .find(|(_, r)| r.contains(p))
            .map(|(b, _)| *b)
    }
}

/// Histogram overlay in the top-left corner, below `top`.
pub fn histogram_rect(s: f64, top: f64) -> Rect {
    let x0 = 12.0 * s;
    Rect {
        x0,
        y0: top,
        x1: x0 + HIST_W * s,
        y1: top + HIST_H * s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn info_layout_stacks_sections() {
        let ai = AiShape {
            negative: true,
            params: 5,
            workflow: true,
        };
        let l = layout((2560.0, 1440.0), 1.0, 60.0, 9, Some(ai));
        assert_eq!(l.rows.len(), 9);
        assert!(l.card.x1 <= 2560.0 && l.card.x0 > 2000.0);
        let prompt = l.prompt.unwrap().1;
        let neg = l.negative.unwrap().0;
        assert!(prompt.y1 <= neg.y0);
        assert_eq!(l.buttons.len(), 3);
        let (b, r) = l.buttons[1];
        assert_eq!(l.hit(r.center()), Some(b));
        assert!(l.card.y1 >= r.y1);
        // Without AI metadata: just the rows.
        let plain = layout((800.0, 600.0), 1.0, 40.0, 3, None);
        assert!(plain.buttons.is_empty() && plain.prompt.is_none());
    }
}
