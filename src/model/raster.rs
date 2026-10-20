//! Orbit camera and a small CPU rasteriser for model thumbnails.

use super::{Mesh, cross, normalize, sub};

/// Orbit camera around `target`: yaw / pitch in degrees, distance.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    pub distance: f32,
    pub target: [f32; 3],
}

/// Vertical field of view, degrees.
pub const FOV: f32 = 35.0;

impl Orbit {
    /// A three-quarter view showing the whole model.
    pub fn fit(mesh: &Mesh) -> Self {
        let r = mesh.radius().max(1e-6);
        Self {
            yaw: 35.0,
            pitch: 25.0,
            distance: r / (FOV.to_radians() / 2.0).sin() * 1.08,
            target: mesh.center(),
        }
    }

    pub fn eye(&self) -> [f32; 3] {
        let (y, p) = (self.yaw.to_radians(), self.pitch.to_radians());
        [
            self.target[0] + self.distance * p.cos() * y.sin(),
            self.target[1] + self.distance * p.sin(),
            self.target[2] + self.distance * p.cos() * y.cos(),
        ]
    }

    /// Camera axes (right, up, back) in world space.
    pub fn axes(&self) -> ([f32; 3], [f32; 3], [f32; 3]) {
        let z = normalize(sub(self.eye(), self.target));
        let x = normalize(cross([0.0, 1.0, 0.0], z));
        let y = cross(z, x);
        (x, y, z)
    }

    /// World → clip, row vectors (`v · M`), right-handed, depth 0..1.
    pub fn view_proj(&self, aspect: f32, radius: f32) -> [[f32; 4]; 4] {
        let (x, y, z) = self.axes();
        let e = self.eye();
        let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
        let view = [
            [x[0], y[0], z[0], 0.0],
            [x[1], y[1], z[1], 0.0],
            [x[2], y[2], z[2], 0.0],
            [-dot(x, e), -dot(y, e), -dot(z, e), 1.0],
        ];
        let near = (self.distance - radius * 1.5).max(self.distance * 0.002);
        let far = self.distance + radius * 6.0 + 1.0;
        let ys = 1.0 / (FOV.to_radians() / 2.0).tan();
        let xs = ys / aspect.max(1e-6);
        let proj = [
            [xs, 0.0, 0.0, 0.0],
            [0.0, ys, 0.0, 0.0],
            [0.0, 0.0, far / (near - far), -1.0],
            [0.0, 0.0, near * far / (near - far), 0.0],
        ];
        mul(&view, &proj)
    }

    /// The ray from the eye through pixel `p` of a `w` × `h` viewport:
    /// (origin, unit direction).
    pub fn ray(&self, p: (f32, f32), w: f32, h: f32) -> ([f32; 3], [f32; 3]) {
        let (x, y, z) = self.axes();
        let t = (FOV.to_radians() / 2.0).tan();
        let sx = (2.0 * p.0 / w.max(1.0) - 1.0) * t * w / h.max(1.0);
        let sy = (1.0 - 2.0 * p.1 / h.max(1.0)) * t;
        let d = normalize([
            x[0] * sx + y[0] * sy - z[0],
            x[1] * sx + y[1] * sy - z[1],
            x[2] * sx + y[2] * sy - z[2],
        ]);
        (self.eye(), d)
    }

    /// Turns by screen pixels (drag): left/right around the vertical axis,
    /// up/down around the horizontal one.
    pub fn rotate(&mut self, dx: f32, dy: f32) {
        self.yaw -= dx * 0.4;
        self.pitch = (self.pitch + dy * 0.4).clamp(-89.0, 89.0);
    }

    /// Moves the target in the view plane by screen pixels on a viewport of
    /// height `h`.
    pub fn pan(&mut self, dx: f32, dy: f32, h: f32) {
        let (x, y, _) = self.axes();
        let k = 2.0 * self.distance * (FOV.to_radians() / 2.0).tan() / h.max(1.0);
        for i in 0..3 {
            self.target[i] += (-x[i] * dx + y[i] * dy) * k;
        }
    }
}

pub fn mul(a: &[[f32; 4]; 4], b: &[[f32; 4]; 4]) -> [[f32; 4]; 4] {
    let mut o = [[0.0; 4]; 4];
    for r in 0..4 {
        for c in 0..4 {
            o[r][c] = (0..4).map(|k| a[r][k] * b[k][c]).sum();
        }
    }
    o
}

/// sRGB → linear for shading on the CPU.
fn lin(c: f32) -> f32 {
    c.powf(2.2)
}

