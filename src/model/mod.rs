//! 3D models (STL, OBJ, 3MF): parsed into one triangle soup with flat
//! normals and optional colours, ready for the GPU and for thumbnails.

pub mod gcode;
mod obj;
pub mod pick;
pub mod raster;
mod stl;
mod threemf;

#[allow(
    unused_imports,
    reason = "the Explorer thumbnail handler (shellext/) reads no DOCX"
)]
pub(crate) use threemf::unzip;

use crate::i18n::tr;
use std::path::Path;

/// Model file extensions (lower case).
pub const EXTENSIONS: &[&str] = &["stl", "obj", "3mf", "gcode", "gco"];

pub fn is_model(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()))
}

/// Default surface colour (sRGB), a soft grey-blue.
pub const DEFAULT_COLOR: [f32; 4] = [0.62, 0.66, 0.72, 1.0];

/// Triangles in a Y-up right-handed space, three vertices each.
#[derive(Clone, Debug, Default)]
pub struct Mesh {
    pub positions: Vec<[f32; 3]>,
    /// Flat (per-triangle) normals, repeated for each vertex.
    pub normals: Vec<[f32; 3]>,
    /// sRGB colours per vertex, if the file has any.
    pub colors: Option<Vec<[f32; 4]>>,
    pub min: [f32; 3],
    pub max: [f32; 3],
    /// G-code: the toolpath (instead of triangles).
    pub path: Option<Box<gcode::Toolpath>>,
    /// Slicer projects with several build plates: their triangles.
    pub plates: Vec<Group>,
    /// 3MF with several objects: each one's triangles, in order.
    pub objects: Vec<Group>,
}

/// A run of triangles to show: its own colours or one `paint`, and how
/// opaque it is.
#[derive(Clone, Debug, PartialEq)]
pub struct Part {
    pub tris: std::ops::Range<usize>,
    pub paint: Option<[f32; 4]>,
    pub alpha: f32,
}

impl Part {
    /// As in the file.
    pub fn plain(tris: std::ops::Range<usize>) -> Self {
        Self {
            tris,
            paint: None,
            alpha: 1.0,
        }
    }
}

/// A named run of triangles: a build plate or an object of a project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Group {
    pub name: String,
    pub tris: std::ops::Range<usize>,
}

impl Mesh {
    pub fn triangles(&self) -> usize {
        self.positions.len() / 3
    }

    /// Size along X, Y, Z in file units (usually millimetres).
    pub fn size(&self) -> [f32; 3] {
        [
            self.max[0] - self.min[0],
            self.max[1] - self.min[1],
            self.max[2] - self.min[2],
        ]
    }

    pub fn center(&self) -> [f32; 3] {
        [
            (self.min[0] + self.max[0]) / 2.0,
            (self.min[1] + self.max[1]) / 2.0,
            (self.min[2] + self.max[2]) / 2.0,
        ]
    }

    /// Radius of the bounding sphere around [`Self::center`].
    pub fn radius(&self) -> f32 {
        let s = self.size();
        (s[0] * s[0] + s[1] * s[1] + s[2] * s[2]).sqrt() / 2.0
    }

    /// The triangles `tris` as a mesh of their own (one plate).
    #[cfg(test)]
    pub fn part(&self, tris: std::ops::Range<usize>) -> Mesh {
        self.pick(&[Part::plain(tris)])
    }

    /// Bounds of the triangles `tris`.
    pub fn bounds(&self, tris: std::ops::Range<usize>) -> ([f32; 3], [f32; 3]) {
        let end = (tris.end * 3).min(self.positions.len());
        let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &self.positions[(tris.start * 3).min(end)..end] {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        if min[0] > max[0] {
            return (self.min, self.max);
        }
        (min, max)
    }

    /// Runs of triangles joined into a mesh of their own, each optionally
    /// painted one colour and see-through (`alpha` < 1). Nothing picked: an
    /// empty mesh with this mesh's bounds.
    pub fn pick(&self, parts: &[Part]) -> Mesh {
        let painted = parts.iter().any(|p| p.paint.is_some() || p.alpha < 1.0);
        let mut m = Mesh::default();
        let mut colors = Vec::new();
        for part in parts {
            let end = (part.tris.end * 3).min(self.positions.len());
            let r = (part.tris.start * 3).min(end)..end;
            m.positions.extend_from_slice(&self.positions[r.clone()]);
            m.normals.extend_from_slice(&self.normals[r.clone()]);
            let start = colors.len();
            if let Some(c) = part.paint {
                colors.extend(std::iter::repeat_n(c, r.len()));
            } else if let Some(own) = &self.colors {
                colors.extend_from_slice(&own[r]);
            } else if painted {
                colors.extend(std::iter::repeat_n(DEFAULT_COLOR, r.len()));
            }
            if part.alpha < 1.0 {
                for c in &mut colors[start..] {
                    c[3] = part.alpha;
                }
            }
        }
        if painted || self.colors.is_some() {
            m.colors = Some(colors);
        }
        let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
        for p in &m.positions {
            for i in 0..3 {
                min[i] = min[i].min(p[i]);
                max[i] = max[i].max(p[i]);
            }
        }
        (m.min, m.max) = if m.positions.is_empty() {
            (self.min, self.max)
        } else {
            (min, max)
        };
        m
    }
}

/// Collects triangles; `z_up` converts from Z-up files (STL, 3MF) to Y-up.
pub(crate) struct Builder {
    z_up: bool,
    mesh: Mesh,
    colors: Vec<[f32; 4]>,
    colored: bool,
}

impl Builder {
    pub fn new(z_up: bool) -> Self {
        Self {
            z_up,
            mesh: Mesh {
                min: [f32::MAX; 3],
                max: [f32::MIN; 3],
                ..Default::default()
            },
            colors: Vec::new(),
            colored: false,
        }
    }

