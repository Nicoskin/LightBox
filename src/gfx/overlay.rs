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
    DWRITE_MEASURING_MODE_NATURAL, DWRITE_PARAGRAPH_ALIGNMENT_CENTER,
    DWRITE_PARAGRAPH_ALIGNMENT_NEAR, DWRITE_TEXT_ALIGNMENT, DWRITE_TEXT_ALIGNMENT_CENTER,
    DWRITE_TEXT_ALIGNMENT_LEADING, DWRITE_TEXT_METRICS, DWRITE_WORD_WRAPPING_NO_WRAP,
    DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory, IDWriteFactory, IDWriteTextFormat,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Dxgi::{IDXGIDevice, IDXGISurface, IDXGISwapChain1};
use windows::core::{BOOL, Interface as _, PCWSTR, w};

use std::collections::HashMap;
use std::rc::Rc;

use crate::view::Rect;

/// What to draw over the image.
#[derive(Default)]
pub struct Ui {
    /// UI scale (DPI / 96).
    pub scale: f32,
    /// Centred message, e.g. why a file could not be opened. First line bold.
    pub message: Option<String>,
    /// Short status in a pill near the bottom (e.g. the zoom level) and its
    /// opacity (for fading out).
    pub badge: Option<(String, f32)>,
    /// Extra distance of the badge from the bottom edge (above the panel).
    pub badge_lift: f32,
    /// Bottom panel and filmstrip.
    pub panel: Option<PanelView>,
    /// Thin border around the window (windowed mode).
    pub border: bool,
    /// Settings card on top of everything.
    pub card: Option<CardView>,
    /// Title bar (windowed) or the corner close button (fullscreen).
    pub caption: Option<CaptionView>,
    /// Pop-up menu (transparency background).
    pub menu: Option<MenuView>,
    /// Straightening dial.
    pub dial: Option<DialView>,
    /// Info panel (I).
    pub info: Option<InfoView>,
    /// Histogram overlay (H).
    pub histogram: Option<HistogramView>,
    /// Tooltip: the control it belongs to and the text.
    pub tooltip: Option<(Rect, String)>,
    /// Captions centred at the top of their rectangles (compare: A / B).
    pub labels: Vec<(Rect, String)>,
    /// Start screen buttons (`selected` = the main one).
    pub home: Vec<SegmentView>,
    /// The crop tool.
    pub crop: Option<CropView>,
    /// The adjustments card.
    pub adjust: Option<AdjustView>,
    /// Points picked on a model (the ruler), joined by a line.
    pub marks: Vec<(f64, f64)>,
}

impl Ui {
    pub fn is_empty(&self) -> bool {
        self.message.is_none()
            && self.badge.is_none()
            && self.panel.is_none()
            && !self.border
            && self.card.is_none()
            && self.caption.is_none()
            && self.menu.is_none()
            && self.dial.is_none()
            && self.info.is_none()
            && self.histogram.is_none()
            && self.tooltip.is_none()
            && self.labels.is_empty()
            && self.home.is_empty()
            && self.crop.is_none()
            && self.adjust.is_none()
            && self.marks.is_empty()
    }
}

/// Button icon.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Icon {
    /// Segoe Fluent Icons / MDL2 Assets glyph.
    Glyph(char),
    /// Glyph mirrored horizontally (e.g. counter-clockwise rotate).
    Mirrored(char),
    Text(&'static str),
    /// Drawn shapes for which no glyph fits.
    Checker,
    Smoothing,
    Filmstrip,
    /// An angle (straightening dial).
    Angle,
    /// Solid swatch of an sRGB grey level.
    Swatch(f32),
    /// A 3 × 3 grid (pixel grid).
    Grid,
    /// Three bars (histogram).
    Histogram,
}