fn srgb(c: f32) -> u8 {
    (c.clamp(0.0, 1.0).powf(1.0 / 2.2) * 255.0 + 0.5) as u8
}

/// Renders a `size` × `size` straight-alpha RGBA thumbnail (orthographic,
/// three-quarter view, transparent background).
pub fn thumbnail(mesh: &Mesh, size: u32) -> Vec<u8> {
    let n = size as usize;
    let mut rgba = vec![0u8; n * n * 4];
    let mut depth = vec![f32::MIN; n * n];
    let cam = Orbit::fit(mesh);
    let (ax, ay, az) = cam.axes();
    let c = mesh.center();
    let r = mesh.radius().max(1e-6);
    let scale = size as f32 / (2.0 * r * 1.05);
    let half = size as f32 / 2.0;
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let project = |p: [f32; 3]| {
        let d = sub(p, c);
        (
            half + dot(d, ax) * scale,
            half - dot(d, ay) * scale,
            dot(d, az),
        )
    };
    let light = normalize([0.4, 0.8, 0.5]);
    for (t, tri) in mesh.positions.as_chunks::<3>().0.iter().enumerate() {
        let normal = mesh.normals[t * 3];
        let p = [project(tri[0]), project(tri[1]), project(tri[2])];
        let area = (p[1].0 - p[0].0) * (p[2].1 - p[0].1) - (p[2].0 - p[0].0) * (p[1].1 - p[0].1);
        if area.abs() < 1e-9 {
            continue;
        }
        let shade_n = if dot(normal, az) < 0.0 {
            [-normal[0], -normal[1], -normal[2]]
        } else {
            normal
        };
        let l = 0.35 + 0.65 * dot(shade_n, light).max(0.0);
        let col = mesh
            .colors
            .as_ref()
            .map_or(super::DEFAULT_COLOR, |cs| cs[t * 3]);
        let px = [
            srgb(lin(col[0]) * l),
            srgb(lin(col[1]) * l),
            srgb(lin(col[2]) * l),
            255,
        ];
        let x0 = p
            .iter()
            .map(|q| q.0)
            .fold(f32::MAX, f32::min)
            .floor()
            .max(0.0) as usize;
        let x1 = (p.iter().map(|q| q.0).fold(f32::MIN, f32::max).ceil() as usize).min(n - 1);
        let y0 = p
            .iter()
            .map(|q| q.1)
            .fold(f32::MAX, f32::min)
            .floor()
            .max(0.0) as usize;
        let y1 = (p.iter().map(|q| q.1).fold(f32::MIN, f32::max).ceil() as usize).min(n - 1);
        for y in y0..=y1 {
            for x in x0..=x1 {
                let (fx, fy) = (x as f32 + 0.5, y as f32 + 0.5);
                let w = |a: (f32, f32, f32), b: (f32, f32, f32)| {
                    (b.0 - a.0) * (fy - a.1) - (b.1 - a.1) * (fx - a.0)
                };
                let (w0, w1, w2) = (w(p[1], p[2]), w(p[2], p[0]), w(p[0], p[1]));
                let inside =
                    (w0 >= 0.0 && w1 >= 0.0 && w2 >= 0.0) || (w0 <= 0.0 && w1 <= 0.0 && w2 <= 0.0);
                if !inside {
                    continue;
                }
                let z = (w0 * p[0].2 + w1 * p[1].2 + w2 * p[2].2) / (w0 + w1 + w2);
                let i = y * n + x;
                if z > depth[i] {
                    depth[i] = z;
                    rgba[i * 4..i * 4 + 4].copy_from_slice(&px);
                }
            }
        }
    }
    rgba
}

/// Line colours of toolpath kinds (sRGB), as in the 3D view.
const PATH_COLORS: [[u8; 3]; 11] = [
    [195, 195, 200],
    [255, 172, 61],
    [255, 229, 97],
    [207, 89, 79],
    [180, 89, 207],
    [247, 118, 137],
    [137, 149, 240],
    [89, 219, 124],
    [89, 211, 219],
    [124, 170, 250],
    [89, 160, 245],
];

