//! Wavefront OBJ (Y-up) with MTL diffuse colours and the common
//! "v x y z r g b" vertex-colour extension.

use std::collections::HashMap;
use std::path::Path;

use super::{Builder, Mesh};

pub fn parse(data: &[u8], dir: Option<&Path>) -> Result<Mesh, String> {
    let text = String::from_utf8_lossy(data);
    let mut verts: Vec<[f32; 3]> = Vec::new();
    let mut vcols: Vec<Option<[f32; 4]>> = Vec::new();
    let mut materials: HashMap<String, [f32; 4]> = HashMap::new();
    let mut color: Option<[f32; 4]> = None;
    let mut b = Builder::new(false);
    for line in text.lines() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next() {
            Some("v") => {
                let n: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if n.len() >= 3 {
                    verts.push([n[0], n[1], n[2]]);
                    vcols.push((n.len() >= 6).then(|| [n[3], n[4], n[5], 1.0]));
                }
            }
            Some("f") => {
                let idx: Vec<usize> = it
                    .filter_map(|t| {
                        let i: i64 = t.split('/').next()?.parse().ok()?;
                        // 1-based; negative counts back from the end.
                        let k = if i < 0 { verts.len() as i64 + i } else { i - 1 };
                        (0..verts.len() as i64).contains(&k).then_some(k as usize)
                    })
                    .collect();
                for k in 1..idx.len().saturating_sub(1) {
                    let tri = [idx[0], idx[k], idx[k + 1]];
                    let c = color.or(vcols[tri[0]]);
                    b.triangle(tri.map(|i| verts[i]), c);
                }
            }
            Some("mtllib") => {
                if let Some(dir) = dir {
                    let name = line["mtllib".len()..].trim();
                    if let Ok(mtl) = std::fs::read(dir.join(name)) {
                        materials.extend(parse_mtl(&mtl));
                    }
                }
            }
            Some("usemtl") => {
                color = materials.get(line["usemtl".len()..].trim()).copied();
            }
            _ => {}
        }
    }
    b.finish()
}

/// `newmtl name` … `Kd r g b` (diffuse colour) … `d` / `Tr` (opacity).
fn parse_mtl(data: &[u8]) -> HashMap<String, [f32; 4]> {
    let text = String::from_utf8_lossy(data);
    let mut out = HashMap::new();
    let mut name = None::<String>;
    for line in text.lines() {
        let line = line.trim();
        let mut it = line.split_whitespace();
        match it.next() {
            Some("newmtl") => name = Some(line["newmtl".len()..].trim().to_string()),
            Some("Kd") => {
                let n: Vec<f32> = it.filter_map(|s| s.parse().ok()).collect();
                if let (Some(nm), [r, g, b2]) = (&name, &n[..]) {
                    out.insert(nm.clone(), [*r, *g, *b2, 1.0]);
                }
            }
            _ => {}
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quads_materials_and_negative_indices() {
        let dir = std::env::temp_dir().join(format!("lb-obj-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("m.mtl"), "newmtl red\nKd 1 0 0\n").unwrap();
        let obj = b"mtllib m.mtl\nv 0 0 0\nv 1 0 0\nv 1 1 0\nv 0 1 0\nusemtl red\nf 1 2 3 4\nf -4 -3 -2\n";
        let m = parse(obj, Some(&dir)).unwrap();
        assert_eq!(m.triangles(), 3); // quad → 2, plus one
        assert_eq!(m.colors.as_ref().unwrap()[0], [1.0, 0.0, 0.0, 1.0]);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
