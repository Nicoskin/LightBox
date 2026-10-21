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
use crate::gfx::{GpuMesh, ModelDraw, VSliderView};
use crate::i18n::{dec, tr};
use crate::model::raster::Orbit;
use crate::model::{self, Mesh, Part, pick};
use crate::t;
use crate::ui::objects;
use crate::ui::toolbar::Action;
use crate::view::Rect;
use windows::Win32::Graphics::Direct3D11::ID3D11Device;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::WindowsAndMessaging::{
    WM_LBUTTONDBLCLK, WM_LBUTTONDOWN, WM_LBUTTONUP, WM_MOUSEMOVE,
};

/// "Display size" a model reports (for the window size in windowed mode).
pub(super) const MODEL_DISPLAY: (f64, f64) = (1280.0, 960.0);

pub(super) struct ModelState {
    pub key: String,
    pub(super) mesh: Arc<Mesh>,
    /// What is shown: the whole mesh, one build plate, the objects not
    /// hidden.
    pub(super) view: Arc<Mesh>,
    gpu: Rc<GpuMesh>,
    cam: Orbit,
    /// G-code: the top layer shown (0-based).
    layer: usize,
    /// Slicer projects: the plate shown (`None`: all of them).
    plate: Option<usize>,
    /// Triangle edges drawn over the surface (W).
    wire: bool,
    /// Section (P): everything above this height is cut away.
    section: Option<f32>,
    /// The section bar is being dragged.
    section_drag: bool,
    /// A click on the model picks a point or a face.
    tool: Option<Tool>,
    /// Ruler points (at most two).
    marks: Vec<[f32; 3]>,
    /// Projects: objects hidden in the objects card.
    pub(super) hidden: Vec<bool>,
    /// Projects: every object in its own contrasting colour.
    pub(super) contrast: bool,
    /// Projects: a colour chosen for an object (over its own or the
    /// contrasting one) and how opaque it is.
    pub(super) paint: Vec<Option<[f32; 4]>>,
    pub(super) alpha: Vec<f32>,
    /// The objects card (O).
    pub(super) card: Option<super::objects::CardState>,
}

impl ModelState {
    /// Builds what is shown from the plate, the hidden objects and the
    /// colours. `refit`: a new view (another plate): the camera starts over.
    pub(super) fn rebuild(&mut self, device: &ID3D11Device, refit: bool) -> Result<()> {
        let all = 0..self.mesh.triangles();
        let base = self.plate.map_or(all, |i| self.mesh.plates[i].tris.clone());
        let plain = !self.contrast
            && !self.hidden.contains(&true)
            && self.paint.iter().all(Option::is_none)
            && self.alpha.iter().all(|a| *a >= 1.0);
        let mut view = if plain && self.plate.is_none() {
            None
        } else if plain || self.mesh.objects.is_empty() {
            Some(self.mesh.pick(&[Part::plain(base.clone())]))
        } else {
            let mut parts: Vec<Part> = self
                .mesh
                .objects
                .iter()
                .enumerate()
                .filter(|(i, _)| !self.hidden.get(*i).copied().unwrap_or(false))
                .filter_map(|(i, o)| {
                    let tris = o.tris.start.max(base.start)..o.tris.end.min(base.end);
                    let paint = self
                        .paint
                        .get(i)
                        .copied()
                        .flatten()
                        .or_else(|| self.contrast.then(|| objects::contrast_color(i)));
                    let alpha = self.alpha.get(i).copied().unwrap_or(1.0);
                    (tris.start < tris.end).then_some(Part { tris, paint, alpha })
                })
                .collect();
            // Opaque parts first: see-through ones are blended over them.
            parts.sort_by_key(|p| p.alpha < 1.0);
            Some(self.mesh.pick(&parts))
        };
        // Hiding objects keeps the bed and the zoom where they were.
        if let Some(v) = &mut view {
            (v.min, v.max) = self.mesh.bounds(base);
        }
        self.view = view.map_or_else(|| Arc::clone(&self.mesh), Arc::new);
        self.gpu = Rc::new(GpuMesh::upload(device, &self.view)?);
        if refit {
            self.cam = Orbit::fit(&self.view);
            self.section = None;
            self.marks.clear();
        }
        Ok(())
    }
}

/// What a click on the model does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Tool {
    Measure,
    LayFlat,
}

pub(super) type ModelRx = (String, Receiver<Result<Mesh, String>>);

