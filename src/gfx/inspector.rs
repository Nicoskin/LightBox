//! Drawing of the inspector overlays: histogram and info panel.

use windows::Win32::Graphics::Direct2D::Common::D2D_RECT_F;
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_ROUNDED_RECT, ID2D1StrokeStyle,
};

use super::overlay::{HistogramView, InfoView, Overlay, rgba};
use super::panel::rf;

fn rounded(rect: D2D_RECT_F, r: f32) -> D2D1_ROUNDED_RECT {
    D2D1_ROUNDED_RECT {
        rect,
        radiusX: r,
        radiusY: r,
    }
}

impl Overlay {
    /// Dark translucent card behind an overlay.
    unsafe fn backdrop(&self, r: D2D_RECT_F, s: f32, alpha: f32) {
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            self.brush.SetColor(&rgba(0.07, 0.07, 0.08, alpha));
            self.ctx
                .FillRoundedRectangle(&rounded(r, 10.0 * s), &self.brush);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.08));
            self.ctx.DrawRoundedRectangle(
                &rounded(r, 10.0 * s),
                &self.brush,
                s,
                None::<&ID2D1StrokeStyle>,
            );
        }
    }

    /// R, G, B and luma histograms as overlapping translucent bars.
    pub(super) unsafe fn histogram(&self, h: &HistogramView, s: f32) {
        let r = rf(h.rect);
        let pad = 10.0 * s;
        let plot = D2D_RECT_F {
            left: r.left + pad,
            top: r.top + pad,
            right: r.right - pad,
            bottom: r.bottom - pad,
        };
        let hist = &h.hist;
        // Scale to the tallest bin, ignoring the clipped ends (0 and 255)
        // whose spikes would flatten everything else.
        let peak = [&hist.r, &hist.g, &hist.b, &hist.luma]
            .iter()
            .flat_map(|c| c[1..255].iter())
            .copied()
            .max()
            .unwrap_or(0)
            .max(1) as f32;
        let bw = (plot.right - plot.left) / 256.0;
        let height = plot.bottom - plot.top;
        // SAFETY: inside BeginDraw/EndDraw.
        unsafe {
            self.backdrop(r, s, 0.72);
            self.ctx
                .PushAxisAlignedClip(&plot, D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            for (bins, color) in [
                (&hist.luma, rgba(1.0, 1.0, 1.0, 0.22)),
                (&hist.r, rgba(1.0, 0.25, 0.25, 0.45)),
                (&hist.g, rgba(0.25, 1.0, 0.35, 0.40)),
                (&hist.b, rgba(0.30, 0.50, 1.0, 0.50)),
            ] {
                self.brush.SetColor(&color);
                for (i, &v) in bins.iter().enumerate() {
                    if v == 0 {
                        continue;
                    }
                    let bar = (v as f32 / peak).min(1.0) * height;
                    let x = plot.left + i as f32 * bw;
                    self.ctx.FillRectangle(
                        &D2D_RECT_F {
                            left: x,
                            top: plot.bottom - bar,
                            right: x + bw + 0.5,
                            bottom: plot.bottom,
                        },
                        &self.brush,
                    );
                }
            }
            self.ctx.PopAxisAlignedClip();
        }
    }

    /// Info panel: title, label / value rows, AI section, buttons.
    pub(super) unsafe fn info(&self, v: &InfoView, s: f32) {
        // SAFETY (whole function): inside BeginDraw/EndDraw.
        unsafe {
            self.backdrop(rf(v.card), s, 0.86);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95));
            self.glyph_str(&v.title.1, rf(v.title.0), &self.formats.heading);
            for (lr, label, vr, value) in &v.rows {
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.55));
                self.glyph_str(label, rf(*lr), &self.formats.detail);
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.92));
                self.glyph_str(value, rf(*vr), &self.formats.detail);
            }
            for (r, text) in &v.headings {
                self.brush.SetColor(&rgba(0.55, 0.75, 1.0, 0.95));
                self.glyph_str(text, rf(*r), &self.formats.caption);
            }
            for (r, text) in &v.blocks {
                let box_r = rf(*r);
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.05));
                self.ctx
                    .FillRoundedRectangle(&rounded(box_r, 6.0 * s), &self.brush);
                let inner = D2D_RECT_F {
                    left: box_r.left + 6.0 * s,
                    top: box_r.top + 4.0 * s,
                    right: box_r.right - 6.0 * s,
                    bottom: box_r.bottom - 4.0 * s,
                };
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.88));
                self.glyph_str(text, inner, &self.formats.wrap);
            }
            for (r, label, hot) in &v.buttons {
                let br = rf(*r);
                self.brush
                    .SetColor(&rgba(1.0, 1.0, 1.0, if *hot { 0.18 } else { 0.09 }));
                self.ctx
                    .FillRoundedRectangle(&rounded(br, 6.0 * s), &self.brush);
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.92));
                self.glyph_str(label, br, &self.formats.option);
            }
        }
    }
}
