//! Direct2D / DirectWrite drawing on top of the image (UI overlays).
//!
//! Shares the D3D11 device and draws straight into the swap chain's back
//! buffer. Created lazily on first use: DirectWrite start-up (font
//! collection) costs several milliseconds that must not delay the first frame.

use anyhow::{Context as _, Result};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_RECT_F, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_BITMAP_OPTIONS_CANNOT_DRAW, D2D1_BITMAP_OPTIONS_TARGET, D2D1_BITMAP_PROPERTIES1,
    D2D1_DEVICE_CONTEXT_OPTIONS_NONE, D2D1_DRAW_TEXT_OPTIONS_NONE,
    D2D1_FACTORY_TYPE_SINGLE_THREADED, D2D1_ROUNDED_RECT, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE,
    D2D1CreateFactory, ID2D1Bitmap1, ID2D1DeviceContext, ID2D1Factory1, ID2D1SolidColorBrush,
};
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT, DWRITE_FONT_WEIGHT_NORMAL, DWRITE_FONT_WEIGHT_SEMI_BOLD,
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_METRICS, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface, IDXGISwapChain1};
use windows::core::{Interface as _, w};

/// What to draw over the image.
#[derive(Clone, Debug, Default)]
pub struct Ui {
    /// UI scale (DPI / 96).
    pub scale: f32,
    /// Centred message, e.g. why a file could not be opened. First line bold.
    pub message: Option<String>,
    /// Short status in a pill near the bottom (e.g. the zoom level) and its
    /// opacity (for fading out).
    pub badge: Option<(String, f32)>,
}

impl Ui {
    pub fn is_empty(&self) -> bool {
        self.message.is_none() && self.badge.is_none()
    }
}

struct Formats {
    title: IDWriteTextFormat,
    body: IDWriteTextFormat,
    badge: IDWriteTextFormat,
    scale: f32,
}

pub struct Overlay {
    ctx: ID2D1DeviceContext,
    target: Option<ID2D1Bitmap1>,
    dwrite: IDWriteFactory,
    formats: Formats,
    brush: ID2D1SolidColorBrush,
}

const fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
    D2D1_COLOR_F { r, g, b, a }
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().collect()
}