/// One frame's model: the mesh (kept alive) and how to draw it.
pub(super) struct FrameModel {
    mesh: Rc<GpuMesh>,
    view_proj: [[f32; 4]; 4],
    eye: [f32; 3],
    light: [f32; 3],
    /// Toolpaths: the segments to draw.
    limit: Option<u32>,
    wire: bool,
    section: Option<f32>,
}

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
            self.notice(tr("Загрузка модели…"));
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
                let layer = mesh
                    .path
                    .as_ref()
                    .map_or(0, |p| p.layer_ends.len().saturating_sub(1));
                let objects = mesh.objects.len();
                let mesh = Arc::new(mesh);
                self.model = Some(ModelState {
                    key,
                    view: Arc::clone(&mesh),
                    mesh,
                    gpu,
                    cam,
                    layer,
                    plate: None,
                    wire: false,
                    section: None,
                    section_drag: false,
                    tool: None,
                    marks: Vec::new(),
                    hidden: vec![false; objects],
                    contrast: false,
                    paint: vec![None; objects],
                    alpha: vec![1.0; objects],
                    card: None,
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
            m.gpu = Rc::new(GpuMesh::upload(&self.gpu.device, &m.view)?);
        }
        Ok(())
    }

    /// What the GPU draws this frame (with the mesh kept alive by `Rc`).
    pub(super) fn model_draw(&self) -> Option<FrameModel> {
        let m = self.current_model()?;
        let (w, h) = self.viewport();
        let vp = m.cam.view_proj((w / h.max(1.0)) as f32, m.view.radius());
        let (x, y, z) = m.cam.axes();
        // Key light from the upper left, a little in front.
        let light = model::normalize([
            -0.45 * x[0] + 0.8 * y[0] + 0.55 * z[0],
            -0.45 * x[1] + 0.8 * y[1] + 0.55 * z[1],
            -0.45 * x[2] + 0.8 * y[2] + 0.55 * z[2],
        ]);
        let limit = m
            .mesh
            .path
            .as_ref()
            .and_then(|p| p.layer_ends.get(m.layer).copied());
        Some(FrameModel {
            mesh: Rc::clone(&m.gpu),
            view_proj: vp,
            eye: m.cam.eye(),
            light,
            limit,
            wire: m.wire,
            section: m.section,
        })
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
        let r = m.view.radius().max(1e-6);
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
        m.cam = Orbit::fit(&m.view);
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

    /// "12 345 треугольников · 120 × 80 × 35" for the caption (G-code: the
    /// layer, print time and filament).
    pub(super) fn model_details(&self) -> Option<String> {
        let m = self.current_model()?;
        if let Some(p) = &m.mesh.path {
            let mut parts = vec![t!("слой {} из {}", m.layer + 1, p.layer_ends.len())];
            parts.extend(p.time.clone());
            parts.extend(p.filament.clone());
            return Some(parts.join("  ·  "));
        }
        let s = m.view.size();
        // Back to the file's Z-up axes for the dimensions: X, Y (depth), Z.
        let text = t!(
            "{} треугольников  ·  {} × {} × {}",
            m.view.triangles(),
            dec(s[0], 1),
            dec(s[2], 1),
            dec(s[1], 1)
        );
        if m.mesh.plates.is_empty() {
            return Some(text);
        }
        let plate = m.plate.map_or_else(
            || tr("все столы").to_string(),
            |i| m.mesh.plates[i].name.clone(),
        );
        Some(format!("{plate}  ·  {text}"))
    }

    /// The model's rows of the info card: one fact per row, so that long
    /// values are not cut off.
    pub(super) fn model_rows(&self) -> Vec<(String, String)> {
        let Some(m) = self.current_model() else {
            return Vec::new();
        };
        if let Some(p) = &m.mesh.path {
            let mut parts = vec![t!("слой {} из {}", m.layer + 1, p.layer_ends.len())];
            parts.extend(p.time.clone());
            parts.extend(p.filament.clone());
            return parts
                .into_iter()
                .enumerate()
                .map(|(i, v)| (if i == 0 { tr("Модель") } else { "" }.into(), v))
                .collect();
        }
        let mut rows = Vec::new();
        if !m.mesh.plates.is_empty() {
            let plate = m.plate.map_or_else(
                || tr("все столы").to_string(),
                |i| m.mesh.plates[i].name.clone(),
            );
            rows.push((tr("Стол").into(), plate));
        }
        let s = m.view.size();
        rows.push((
            tr("Модель").into(),
            t!("{} треугольников", m.view.triangles()),
        ));
        // Back to the file's Z-up axes: X, Y (depth), Z.
        rows.push((
            tr("Размер").into(),
            t!("{} × {} × {} мм", dec(s[0], 1), dec(s[2], 1), dec(s[1], 1)),
        ));
        rows
    }
}

