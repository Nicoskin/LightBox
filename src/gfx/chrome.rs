//! Drawing of the window caption, the pop-up menu and the straightening dial.

use anyhow::Result;
use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ELLIPSE, D2D1_ROUNDED_RECT, ID2D1StrokeStyle,
};
use windows_numerics::Vector2;

use super::overlay::{
    AdjustView, CaptionView, CropView, DialView, MenuView, Overlay, SegmentView, VideoBarView, rgba,
};
use super::panel::rf;
use crate::view::Rect;

/// Windows 11 caption "close" red.
const CLOSE_RED: (f32, f32, f32) = (0.77, 0.17, 0.11);
/// Accent used for toggles and the dial needle.
const ACCENT: (f32, f32, f32) = (0.30, 0.56, 1.0);
/// Title bar background (#1C1C1C).
const TITLE_BG: f32 = 0.11;

fn rounded(rect: D2D_RECT_F, r: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect,
        radiusX: r,
        radiusY: r,
    }
}

impl Overlay {
    /// Windowed title bar with its buttons, or the round close button in
    /// the corner (fullscreen).
    pub(super) unsafe fn caption(&self, c: &CaptionView) {
        let o = c.opacity;
        // SAFETY (whole function): inside BeginDraw/EndDraw.
        unsafe {
            if let Some(bar) = c.bar {
                self.brush.SetColor(&rgba(TITLE_BG, TITLE_BG, TITLE_BG, o));
                self.ctx.FillRectangle(&rf(bar), &self.brush);
            }
            if let Some((r, title)) = &c.title {
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.78 * o));
                self.glyph_str(title, rf(*r), &self.formats.detail);
            }
            for b in &c.buttons {
                let r = rf(b.rect);
                if c.round {
                    let (cx, cy) = b.rect.center();
                    let rad = (r.right - r.left) / 2.0;
                    let fill = if b.hot && b.close {
                        rgba(CLOSE_RED.0, CLOSE_RED.1, CLOSE_RED.2, 0.95 * o)
                    } else {
                        rgba(0.0, 0.0, 0.0, if b.hot { 0.62 } else { 0.42 } * o)
                    };
                    self.brush.SetColor(&fill);
                    self.ctx.FillEllipse(
                        &D2D1_ELLIPSE {
                            point: Vector2 {
                                X: cx as f32,
                                Y: cy as f32,
                            },
                            radiusX: rad,
                            radiusY: rad,
                        },
                        &self.brush,
                    );
                    self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95 * o));
                    self.glyph(b.glyph, r, &self.formats.icon);
                    continue;
                }
                if b.hot || b.pressed {
                    let fill = match (b.close, b.pressed) {
                        (true, false) => rgba(CLOSE_RED.0, CLOSE_RED.1, CLOSE_RED.2, o),
                        (true, true) => rgba(CLOSE_RED.0, CLOSE_RED.1, CLOSE_RED.2, 0.7 * o),
                        (false, false) => rgba(1.0, 1.0, 1.0, 0.09 * o),
                        (false, true) => rgba(1.0, 1.0, 1.0, 0.05 * o),
                    };
                    self.brush.SetColor(&fill);
                    self.ctx.FillRectangle(&r, &self.brush);
                }
                let a = if b.hot { 1.0 } else { 0.85 };
                if b.active {
                    self.brush.SetColor(&rgba(ACCENT.0, ACCENT.1, ACCENT.2, o));
                } else {
                    self.brush.SetColor(&rgba(1.0, 1.0, 1.0, a * o));
                }
                self.glyph(b.glyph, r, &self.formats.small_icon);
            }
        }
    }

    /// Checkerboard or solid swatch in `r`.
    unsafe fn swatch(&self, r: D2D_RECT_F, gray: Option<f32>, s: f32) {
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            match gray {
                Some(g) => {
                    self.brush.SetColor(&rgba(g, g, g, 1.0));
                    self.ctx.FillRectangle(&r, &self.brush);
                }
                None => {
                    let n = 4;
                    let cw = (r.right - r.left) / n as f32;
                    let ch = (r.bottom - r.top) / n as f32;
                    for i in 0..n {
                        for j in 0..n {
                            let v = if (i + j) % 2 == 0 { 1.0 } else { 0.8 };
                            self.brush.SetColor(&rgba(v, v, v, 1.0));
                            let x = r.left + i as f32 * cw;
                            let y = r.top + j as f32 * ch;
                            self.ctx.FillRectangle(
                                &D2D_RECT_F {
                                    left: x,
                                    top: y,
                                    right: x + cw,
                                    bottom: y + ch,
                                },
                                &self.brush,
                            );
                        }
                    }
                }
            }
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.35));
            self.ctx
                .DrawRectangle(&r, &self.brush, s, None::<&ID2D1StrokeStyle>);
        }
    }

    /// Small menu card: swatch, label and a check mark on the selected item.
    pub(super) unsafe fn menu(&self, m: &MenuView, s: f32) {
        let r = rf(m.rect);
        let radius = 8.0 * s;
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            for (grow, a) in [(6.0, 0.10), (3.0, 0.16), (1.0, 0.22)] {
                let g = grow * s;
                self.brush.SetColor(&rgba(0.0, 0.0, 0.0, a));
                self.ctx.FillRoundedRectangle(
                    &rounded(
                        D2D_RECT_F {
                            left: r.left - g,
                            top: r.top - g + 2.0 * s,
                            right: r.right + g,
                            bottom: r.bottom + g + 2.0 * s,
                        },
                        radius + g,
                    ),
                    &self.brush,
                );
            }
            self.brush.SetColor(&rgba(0.125, 0.125, 0.135, 0.98));
            self.ctx
                .FillRoundedRectangle(&rounded(r, radius), &self.brush);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.10));
            self.ctx.DrawRoundedRectangle(
                &rounded(r, radius),
                &self.brush,
                s,
                None::<&ID2D1StrokeStyle>,
            );
            for item in &m.items {
                let ir = rf(item.rect);
                if item.hot {
                    self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.10));
                    self.ctx
                        .FillRoundedRectangle(&rounded(ir, 5.0 * s), &self.brush);
                }
                let cy = (ir.top + ir.bottom) / 2.0;
                let sw = 16.0 * s;
                let sx = ir.left + 10.0 * s;
                self.swatch(
                    D2D_RECT_F {
                        left: sx,
                        top: cy - sw / 2.0,
                        right: sx + sw,
                        bottom: cy + sw / 2.0,
                    },
                    item.swatch,
                    s,
                );
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.92));
                self.glyph_str(
                    &item.label,
                    D2D_RECT_F {
                        left: sx + sw + 10.0 * s,
                        right: ir.right - 28.0 * s,
                        ..ir
                    },
                    &self.formats.label,
                );
                if item.selected {
                    self.brush
                        .SetColor(&rgba(ACCENT.0, ACCENT.1, ACCENT.2, 1.0));
                    self.glyph(
                        '\u{E73E}',
                        D2D_RECT_F {
                            left: ir.right - 30.0 * s,
                            right: ir.right - 4.0 * s,
                            ..ir
                        },
                        &self.formats.small_icon,
                    );
                }
            }
        }
    }

    /// The straightening dial: value on top, degree ruler below with the
    /// needle in the middle, "0°" reset button on the right.
    pub(super) unsafe fn dial(&self, d: &DialView, s: f32) -> Result<()> {
        let o = d.opacity;
        let r = rf(d.rect);
        let ruler = d.ruler;
        let cx = (ruler.x0 + ruler.x1) / 2.0;
        let half = ruler.width() / 2.0;
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.66 * o));
            self.ctx
                .FillRoundedRectangle(&rounded(r, 12.0 * s), &self.brush);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.08 * o));
            self.ctx.DrawRoundedRectangle(
                &rounded(r, 12.0 * s),
                &self.brush,
                s,
                None::<&ID2D1StrokeStyle>,
            );

            // Value.
            let value = Rect {
                y1: ruler.y0 + 26.0 * f64::from(s),
                ..ruler
            };
            let (vr, vg, vb) = if d.angle == 0.0 {
                (1.0, 1.0, 1.0)
            } else {
                ACCENT
            };
            self.brush.SetColor(&rgba(vr, vg, vb, 0.95 * o));
            self.glyph_str(&d.label, rf(value), &self.formats.value);

            // Ticks, fading towards the ends.
            self.ctx
                .PushAxisAlignedClip(&rf(ruler), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            let base = ruler.y1 - 9.0 * f64::from(s);
            let span = half / d.px_per_deg;
            let first = (d.angle - span).floor().max(-180.0) as i32;
            let last = (d.angle + span).ceil().min(180.0) as i32;
            for deg in first..=last {
                let x = cx + (f64::from(deg) - d.angle) * d.px_per_deg;
                let t = ((x - cx).abs() / half).min(1.0) as f32;
                let fade = 1.0 - t * t;
                let (len, a) = if deg % 10 == 0 {
                    (13.0, 0.9)
                } else if deg % 5 == 0 {
                    (9.0, 0.6)
                } else {
                    (5.0, 0.4)
                };
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, a * fade * o));
                let x = x.round() as f32 + 0.5;
                self.ctx.DrawLine(
                    Vector2 {
                        X: x,
                        Y: base as f32 - len * s,
                    },
                    Vector2 {
                        X: x,
                        Y: base as f32,
                    },
                    &self.brush,
                    s.max(1.0),
                    None::<&ID2D1StrokeStyle>,
                );
            }
            // Needle.
            self.brush.SetColor(&rgba(ACCENT.0, ACCENT.1, ACCENT.2, o));
            self.ctx.DrawLine(
                Vector2 {
                    X: cx as f32,
                    Y: base as f32 - 18.0 * s,
                },
                Vector2 {
                    X: cx as f32,
                    Y: base as f32 + 4.0 * s,
                },
                &self.brush,
                2.0 * s,
                None::<&ID2D1StrokeStyle>,
            );
            self.ctx.PopAxisAlignedClip();

            // Separator and reset.
            let rr = rf(d.reset);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.12 * o));
            self.ctx.FillRectangle(
                &D2D_RECT_F {
                    left: rr.left,
                    top: rr.top + 10.0 * s,
                    right: rr.left + s,
                    bottom: rr.bottom - 10.0 * s,
                },
                &self.brush,
            );
            let inner = D2D_RECT_F {
                left: rr.left + 4.0 * s,
                top: rr.top + 8.0 * s,
                right: rr.right - 6.0 * s,
                bottom: rr.bottom - 8.0 * s,
            };
            if d.reset_hot {
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.14 * o));
                self.ctx
                    .FillRoundedRectangle(&rounded(inner, 6.0 * s), &self.brush);
            }
            let a = if d.angle == 0.0 { 0.35 } else { 0.9 };
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, a * o));
            self.glyph_str("0°", inner, &self.formats.value);
        }
        Ok(())
    }
}

