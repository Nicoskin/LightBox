//! Comparing two photos (C): side by side with zoom and pan in step, or in
//! one place, holding Space to see the other. The photo that was current
//! when comparing started is "A" and stays; browsing changes "B".

use std::rc::Rc;

use anyhow::Result;

use super::{App, Mode};
use crate::gfx::{BarView, GpuImage, Layer, SegmentView};
use crate::i18n::tr;
use crate::t;
use crate::view::{Rect, View};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

/// Gap between the halves, 96-DPI pixels.
const GAP_DIP: f64 = 4.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum CompareLayout {
    SideBySide,
    /// B in place; holding Space shows A.
    Overlay,
    /// |B − A| per pixel, amplified (Shift + wheel).
    Difference,
}

/// Difference gains Shift + wheel steps through.
const GAINS: [f32; 5] = [1.0, 2.0, 4.0, 8.0, 16.0];

pub(super) struct Compare {
    pub key: String,
    pub name: String,
    pub tex: Rc<GpuImage>,
    pub layout: CompareLayout,
    /// Space held (overlay: A shown instead of B).
    pub held: bool,
    /// Difference layout: index into `GAINS`.
    pub gain: usize,
    /// The bar button under the pointer.
    pub hot: Option<BarHit>,
    /// The bar button pressed (`Some(None)`: the bar itself).
    pub pressed: Option<Option<BarHit>>,
}

/// Moves a layer by `dx` screen pixels (the right half).
pub(super) fn shift(layer: &mut Layer<'_>, dx: f32) {
    for p in &mut layer.parts {
        p.quad.dst[0] += dx;
        p.quad.dst[2] += dx;
    }
    layer.origin[0] += dx;
    layer.pivot[0] += dx;
}

impl App {
    fn side_by_side(&self) -> bool {
        self.compare
            .as_ref()
            .is_some_and(|c| c.layout == CompareLayout::SideBySide)
    }

    /// Width of one half and where the right half starts.
    fn halves(&self) -> (f64, f64) {
        let (w, _) = self.viewport();
        let gap = (GAP_DIP * self.ui_scale()).round();
        let half = ((w - gap) / 2.0).floor();
        (half, half + gap)
    }

    /// Viewport the image math works in: the right half side by side.
    pub(super) fn image_vp(&self) -> (f64, f64) {
        let vp = self.viewport();
        if self.side_by_side() {
            (self.halves().0, vp.1)
        } else {
            vp
        }
    }

    /// Screen point → point in the image viewport (either half maps to the
    /// same place: zoom and pan stay in step).
    pub(super) fn to_image_space(&self, p: (f64, f64)) -> (f64, f64) {
        if !self.side_by_side() {
            return p;
        }
        let (half, right) = self.halves();
        if p.0 >= right {
            (p.0 - right, p.1)
        } else {
            (p.0.min(half), p.1)
        }
    }

    /// `true` if `p` is over the left half (photo A).
    pub(super) fn over_a(&self, p: (f64, f64)) -> bool {
        self.side_by_side() && p.0 < self.halves().0
    }

    /// The view of A matching B's: same relative place and magnification.
    pub(super) fn a_view(&self, key: &str, tex: &GpuImage) -> Option<View> {
        let b = self.display()?;
        let a = self.display_of(key, tex);
        let k = b.0 / a.0;
        Some(View {
            scale: self.view.scale * k,
            center: (self.view.center.0 / k, self.view.center.1 / k),
            angle: self.view.angle,
        })
    }