impl Overlay {
    pub fn new(device: &ID3D11Device, scale: f32) -> Result<Self> {
        // SAFETY: plain COM object creation; all arguments outlive the calls.
        unsafe {
            let factory: ID2D1Factory1 =
                D2D1CreateFactory(D2D1_FACTORY_TYPE_SINGLE_THREADED, None)?;
            let dxgi: IDXGIDevice = device.cast()?;
            let d2d_device = factory.CreateDevice(&dxgi)?;
            let ctx = d2d_device.CreateDeviceContext(D2D1_DEVICE_CONTEXT_OPTIONS_NONE)?;
            // Work in physical pixels; the caller scales by DPI.
            ctx.SetDpi(96.0, 96.0);
            // ClearType needs an opaque target; ours is transparent.
            ctx.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            let dwrite: IDWriteFactory = DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED)?;
            let formats = Formats::new(&dwrite, scale)?;
            let brush = ctx.CreateSolidColorBrush(&rgba(1.0, 1.0, 1.0, 1.0), None)?;
            Ok(Self {
                ctx,
                target: None,
                dwrite,
                formats,
                brush,
            })
        }
    }

    /// Drops the reference to the back buffer (required before ResizeBuffers).
    pub fn release_target(&mut self) {
        // SAFETY: valid context; unbinding the target.
        unsafe { self.ctx.SetTarget(None) };
        self.target = None;
    }

    pub fn draw(&mut self, swap_chain: &IDXGISwapChain1, ui: &Ui) -> Result<()> {
        if ui.is_empty() {
            return Ok(());
        }
        if (ui.scale - self.formats.scale).abs() > f32::EPSILON {
            self.formats = Formats::new(&self.dwrite, ui.scale)?;
        }
        // SAFETY: the target bitmap wraps the current back buffer; drawing
        // happens between BeginDraw/EndDraw on the UI thread.
        unsafe {
            if self.target.is_none() {
                let surface: IDXGISurface = swap_chain.GetBuffer(0)?;
                let props = D2D1_BITMAP_PROPERTIES1 {
                    pixelFormat: D2D1_PIXEL_FORMAT {
                        format: DXGI_FORMAT_B8G8R8A8_UNORM,
                        alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                    },
                    dpiX: 96.0,
                    dpiY: 96.0,
                    bitmapOptions: D2D1_BITMAP_OPTIONS_TARGET | D2D1_BITMAP_OPTIONS_CANNOT_DRAW,
                    ..Default::default()
                };
                let bmp = self
                    .ctx
                    .CreateBitmapFromDxgiSurface(&surface, Some(&props))
                    .context("CreateBitmapFromDxgiSurface")?;
                self.ctx.SetTarget(&bmp);
                self.target = Some(bmp);
            }
            self.ctx.BeginDraw();
            if let Some(msg) = &ui.message {
                self.message(msg, ui.scale);
            }
            if let Some((text, opacity)) = &ui.badge {
                self.badge(text, *opacity, ui.scale)?;
            }
            self.ctx.EndDraw(None, None)?;
        }
        Ok(())
    }

    /// Centred multi-line message; the first line is the title.
    unsafe fn message(&self, msg: &str, s: f32) {
        let size = unsafe { self.ctx.GetSize() };
        let (title, rest) = msg.split_once('\n').unwrap_or((msg, ""));
        let cy = size.height / 2.0;
        let lines = [
            (title, &self.formats.title, cy - 18.0 * s),
            (rest, &self.formats.body, cy + 22.0 * s),
        ];
        for (text, format, y) in lines {
            if text.is_empty() {
                continue;
            }
            let text = wide(text);
            let rect = |dy: f32| D2D_RECT_F {
                left: 0.0,
                top: y - 40.0 * s + dy,
                right: size.width,
                bottom: y + 40.0 * s + dy,
            };
            // Soft shadow for legibility over any photo / desktop.
            for (dy, color) in [
                (1.5 * s, rgba(0.0, 0.0, 0.0, 0.7)),
                (0.0, rgba(1.0, 1.0, 1.0, 0.95)),
            ] {
                unsafe {
                    self.brush.SetColor(&color);
                    self.ctx.DrawText(
                        &text,
                        format,
                        &rect(dy),
                        &self.brush,
                        D2D1_DRAW_TEXT_OPTIONS_NONE,
                        DWRITE_MEASURING_MODE_NATURAL,
                    );
                }
            }
        }
    }

    /// Rounded dark pill with white text, centred near the bottom edge.
    unsafe fn badge(&self, text: &str, opacity: f32, s: f32) -> Result<()> {
        let size = unsafe { self.ctx.GetSize() };
        let text = wide(text);
        let mut m = DWRITE_TEXT_METRICS::default();
        unsafe {
            let layout = self.dwrite.CreateTextLayout(
                &text,
                &self.formats.badge,
                size.width,
                size.height,
            )?;
            layout.GetMetrics(&mut m)?;
        }
        let (pad_x, h) = (14.0 * s, 32.0 * s);
        let w = m.width + 2.0 * pad_x;
        let cx = size.width / 2.0;
        let bottom = size.height - 40.0 * s;
        let rect = D2D_RECT_F {
            left: cx - w / 2.0,
            top: bottom - h,
            right: cx + w / 2.0,
            bottom,
        };
        unsafe {
            self.brush.SetColor(&rgba(0.08, 0.08, 0.08, 0.72 * opacity));
            self.ctx.FillRoundedRectangle(
                &D2D1_ROUNDED_RECT {
                    rect,
                    radiusX: h / 2.0,
                    radiusY: h / 2.0,
                },
                &self.brush,
            );
            self.brush.SetColor(&rgba(1.0, 1.0, 1.0, 0.95 * opacity));
            self.ctx.DrawText(
                &text,
                &self.formats.badge,
                &rect,
                &self.brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
                DWRITE_MEASURING_MODE_NATURAL,
            );
        }
        Ok(())
    }
}

impl Formats {
    fn new(dwrite: &IDWriteFactory, s: f32) -> Result<Self> {
        let make = |weight: DWRITE_FONT_WEIGHT, size: f32| -> Result<IDWriteTextFormat> {
            // SAFETY: plain COM calls with static strings.
            unsafe {
                let f = dwrite.CreateTextFormat(
                    w!("Segoe UI"),
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size * s,
                    w!(""),
                )?;
                f.SetTextAlignment(DWRITE_TEXT_ALIGNMENT_CENTER)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                Ok(f)
            }
        };
        Ok(Self {
            title: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 22.0)?,
            body: make(DWRITE_FONT_WEIGHT_NORMAL, 15.0)?,
            badge: make(DWRITE_FONT_WEIGHT_SEMI_BOLD, 15.0)?,
            scale: s,
        })
    }
}
