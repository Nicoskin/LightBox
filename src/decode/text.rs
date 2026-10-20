//! Text files as documents: the text is laid out with DirectWrite on
//! A4-sized pages and a page is drawn into a bitmap, so that a text file is
//! viewed like a PDF (fit, zoom, page by page). Code and data files get a
//! monospaced font. Runs on worker threads.

use std::path::Path;

use windows::Win32::Globalization::{MULTI_BYTE_TO_WIDE_CHAR_FLAGS, MultiByteToWideChar};
use windows::Win32::Graphics::Direct2D::Common::{
    D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_COLOR_F, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{
    D2D1_DRAW_TEXT_OPTIONS_NONE, D2D1_FACTORY_TYPE_MULTI_THREADED, D2D1_RENDER_TARGET_PROPERTIES,
    D2D1_RENDER_TARGET_TYPE_SOFTWARE, D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE, D2D1CreateFactory,
    ID2D1Factory,
};
use windows::Win32::Graphics::DirectWrite::{
    DWRITE_FACTORY_TYPE_SHARED, DWRITE_FONT_STRETCH_NORMAL, DWRITE_FONT_STYLE_NORMAL,
    DWRITE_FONT_WEIGHT_NORMAL, DWRITE_LINE_METRICS, DWRITE_WORD_WRAPPING_WRAP, DWriteCreateFactory,
    IDWriteFactory, IDWriteTextFormat, IDWriteTextLayout,
};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICImagingFactory,
    WICBitmapCacheOnLoad,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx,
};
use windows::core::w;
use windows_numerics::Matrix3x2;

use super::{DecodeError, Raw, RawImage, SizeHint};
use crate::i18n::tr;

/// Extensions shown as text documents (lower case).
pub const EXTENSIONS: &[&str] = &[
    "txt",
    "text",
    "log",
    "md",
    "markdown",
    "rst",
    "nfo",
    "srt",
    "csv",
    "tsv",
    "json",
    "jsonl",
    "xml",
    "yaml",
    "yml",
    "toml",
    "ini",
    "cfg",
    "conf",
    "properties",
    "env",
    "rs",
    "py",
    "js",
    "mjs",
    "ts",
    "tsx",
    "jsx",
    "c",
    "h",
    "cc",
    "cpp",
    "hpp",
    "cs",
    "java",
    "kt",
    "go",
    "rb",
    "php",
    "lua",
    "sh",
    "bash",
    "ps1",
    "psm1",
    "bat",
    "cmd",
    "sql",
    "html",
    "htm",
    "css",
    "scss",
    "vue",
    "gcode",
    "nc",
    "diff",
    "patch",
    "gitignore",
];

/// Prose: a proportional font; everything else is code or data.
const PROSE: &[&str] = &["txt", "text", "md", "markdown", "rst", "nfo", "srt"];

/// Most text read (UTF-16 units after decoding): longer files show their
/// beginning.
const MAX_CHARS: usize = 1_000_000;

/// A4 at 96 DPI, with margins, in DIPs.
pub const PAGE: (f64, f64) = (794.0, 1123.0);
const MARGIN_X: f32 = 60.0;
const MARGIN_Y: f32 = 64.0;

const MAX_SIDE: f64 = 8192.0;
const MIN_SIDE: f64 = 1600.0;

fn err(e: windows::core::Error) -> DecodeError {
    DecodeError::Other(crate::t!("текст: {}", e.message()))
}

