//! Drawing of the bottom panel, the filmstrip and the window border.

use std::collections::HashMap;

use anyhow::Result;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_ANTIALIAS_MODE_PER_PRIMITIVE, D2D1_BITMAP_OPTIONS_NONE, D2D1_BITMAP_PROPERTIES1,
    D2D1_DRAW_TEXT_OPTIONS_CLIP, D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_ELLIPSE,
    D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC, D2D1_ROUNDED_RECT, ID2D1StrokeStyle,
};
use windows::Win32::Graphics::DirectWrite::{DWRITE_MEASURING_MODE_NATURAL, IDWriteTextFormat};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows_numerics::{Matrix3x2, Vector2};

use super::overlay::{
    ButtonView, CardView, CellView, Icon, Overlay, PanelView, TextStyle, TextView, rgba,
};
use crate::view::Rect;

pub(super) fn rf(r: Rect) -> D2D_RECT_F {
    D2D_RECT_F {
        left: r.x0 as f32,
        top: r.y0 as f32,
        right: r.x1 as f32,
        bottom: r.y1 as f32,
    }
}

impl Overlay {
    pub(super) unsafe fn panel(&mut self, p: &PanelView, s: f32) -> Result<()> {
        let o = p.opacity;
        unsafe {
            if let Some(strip) = p.strip {
                self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.42 * o));
                self.ctx.FillRectangle(&rf(strip), &self.brush);
            }
            self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.62 * o));
            self.ctx.FillRectangle(&rf(p.bar), &self.brush);
            // Hairline on top of the bar.
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.07 * o));
            let mut line = p.bar;
            line.y1 = line.y0 + f64::from(s.max(1.0));
            self.ctx.FillRectangle(&rf(line), &self.brush);
        }
        if let Some(strip) = p.strip {
            // Cells glide in and out at the strip's ends.
            // SAFETY: inside BeginDraw/EndDraw; popped right after.
            unsafe {
                self.ctx
                    .PushAxisAlignedClip(&rf(strip), D2D1_ANTIALIAS_MODE_PER_PRIMITIVE);
            }
            let r = self.cells(&p.cells, o, s);
            // SAFETY: matches the push above.
            unsafe { self.ctx.PopAxisAlignedClip() };
            r?;
        }
        for b in &p.buttons {
            // SAFETY: inside BeginDraw/EndDraw (see `Overlay::draw`).
            unsafe { self.button(b, o, s) };
        }
        for t in &p.texts {
            // SAFETY: as above.
            unsafe { self.text(t, o) };
        }
        Ok(())
    }

    unsafe fn button(&self, b: &ButtonView, o: f32, s: f32) {
        let r = rf(b.rect);
        let (cx, cy) = b.rect.center();
        let (cx, cy) = (cx as f32, cy as f32);
        if !b.enabled {
            unsafe {
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.28 * o));
                self.icon(b, r, cx, cy, o, s, 0.28);
            }
            return;
        }
        let back = if b.pressed {
            0.28
        } else if b.hot {
            0.18
        } else if b.round {
            0.08
        } else {
            0.0
        };
        unsafe {
            if b.active {
                // Toggle on: soft accent.
                self.brush.SetColor(&rgba(0.30, 0.56, 1.0, 0.45 * o));
                self.fill_shape(r, b.round);
            }
            if back > 0.0 {
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, back * o));
                self.fill_shape(r, b.round);
            }
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.92 * o));
            self.icon(b, r, cx, cy, o, s, 0.92);
        }
    }

    /// Draws the icon of `b` in the current brush colour; `tone` (the icon
    /// alpha) also scales the multi-tone shape icons.
    #[allow(clippy::too_many_arguments)]
    unsafe fn icon(
        &self,
        b: &ButtonView,
        r: D2D_RECT_F,
        cx: f32,
        cy: f32,
        o: f32,
        s: f32,
        tone: f32,
    ) {
        let o = o * tone / 0.92;
        unsafe {
            match b.icon {
                Icon::Glyph(c) => self.glyph(c, r, &self.formats.icon),
                Icon::Mirrored(c) => {
                    // Mirror about the button's vertical centre line.
                    let m = Matrix3x2 {
                        M11: -1.0,
                        M12: 0.0,
                        M21: 0.0,
                        M22: 1.0,
                        M31: 2.0 * cx,
                        M32: 0.0,
                    };
                    self.ctx.SetTransform(&m);
                    self.glyph(c, r, &self.formats.icon);
                    self.ctx.SetTransform(&Matrix3x2::identity());
                }
                Icon::Text(t) => self.glyph_str(t, r, &self.formats.value),
                Icon::Checker => {
                    let cell = 4.0 * s;
                    for i in 0..4 {
                        for j in 0..4 {
                            let a = if (i + j) % 2 == 0 { 0.95 } else { 0.35 };
                            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, a * o));
                            let x = cx - 2.0 * cell + i as f32 * cell;
                            let y = cy - 2.0 * cell + j as f32 * cell;
                            self.ctx.FillRectangle(
                                &D2D_RECT_F {
                                    left: x,
                                    top: y,
                                    right: x + cell,
                                    bottom: y + cell,
                                },
                                &self.brush,
                            );
                        }
                    }
                }
                Icon::Smoothing => {
                    // A pixel staircase with a smooth curve over it.
                    let px = 3.0 * s;
                    self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.45 * o));
                    for k in 0..5 {
                        let x = cx - 7.5 * s + k as f32 * px;
                        let y = cy + 4.5 * s - k as f32 * px;
                        self.ctx.FillRectangle(
                            &D2D_RECT_F {
                                left: x,
                                top: y,
                                right: x + px,
                                bottom: y + px,
                            },
                            &self.brush,
                        );
                    }
                    self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95 * o));
                    self.ctx.DrawLine(
                        Vector2 {
                            X: cx - 7.0 * s,
                            Y: cy + 7.0 * s,
                        },
                        Vector2 {
                            X: cx + 7.0 * s,
                            Y: cy - 7.0 * s,
                        },
                        &self.brush,
                        1.6 * s,
                        None::<&ID2D1StrokeStyle>,
                    );
                }
                Icon::Angle => {
                    // "∠": a base line and a slanted arm with a small arc.
                    let (x0, y0) = (cx - 8.0 * s, cy + 6.0 * s);
                    let w = 1.6 * s;
                    let none = None::<&ID2D1StrokeStyle>;
                    let p = |x: f32, y: f32| Vector2 { X: x, Y: y };
                    self.ctx
                        .DrawLine(p(x0, y0), p(cx + 8.0 * s, y0), &self.brush, w, none);
                    self.ctx.DrawLine(
                        p(x0, y0),
                        p(cx + 6.0 * s, cy - 7.0 * s),
                        &self.brush,
                        w,
                        none,
                    );
                    self.ctx.DrawLine(
                        p(cx + 1.0 * s, y0),
                        p(cx - 0.5 * s, cy + 0.8 * s),
                        &self.brush,
                        w * 0.8,
                        none,
                    );
                }
                Icon::Swatch(g) => {
                    let h = 7.0 * s;
                    let r = D2D_RECT_F {
                        left: cx - h,
                        top: cy - h,
                        right: cx + h,
                        bottom: cy + h,
                    };
                    self.brush.SetColor(&rgba(g, g, g, o));
                    self.ctx.FillRectangle(&r, &self.brush);
                    self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.6 * o));
                    self.ctx
                        .DrawRectangle(&r, &self.brush, s, None::<&ID2D1StrokeStyle>);
                }
                Icon::Filmstrip => {
                    let (w, h, gap) = (5.0 * s, 8.0 * s, 1.5 * s);
                    for k in -1..=1 {
                        let x = cx + k as f32 * (w + gap) - w / 2.0;
                        self.ctx.FillRoundedRectangle(
                            &D2D1_ROUNDED_RECT {
                                rect: D2D_RECT_F {
                                    left: x,
                                    top: cy - h / 2.0,
                                    right: x + w,
                                    bottom: cy + h / 2.0,
                                },
                                radiusX: s,
                                radiusY: s,
                            },
                            &self.brush,
                        );
                    }
                }
            }
        }
    }

    pub(super) unsafe fn fill_shape(&self, r: D2D_RECT_F, round: bool) {
        unsafe {
            if round {
                let rx = (r.right - r.left) / 2.0;
                self.ctx.FillEllipse(
                    &D2D1_ELLIPSE {
                        point: Vector2 {
                            X: r.left + rx,
                            Y: r.top + (r.bottom - r.top) / 2.0,
                        },
                        radiusX: rx,
                        radiusY: (r.bottom - r.top) / 2.0,
                    },
                    &self.brush,
                );
            } else {
                let rad = (r.bottom - r.top) * 0.18;
                self.ctx.FillRoundedRectangle(
                    &D2D1_ROUNDED_RECT {
                        rect: r,
                        radiusX: rad,
                        radiusY: rad,
                    },
                    &self.brush,
                );
            }
        }
    }

    pub(super) unsafe fn glyph(&self, c: char, r: D2D_RECT_F, format: &IDWriteTextFormat) {
        let mut buf = [0u16; 2];
        let text = c.encode_utf16(&mut buf);
        unsafe {
            self.ctx.DrawText(
                text,
                format,
                &r,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    pub(super) unsafe fn glyph_str(&self, t: &str, r: D2D_RECT_F, format: &IDWriteTextFormat) {
        let text: Vec<u16> = t.encode_utf16().collect();
        unsafe {
            self.ctx.DrawText(
                &text,
                format,
                &r,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_CLIP,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
    }

    unsafe fn text(&self, t: &TextView, o: f32) {
        let (format, alpha) = match t.style {
            TextStyle::Caption => (&self.formats.caption, 0.95),
            TextStyle::Detail => (&self.formats.detail, 0.65),
            TextStyle::Value => (&self.formats.value, 0.92),
        };
        unsafe {
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, alpha * o));
            self.glyph_str(&t.text, rf(t.rect), format);
        }
    }

    /// Filmstrip thumbnails, aspect-fitted into their cells.
    fn cells(&mut self, cells: &[CellView], o: f32, s: f32) -> Result<()> {
        let mut used = HashMap::with_capacity(cells.len());
        for c in cells {
            let r = rf(c.rect);
            let bitmap = match &c.thumb {
                Some(t) => {
                    let bmp = match self.thumbs.remove(&t.id) {
                        Some(b) => b,
                        None => {
                            let props = D2D1_BITMAP_PROPERTIES1 {
                                pixelFormat: D2D1_PIXEL_FORMAT {
                                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                                },
                                dpiX: 96.0,
                                dpiY: 96.0,
                                bitmapOptions: D2D1_BITMAP_OPTIONS_NONE,
                                ..Default::default()
                            };
                            // SAFETY: the pixel buffer matches width × height × 4.
                            unsafe {
                                self.ctx.CreateBitmap(
                                    D2D_SIZE_U {
                                        width: t.width,
                                        height: t.height,
                                    },
                                    Some(t.bgra.as_ptr().cast()),
                                    t.width * 4,
                                    &props,
                                )?
                            }
                        }
                    };
                    used.insert(t.id, bmp.clone());
                    Some((bmp, t.width, t.height))
                }
                None => None,
            };
            // SAFETY: inside BeginDraw/EndDraw.
            unsafe {
                // Cell backdrop.
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.06 * o));
                self.ctx.FillRectangle(&r, &self.brush);
                let mut frame = r;
                if let Some((bmp, w, h)) = bitmap {
                    let (cw, ch) = (r.right - r.left, r.bottom - r.top);
                    let k = (cw / w as f32).min(ch / h as f32);
                    let (dw, dh) = (w as f32 * k, h as f32 * k);
                    let dst = D2D_RECT_F {
                        left: r.left + (cw - dw) / 2.0,
                        top: r.top + (ch - dh) / 2.0,
                        right: r.left + (cw + dw) / 2.0,
                        bottom: r.top + (ch + dh) / 2.0,
                    };
                    let dim = if c.current || c.hot { 1.0 } else { 0.72 };
                    self.ctx.DrawBitmap(
                        &bmp,
                        Some(&dst),
                        o * dim,
                        D2D1_INTERPOLATION_MODE_HIGH_QUALITY_CUBIC,
                        None,
                        None,
                    );
                    frame = dst;
                }
                if c.current || c.hot {
                    let a = if c.current { 0.95 } else { 0.5 };
                    self.brush.SetColor(&rgba(1.0, 1.0, 1.0, a * o));
                    let w = if c.current { 2.0 * s } else { s };
                    let inset = w / 2.0;
                    self.ctx.DrawRectangle(
                        &D2D_RECT_F {
                            left: frame.left - inset,
                            top: frame.top - inset,
                            right: frame.right + inset,
                            bottom: frame.bottom + inset,
                        },
                        &self.brush,
                        w,
                        None::<&ID2D1StrokeStyle>,
                    );
                }
            }
        }
        // Keep bitmaps of visible thumbnails only.
        self.thumbs = used;
        Ok(())
    }

    /// One-pixel light border just inside the window edge.
    pub(super) unsafe fn border(&self, s: f32) {
        let size = unsafe { self.ctx.GetSize() };
        let w = s.max(1.0).round();
        unsafe {
            self.ctx.PushAxisAlignedClip(
                &D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: size.width,
                    bottom: size.height,
                },
                D2D1_ANTIALIAS_MODE_PER_PRIMITIVE,
            );
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.22));
            self.ctx.DrawRectangle(
                &D2D_RECT_F {
                    left: w / 2.0,
                    top: w / 2.0,
                    right: size.width - w / 2.0,
                    bottom: size.height - w / 2.0,
                },
                &self.brush,
                w,
                None::<&ID2D1StrokeStyle>,
            );
            self.ctx.PopAxisAlignedClip();
        }
    }
}