    /// The layers to draw while comparing.
    pub(super) fn compare_layers<'a>(
        &self,
        (a_key, a_tex, layout, held): (&str, &'a GpuImage, CompareLayout, bool),
        b: Option<(&str, &'a GpuImage)>,
        detail: Option<&'a GpuImage>,
    ) -> Vec<Layer<'a>> {
        let mut out = Vec::new();
        let a_view = self.a_view(a_key, a_tex);
        match layout {
            CompareLayout::SideBySide => {
                if let Some(v) = a_view {
                    out.extend(self.layer(a_key, a_tex, &v, 1.0, None));
                }
                if let Some((key, img)) = b
                    && let Some(mut l) = self.layer(key, img, &self.view, 1.0, detail)
                {
                    shift(&mut l, self.halves().1 as f32);
                    out.push(l);
                }
            }
            // A first (drawn off screen), then B against it.
            CompareLayout::Difference => {
                if let (Some(v), Some((key, img))) = (a_view, b) {
                    let plain = |mut l: Layer<'a>| {
                        l.background = crate::gfx::Background::None;
                        l.grid = false;
                        l.channel = 0;
                        l
                    };
                    out.extend(self.layer(a_key, a_tex, &v, 1.0, None).map(plain));
                    out.extend(self.layer(key, img, &self.view, 1.0, detail).map(plain));
                }
            }
            CompareLayout::Overlay => match (held, a_view, b) {
                (true, Some(v), _) => out.extend(self.layer(a_key, a_tex, &v, 1.0, None)),
                (_, _, Some((key, img))) => {
                    out.extend(self.layer(key, img, &self.view, 1.0, detail));
                }
                _ => {}
            },
        }
        out
    }

    /// Captions over the halves: "A · name", "B · name".
    pub(super) fn compare_labels(&self) -> Vec<(Rect, String)> {
        let Some(c) = &self.compare else {
            return Vec::new();
        };
        let s = self.ui_scale();
        let top = if self.mode == Mode::Windowed {
            (crate::ui::caption::TITLE_H + 8.0) * s
        } else {
            12.0 * s
        };
        let b_name = Self::file_name(&self.current);
        let label = |x0: f64, x1: f64, text: String| {
            (
                Rect {
                    x0,
                    y0: top,
                    x1,
                    y1: top + 26.0 * s,
                },
                text,
            )
        };
        match c.layout {
            CompareLayout::SideBySide => {
                let (half, right) = self.halves();
                vec![
                    label(0.0, half, format!("A · {}", c.name)),
                    label(right, self.viewport().0, format!("B · {b_name}")),
                ]
            }
            CompareLayout::Difference => vec![label(
                0.0,
                self.viewport().0,
                t!(
                    "Разность A − B · усиление ×{} (Shift + колесо)",
                    GAINS[c.gain]
                ),
            )],
            CompareLayout::Overlay => {
                let (text, w) = if c.held {
                    (format!("A · {}", c.name), self.viewport().0)
                } else {
                    (
                        t!("B · {}   (держите пробел — A)", b_name),
                        self.viewport().0,
                    )
                };
                vec![label(0.0, w, text)]
            }
        }
    }

    /// C: start comparing the current photo (A) with the next one (B), or
    /// stop.
    pub(super) fn toggle_compare(&mut self) -> Result<()> {
        if self.compare.take().is_some() {
            return self.refit_and_render();
        }
        if self.files.len() < 2 || self.preview() {
            self.notice(tr("Для сравнения нужна папка с несколькими фото"));
            return self.render();
        }
        let Some((key, tex)) = self.shown.clone() else {
            return Ok(());
        };
        self.stop_slideshow();
        self.compare = Some(Compare {
            key: key.clone(),
            name: Self::file_name(&self.current),
            tex,
            layout: CompareLayout::SideBySide,
            held: false,
            gain: 2,
            hot: None,
            pressed: None,
        });
        let next = if self.index + 1 < self.files.len() {
            self.index + 1
        } else {
            self.index - 1
        };
        self.go_to(next)?;
        self.anim = None;
        self.notice(tr("Сравнение: ← → меняют B · V — вид · C — выход"));
        self.refit_and_render()
    }

    /// V: side by side → one place (Space shows A) → difference.
    pub(super) fn toggle_compare_layout(&mut self) -> Result<()> {
        let Some(c) = &mut self.compare else {
            return Ok(());
        };
        c.layout = match c.layout {
            CompareLayout::SideBySide => CompareLayout::Overlay,
            CompareLayout::Overlay => CompareLayout::Difference,
            CompareLayout::Difference => CompareLayout::SideBySide,
        };
        self.refit_and_render()
    }

    /// Space pressed / released in the one-place layout.
    pub(super) fn hold_a(&mut self, held: bool) -> Result<bool> {
        let Some(c) = &mut self.compare else {
            return Ok(false);
        };
        if c.layout != CompareLayout::Overlay {
            return Ok(false);
        }
        if c.held != held {
            c.held = held;
            self.render()?;
        }
        Ok(true)
    }

    /// The difference gain to draw with, while the difference is shown.
    pub(super) fn diff_gain(&self) -> Option<f32> {
        self.compare
            .as_ref()
            .filter(|c| c.layout == CompareLayout::Difference)
            .map(|c| GAINS[c.gain])
    }

    /// Shift + wheel in the difference layout: more or less gain. Returns
    /// `false` when not comparing that way.
    pub(super) fn step_diff_gain(&mut self, up: bool) -> Result<bool> {
        let Some(c) = self
            .compare
            .as_mut()
            .filter(|c| c.layout == CompareLayout::Difference)
        else {
            return Ok(false);
        };
        c.gain = if up {
            (c.gain + 1).min(GAINS.len() - 1)
        } else {
            c.gain.saturating_sub(1)
        };
        self.render()?;
        Ok(true)
    }

    fn refit_and_render(&mut self) -> Result<()> {
        if let Some(v) = self.fit_view() {
            self.view = v;
            self.fitted = true;
        }
        self.render()
    }

    /// A is gone from the folder or no longer comparable.
    pub(super) fn compare_key_removed(&mut self, key: &str) {
        if self.compare.as_ref().is_some_and(|c| c.key == key) {
            self.compare = None;
        }
    }
}