/// Bytes → UTF-16: BOMs (UTF-8, UTF-16 LE/BE), valid UTF-8, otherwise the
/// Cyrillic Windows code page (1251). Returns the text and the encoding.
pub fn decode_bytes(data: &[u8]) -> (Vec<u16>, &'static str) {
    if let Some(rest) = data.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        return (
            String::from_utf8_lossy(rest).encode_utf16().collect(),
            "UTF-8",
        );
    }
    if let Some(rest) = data.strip_prefix(&[0xFF, 0xFE]) {
        let v = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_le_bytes(*b))
            .collect();
        return (v, "UTF-16");
    }
    if let Some(rest) = data.strip_prefix(&[0xFE, 0xFF]) {
        let v = rest
            .as_chunks::<2>()
            .0
            .iter()
            .map(|b| u16::from_be_bytes(*b))
            .collect();
        return (v, "UTF-16 BE");
    }
    // Cut at a character boundary: a long file may stop mid-sequence.
    let mut end = data.len();
    while end > 0
        && end > data.len().saturating_sub(4)
        && std::str::from_utf8(&data[..end]).is_err()
    {
        end -= 1;
    }
    if let Ok(s) = std::str::from_utf8(&data[..end]) {
        return (s.encode_utf16().collect(), "UTF-8");
    }
    // SAFETY: plain conversion into a buffer of the size it asked for.
    unsafe {
        let n = MultiByteToWideChar(1251, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), data, None);
        let mut v = vec![0u16; n.max(0) as usize];
        MultiByteToWideChar(1251, MULTI_BYTE_TO_WIDE_CHAR_FLAGS(0), data, Some(&mut v));
        (v, "Windows-1251")
    }
}

/// A text file laid out on pages.
pub struct TextDoc {
    text: Vec<u16>,
    /// Each page's range in `text`.
    pages: Vec<(usize, usize)>,
    mono: bool,
}

/// DirectWrite / Direct2D / WIC objects of the current thread.
struct Ctx {
    dwrite: IDWriteFactory,
    d2d: ID2D1Factory,
    wic: IWICImagingFactory,
}

impl Ctx {
    fn new() -> Result<Self, DecodeError> {
        // SAFETY: COM for this worker thread; factory creation has no other
        // preconditions.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            Ok(Self {
                dwrite: DWriteCreateFactory(DWRITE_FACTORY_TYPE_SHARED).map_err(err)?,
                d2d: D2D1CreateFactory(D2D1_FACTORY_TYPE_MULTI_THREADED, None).map_err(err)?,
                wic: CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)
                    .map_err(err)?,
            })
        }
    }

    fn format(&self, mono: bool) -> Result<IDWriteTextFormat, DecodeError> {
        let (family, size) = if mono {
            (w!("Cascadia Mono"), 13.0)
        } else {
            (w!("Segoe UI"), 15.0)
        };
        // SAFETY: plain factory call with static strings.
        unsafe {
            let f = self
                .dwrite
                .CreateTextFormat(
                    family,
                    None,
                    DWRITE_FONT_WEIGHT_NORMAL,
                    DWRITE_FONT_STYLE_NORMAL,
                    DWRITE_FONT_STRETCH_NORMAL,
                    size,
                    w!("ru-ru"),
                )
                .map_err(err)?;
            f.SetWordWrapping(DWRITE_WORD_WRAPPING_WRAP).map_err(err)?;
            f.SetIncrementalTabStop(size * 0.6 * 4.0).map_err(err)?;
            Ok(f)
        }
    }

    fn layout(&self, text: &[u16], mono: bool) -> Result<IDWriteTextLayout, DecodeError> {
        let f = self.format(mono)?;
        // SAFETY: `text` outlives the call (the layout copies it).
        unsafe {
            self.dwrite
                .CreateTextLayout(text, &f, PAGE.0 as f32 - 2.0 * MARGIN_X, f32::MAX)
                .map_err(err)
        }
    }
}

