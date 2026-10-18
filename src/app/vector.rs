//! Vector detail for SVG: when the view magnifies the base raster, the
//! visible region (plus a margin for panning) is rendered again at screen
//! resolution on a worker thread and drawn instead of the raster. Outlines
//! therefore stay smooth at any zoom.

use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;

use anyhow::Result;

use super::{App, WM_APP_VECTOR, post};
use crate::cache;
use crate::decode::{DecodedImage, svg};
use crate::gfx::GpuImage;
use crate::orientation::Orientation;
use crate::view::{self, Quad, Rect, View};

/// Extra area rendered around the visible region, per side (fraction).
const MARGIN: f64 = 0.25;
/// Output limits: per side (texture size) and in total (render time).
const MAX_SIDE: f64 = 8192.0;
const MAX_PIXELS: f64 = 24e6;

/// What the worker should render.
struct Request {
    path: PathBuf,
    key: String,
    /// Size of the base raster the region is expressed in.
    raster: (u32, u32),
    region: [f64; 4],
    out: (u32, u32),
}

struct Done {
    key: String,
    region: [f64; 4],
    /// Output pixels per raster pixel.
    k: f64,
    image: Result<DecodedImage, String>,
}

/// A rendered region of the current drawing, on the GPU.
pub(super) struct Detail {
    key: String,
    region: [f64; 4],
    k: f64,
    tex: Rc<GpuImage>,
}

type Slot = Arc<(Mutex<Option<Request>>, Condvar)>;

/// Renders one region at a time; a new request replaces a waiting one.
pub(super) struct VectorWorker {
    slot: Slot,
    results: Receiver<Done>,
    /// Last request sent (to avoid sending the same one twice).
    last: Option<(String, [f64; 4], (u32, u32))>,
}

impl VectorWorker {
    fn new(hwnd: isize) -> Self {
        let slot: Slot = Arc::new((Mutex::new(None), Condvar::new()));
        let (tx, results) = channel();
        let worker_slot = Arc::clone(&slot);
        let spawned = thread::Builder::new()
            .name("vector".into())
            .spawn(move || worker(&worker_slot, &tx, hwnd));
        if let Err(e) = spawned {
            tracing::error!("cannot start the vector thread: {e}");
        }
        Self {
            slot,
            results,
            last: None,
        }
    }

    fn request(&mut self, r: Request) {
        let id = (r.key.clone(), r.region, r.out);
        if self.last.as_ref() == Some(&id) {
            return;
        }
        self.last = Some(id);
        let (lock, wake) = &*self.slot;
        if let Ok(mut s) = lock.lock() {
            *s = Some(r);
            wake.notify_one();
        }
    }
}

fn worker(slot: &Slot, tx: &Sender<Done>, hwnd: isize) {
    // The parsed drawing is kept between requests (zooming re-renders it).
    let mut tree: Option<(PathBuf, svg::Tree)> = None;
    loop {
        let req = {
            let (lock, wake) = &**slot;
            let Ok(mut s) = lock.lock() else {
                return;
            };
            loop {
                if let Some(r) = s.take() {
                    break r;
                }
                s = match wake.wait(s) {
                    Ok(s) => s,
                    Err(_) => return,
                };
            }
        };
        if tree.as_ref().is_none_or(|(p, _)| *p != req.path) {
            tree = std::fs::read(&req.path)
                .map_err(|e| e.to_string())
                .and_then(|d| svg::parse(&d).map_err(|e| e.to_string()))
                .map(|t| (req.path.clone(), t))
                .inspect_err(|e| tracing::warn!("vector: {e}"))
                .ok();
        }
        let image = match &tree {
            Some((_, t)) => {
                svg::render_region(t, req.raster, req.region, req.out).map_err(|e| e.to_string())
            }
            None => Err("cannot read the drawing".into()),
        };
        let done = Done {
            key: req.key,
            region: req.region,
            k: f64::from(req.out.0) / (req.region[2] - req.region[0]),
            image,
        };
        if tx.send(done).is_err() {
            return;
        }
        post(hwnd, WM_APP_VECTOR);
    }
}

/// SVG files get vector detail.
fn is_vector(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| e.eq_ignore_ascii_case("svg") || e.eq_ignore_ascii_case("svgz"))
}

/// Part of a `raster`-sized image (pixels) visible on screen, where the
/// image occupies `rect` at `k` screen pixels per raster pixel and `clip`
/// is the visible screen region.
fn visible_region(rect: Rect, clip: Rect, k: f64, raster: (f64, f64)) -> Option<[f64; 4]> {
    let x0 = ((clip.x0.max(rect.x0) - rect.x0) / k).max(0.0);
    let y0 = ((clip.y0.max(rect.y0) - rect.y0) / k).max(0.0);
    let x1 = ((clip.x1.min(rect.x1) - rect.x0) / k).min(raster.0);
    let y1 = ((clip.y1.min(rect.y1) - rect.y0) / k).min(raster.1);
    (x1 > x0 && y1 > y0).then_some([x0, y0, x1, y1])
}

fn covers(outer: [f64; 4], inner: [f64; 4]) -> bool {
    outer[0] <= inner[0] + 1e-6
        && outer[1] <= inner[1] + 1e-6
        && outer[2] >= inner[2] - 1e-6
        && outer[3] >= inner[3] - 1e-6
}

impl App {
    /// Screen geometry of the current drawing for `view`: its rectangle,
    /// the visible clip and screen pixels per raster pixel. `None` if the
    /// image is not a drawing shown upright.
    fn vector_geometry(&self, key: &str, img: &GpuImage, view: &View) -> Option<(Rect, Rect, f64)> {
        if !is_vector(&self.current) || self.orientation_of(key, img) != Orientation::IDENTITY {
            return None;
        }
        let vp = self.viewport();
        let disp = self.display_of(key, img);
        let k = view.scale * disp.0 / f64::from(img.width);
        Some((
            view.image_rect(disp, vp),
            view::view_clip(vp, view.angle),
            k,
        ))
    }