/// What a button of the compare bar does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum BarHit {
    PrevB,
    NextB,
    Layout(CompareLayout),
    /// Held down: A instead of B (one-place layout).
    HoldA,
    /// The difference gain, round the steps.
    Gain,
    Close,
}

/// The bar's buttons laid out: what each does, its text, where.
type BarButtons = Vec<(BarHit, String, Rect)>;

/// Bar sizes, 96-DPI pixels.
const BAR_H: f64 = 40.0;
const BAR_PAD: f64 = 5.0;
const BAR_GAP: f64 = 6.0;

// The compare bar: the same controls as the keys, for the mouse.
impl App {
    /// The bar's buttons with their labels and widths (96-DPI pixels).
    fn bar_buttons(&self) -> Vec<(BarHit, String, f64)> {
        let Some(c) = &self.compare else {
            return Vec::new();
        };
        let mut v = vec![
            (BarHit::PrevB, "◀ B".to_string(), 64.0),
            (BarHit::NextB, "B ▶".to_string(), 64.0),
            (
                BarHit::Layout(CompareLayout::SideBySide),
                tr("Рядом").into(),
                110.0,
            ),
            (
                BarHit::Layout(CompareLayout::Overlay),
                tr("В одном месте").into(),
                140.0,
            ),
            (
                BarHit::Layout(CompareLayout::Difference),
                tr("Разность").into(),
                110.0,
            ),
        ];
        match c.layout {
            CompareLayout::Overlay => v.push((BarHit::HoldA, tr("Держите: A").into(), 120.0)),
            CompareLayout::Difference => {
                v.push((BarHit::Gain, t!("Усиление ×{}", GAINS[c.gain]), 140.0));
            }
            CompareLayout::SideBySide => {}
        }
        v.push((BarHit::Close, tr("Закрыть").into(), 100.0));
        v
    }