impl Overlay {
    /// Small dark label above (or below, near the top) the control at
    /// `anchor`, kept inside the window.
    pub(super) unsafe fn tooltip(&self, anchor: Rect, text: &str, s: f32) -> Result<()> {
        use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS;
        let size = unsafe { self.ctx.GetSize() };
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut m = DWRITE_TEXT_METRICS::default();
        // SAFETY: plain DirectWrite calls; the layout lives for the call.
        unsafe {
            let layout = self.dwrite.CreateTextLayout(
                &wide,
                &self.formats.detail,
                size.width,
                size.height,
            )?;
            layout.GetMetrics(&mut m)?;
        }
        let (pad, h, gap) = (9.0 * s, 26.0 * s, 8.0 * s);
        let w = m.width + 2.0 * pad;
        let cx = ((anchor.x0 + anchor.x1) / 2.0) as f32;
        let left = (cx - w / 2.0).clamp(4.0 * s, (size.width - w - 4.0 * s).max(0.0));
        let above = anchor.y0 as f32 - gap - h;
        let top = if above >= 0.0 {
            above
        } else {
            anchor.y1 as f32 + gap
        };
        let r = D2D_RECT_F {
            left,
            top,
            right: left + w,
            bottom: top + h,
        };
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.25));
            self.ctx.FillRoundedRectangle(
                &rounded(
                    D2D_RECT_F {
                        top: r.top + 2.0 * s,
                        bottom: r.bottom + 2.0 * s,
                        ..r
                    },
                    6.0 * s,
                ),
                &self.brush,
            );
            self.brush.SetColor(&rgba(0.16, 0.16, 0.17, 0.97));
            self.ctx
                .FillRoundedRectangle(&rounded(r, 6.0 * s), &self.brush);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.12));
            self.ctx.DrawRoundedRectangle(
                &rounded(r, 6.0 * s),
                &self.brush,
                s,
                None::<&ID2D1StrokeStyle>,
            );
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95));
            self.glyph_str(
                text,
                D2D_RECT_F {
                    left: r.left + pad,
                    right: r.right - pad + 2.0,
                    ..r
                },
                &self.formats.detail,
            );
        }
        Ok(())
    }
}