impl TextDoc {
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        let ext = path
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        let mut file = std::fs::File::open(path)?;
        let mut data = Vec::new();
        std::io::Read::read_to_end(
            &mut std::io::Read::take(&mut file, (MAX_CHARS * 4) as u64),
            &mut data,
        )?;
        let len = std::fs::metadata(path).map_or(0, |m| m.len());
        let (mut text, _) = decode_bytes(&data);
        let mut truncated = len > data.len() as u64;
        if text.len() > MAX_CHARS {
            text.truncate(MAX_CHARS);
            truncated = true;
        }
        // Windows line ends count as one.
        text.retain(|&c| c != u16::from(b'\r'));
        if truncated {
            text.extend(tr("\n\n… файл длиннее: показано его начало").encode_utf16());
        }
        let mono = !PROSE.contains(&ext.as_str());
        let mut doc = Self {
            text,
            pages: Vec::new(),
            mono,
        };
        doc.paginate(&Ctx::new()?)?;
        Ok(doc)
    }

    /// Splits the laid-out lines into pages of the printable height.
    fn paginate(&mut self, ctx: &Ctx) -> Result<(), DecodeError> {
        let layout = ctx.layout(&self.text, self.mono)?;
        let mut n = 0;
        // SAFETY: the first call reports the count, the second fills it.
        let lines = unsafe {
            let _ = layout.GetLineMetrics(None, &mut n);
            let mut v = vec![DWRITE_LINE_METRICS::default(); n as usize];
            layout.GetLineMetrics(Some(&mut v), &mut n).map_err(err)?;
            v
        };
        let room = PAGE.1 as f32 - 2.0 * MARGIN_Y;
        let (mut start, mut pos, mut used) = (0usize, 0usize, 0f32);
        for l in &lines {
            if used + l.height > room && pos > start {
                self.pages.push((start, pos));
                start = pos;
                used = 0.0;
            }
            used += l.height;
            pos += l.length as usize;
        }
        self.pages
            .push((start, pos.min(self.text.len()).max(start)));
        Ok(())
    }

    pub fn pages(&self) -> u32 {
        self.pages.len() as u32
    }

    /// Renders `region` (x0, y0, x1, y1 in pixels of a `raster`-sized
    /// rendering of the whole page) into `out` pixels: black on white.
    pub fn render(
        &self,
        page: u32,
        raster: (u32, u32),
        region: [f64; 4],
        out: (u32, u32),
    ) -> Result<RawImage, DecodeError> {
        let ctx = Ctx::new()?;
        let (a, b) = self.pages.get(page as usize).copied().unwrap_or((0, 0));
        let layout = ctx.layout(&self.text[a..b], self.mono)?;
        // Output pixels per DIP, and the region's corner in DIPs.
        let base = f64::from(raster.0) / PAGE.0;
        let k = f64::from(out.0) / (region[2] - region[0]) * base;
        let (ox, oy) = (region[0] / base, region[1] / base);
        // SAFETY: COM objects of this thread; the bitmap outlives the
        // render target; the buffer is the size of the copy.
        unsafe {
            let bmp = ctx
                .wic
                .CreateBitmap(
                    out.0,
                    out.1,
                    &GUID_WICPixelFormat32bppPBGRA,
                    WICBitmapCacheOnLoad,
                )
                .map_err(err)?;
            let props = D2D1_RENDER_TARGET_PROPERTIES {
                r#type: D2D1_RENDER_TARGET_TYPE_SOFTWARE,
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                ..Default::default()
            };
            let rt = ctx
                .d2d
                .CreateWicBitmapRenderTarget(&bmp, &props)
                .map_err(err)?;
            let brush = rt
                .CreateSolidColorBrush(
                    &D2D1_COLOR_F {
                        r: 0.1,
                        g: 0.1,
                        b: 0.12,
                        a: 1.0,
                    },
                    None,
                )
                .map_err(err)?;
            rt.SetTextAntialiasMode(D2D1_TEXT_ANTIALIAS_MODE_GRAYSCALE);
            rt.BeginDraw();
            rt.Clear(Some(&D2D1_COLOR_F {
                r: 1.0,
                g: 1.0,
                b: 1.0,
                a: 1.0,
            }));
            let (k, ox, oy) = (k as f32, ox as f32, oy as f32);
            rt.SetTransform(&Matrix3x2 {
                M11: k,
                M12: 0.0,
                M21: 0.0,
                M22: k,
                M31: -ox * k,
                M32: -oy * k,
            });
            rt.DrawTextLayout(
                windows_numerics::Vector2 {
                    X: MARGIN_X,
                    Y: MARGIN_Y,
                },
                &layout,
                &brush,
                D2D1_DRAW_TEXT_OPTIONS_NONE,
            );
            rt.EndDraw(None, None).map_err(err)?;
            let stride = out.0 * 4;
            let mut px = vec![0u8; (stride * out.1) as usize];
            bmp.CopyPixels(std::ptr::null(), stride, &mut px)
                .map_err(err)?;
            // Opaque BGRA → RGBA.
            for p in px.as_chunks_mut::<4>().0 {
                p.swap(0, 2);
                p[3] = 255;
            }
            Ok(RawImage::new(out.0, out.1, Raw::Rgba8(px)))
        }
    }
}