    /// The bar above the panel and its buttons.
    fn bar_layout(&self) -> Option<(Rect, BarButtons)> {
        let buttons = self.bar_buttons();
        if buttons.is_empty() {
            return None;
        }
        let s = self.ui_scale();
        let (vw, vh) = self.viewport();
        let total: f64 = buttons.iter().map(|(_, _, w)| w * s).sum::<f64>()
            + BAR_GAP * s * (buttons.len() - 1) as f64
            + 2.0 * BAR_PAD * s;
        let y1 = vh - self.panel_height() - 12.0 * s;
        let x0 = ((vw - total) / 2.0).round();
        let card = Rect {
            x0,
            y0: y1 - BAR_H * s,
            x1: x0 + total,
            y1,
        };
        let mut x = x0 + BAR_PAD * s;
        let rects = buttons
            .into_iter()
            .map(|(hit, text, w)| {
                let r = Rect {
                    x0: x,
                    y0: card.y0 + BAR_PAD * s,
                    x1: x + w * s,
                    y1: card.y1 - BAR_PAD * s,
                };
                x = r.x1 + BAR_GAP * s;
                (hit, text, r)
            })
            .collect();
        Some((card, rects))
    }

    fn compare_bar_hit(&self, p: (f64, f64)) -> Option<Option<BarHit>> {
        let (card, rects) = self.bar_layout()?;
        card.contains(p).then(|| {
            rects
                .iter()
                .find(|(_, _, r)| r.contains(p))
                .map(|(h, ..)| *h)
        })
    }

    /// Top of the compare bar (notices go above it).
    pub(super) fn compare_bar_top(&self) -> Option<f64> {
        self.bar_layout().map(|(card, _)| card.y0)
    }

    pub(super) fn compare_bar_view(&self) -> Option<BarView> {
        let (card, rects) = self.bar_layout()?;
        let c = self.compare.as_ref()?;
        let n = rects.len();
        Some(BarView {
            card,
            buttons: rects
                .into_iter()
                .enumerate()
                .map(|(i, (hit, text, rect))| SegmentView {
                    rect,
                    text,
                    selected: match hit {
                        BarHit::Layout(l) => l == c.layout,
                        BarHit::HoldA => c.held,
                        _ => false,
                    },
                    hot: c.hot == Some(hit),
                    first: i == 0,
                    last: i + 1 == n,
                })
                .collect(),
        })
    }

    /// Mouse over the bar; `true` when consumed.
    pub(super) fn compare_input(&mut self, msg: u32, p: (f64, f64)) -> Result<bool> {
        let hit = self.compare_bar_hit(p);
        let Some(c) = self.compare.as_mut() else {
            return Ok(false);
        };
        match msg {
            WM_MOUSEMOVE => {
                let hot = hit.flatten();
                if c.hot != hot {
                    c.hot = hot;
                    self.render()?;
                }
                Ok(false)
            }
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK => {
                let Some(h) = hit else {
                    return Ok(false);
                };
                c.pressed = Some(h);
                if h == Some(BarHit::HoldA) {
                    self.hold_a(true)?;
                }
                Ok(true)
            }
            WM_LBUTTONUP => {
                let Some(pressed) = c.pressed.take() else {
                    return Ok(hit.is_some());
                };
                // Pressed on the bar between the buttons.
                let Some(pressed) = pressed else {
                    return Ok(true);
                };
                if pressed == BarHit::HoldA {
                    self.hold_a(false)?;
                    return Ok(true);
                }
                if hit.flatten() != Some(pressed) {
                    return Ok(true);
                }
                match pressed {
                    BarHit::PrevB => self.step(-1)?,
                    BarHit::NextB => self.step(1)?,
                    BarHit::Layout(l) => {
                        c.layout = l;
                        c.held = false;
                        self.refit_and_render()?;
                    }
                    BarHit::Gain => {
                        c.gain = (c.gain + 1) % GAINS.len();
                        self.render()?;
                    }
                    BarHit::Close => self.toggle_compare()?,
                    BarHit::HoldA => {}
                }
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