impl Overlay {
    /// Caption pill centred at the top of `r`.
    pub(super) unsafe fn label(&self, r: Rect, text: &str, s: f32) -> Result<()> {
        use windows::Win32::Graphics::DirectWrite::DWRITE_TEXT_METRICS;
        let wide: Vec<u16> = text.encode_utf16().collect();
        let mut m = DWRITE_TEXT_METRICS::default();
        // SAFETY: plain DirectWrite calls; the layout lives for the call.
        unsafe {
            let layout = self.dwrite.CreateTextLayout(
                &wide,
                &self.formats.detail,
                (r.x1 - r.x0) as f32,
                100.0,
            )?;
            layout.GetMetrics(&mut m)?;
        }
        let pad = 10.0 * s;
        let w = (m.width + 2.0 * pad).min((r.x1 - r.x0) as f32);
        let cx = ((r.x0 + r.x1) / 2.0) as f32;
        let pill = D2D_RECT_F {
            left: cx - w / 2.0,
            top: r.y0 as f32,
            right: cx + w / 2.0,
            bottom: r.y1 as f32,
        };
        let radius = (pill.bottom - pill.top) / 2.0;
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.62));
            self.ctx
                .FillRoundedRectangle(&rounded(pill, radius), &self.brush);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95));
            self.glyph_str(
                text,
                D2D_RECT_F {
                    left: pill.left + pad,
                    right: pill.right - pad + 2.0,
                    ..pill
                },
                &self.formats.detail,
            );
        }
        Ok(())
    }
}