/// Raster size of a page for a screen box (see `pdf::raster_size`).
pub fn raster_size(hint: Option<SizeHint>) -> (u32, u32) {
    let target = hint.map_or(MIN_SIDE, |h| f64::from(h.box_width.max(h.box_height)));
    let longest = PAGE.0.max(PAGE.1);
    let k = (target.max(MIN_SIDE) / longest).min(MAX_SIDE / longest);
    ((PAGE.0 * k).round() as u32, (PAGE.1 * k).round() as u32)
}

/// The first page, fitted to the screen box; with the page count.
pub fn decode(path: &Path, hint: Option<SizeHint>) -> Result<RawImage, DecodeError> {
    let doc = TextDoc::open(path)?;
    let raster = raster_size(hint);
    let region = [0.0, 0.0, f64::from(raster.0), f64::from(raster.1)];
    let mut img = doc.render(0, raster, region, raster)?;
    img.pages = doc.pages();
    Ok(img)
}

/// `true` for files shown as text documents.
pub fn is_text(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encodings() {
        let (t, e) = decode_bytes("Привет".as_bytes());
        assert_eq!(
            (String::from_utf16_lossy(&t).as_str(), e),
            ("Привет", "UTF-8")
        );
        // "Привет" in Windows-1251.
        let cp = [0xCF, 0xF0, 0xE8, 0xE2, 0xE5, 0xF2];
        let (t, e) = decode_bytes(&cp);
        assert_eq!(
            (String::from_utf16_lossy(&t).as_str(), e),
            ("Привет", "Windows-1251")
        );
        let (t, e) = decode_bytes(&[0xFF, 0xFE, b'h', 0, b'i', 0]);
        assert_eq!((String::from_utf16_lossy(&t).as_str(), e), ("hi", "UTF-16"));
        // A UTF-8 file cut in the middle of a character is still UTF-8.
        let mut cut = "abcж".as_bytes().to_vec();
        cut.pop();
        assert_eq!(decode_bytes(&cut).1, "UTF-8");
    }

    #[test]
    fn long_text_gets_pages_and_renders() {
        let dir = std::env::temp_dir().join("lightbox-text-test");
        std::fs::create_dir_all(&dir).unwrap();
        let p = dir.join("long.txt");
        let text: String = (0..300).map(|i| format!("Строка номер {i}\n")).collect();
        std::fs::write(&p, text).unwrap();
        let doc = TextDoc::open(&p).unwrap();
        assert!(doc.pages() >= 4, "{}", doc.pages());
        // Pages cover the text without gaps.
        assert_eq!(doc.pages[0].0, 0);
        for w in doc.pages.windows(2) {
            assert_eq!(w[0].1, w[1].0);
        }
        let img = decode(&p, None).unwrap();
        assert_eq!(img.pages, doc.pages());
        let Raw::Rgba8(px) = &img.data else { panic!() };
        // White page with some dark text.
        assert!(px.as_chunks::<4>().0.iter().any(|p| p[0] < 100));
        assert_eq!(&px[..4], &[255, 255, 255, 255]);
    }
}