    /// Asks for a sharper rendering of the visible region when the view
    /// magnifies the base raster (and none that is good enough exists).
    pub(super) fn update_detail(&mut self) {
        let Some((key, img)) = self.shown.clone() else {
            self.detail = None;
            return;
        };
        if self.detail.as_ref().is_some_and(|d| d.key != key) {
            self.detail = None;
        }
        let view = self.zoom_target();
        let Some((rect, clip, k)) = self.vector_geometry(&key, &img, &view) else {
            return;
        };
        if k <= 1.02 {
            return; // the raster is sharp enough
        }
        let raster = (f64::from(img.width), f64::from(img.height));
        let Some(visible) = visible_region(rect, clip, k, raster) else {
            return;
        };
        if self
            .detail
            .as_ref()
            .is_some_and(|d| covers(d.region, visible) && (0.97..=1.6).contains(&(d.k / k)))
        {
            return;
        }
        let (mx, my) = (
            (visible[2] - visible[0]) * MARGIN,
            (visible[3] - visible[1]) * MARGIN,
        );
        let region = [
            (visible[0] - mx).max(0.0),
            (visible[1] - my).max(0.0),
            (visible[2] + mx).min(raster.0),
            (visible[3] + my).min(raster.1),
        ];
        let (w, h) = ((region[2] - region[0]) * k, (region[3] - region[1]) * k);
        let f = (MAX_SIDE / w)
            .min(MAX_SIDE / h)
            .min((MAX_PIXELS / (w * h)).sqrt())
            .min(1.0);
        // Whole pixels; the region is widened to match the rounded size.
        let out = ((w * f).round().max(1.0), (h * f).round().max(1.0));
        let kk = k * f;
        let region = [
            region[0],
            region[1],
            region[0] + out.0 / kk,
            region[1] + out.1 / kk,
        ];
        tracing::debug!("vector request {region:?} -> {out:?} (k {kk:.2})");
        let hwnd = self.hwnd.0 as isize;
        self.vector
            .get_or_insert_with(|| VectorWorker::new(hwnd))
            .request(Request {
                path: self.current.clone(),
                key,
                raster: (img.width, img.height),
                region,
                out: (out.0 as u32, out.1 as u32),
            });
    }

    /// A rendered region arrived: upload it and redraw.
    pub(super) fn on_vector(&mut self) -> Result<()> {
        let mut latest = None;
        while let Some(done) = self.vector.as_ref().and_then(|v| v.results.try_recv().ok()) {
            latest = Some(done);
        }
        let Some(done) = latest else {
            return Ok(());
        };
        tracing::debug!("vector done {:?} ok={}", done.region, done.image.is_ok());
        if done.key != cache::key(&self.current) {
            return Ok(());
        }
        let image = match done.image {
            Ok(i) => i,
            Err(e) => {
                tracing::warn!("vector render: {e}");
                return Ok(());
            }
        };
        self.detail = Some(Detail {
            key: done.key,
            region: done.region,
            k: done.k,
            tex: Rc::new(GpuImage::upload(&self.gpu, &image)?),
        });
        self.render()
    }

    /// The texture of the latest rendered region (drawn by `layer`).
    pub(super) fn detail_tex(&self) -> Option<Rc<GpuImage>> {
        self.detail.as_ref().map(|d| Rc::clone(&d.tex))
    }

    /// Where to draw the rendered region for `img` (key `key`) with `view`:
    /// its visible quad and image-edge flags, if it covers everything visible.
    pub(super) fn detail_part(
        &self,
        key: &str,
        img: &GpuImage,
        view: &View,
    ) -> Option<(Quad, u32)> {
        let d = self.detail.as_ref().filter(|d| d.key == key)?;
        let (rect, clip, k) = self.vector_geometry(key, img, view)?;
        if k <= 1.0 {
            return None;
        }
        let raster = (f64::from(img.width), f64::from(img.height));
        let visible = visible_region(rect, clip, k, raster)?;
        if !covers(d.region, visible) {
            return None;
        }
        let r = d.region;
        let screen = Rect {
            x0: rect.x0 + r[0] * k,
            y0: rect.y0 + r[1] * k,
            x1: rect.x0 + r[2] * k,
            y1: rect.y0 + r[3] * k,
        };
        let quad: Quad = view::visible_quad_in(screen, clip)?;
        let edges = u32::from(r[0] <= 0.0)
            | u32::from(r[1] <= 0.0) << 1
            | u32::from(r[2] >= raster.0) << 2
            | u32::from(r[3] >= raster.1) << 3;
        Some((quad, edges))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_region_maps_screen_to_raster() {
        // Raster 1000 × 500 shown at 4× with its origin at (-400, -200).
        let rect = Rect {
            x0: -400.0,
            y0: -200.0,
            x1: 3600.0,
            y1: 1800.0,
        };
        let clip = Rect {
            x0: 0.0,
            y0: 0.0,
            x1: 800.0,
            y1: 600.0,
        };
        let r = visible_region(rect, clip, 4.0, (1000.0, 500.0)).unwrap();
        assert_eq!(r, [100.0, 50.0, 300.0, 200.0]);
        assert!(covers([0.0, 0.0, 1000.0, 500.0], r));
        assert!(!covers([150.0, 0.0, 1000.0, 500.0], r));
        assert!(is_vector(Path::new("a/B.SVG")) && !is_vector(Path::new("a.png")));
    }
}