impl Overlay {
    /// Ruler points on a model: accent dots joined by a line, both with a
    /// dark outline to stand out on any surface.
    pub(super) unsafe fn marks(&self, points: &[(f64, f64)], s: f32) {
        let at = |p: (f64, f64)| Vector2 {
            X: p.0 as f32,
            Y: p.1 as f32,
        };
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            for (color, grow) in [
                (rgba(0.0, 0.0, 0.0, 0.6), 2.0 * s),
                (rgba(ACCENT.0, ACCENT.1, ACCENT.2, 1.0), 0.0),
            ] {
                self.brush.SetColor(&color);
                for w in points.windows(2) {
                    self.ctx.DrawLine(
                        at(w[0]),
                        at(w[1]),
                        &self.brush,
                        2.0 * s + grow,
                        None::<&ID2D1StrokeStyle>,
                    );
                }
                for p in points {
                    let r = 5.0 * s + grow / 2.0;
                    self.ctx.FillEllipse(
                        &D2D1_ELLIPSE {
                            point: at(*p),
                            radiusX: r,
                            radiusY: r,
                        },
                        &self.brush,
                    );
                }
            }
        }
    }

    /// Start screen buttons: the main one in the accent colour.
    pub(super) unsafe fn home_buttons(&self, buttons: &[SegmentView], s: f32) {
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            for b in buttons {
                let r = rf(b.rect);
                let (fill, text_a) = match (b.selected, b.hot) {
                    (true, true) => (rgba(0.38, 0.62, 1.0, 1.0), 1.0),
                    (true, false) => (rgba(ACCENT.0, ACCENT.1, ACCENT.2, 0.95), 1.0),
                    (false, true) => (rgba(1.0, 1.0, 1.0, 0.16), 1.0),
                    (false, false) => (rgba(1.0, 1.0, 1.0, 0.09), 0.85),
                };
                self.brush.SetColor(&fill);
                self.ctx
                    .FillRoundedRectangle(&rounded(r, 8.0 * s), &self.brush);
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, text_a));
                self.glyph_str(&b.text, r, &self.formats.option);
            }
        }
    }
}