/// The GPU draw description for `model_draw`'s result.
pub(super) fn draw_desc(d: &FrameModel) -> ModelDraw<'_> {
    ModelDraw {
        mesh: &d.mesh,
        view_proj: d.view_proj,
        eye: d.eye,
        light: d.light,
        limit: d.limit,
        wire: d.wire,
        section: d.section,
    }
}

// G-code layers.
impl App {
    /// ↑ / ↓ (Shift: 10): the top layer shown. `false` when the current
    /// file is not a toolpath.
    pub(super) fn step_layer(&mut self, delta: i64) -> Result<bool> {
        let key = cache::key(&self.current);
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        let Some(p) = &m.mesh.path else {
            return Ok(false);
        };
        let last = p.layer_ends.len().saturating_sub(1) as i64;
        m.layer = (m.layer as i64 + delta).clamp(0, last) as usize;
        let z = p.layer_z.get(m.layer).copied().unwrap_or(0.0);
        let text = t!("Слой {} из {} · {} мм", m.layer + 1, last + 1, dec(z, 2));
        self.notice(&text);
        self.render()?;
        Ok(true)
    }

    /// ↑ / ↓ in a slicer project with several plates: all plates, then
    /// each one alone. `false` when the model has no plates.
    pub(super) fn step_plate(&mut self, delta: i64) -> Result<bool> {
        let key = cache::key(&self.current);
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        let n = m.mesh.plates.len();
        if n == 0 {
            return Ok(false);
        }
        let at = m.plate.map_or(0, |i| i as i64 + 1);
        let to = (at + delta).clamp(0, n as i64);
        if to != at {
            m.plate = (to > 0).then(|| to as usize - 1);
            m.rebuild(&self.gpu.device, true)?;
        }
        let text = match m.plate {
            Some(i) => t!("{} из {}", m.mesh.plates[i].name, n),
            None => t!("Все столы ({})", n),
        };
        self.notice(&text);
        self.render()?;
        Ok(true)
    }
}

// Model tools: wireframe, section, ruler, lay flat.
impl App {
    /// The loaded mesh of the current file (not a toolpath).
    fn mesh_model(&self) -> Option<&ModelState> {
        self.current_model().filter(|m| m.mesh.path.is_none())
    }

    /// `true` when the panel shows the model tools.
    pub(super) fn model_tools(&self) -> bool {
        self.mesh_model().is_some()
    }

    /// Whether a model tool button is on.
    pub(super) fn model_tool_on(&self, a: Action) -> bool {
        self.mesh_model().is_some_and(|m| match a {
            Action::Wireframe => m.wire,
            Action::Section => m.section.is_some(),
            Action::Measure => m.tool == Some(Tool::Measure),
            Action::LayFlat => m.tool == Some(Tool::LayFlat),
            _ => false,
        })
    }

    /// W: triangle edges over the surface.
    pub(super) fn toggle_wire(&mut self) -> Result<()> {
        let key = cache::key(&self.current);
        let Some(m) = self
            .model
            .as_mut()
            .filter(|m| m.key == key && m.mesh.path.is_none())
        else {
            return Ok(());
        };
        m.wire = !m.wire;
        let on = m.wire;
        self.notice(if on {
            tr("Каркас")
        } else {
            tr("Каркас скрыт")
        });
        self.render()
    }

    /// P: cut the model at half its height (↑ / ↓ move the cut).
    pub(super) fn toggle_section(&mut self) -> Result<()> {
        let key = cache::key(&self.current);
        let Some(m) = self
            .model
            .as_mut()
            .filter(|m| m.key == key && m.mesh.path.is_none())
        else {
            return Ok(());
        };
        m.section = match m.section {
            Some(_) => None,
            None => Some((m.view.min[1] + m.view.max[1]) / 2.0),
        };
        let text = section_text(m);
        self.notice(&text);
        self.render()
    }

