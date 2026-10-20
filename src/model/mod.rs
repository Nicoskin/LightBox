//! 3D models (STL, OBJ, 3MF): parsed into one triangle soup with flat
//! normals and optional colours, ready for the GPU and for thumbnails.

pub mod gcode;
mod obj;
pub mod raster;
mod stl;
mod threemf;

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

    pub fn finish(mut self) -> Result<Mesh, String> {
        if self.mesh.positions.is_empty() {
            return Err("в модели нет треугольников".into());
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
    match ext.as_str() {
        "stl" => stl::parse(&data),
        "obj" => obj::parse(&data, path.parent()),
        "3mf" if is_gcode_3mf(path) => gcode::parse(&data),
        "3mf" => threemf::parse(&data),
        "gcode" | "gco" => gcode::parse(&data),
        _ => Err("неизвестный формат модели".into()),
    }
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