impl Overlay {
    /// The settings card: dark rounded panel, title, close button and rows
    /// of segmented choices.
    pub(super) unsafe fn card(&self, c: &CardView, s: f32) {
        let r = rf(c.rect);
        let radius = 12.0 * s;
        let rounded = |rect: D2D_RECT_F, rad: f32| D2D1_ROUNDED_RECT {
            rect,
            radiusX: rad,
            radiusY: rad,
        };
        unsafe {
            // Dim everything behind the card.
            let size = self.ctx.GetSize();
            self.brush.SetColor(&rgba(0.0, 0.0, 0.0, 0.35));
            self.ctx.FillRectangle(
                &D2D_RECT_F {
                    left: 0.0,
                    top: 0.0,
                    right: size.width,
                    bottom: size.height,
                },
                &self.brush,
            );
            // Soft shadow, then the card.
            for (grow, a) in [(10.0, 0.10), (5.0, 0.16), (2.0, 0.22)] {
                let g = grow * s;
                self.brush.SetColor(&rgba(0.0, 0.0, 0.0, a));
                self.ctx.FillRoundedRectangle(
                    &rounded(
                        D2D_RECT_F {
                            left: r.left - g,
                            top: r.top - g + 4.0 * s,
                            right: r.right + g,
                            bottom: r.bottom + g + 4.0 * s,
                        },
                        radius + g,
                    ),
                    &self.brush,
                );
            }
            self.brush.SetColor(&rgba(0.125, 0.125, 0.135, 0.98));
            self.ctx
                .FillRoundedRectangle(&rounded(r, radius), &self.brush);
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.08));
            self.ctx.DrawRoundedRectangle(
                &rounded(r, radius),
                &self.brush,
                s,
                None::<&ID2D1StrokeStyle>,
            );

            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95));
            self.glyph_str(&c.title.1, rf(c.title.0), &self.formats.heading);

            let (close, hot) = (rf(c.close.0), c.close.1);
            if hot {
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.14));
                self.ctx
                    .FillRoundedRectangle(&rounded(close, 6.0 * s), &self.brush);
            }
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.85));
            self.glyph('\u{E711}', close, &self.formats.icon);

            for (rect, text) in &c.labels {
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.78));
                self.glyph_str(text, rf(*rect), &self.formats.label);
            }
            for seg in &c.segments {
                let sr = rf(seg.rect);
                let inner = D2D_RECT_F {
                    left: sr.left + if seg.first { 0.0 } else { 1.0 },
                    top: sr.top,
                    right: sr.right - if seg.last { 0.0 } else { 1.0 },
                    bottom: sr.bottom,
                };
                let (fill, text_a) = if seg.selected {
                    (rgba(0.30, 0.56, 1.0, 0.92), 1.0)
                } else if seg.hot {
                    (rgba(1.0, 1.0, 1.0, 0.13), 0.95)
                } else {
                    (rgba(1.0, 1.0, 1.0, 0.06), 0.75)
                };
                self.brush.SetColor(&fill);
                self.ctx
                    .FillRoundedRectangle(&rounded(inner, 6.0 * s), &self.brush);
                self.brush.SetColor(&rgba(1.0, 1.0, 1.0, text_a));
                self.glyph_str(&seg.text, inner, &self.formats.option);
            }
        }
    }
}