    /// ↑ / ↓ (Shift: 10 steps) with a section on: moves the cut by 1 % of
    /// the height. `false` when there is no section.
    pub(super) fn step_section(&mut self, delta: i64) -> Result<bool> {
        let key = cache::key(&self.current);
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        let Some(h) = m.section else {
            return Ok(false);
        };
        let (lo, hi) = (m.view.min[1], m.view.max[1]);
        m.section = Some((h + (hi - lo) / 100.0 * delta as f32).clamp(lo, hi));
        let text = section_text(m);
        self.notice(&text);
        self.render()?;
        Ok(true)
    }

    /// D / B: what a click on the model does (the ruler, laying a face on
    /// the bed); the same key again turns it off.
    pub(super) fn toggle_model_tool(&mut self, tool: Tool) -> Result<()> {
        let key = cache::key(&self.current);
        let Some(m) = self
            .model
            .as_mut()
            .filter(|m| m.key == key && m.mesh.path.is_none())
        else {
            return Ok(());
        };
        m.tool = (m.tool != Some(tool)).then_some(tool);
        m.marks.clear();
        match m.tool {
            Some(Tool::Measure) => self.notice(tr("Линейка: щёлкните две точки на модели")),
            Some(Tool::LayFlat) => self.notice(tr("Щёлкните грань, которая ляжет на стол")),
            None => self.notice = None,
        }
        self.render()
    }

    /// Esc: turns the active model tool off. `false` if there was none.
    pub(super) fn end_model_tool(&mut self) -> Result<bool> {
        let key = cache::key(&self.current);
        let Some(m) = self
            .model
            .as_mut()
            .filter(|m| m.key == key && m.tool.is_some())
        else {
            return Ok(false);
        };
        m.tool = None;
        m.marks.clear();
        self.notice = None;
        self.render()?;
        Ok(true)
    }

    /// A click on the model with a tool on: a ruler point or the face to
    /// lay down. `false` when no tool is on (the click does what it would).
    pub(super) fn model_click(&mut self, p: (f64, f64)) -> Result<bool> {
        let (w, h) = self.viewport();
        let key = cache::key(&self.current);
        let Some(m) = self
            .model
            .as_mut()
            .filter(|m| m.key == key && m.tool.is_some())
        else {
            return Ok(false);
        };
        let (o, d) = m.cam.ray((p.0 as f32, p.1 as f32), w as f32, h as f32);
        let Some((t, tri)) = pick::ray_hit(&m.view, o, d, m.section) else {
            return Ok(true);
        };
        let text = match m.tool {
            Some(Tool::Measure) => {
                if m.marks.len() >= 2 {
                    m.marks.clear();
                }
                m.marks
                    .push([o[0] + d[0] * t, o[1] + d[1] * t, o[2] + d[2] * t]);
                match m.marks[..] {
                    [a, b] => ruler_text(a, b),
                    _ => tr("Щёлкните вторую точку").to_string(),
                }
            }
            _ => {
                // The side facing the eye is the outside, whichever way the
                // file's normal points.
                let n = m.view.normals[tri * 3];
                let n = if n[0] * d[0] + n[1] * d[1] + n[2] * d[2] > 0.0 {
                    n.map(|c| -c)
                } else {
                    n
                };
                m.view = Arc::new(pick::lay_flat(&m.view, n));
                m.gpu = Rc::new(GpuMesh::upload(&self.gpu.device, &m.view)?);
                m.cam = Orbit::fit(&m.view);
                m.tool = None;
                m.section = None;
                tr("Модель лежит выбранной гранью на столе").to_string()
            }
        };
        self.notice(&text);
        self.render()?;
        Ok(true)
    }

    /// Ruler points on the screen, in pixels.
    pub(super) fn model_marks(&self) -> Vec<(f64, f64)> {
        let Some(m) = self.mesh_model().filter(|m| !m.marks.is_empty()) else {
            return Vec::new();
        };
        let (w, h) = self.viewport();
        let vp = m.cam.view_proj((w / h.max(1.0)) as f32, m.view.radius());
        m.marks
            .iter()
            .filter_map(|p| {
                let v = [p[0], p[1], p[2], 1.0];
                let c: Vec<f32> = (0..4)
                    .map(|j| (0..4).map(|k| v[k] * vp[k][j]).sum())
                    .collect();
                (c[3] > 0.0).then(|| {
                    (
                        f64::from(c[0] / c[3] + 1.0) / 2.0 * w,
                        f64::from(1.0 - c[1] / c[3]) / 2.0 * h,
                    )
                })
            })
            .collect()
    }