impl Overlay {
    /// The crop tool: everything outside the frame darkened, the frame with
    /// a rule-of-thirds grid and handles, its size, and the bar.
    pub(super) unsafe fn crop(&self, c: &CropView, s: f32) {
        let img = rf(c.image);
        let f = rf(c.frame);
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.58));
            for r in [
                D2D_RECT_F {
                    bottom: f.top,
                    ..img
                },
                D2D_RECT_F {
                    top: f.bottom,
                    ..img
                },
                D2D_RECT_F {
                    top: f.top,
                    bottom: f.bottom,
                    right: f.left,
                    ..img
                },
                D2D_RECT_F {
                    top: f.top,
                    bottom: f.bottom,
                    left: f.right,
                    ..img
                },
            ] {
                if r.right > r.left && r.bottom > r.top {
                    self.ctx.FillRectangle(&r, &self.brush);
                }
            }
            // Thirds.
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.35));
            let (w, h) = (f.right - f.left, f.bottom - f.top);
            for k in [1.0 / 3.0, 2.0 / 3.0] {
                let x = f.left + w * k;
                let y = f.top + h * k;
                self.ctx.DrawLine(
                    Vector2 { X: x, Y: f.top },
                    Vector2 { X: x, Y: f.bottom },
                    &self.brush,
                    s.max(1.0),
                    None::<&ID2D1StrokeStyle>,
                );
                self.ctx.DrawLine(
                    Vector2 { X: f.left, Y: y },
                    Vector2 { X: f.right, Y: y },
                    &self.brush,
                    s.max(1.0),
                    None::<&ID2D1StrokeStyle>,
                );
            }
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95));
            self.ctx
                .DrawRectangle(&f, &self.brush, 1.5 * s, None::<&ID2D1StrokeStyle>);
            // Handles: corners and edge middles.
            let hs = 5.0 * s;
            let (cx, cy) = ((f.left + f.right) / 2.0, (f.top + f.bottom) / 2.0);
            for (x, y) in [
                (f.left, f.top),
                (cx, f.top),
                (f.right, f.top),
                (f.right, cy),
                (f.right, f.bottom),
                (cx, f.bottom),
                (f.left, f.bottom),
                (f.left, cy),
            ] {
                let r = D2D_RECT_F {
                    left: x - hs,
                    top: y - hs,
                    right: x + hs,
                    bottom: y + hs,
                };
                self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.6));
                self.ctx.FillRectangle(
                    &D2D_RECT_F {
                        left: r.left - s,
                        top: r.top - s,
                        right: r.right + s,
                        bottom: r.bottom + s,
                    },
                    &self.brush,
                );
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 1.0));
                self.ctx.FillRectangle(&r, &self.brush);
            }
            // Size above the frame.
            let label = D2D_RECT_F {
                left: f.left,
                top: (f.top - 26.0 * s).max(img.top),
                right: f.left + 130.0 * s,
                bottom: (f.top - 4.0 * s).max(img.top + 22.0 * s),
            };
            self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.6));
            self.ctx
                .FillRoundedRectangle(&rounded(label, 4.0 * s), &self.brush);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95));
            self.glyph_str(&c.size, label, &self.formats.option);
            // The bar.
            for seg in &c.segments {
                let r = rf(seg.rect);
                let fill = if seg.selected {
                    rgba(
                        ACCENT.0,
                        ACCENT.1,
                        ACCENT.2,
                        if seg.hot { 1.0 } else { 0.92 },
                    )
                } else if seg.hot {
                    rgba(0.22, 0.22, 0.24, 0.96)
                } else {
                    rgba(0.13, 0.13, 0.14, 0.94)
                };
                self.brush.SetColor(&fill);
                self.ctx
                    .FillRoundedRectangle(&rounded(r, 6.0 * s), &self.brush);
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95));
                self.glyph_str(&seg.text, r, &self.formats.option);
            }
        }
    }
}