/// Premultiplied BGRA thumbnail; `id` identifies it for bitmap caching.
pub struct Thumb {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

pub struct ButtonView {
    pub rect: Rect,
    pub icon: Icon,
    pub round: bool,
    pub hot: bool,
    pub pressed: bool,
    /// Toggle that is on.
    pub active: bool,
    /// Greyed out (e.g. "Save" with nothing to save).
    pub enabled: bool,
}

pub struct TextView {
    pub rect: Rect,
    pub text: String,
    pub style: TextStyle,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextStyle {
    /// Left-aligned file name.
    Caption,
    /// Left-aligned secondary line.
    Detail,
    /// Centred, e.g. the zoom percentage.
    Value,
}

pub struct CellView {
    pub rect: Rect,
    pub thumb: Option<Rc<Thumb>>,
    pub current: bool,
    pub hot: bool,
    /// Picked for batch rotation.
    pub selected: bool,
    /// The user's rotation of this photo (drawn turned).
    pub turn: crate::orientation::Orientation,
    /// Star rating (0 = none).
    pub stars: u8,
}

/// The settings card.
pub struct CardView {
    pub rect: Rect,
    pub title: (Rect, String),
    pub close: (Rect, bool),
    /// Page switcher (`selected` = the page shown).
    pub tabs: Vec<SegmentView>,
    pub labels: Vec<(Rect, String)>,
    /// Dim line at the bottom (version).
    pub footer: (Rect, String),
    /// Labels and segments are clipped to this (the scrolling keys page).
    pub clip: Option<Rect>,
    /// A button in the footer: rect, text, hot.
    pub button: Option<(Rect, String, bool)>,
    /// Segments: rect, text, selected, hot, first / last in its row.
    pub segments: Vec<SegmentView>,
}

pub struct SegmentView {
    pub rect: Rect,
    pub text: String,
    pub selected: bool,
    pub hot: bool,
    pub first: bool,
    pub last: bool,
}

pub struct PanelView {
    pub opacity: f32,
    pub bar: Rect,
    pub strip: Option<Rect>,
    pub buttons: Vec<ButtonView>,
    pub texts: Vec<TextView>,
    pub cells: Vec<CellView>,
    /// Colour swatch of the pixel under the cursor (straight sRGB).
    pub swatch: Option<(Rect, [u8; 4])>,
}

pub(super) struct Formats {
    title: IDWriteTextFormat,
    body: IDWriteTextFormat,
    badge: IDWriteTextFormat,
    pub(super) icon: IDWriteTextFormat,
    pub(super) small_icon: IDWriteTextFormat,
    pub(super) caption: IDWriteTextFormat,
    pub(super) detail: IDWriteTextFormat,
    pub(super) value: IDWriteTextFormat,
    pub(super) heading: IDWriteTextFormat,
    pub(super) label: IDWriteTextFormat,
    pub(super) option: IDWriteTextFormat,
    /// Left-aligned, top-aligned, wrapping (prompts).
    pub(super) wrap: IDWriteTextFormat,
    scale: f32,
}

pub struct Overlay {
    pub(super) ctx: ID2D1DeviceContext,
    target: Option<ID2D1Bitmap1>,
    pub(super) dwrite: IDWriteFactory,
    pub(super) formats: Formats,
    pub(super) brush: ID2D1SolidColorBrush,
    /// D2D bitmaps of filmstrip thumbnails, by [`Thumb::id`].
    pub(super) thumbs: HashMap<u64, ID2D1Bitmap1>,
}

pub(super) const fn rgba(r: f32, g: f32, b: f32, a: f32) -> D2D1_COLOR_F {
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
                thumbs: HashMap::new(),
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
            if !ui.home.is_empty() {
                self.home_buttons(&ui.home, ui.scale);
            }
            if !ui.marks.is_empty() {
                self.marks(&ui.marks, ui.scale);
            }
            if let Some(c) = &ui.crop {
                self.crop(c, ui.scale);
            }
            if let Some(a) = &ui.adjust {
                self.adjust_card(a, ui.scale);
            }
            if let Some(caption) = &ui.caption {
                self.caption(caption);
            }
            if let Some(panel) = &ui.panel {
                self.panel(panel, ui.scale)?;
            } else {
                self.thumbs.clear();
            }
            if let Some(dial) = &ui.dial {
                self.dial(dial, ui.scale)?;
            }
            if let Some(h) = &ui.histogram {
                self.histogram(h, ui.scale);
            }
            if let Some(info) = &ui.info {
                self.info(info, ui.scale);
            }
            if let Some(menu) = &ui.menu {
                self.menu(menu, ui.scale);
            }
            if let Some((text, opacity)) = &ui.badge {
                self.badge(text, *opacity, ui.scale, ui.badge_lift)?;
            }
            if ui.border {
                self.border(ui.scale);
            }
            if let Some(card) = &ui.card {
                self.card(card, ui.scale);
            }
            for (r, text) in &ui.labels {
                self.label(*r, text, ui.scale)?;
            }
            if let Some((anchor, text)) = &ui.tooltip {
                self.tooltip(*anchor, text, ui.scale)?;
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
    unsafe fn badge(&self, text: &str, opacity: f32, s: f32, lift: f32) -> Result<()> {
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
        let bottom = size.height - 40.0 * s - lift;
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
        let icons = icon_family(dwrite);
        let make = |family: PCWSTR,
                    weight: DWRITE_FONT_WEIGHT,
                    size: f32,
                    align: DWRITE_TEXT_ALIGNMENT|
         -> Result<IDWriteTextFormat> {
            // SAFETY: plain COM calls with static strings.
            unsafe {
                let f = dwrite.CreateTextFormat(
                    family,
                    None,
                    weight,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size * s,
                    w!(""),
                )?;
                f.SetTextAlignment(align)?;
                f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_CENTER)?;
                f.SetWordWrapping(DWRITE_WORD_WRAPPING_NO_WRAP)?;
                Ok(f)
            }
        };
        let ui = w!("Segoe UI");
        let (center, left) = (DWRITE_TEXT_ALIGNMENT_CENTER, DWRITE_TEXT_ALIGNMENT_LEADING);
        Ok(Self {
            title: make(ui, DWRITE_FONT_WEIGHT_SEMI_BOLD, 22.0, center)?,
            body: make(ui, DWRITE_FONT_WEIGHT_NORMAL, 15.0, center)?,
            badge: make(ui, DWRITE_FONT_WEIGHT_SEMI_BOLD, 15.0, center)?,
            icon: make(icons, DWRITE_FONT_WEIGHT_NORMAL, 16.0, center)?,
            small_icon: make(icons, DWRITE_FONT_WEIGHT_NORMAL, 10.0, center)?,
            caption: make(ui, DWRITE_FONT_WEIGHT_SEMI_BOLD, 14.0, left)?,
            detail: make(ui, DWRITE_FONT_WEIGHT_NORMAL, 12.0, left)?,
            value: make(ui, DWRITE_FONT_WEIGHT_SEMI_BOLD, 13.0, center)?,
            heading: make(ui, DWRITE_FONT_WEIGHT_SEMI_BOLD, 18.0, left)?,
            label: make(ui, DWRITE_FONT_WEIGHT_NORMAL, 13.5, left)?,
            option: make(ui, DWRITE_FONT_WEIGHT_NORMAL, 13.0, center)?,
            wrap: {
                let f = make(ui, DWRITE_FONT_WEIGHT_NORMAL, 12.5, left)?;
                // SAFETY: plain COM setters on a fresh format.
                unsafe {
                    f.SetParagraphAlignment(DWRITE_PARAGRAPH_ALIGNMENT_NEAR)?;
                    f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP)?;
                }
                f
            },
            scale: s,
        })
    }
}

/// "Segoe Fluent Icons" (Windows 11) or "Segoe MDL2 Assets" (Windows 10);
/// both use the same code points for the glyphs we need.
fn icon_family(dwrite: &IDWriteFactory) -> PCWSTR {
    let fluent = w!("Segoe Fluent Icons");
    let mut collection = None;
    // SAFETY: out-pointers are valid for the calls.
    let found = unsafe {
        dwrite
            .GetSystemFontCollection(&mut collection, false)
            .ok()
            .and(collection)
            .is_some_and(|c| {
                let (mut index, mut exists) = (0u32, BOOL(0));
                c.FindFamilyName(fluent, &mut index, &mut exists).is_ok() && exists.as_bool()
            })
    };
    if found {
        fluent
    } else {
        w!("Segoe MDL2 Assets")
    }
}

/// Title bar / corner close button.
pub struct CaptionView {
    /// Windowed title bar background.
    pub bar: Option<Rect>,
    pub title: Option<(Rect, String)>,
    pub buttons: Vec<CaptionButtonView>,
    /// Fullscreen style: round buttons over the picture.
    pub round: bool,
    pub opacity: f32,
}

pub struct CaptionButtonView {
    pub rect: Rect,
    pub glyph: char,
    /// The close button (red when hovered).
    pub close: bool,
    pub hot: bool,
    pub pressed: bool,
    /// A toggle that is on (always on top).
    pub active: bool,
}

pub struct MenuView {
    pub rect: Rect,
    pub items: Vec<MenuItemView>,
}

pub struct MenuItemView {
    pub rect: Rect,
    pub label: String,
    /// sRGB grey of a solid swatch; `None` = checkerboard.
    pub swatch: Option<f32>,
    pub selected: bool,
    pub hot: bool,
}

pub struct DialView {
    pub rect: Rect,
    pub ruler: Rect,
    pub reset: Rect,
    /// Current angle, degrees, and its label.
    pub angle: f64,
    pub label: String,
    /// Ruler spacing, screen pixels per degree.
    pub px_per_deg: f64,
    pub reset_hot: bool,
    pub opacity: f32,
}

/// Info panel: rows of label / value, headings, wrapped text blocks and
/// buttons.
pub struct InfoView {
    pub card: Rect,
    pub title: (Rect, String),
    /// Label rect, label, value rect, value.
    pub rows: Vec<(Rect, String, Rect, String)>,
    pub headings: Vec<(Rect, String)>,
    /// Wrapped text in a box (clipped).
    pub blocks: Vec<(Rect, String)>,
    /// Rect, label, hot.
    pub buttons: Vec<(Rect, String, bool)>,
    /// GPS map: rect, image (once rendered), status text.
    pub map: Option<(Rect, Option<Rc<Thumb>>, String)>,
}

/// Histogram overlay.
pub struct HistogramView {
    pub rect: Rect,
    pub hist: Rc<crate::inspect::Histogram>,
}

/// The crop tool: the photo and the frame on screen, the frame's size in
/// pixels, the bar (aspects, apply, cancel).
pub struct CropView {
    pub image: Rect,
    pub frame: Rect,
    pub size: String,
    pub segments: Vec<SegmentView>,
}

/// One slider of the adjustments card.
pub struct SliderView {
    pub label: (Rect, String),
    pub track: Rect,
    /// Screen x of the knob and of the zero mark.
    pub knob: f64,
    pub zero: f64,
    pub value: (Rect, String),
    pub hot: bool,
}

/// The adjustments card (A).
pub struct AdjustView {
    pub card: Rect,
    pub sliders: Vec<SliderView>,
    pub note: (Rect, String),
    pub buttons: Vec<SegmentView>,
}