    /// The ruler's length next to its middle.
    pub(super) fn ruler_label(&self) -> Option<(Rect, String)> {
        let m = self.mesh_model()?;
        let [a, b] = m.marks[..] else {
            return None;
        };
        let [p, q] = self.model_marks()[..] else {
            return None;
        };
        let s = self.ui_scale();
        let (cx, cy) = ((p.0 + q.0) / 2.0, (p.1 + q.1) / 2.0);
        Some((
            Rect {
                x0: cx - 240.0 * s,
                y0: cy + 12.0 * s,
                x1: cx + 240.0 * s,
                y1: cy + 38.0 * s,
            },
            ruler_text(a, b),
        ))
    }
}

/// "Разрез: 12,4 из 40,0 мм" (height above the model's bottom).
fn section_text(m: &ModelState) -> String {
    let (lo, hi) = (m.view.min[1], m.view.max[1]);
    match m.section {
        Some(h) => t!("Разрез: {} из {} мм", dec(h - lo, 1), dec(hi - lo, 1)),
        None => tr("Разрез выключен").to_string(),
    }
}

/// "34,12 мм  ·  X 10,00  Y 5,00  Z 32,30": the ruler's length and its
/// extent along the file's axes (Z up).
fn ruler_text(a: [f32; 3], b: [f32; 3]) -> String {
    let d = model::sub(b, a);
    let len = (d[0] * d[0] + d[1] * d[1] + d[2] * d[2]).sqrt();
    t!(
        "{} мм  ·  X {}  Y {}  Z {}",
        dec(len, 2),
        dec(d[0].abs(), 2),
        dec(d[2].abs(), 2),
        dec(d[1].abs(), 2)
    )
}

/// Section bar sizes, 96-DPI pixels.
const SECTION_W: f64 = 56.0;
const SECTION_PAD: f64 = 14.0;
const SECTION_LABEL_H: f64 = 26.0;

// The section bar: a vertical slider on the right while a cut is on.
impl App {
    /// The bar's card and its track (top = the model's top).
    fn section_bar(&self) -> Option<(Rect, Rect)> {
        self.mesh_model().filter(|m| m.section.is_some())?;
        let s = self.ui_scale();
        let (vw, vh) = self.viewport();
        let top = self.overlay_top(true) + 8.0 * s;
        let bottom = vh - self.panel_height() - 16.0 * s;
        let card = Rect {
            x0: vw - (16.0 + SECTION_W) * s,
            y0: top,
            x1: vw - 16.0 * s,
            y1: bottom.max(top + 160.0 * s),
        };
        let track = Rect {
            x0: card.x0,
            y0: card.y0 + (SECTION_PAD + SECTION_LABEL_H) * s,
            x1: card.x1,
            y1: card.y1 - SECTION_PAD * s,
        };
        Some((card, track))
    }

    pub(super) fn section_view(&self) -> Option<VSliderView> {
        let (card, track) = self.section_bar()?;
        let m = self.mesh_model()?;
        let (lo, hi) = (m.view.min[1], m.view.max[1]);
        let h = m.section?;
        Some(VSliderView {
            card,
            track,
            value: if hi > lo { (h - lo) / (hi - lo) } else { 1.0 },
            label: t!("{} мм", dec(h - lo, 1)),
            hot: m.section_drag,
        })
    }

    /// Mouse on the section bar; `true` when consumed.
    pub(super) fn section_input(&mut self, msg: u32, p: (f64, f64)) -> Result<bool> {
        let Some((card, track)) = self.section_bar() else {
            return Ok(false);
        };
        let key = cache::key(&self.current);
        let Some(m) = self.model.as_mut().filter(|m| m.key == key) else {
            return Ok(false);
        };
        let set = |m: &mut ModelState| {
            let (lo, hi) = (m.view.min[1], m.view.max[1]);
            let f = ((track.y1 - p.1) / track.height().max(1.0)).clamp(0.0, 1.0) as f32;
            m.section = Some(lo + (hi - lo) * f);
        };
        match msg {
            WM_LBUTTONDOWN | WM_LBUTTONDBLCLK if card.contains(p) => {
                m.section_drag = true;
                set(m);
                // SAFETY: own window.
                unsafe { SetCapture(self.hwnd) };
                self.render()?;
                Ok(true)
            }
            WM_MOUSEMOVE if m.section_drag => {
                set(m);
                self.render()?;
                Ok(true)
            }
            WM_LBUTTONUP if m.section_drag => {
                m.section_drag = false;
                // SAFETY: releasing our own capture.
                unsafe {
                    let _ = ReleaseCapture();
                }
                self.render()?;
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}
