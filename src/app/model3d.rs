//! 3D models in the viewer: loaded in the background, shown with an orbit
//! camera (drag turns, Shift / Ctrl + drag moves, wheel zooms, 0 or double
//! click resets) and browsed like photos.

use std::rc::Rc;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::thread;

use anyhow::Result;

use super::{App, WM_APP_MODEL, post};
use crate::cache;
use crate::gfx::{GpuMesh, ModelDraw};
use crate::model::raster::Orbit;
use crate::model::{self, Mesh};

/// "Display size" a model reports (for the window size in windowed mode).
pub(super) const MODEL_DISPLAY: (f64, f64) = (1280.0, 960.0);

pub(super) struct ModelState {
    pub key: String,
    mesh: Arc<Mesh>,
    gpu: Rc<GpuMesh>,
    cam: Orbit,
}

pub(super) type ModelRx = (String, Receiver<Result<Mesh, String>>);

/// One frame's model: the mesh (kept alive), view-projection, eye, light.
pub(super) type FrameModel = (Rc<GpuMesh>, [[f32; 4]; 4], [f32; 3], [f32; 3]);

impl App {
    /// `true` while the current file is a model (shown or loading).
    pub(super) fn model_mode(&self) -> bool {
        model::is_model(&self.current)
    }

    /// The loaded model of the current file.
    fn current_model(&self) -> Option<&ModelState> {
        let key = cache::key(&self.current);
        self.model.as_ref().filter(|m| m.key == key)
    }

    /// Shows the current file as a model, loading it first if needed.
    /// Returns `true` if a redraw is needed.
    pub(super) fn show_model(&mut self) -> Result<bool> {
        let key = cache::key(&self.current);
        self.shown = None;
        self.stop_animation();
        if self.current_model().is_some() {
            return Ok(true);
        }
        self.model = None;
        if self.model_rx.as_ref().is_some_and(|(k, _)| *k == key) {
            return Ok(true);
        }
        let (tx, rx) = channel();
        let path = self.current.clone();
        let hwnd = self.hwnd.0 as isize;
        let spawned = thread::Builder::new().name("model".into()).spawn(move || {
            if tx.send(model::load(&path)).is_ok() {
                post(hwnd, WM_APP_MODEL);
            }
        });
        if spawned.is_ok() {
            self.model_rx = Some((key, rx));
            self.notice("Загрузка модели…");
        }
        Ok(true)
    }

    pub(super) fn on_model(&mut self) -> Result<()> {
        let Some((key, rx)) = &self.model_rx else {
            return Ok(());
        };
        let Ok(result) = rx.try_recv() else {
            return Ok(());
        };
        let key = key.clone();
        self.model_rx = None;
        if key != cache::key(&self.current) {
            return Ok(());
        }
        match result {
            Ok(mesh) => {
                let gpu = Rc::new(GpuMesh::upload(&self.gpu.device, &mesh)?);
                let cam = Orbit::fit(&mesh);
                self.model = Some(ModelState {
                    key,
                    mesh: Arc::new(mesh),
                    gpu,
                    cam,
                });
                self.error = None;
                self.notice = None;
                self.badge = false;
            }
            Err(e) => self.error = Some(e),
        }
        if self.pending_show {
            return self.reveal();
        }
        self.render()
    }

    /// After a GPU switch: upload the mesh to the new device.
    pub(super) fn reupload_model(&mut self) -> Result<()> {
        if let Some(m) = &mut self.model {
            m.gpu = Rc::new(GpuMesh::upload(&self.gpu.device, &m.mesh)?);
        }
        Ok(())
    }

    /// What the GPU draws this frame (with the mesh kept alive by `Rc`).
    pub(super) fn model_draw(&self) -> Option<FrameModel> {
        let m = self.current_model()?;
        let (w, h) = self.viewport();
        let vp = m.cam.view_proj((w / h.max(1.0)) as f32, m.mesh.radius());
        let (x, y, z) = m.cam.axes();
        // Key light from the upper left, a little in front.
        let light = model::normalize([
            -0.45 * x[0] + 0.8 * y[0] + 0.55 * z[0],
            -0.45 * x[1] + 0.8 * y[1] + 0.55 * z[1],
            -0.45 * x[2] + 0.8 * y[2] + 0.55 * z[2],
        ]);
        Some((Rc::clone(&m.gpu), vp, m.cam.eye(), light))
    }

    /// Mouse drag in the picture: turn, or move with Shift / Ctrl.
    pub(super) fn drag_model(&mut self, dx: f64, dy: f64, pan: bool) -> Result<bool> {
        let h = self.viewport().1 as f32;
        let key = cache::key(&self.current);
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        if pan {
            m.cam.pan(dx as f32, dy as f32, h);
        } else {
            m.cam.rotate(dx as f32, dy as f32);
        }
        self.render()?;
        Ok(true)
    }

    /// Wheel / + / −: closer or farther (`factor` > 1 is closer).
    pub(super) fn zoom_model(&mut self, factor: f64) -> Result<bool> {
        let key = cache::key(&self.current);
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        let r = m.mesh.radius().max(1e-6);
        m.cam.distance = (m.cam.distance / factor as f32).clamp(r * 0.05, r * 50.0);
        self.render()?;
        Ok(true)
    }

    /// 0 / Fit / double click: the starting view.
    pub(super) fn reset_model(&mut self) -> Result<bool> {
        let key = cache::key(&self.current);
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        m.cam = Orbit::fit(&m.mesh);
        self.render()?;
        Ok(true)
    }

    /// R / L: turn the model by 45°.
    pub(super) fn turn_model(&mut self, degrees: f32) -> Result<bool> {
        let key = cache::key(&self.current);
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        m.cam.yaw -= degrees;
        self.render()?;
        Ok(true)
    }

    /// "12 345 треугольников · 120 × 80 × 35" for the caption.
    pub(super) fn model_details(&self) -> Option<String> {
        let m = self.current_model()?;
        let s = m.mesh.size();
        // Back to the file's Z-up axes for the dimensions: X, Y (depth), Z.
        Some(format!(
            "{} треугольников  ·  {:.1} × {:.1} × {:.1}",
            m.mesh.triangles(),
            s[0],
            s[2],
            s[1]
        ))
    }
}

/// The GPU draw description for `model_draw`'s result.
pub(super) fn draw_desc(d: &FrameModel) -> ModelDraw<'_> {
    ModelDraw {
        mesh: &d.0,
        view_proj: d.1,
        eye: d.2,
        light: d.3,
    }
}
