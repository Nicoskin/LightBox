//! Picking on models: the triangle under the cursor (a ray from the camera
//! against every triangle), and turning a model so a face lies on the bed.

use super::{Mesh, cross, normalize, sub};

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

/// The nearest triangle the ray `origin + t · dir` hits: (t, triangle).
/// Both sides count. With `cut`, points above that height are ignored
/// (they are cut away by the section plane).
pub fn ray_hit(
    mesh: &Mesh,
    origin: [f32; 3],
    dir: [f32; 3],
    cut: Option<f32>,
) -> Option<(f32, usize)> {
    let mut best: Option<(f32, usize)> = None;
    for (i, tri) in mesh.positions.as_chunks::<3>().0.iter().enumerate() {
        // Möller–Trumbore.
        let e1 = sub(tri[1], tri[0]);
        let e2 = sub(tri[2], tri[0]);
        let p = cross(dir, e2);
        let det = dot(e1, p);
        if det.abs() < 1e-12 {
            continue;
        }
        let inv = 1.0 / det;
        let s = sub(origin, tri[0]);
        let u = dot(s, p) * inv;
        if !(0.0..=1.0).contains(&u) {
            continue;
        }
        let q = cross(s, e1);
        let v = dot(dir, q) * inv;
        if v < 0.0 || u + v > 1.0 {
            continue;
        }
        let t = dot(e2, q) * inv;
        if t <= 0.0 || best.is_some_and(|(b, _)| t >= b) {
            continue;
        }
        if cut.is_some_and(|h| origin[1] + dir[1] * t > h) {
            continue;
        }
        best = Some((t, i));
    }
    best
}

/// The model turned about its centre so that `normal` points straight down
/// (onto the bed).
pub fn lay_flat(mesh: &Mesh, normal: [f32; 3]) -> Mesh {
    let n = normalize(normal);
    let down = [0.0, -1.0, 0.0];
    let c = mesh.center();
    // Rodrigues: about the axis n × down by the angle between them; for a
    // face already facing up, half a turn about X.
    let axis = cross(n, down);
    let sin = dot(axis, axis).sqrt();
    let cos = dot(n, down);
    let (k, sin, cos) = if sin > 1e-6 {
        (normalize(axis), sin, cos)
    } else if cos > 0.0 {
        return mesh.clone();
    } else {
        ([1.0, 0.0, 0.0], 0.0, -1.0)
    };
    let turn = |v: [f32; 3]| {
        let kxv = cross(k, v);
        let kv = dot(k, v) * (1.0 - cos);
        [
            v[0] * cos + kxv[0] * sin + k[0] * kv,
            v[1] * cos + kxv[1] * sin + k[1] * kv,
            v[2] * cos + kxv[2] * sin + k[2] * kv,
        ]
    };
    let positions: Vec<[f32; 3]> = mesh
        .positions
        .iter()
        .map(|p| {
            let r = turn(sub(*p, c));
            [r[0] + c[0], r[1] + c[1], r[2] + c[2]]
        })
        .collect();
    let (mut min, mut max) = ([f32::MAX; 3], [f32::MIN; 3]);
    for p in &positions {
        for i in 0..3 {
            min[i] = min[i].min(p[i]);
            max[i] = max[i].max(p[i]);
        }
    }
    Mesh {
        normals: mesh.normals.iter().map(|v| turn(*v)).collect(),
        positions,
        colors: mesh.colors.clone(),
        min,
        max,
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Builder;

    /// A wedge: a right triangle in XY extruded along Z (Y up, file in Y-up).
    fn wedge() -> Mesh {
        let mut b = Builder::new(false);
        let (a, bb, c) = ([0.0, 0.0], [10.0, 0.0], [0.0, 10.0]);
        let at = |p: [f32; 2], z: f32| [p[0], p[1], z];
        // Ends.
        b.triangle([at(a, 0.0), at(c, 0.0), at(bb, 0.0)], None);
        b.triangle([at(a, 5.0), at(bb, 5.0), at(c, 5.0)], None);
        // Sides: bottom (y = 0), back (x = 0) and the slope.
        for (p, q) in [(a, bb), (bb, c), (c, a)] {
            b.triangle([at(p, 0.0), at(q, 0.0), at(q, 5.0)], None);
            b.triangle([at(p, 0.0), at(q, 5.0), at(p, 5.0)], None);
        }
        b.finish().unwrap()
    }

    #[test]
    fn ray_hits_the_nearest_face() {
        let m = wedge();
        // Straight down onto the slope at x = 2: y = 8 there.
        let (t, i) = ray_hit(&m, [2.0, 50.0, 2.5], [0.0, -1.0, 0.0], None).unwrap();
        assert!((t - 42.0).abs() < 1e-4, "{t}");
        assert!(i == 4 || i == 5, "{i}");
        // Cut at y = 5: the slope above is gone, the bottom is next.
        let (t, _) = ray_hit(&m, [2.0, 50.0, 2.5], [0.0, -1.0, 0.0], Some(5.0)).unwrap();
        assert!((t - 50.0).abs() < 1e-4, "{t}");
        assert!(ray_hit(&m, [20.0, 50.0, 2.5], [0.0, -1.0, 0.0], None).is_none());
    }

    #[test]
    fn slope_laid_flat_faces_down() {
        let m = wedge();
        let slope = m.normals[4 * 3];
        let flat = lay_flat(&m, slope);
        assert!((flat.normals[4 * 3][1] + 1.0).abs() < 1e-5);
        // The slope is now the lowest face: all its vertices at min y.
        for p in &flat.positions[12..18] {
            assert!((p[1] - flat.min[1]).abs() < 1e-3, "{p:?}");
        }
        // Lengths survive.
        let len = |m: &Mesh| {
            let d = sub(m.positions[13], m.positions[12]);
            dot(d, d).sqrt()
        };
        assert!((len(&m) - len(&flat)).abs() < 1e-4);
        // Facing up: half a turn; facing down already: unchanged.
        let top = lay_flat(&m, [0.0, 1.0, 0.0]);
        assert!((top.max[1] - m.max[1]).abs() < 1e-4 && top.normals[6][1] > 0.99);
        assert_eq!(lay_flat(&m, [0.0, -1.0, 0.0]).positions, m.positions);
    }
}