    /// Adds a triangle (counter-clockwise seen from outside), with an
    /// optional colour.
    pub fn triangle(&mut self, v: [[f32; 3]; 3], color: Option<[f32; 4]>) {
        if v.iter().flatten().any(|c| !c.is_finite()) {
            return;
        }
        let v = if self.z_up {
            v.map(|[x, y, z]| [x, z, -y])
        } else {
            v
        };
        let e1 = sub(v[1], v[0]);
        let e2 = sub(v[2], v[0]);
        let n = normalize(cross(e1, e2));
        for p in v {
            for ((lo, hi), c) in self.mesh.min.iter_mut().zip(&mut self.mesh.max).zip(p) {
                *lo = lo.min(c);
                *hi = hi.max(c);
            }
            self.mesh.positions.push(p);
            self.mesh.normals.push(n);
            self.colors.push(color.unwrap_or(DEFAULT_COLOR));
        }
        self.colored |= color.is_some();
    }

    /// Triangles added so far.
    pub fn count(&self) -> usize {
        self.mesh.positions.len() / 3
    }

    pub fn set_plates(&mut self, plates: Vec<Group>) {
        self.mesh.plates = plates;
    }

    pub fn set_objects(&mut self, objects: Vec<Group>) {
        self.mesh.objects = objects;
    }

    pub fn finish(mut self) -> Result<Mesh, String> {
        if self.mesh.positions.is_empty() {
            return Err(tr("в модели нет треугольников").into());
        }
        if self.colored {
            self.mesh.colors = Some(self.colors);
        }
        Ok(self.mesh)
    }
}

pub(crate) fn sub(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

pub(crate) fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

pub(crate) fn normalize(v: [f32; 3]) -> [f32; 3] {
    let l = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if l > 0.0 {
        [v[0] / l, v[1] / l, v[2] / l]
    } else {
        [0.0, 1.0, 0.0]
    }
}

/// Reads a model file.
pub fn load(path: &Path) -> Result<Mesh, String> {
    let data = std::fs::read(path).map_err(|e| e.to_string())?;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if is_gcode_3mf(path) {
        return gcode::parse(&data);
    }
    parse(&data, &ext, path.parent())
}

/// Parses a model file's bytes; `ext` is its lower-case extension, `dir`
/// the folder an OBJ's material library is looked up in.
pub fn parse(data: &[u8], ext: &str, dir: Option<&Path>) -> Result<Mesh, String> {
    match ext {
        "stl" => stl::parse(data),
        "obj" => obj::parse(data, dir),
        "3mf" => threemf::parse(data),
        "gcode" | "gco" => gcode::parse(data),
        _ => Err(tr("неизвестный формат модели").into()),
    }
}

/// The preview image a slicer saved in a 3MF package (PNG or JPEG bytes).
#[allow(
    dead_code,
    reason = "used by the Explorer thumbnail handler (shellext/)"
)]
pub fn embedded_thumbnail(data: &[u8]) -> Option<Vec<u8>> {
    threemf::thumbnail(data)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn z_up_becomes_y_up() {
        let mut b = Builder::new(true);
        b.triangle([[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]], None);
        let m = b.finish().unwrap();
        // The triangle lay flat (normal +Z); now it faces +Y.
        assert_eq!(m.normals[0], [0.0, 1.0, 0.0]);
        assert_eq!(m.triangles(), 1);
        assert!(m.colors.is_none());
        assert!(Builder::new(false).finish().is_err());
    }
}

#[cfg(test)]
mod samples {
    /// The generated samples load (run gen_testdata first).
    #[test]
    fn testdata_models() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata");
        for (name, tris) in [("torus.stl", 4096), ("cube.obj", 12), ("boxes.3mf", 24)] {
            let p = dir.join(name);
            if !p.exists() {
                continue;
            }
            let m = super::load(&p).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(m.triangles(), tris, "{name}");
            if name != "torus.stl" {
                assert!(m.colors.is_some(), "{name}");
            }
        }
    }
}

/// Bambu Studio's printable project (`name.gcode.3mf`).
pub fn is_gcode_3mf(path: &Path) -> bool {
    path.file_name()
        .and_then(|n| n.to_str())
        .is_some_and(|n| n.to_ascii_lowercase().ends_with(".gcode.3mf"))
}