/// A `size` × `size` straight-alpha RGBA thumbnail of a toolpath: its
/// extruded lines in the same three-quarter view as models.
pub fn path_thumbnail(mesh: &Mesh, size: u32) -> Vec<u8> {
    let n = size as usize;
    let mut rgba = vec![0u8; n * n * 4];
    let mut depth = vec![f32::MIN; n * n];
    let Some(path) = &mesh.path else {
        return rgba;
    };
    let cam = Orbit::fit(mesh);
    let (ax, ay, az) = cam.axes();
    let c = mesh.center();
    let r = mesh.radius().max(1e-6);
    let scale = size as f32 / (2.0 * r * 1.05);
    let half = size as f32 / 2.0;
    let dot = |a: [f32; 3], b: [f32; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let project = |p: [f32; 3]| {
        let d = sub(p, c);
        (
            half + dot(d, ax) * scale,
            half - dot(d, ay) * scale,
            dot(d, az),
        )
    };
    // At most ~200k segments drawn: big prints are thinned evenly.
    let step = (path.segs.len() / 200_000).max(1);
    for s in path.segs.iter().step_by(step) {
        let (a, b) = (project(s.a), project(s.b));
        let col = PATH_COLORS[(s.kind as usize).min(PATH_COLORS.len() - 1)];
        let steps = ((b.0 - a.0).abs().max((b.1 - a.1).abs()).ceil() as usize).max(1);
        for i in 0..=steps {
            let t = i as f32 / steps as f32;
            let (x, y) = (a.0 + (b.0 - a.0) * t, a.1 + (b.1 - a.1) * t);
            let z = a.2 + (b.2 - a.2) * t;
            if x < 0.0 || y < 0.0 || x >= size as f32 || y >= size as f32 {
                continue;
            }
            let k = y as usize * n + x as usize;
            if z > depth[k] {
                depth[k] = z;
                rgba[k * 4..k * 4 + 4].copy_from_slice(&[col[0], col[1], col[2], 255]);
            }
        }
    }
    rgba
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Builder;

    fn cube() -> Mesh {
        let mut b = Builder::new(false);
        let v = |x: f32, y: f32, z: f32| [x, y, z];
        let quads = [
            [v(0., 0., 1.), v(1., 0., 1.), v(1., 1., 1.), v(0., 1., 1.)],
            [v(1., 0., 0.), v(0., 0., 0.), v(0., 1., 0.), v(1., 1., 0.)],
            [v(0., 1., 1.), v(1., 1., 1.), v(1., 1., 0.), v(0., 1., 0.)],
            [v(0., 0., 0.), v(1., 0., 0.), v(1., 0., 1.), v(0., 0., 1.)],
            [v(1., 0., 1.), v(1., 0., 0.), v(1., 1., 0.), v(1., 1., 1.)],
            [v(0., 0., 0.), v(0., 0., 1.), v(0., 1., 1.), v(0., 1., 0.)],
        ];
        for q in quads {
            b.triangle([q[0], q[1], q[2]], None);
            b.triangle([q[0], q[2], q[3]], None);
        }
        b.finish().unwrap()
    }

    #[test]
    fn thumbnail_has_model_in_the_middle() {
        let px = thumbnail(&cube(), 64);
        let alpha = |x: usize, y: usize| px[(y * 64 + x) * 4 + 3];
        assert_eq!(alpha(32, 32), 255);
        assert_eq!(alpha(0, 0), 0);
    }

    #[test]
    fn camera_sees_the_target_in_the_centre() {
        let m = cube();
        let cam = Orbit::fit(&m);
        let vp = cam.view_proj(1.5, m.radius());
        let c = m.center();
        let v = [c[0], c[1], c[2], 1.0];
        let clip: Vec<f32> = (0..4)
            .map(|j| (0..4).map(|k| v[k] * vp[k][j]).sum())
            .collect();
        assert!((clip[0] / clip[3]).abs() < 1e-4 && (clip[1] / clip[3]).abs() < 1e-4);
        let z = clip[2] / clip[3];
        assert!((0.0..1.0).contains(&z), "{z}");
    }

    #[test]
    fn ray_through_a_pixel_projects_back_to_it() {
        let m = cube();
        let cam = Orbit::fit(&m);
        let (w, h) = (300.0, 200.0);
        let vp = cam.view_proj(w / h, m.radius());
        let (o, d) = cam.ray((210.0, 60.0), w, h);
        let p = [o[0] + d[0] * 2.0, o[1] + d[1] * 2.0, o[2] + d[2] * 2.0, 1.0];
        let clip: Vec<f32> = (0..4)
            .map(|j| (0..4).map(|k| p[k] * vp[k][j]).sum())
            .collect();
        let sx = (clip[0] / clip[3] + 1.0) / 2.0 * w;
        let sy = (1.0 - clip[1] / clip[3]) / 2.0 * h;
        assert!(
            (sx - 210.0).abs() < 0.01 && (sy - 60.0).abs() < 0.01,
            "{sx} {sy}"
        );
    }
}