impl Overlay {
    /// The adjustments card: sliders with a zero mark, values, a note and
    /// the buttons.
    pub(super) unsafe fn adjust_card(&self, a: &AdjustView, s: f32) {
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            let card = rf(a.card);
            self.brush.SetColor(&rgba(0.11, 0.11, 0.12, 0.94));
            self.ctx
                .FillRoundedRectangle(&rounded(card, 10.0 * s), &self.brush);
            for sl in &a.sliders {
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.8));
                self.glyph_str(&sl.label.1, rf(sl.label.0), &self.formats.label);
                let t = rf(sl.track);
                let cy = (t.top + t.bottom) / 2.0;
                let (x0, x1) = (t.left + 10.0 * s, t.right - 10.0 * s);
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.18));
                self.ctx.FillRoundedRectangle(
                    &rounded(
                        D2D_RECT_F {
                            left: x0,
                            top: cy - 2.0 * s,
                            right: x1,
                            bottom: cy + 2.0 * s,
                        },
                        2.0 * s,
                    ),
                    &self.brush,
                );
                // From zero to the value, in the accent colour.
                let (z, k) = (sl.zero as f32, sl.knob as f32);
                self.brush
                    .SetColor(&rgba(ACCENT.0, ACCENT.1, ACCENT.2, 0.9));
                self.ctx.FillRectangle(
                    &D2D_RECT_F {
                        left: z.min(k),
                        top: cy - 2.0 * s,
                        right: z.max(k),
                        bottom: cy + 2.0 * s,
                    },
                    &self.brush,
                );
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.45));
                self.ctx.FillRectangle(
                    &D2D_RECT_F {
                        left: z - 0.75 * s,
                        top: cy - 7.0 * s,
                        right: z + 0.75 * s,
                        bottom: cy + 7.0 * s,
                    },
                    &self.brush,
                );
                let r = if sl.hot { 8.0 } else { 7.0 } * s;
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 1.0));
                self.ctx.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: Vector2 { X: k, Y: cy },
                        radiusX: r,
                        radiusY: r,
                    },
                    &self.brush,
                );
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.9));
                self.glyph_str(&sl.value.1, rf(sl.value.0), &self.formats.option);
            }
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.55));
            self.glyph_str(&a.note.1, rf(a.note.0), &self.formats.label);
            for b in &a.buttons {
                let r = rf(b.rect);
                let fill = if b.selected {
                    rgba(ACCENT.0, ACCENT.1, ACCENT.2, if b.hot { 1.0 } else { 0.9 })
                } else if b.hot {
                    rgba(1.0, 1.0, 1.0, 0.16)
                } else {
                    rgba(1.0, 1.0, 1.0, 0.08)
                };
                self.brush.SetColor(&fill);
                self.ctx
                    .FillRoundedRectangle(&rounded(r, 6.0 * s), &self.brush);
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95));
                self.glyph_str(&b.text, r, &self.formats.option);
            }
        }
    }
}

impl Overlay {
    /// The video bar: a round play / pause button, the track and the time.
    pub(super) unsafe fn video_bar(&self, v: &VideoBarView, s: f32) {
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            let bar = rf(v.bar);
            self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.55));
            self.ctx
                .FillRoundedRectangle(&rounded(bar, 8.0 * s), &self.brush);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95));
            let glyph = if v.playing { '\u{E769}' } else { '\u{E768}' };
            self.glyph(glyph, rf(v.button), &self.formats.small_icon);
            let t = rf(v.track);
            let cy = (t.top + t.bottom) / 2.0;
            let line = |x0: f32, x1: f32| D2D_RECT_F {
                left: x0,
                top: cy - 2.0 * s,
                right: x1,
                bottom: cy + 2.0 * s,
            };
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.25));
            self.ctx
                .FillRoundedRectangle(&rounded(line(t.left, t.right), 2.0 * s), &self.brush);
            let x = t.left + (t.right - t.left) * v.progress as f32;
            self.brush
                .SetColor(&rgba(ACCENT.0, ACCENT.1, ACCENT.2, 1.0));
            self.ctx
                .FillRoundedRectangle(&rounded(line(t.left, x), 2.0 * s), &self.brush);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 1.0));
            self.ctx.FillEllipse(
                &D2D1_ELLIPSE {
                    point: Vector2 { X: x, Y: cy },
                    radiusX: 6.0 * s,
                    radiusY: 6.0 * s,
                },
                &self.brush,
            );
            let text = D2D_RECT_F {
                left: t.right + 8.0 * s,
                ..bar
            };
            let label = if v.muted {
                crate::t!("{}  · без звука", v.time)
            } else {
                v.time.clone()
            };
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.9));
            self.glyph_str(&label, text, &self.formats.option);
        }
    }
}
