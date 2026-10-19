//! PDF pages, rendered by the PDF engine built into Windows
//! (Windows.Data.Pdf). The first page goes through the normal decoder;
//! other pages and sharper zoomed-in regions come from the vector worker
//! (`app/vector.rs`). Calls block, so they run on worker threads only.

use std::path::Path;

use windows::Data::Pdf::{PdfDocument, PdfPageRenderOptions};
use windows::Foundation::Rect;
use windows::Storage::StorageFile;
use windows::Storage::Streams::{DataReader, InMemoryRandomAccessStream};
use windows::UI::Color;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx};
use windows::core::HSTRING;

use super::{DecodeError, RawImage, SizeHint};

/// Longest side of a page raster.
const MAX_SIDE: f64 = 8192.0;
/// Shortest "longest side" of a page raster (small screens, no hint).
const MIN_SIDE: f64 = 1600.0;

fn err(e: windows::core::Error) -> DecodeError {
    DecodeError::Other(format!("PDF: {}", e.message()))
}

pub struct Pdf {
    doc: PdfDocument,
}

impl Pdf {
    pub fn open(path: &Path) -> Result<Self, DecodeError> {
        // SAFETY: initialises COM for this (worker) thread; a thread that
        // already has COM keeps its apartment.
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        let abs = std::path::absolute(path)?;
        let file = StorageFile::GetFileFromPathAsync(&HSTRING::from(abs.as_os_str()))
            .and_then(|op| op.join())
            .map_err(err)?;
        let doc = PdfDocument::LoadFromFileAsync(&file)
            .and_then(|op| op.join())
            .map_err(|e| {
                if e.code().0 as u32 == 0x8007_052B {
                    DecodeError::Other("PDF защищён паролем".into())
                } else {
                    err(e)
                }
            })?;
        Ok(Self { doc })
    }

    pub fn pages(&self) -> u32 {
        self.doc.PageCount().unwrap_or(0)
    }

    /// Page size in DIPs (1/96 inch).
    pub fn page_size(&self, page: u32) -> Result<(f64, f64), DecodeError> {
        let s = self.doc.GetPage(page).and_then(|p| p.Size()).map_err(err)?;
        Ok((f64::from(s.Width).max(1.0), f64::from(s.Height).max(1.0)))
    }

    /// Size of the base raster of `page` for a screen box.
    pub fn raster_size(
        &self,
        page: u32,
        hint: Option<SizeHint>,
    ) -> Result<(u32, u32), DecodeError> {
        Ok(raster_size(self.page_size(page)?, hint))
    }

    /// Renders `region` (x0, y0, x1, y1 in pixels of a `raster`-sized
    /// rendering of the whole page) into `out` pixels, on white.
    pub fn render(
        &self,
        page: u32,
        raster: (u32, u32),
        region: [f64; 4],
        out: (u32, u32),
    ) -> Result<RawImage, DecodeError> {
        let p = self.doc.GetPage(page).map_err(err)?;
        let size = p.Size().map_err(err)?;
        // Page DIPs per raster pixel.
        let k = f64::from(size.Width) / f64::from(raster.0.max(1));
        let opts = PdfPageRenderOptions::new().map_err(err)?;
        let set = || -> windows::core::Result<()> {
            opts.SetSourceRect(Rect {
                X: (region[0] * k) as f32,
                Y: (region[1] * k) as f32,
                Width: ((region[2] - region[0]) * k) as f32,
                Height: ((region[3] - region[1]) * k) as f32,
            })?;
            opts.SetDestinationWidth(out.0.max(1))?;
            opts.SetDestinationHeight(out.1.max(1))?;
            opts.SetBackgroundColor(Color {
                A: 255,
                R: 255,
                G: 255,
                B: 255,
            })
        };
        set().map_err(err)?;
        let stream = InMemoryRandomAccessStream::new().map_err(err)?;
        p.RenderWithOptionsToStreamAsync(&stream, &opts)
            .and_then(|op| op.join())
            .map_err(err)?;
        // The page arrives as a PNG.
        let read = || -> windows::core::Result<Vec<u8>> {
            let len = u32::try_from(stream.Size()?).unwrap_or(u32::MAX);
            let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
            reader.LoadAsync(len)?.join()?;
            let mut buf = vec![0u8; len as usize];
            reader.ReadBytes(&mut buf)?;
            Ok(buf)
        };
        let png = read().map_err(err)?;
        super::png::decode(&png)
    }
}

/// Raster size for a page of `page` DIPs: like SVG, at least the screen
/// box (sharp when fitted), at most `MAX_SIDE`.
pub fn raster_size(page: (f64, f64), hint: Option<SizeHint>) -> (u32, u32) {
    let target = hint.map_or(MIN_SIDE, |h| f64::from(h.box_width.max(h.box_height)));
    let longest = page.0.max(page.1);
    let k = (target.max(MIN_SIDE).max(longest) / longest).min(MAX_SIDE / longest);
    (
        (page.0 * k).round().max(1.0) as u32,
        (page.1 * k).round().max(1.0) as u32,
    )
}

/// The first page, fitted to the screen box; with the page count.
pub fn decode(path: &Path, hint: Option<SizeHint>) -> Result<RawImage, DecodeError> {
    let pdf = Pdf::open(path)?;
    let pages = pdf.pages();
    if pages == 0 {
        return Err(DecodeError::Other("в PDF нет страниц".into()));
    }
    let raster = pdf.raster_size(0, hint)?;
    let region = [0.0, 0.0, f64::from(raster.0), f64::from(raster.1)];
    let mut img = pdf.render(0, raster, region, raster)?;
    img.pages = pages;
    Ok(img)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raster_fits_the_screen_box() {
        let a4 = (794.0, 1123.0);
        let hint = SizeHint {
            box_width: 2560,
            box_height: 1440,
        };
        let (w, h) = raster_size(a4, Some(hint));
        assert_eq!(h, 2560);
        assert!((f64::from(w) / f64::from(h) - a4.0 / a4.1).abs() < 0.01);
        // Huge pages are capped.
        let (w, _) = raster_size((20000.0, 100.0), None);
        assert_eq!(w, 8192);
    }

    /// The generated two-page sample opens (run gen_testdata first).
    #[test]
    fn testdata_pdf() {
        let p = Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/sample.pdf");
        if !p.exists() {
            return;
        }
        let pdf = Pdf::open(&p).unwrap();
        assert_eq!(pdf.pages(), 2);
        let img = decode(&p, None).unwrap();
        assert_eq!(img.pages, 2);
        assert!(img.width > 100 && img.height > img.width);
    }
}
