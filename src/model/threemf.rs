//! 3MF: a ZIP package with the model in XML (Z-up, units in the file).
//! Objects may be meshes or assemblies of other objects (components),
//! each placed by a transform; the build lists what is printed.

use std::collections::HashMap;

use roxmltree::{Document, Node};

use super::{Builder, Mesh};

/// 3 × 4 affine transform, 3MF order: m00 m01 m02 m10 … m32 (row vectors).
type Transform = [f32; 12];
const IDENTITY: Transform = [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

fn apply(t: &Transform, p: [f32; 3]) -> [f32; 3] {
    [
        p[0] * t[0] + p[1] * t[3] + p[2] * t[6] + t[9],
        p[0] * t[1] + p[1] * t[4] + p[2] * t[7] + t[10],
        p[0] * t[2] + p[1] * t[5] + p[2] * t[8] + t[11],
    ]
}

/// `a` then `b`.
fn compose(a: &Transform, b: &Transform) -> Transform {
    let mut out = [0.0; 12];
    for r in 0..4 {
        let row = if r < 3 {
            [a[r * 3], a[r * 3 + 1], a[r * 3 + 2], 0.0]
        } else {
            [a[9], a[10], a[11], 1.0]
        };
        for c in 0..3 {
            out[r * 3 + c] =
                row[0] * b[c] + row[1] * b[3 + c] + row[2] * b[6 + c] + row[3] * b[9 + c];
        }
    }
    out
}

fn parse_transform(s: Option<&str>) -> Transform {
    let Some(s) = s else {
        return IDENTITY;
    };
    let v: Vec<f32> = s
        .split_whitespace()
        .filter_map(|x| x.parse().ok())
        .collect();
    v.try_into().unwrap_or(IDENTITY)
}

/// "#RRGGBB" / "#RRGGBBAA" → sRGB.
fn parse_color(s: &str) -> Option<[f32; 4]> {
    let h = s.strip_prefix('#')?;
    let byte = |i: usize| u8::from_str_radix(h.get(i..i + 2)?, 16).ok();
    let a = if h.len() >= 8 { byte(6)? } else { 255 };
    Some([byte(0)?, byte(2)?, byte(4)?, a].map(|v| f32::from(v) / 255.0))
}

pub fn parse(data: &[u8]) -> Result<Mesh, String> {
    let files = unzip(data)?;
    // The model part is named in _rels/.rels; "3D/3dmodel.model" by default.
    let model_name = files
        .get("_rels/.rels")
        .and_then(|r| {
            let text = String::from_utf8_lossy(r).into_owned();
            let doc = Document::parse(&text).ok()?;
            doc.descendants()
                .find(|n| n.attribute("Type").is_some_and(|t| t.ends_with("/3dmodel")))
                .and_then(|n| n.attribute("Target"))
                .map(|t| t.trim_start_matches('/').to_string())
        })
        .unwrap_or_else(|| "3D/3dmodel.model".into());
    let xml = files
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(&model_name))
        .map(|(_, v)| v)
        .ok_or("3MF: нет модели в пакете")?;
    let text = String::from_utf8_lossy(xml);
    let doc = Document::parse(&text).map_err(|e| format!("3MF: {e}"))?;
    let root = doc.root_element();

    // Colours: base materials (by group id, index) and colour groups.
    let mut colors: HashMap<(String, usize), [f32; 4]> = HashMap::new();
    for g in root.descendants() {
        let name = g.tag_name().name();
        if name != "basematerials" && name != "colorgroup" {
            continue;
        }
        let Some(id) = g.attribute("id") else {
            continue;
        };
        for (i, item) in g.children().filter(Node::is_element).enumerate() {
            let c = item.attribute("displaycolor").or(item.attribute("color"));
            if let Some(c) = c.and_then(parse_color) {
                colors.insert((id.to_string(), i), c);
            }
        }
    }
    let objects: HashMap<&str, Node> = root
        .descendants()
        .filter(|n| n.tag_name().name() == "object")
        .filter_map(|n| Some((n.attribute("id")?, n)))
        .collect();

    let mut b = Builder::new(true);
    let build = root.children().find(|n| n.tag_name().name() == "build");
    let items: Vec<(String, Transform)> = match build {
        Some(build) => build
            .children()
            .filter(|n| n.tag_name().name() == "item")
            .filter_map(|n| {
                Some((
                    n.attribute("objectid")?.to_string(),
                    parse_transform(n.attribute("transform")),
                ))
            })
            .collect(),
        None => objects
            .keys()
            .map(|k| ((*k).to_string(), IDENTITY))
            .collect(),
    };
    for (id, t) in items {
        add_object(&objects, &colors, &id, &t, &mut b, 0);
    }
    b.finish()
}

fn add_object(
    objects: &HashMap<&str, Node>,
    colors: &HashMap<(String, usize), [f32; 4]>,
    id: &str,
    t: &Transform,
    b: &mut Builder,
    depth: u32,
) {
    let Some(obj) = objects.get(id) else {
        return;
    };
    if depth > 16 {
        return; // cyclic components
    }
    let pid = obj.attribute("pid").map(str::to_string);
    let pindex: usize = obj
        .attribute("pindex")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    for child in obj.children().filter(Node::is_element) {
        match child.tag_name().name() {
            "mesh" => {
                let verts: Vec<[f32; 3]> = child
                    .descendants()
                    .filter(|n| n.tag_name().name() == "vertex")
                    .map(|n| {
                        let c = |k| n.attribute(k).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                        apply(t, [c("x"), c("y"), c("z")])
                    })
                    .collect();
                for tri in child
                    .descendants()
                    .filter(|n| n.tag_name().name() == "triangle")
                {
                    let i = |k| tri.attribute(k).and_then(|s| s.parse::<usize>().ok());
                    let (Some(a), Some(b2), Some(c)) = (i("v1"), i("v2"), i("v3")) else {
                        continue;
                    };
                    if a.max(b2).max(c) >= verts.len() {
                        continue;
                    }
                    let tri_pid = tri.attribute("pid").map(str::to_string).or(pid.clone());
                    let p1 = i("p1").unwrap_or(pindex);
                    let color = tri_pid.and_then(|g| colors.get(&(g, p1)).copied());
                    b.triangle([verts[a], verts[b2], verts[c]], color);
                }
            }
            "components" => {
                for comp in child
                    .children()
                    .filter(|n| n.tag_name().name() == "component")
                {
                    if let Some(oid) = comp.attribute("objectid") {
                        let ct = compose(&parse_transform(comp.attribute("transform")), t);
                        add_object(objects, colors, oid, &ct, b, depth + 1);
                    }
                }
            }
            _ => {}
        }
    }
}

/// The files of a ZIP archive (stored or deflated), by name.
pub(crate) fn unzip(data: &[u8]) -> Result<HashMap<String, Vec<u8>>, String> {
    let u16le = |o: usize| data.get(o..o + 2).map(|b| u16::from_le_bytes([b[0], b[1]]));
    let u32le = |o: usize| {
        data.get(o..o + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
    };
    // End of central directory: the last "PK\x05\x06".
    let eocd = (0..data.len().saturating_sub(21))
        .rev()
        .take(70_000)
        .find(|&i| data[i..].starts_with(b"PK\x05\x06"))
        .ok_or("3MF: не ZIP-архив")?;
    let count = u16le(eocd + 10).ok_or("ZIP")? as usize;
    let mut p = u32le(eocd + 16).ok_or("ZIP")? as usize;
    let mut out = HashMap::new();
    for _ in 0..count {
        if u32le(p) != Some(0x0201_4b50) {
            break;
        }
        let method = u16le(p + 10).unwrap_or(0);
        let csize = u32le(p + 20).unwrap_or(0) as usize;
        let name_len = u16le(p + 28).unwrap_or(0) as usize;
        let extra = u16le(p + 30).unwrap_or(0) as usize;
        let comment = u16le(p + 32).unwrap_or(0) as usize;
        let local = u32le(p + 42).unwrap_or(0) as usize;
        let name = String::from_utf8_lossy(data.get(p + 46..p + 46 + name_len).unwrap_or(&[]))
            .into_owned();
        p += 46 + name_len + extra + comment;
        // Local header: its own name / extra lengths precede the data.
        let (Some(ln), Some(le)) = (u16le(local + 26), u16le(local + 28)) else {
            continue;
        };
        let start = local + 30 + ln as usize + le as usize;
        let Some(raw) = data.get(start..start + csize) else {
            continue;
        };
        let bytes = match method {
            0 => raw.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec(raw).map_err(|e| format!("ZIP: {e:?}"))?,
            _ => continue,
        };
        out.insert(name, bytes);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored (uncompressed) ZIP with the given files.
    pub(crate) fn zip(files: &[(&str, &[u8])]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut central = Vec::new();
        for (name, data) in files {
            let off = out.len() as u32;
            let crc = crc32fast::hash(data);
            let hdr = |sig: u32, central: bool| {
                let mut h = sig.to_le_bytes().to_vec();
                if central {
                    h.extend_from_slice(&20u16.to_le_bytes());
                }
                h.extend_from_slice(&20u16.to_le_bytes()); // version
                h.extend_from_slice(&0u16.to_le_bytes()); // flags
                h.extend_from_slice(&0u16.to_le_bytes()); // stored
                h.extend_from_slice(&0u32.to_le_bytes()); // time / date
                h.extend_from_slice(&crc.to_le_bytes());
                h.extend_from_slice(&(data.len() as u32).to_le_bytes());
                h.extend_from_slice(&(data.len() as u32).to_le_bytes());
                h.extend_from_slice(&(name.len() as u16).to_le_bytes());
                h.extend_from_slice(&0u16.to_le_bytes()); // extra
                if central {
                    h.extend_from_slice(&[0; 6]); // comment, disk, internal attrs
                    h.extend_from_slice(&0u32.to_le_bytes()); // ext attrs
                    h.extend_from_slice(&off.to_le_bytes());
                }
                h.extend_from_slice(name.as_bytes());
                h
            };
            out.extend(hdr(0x0403_4b50, false));
            out.extend_from_slice(data);
            central.extend(hdr(0x0201_4b50, true));
        }
        let cd = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(b"PK\x05\x06\0\0\0\0");
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&cd.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn components_transforms_and_colours() {
        let model = br##"<?xml version="1.0"?>
<model unit="millimeter" xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02">
 <resources>
  <basematerials id="5"><base name="red" displaycolor="#FF0000"/></basematerials>
  <object id="1" type="model" pid="5" pindex="0"><mesh>
   <vertices><vertex x="0" y="0" z="0"/><vertex x="10" y="0" z="0"/><vertex x="0" y="10" z="0"/></vertices>
   <triangles><triangle v1="0" v2="1" v3="2"/></triangles>
  </mesh></object>
  <object id="2" type="model"><components>
   <component objectid="1"/>
   <component objectid="1" transform="1 0 0 0 1 0 0 0 1 100 0 0"/>
  </components></object>
 </resources>
 <build><item objectid="2" transform="1 0 0 0 1 0 0 0 1 0 0 5"/></build>
</model>"##;
        let data = zip(&[("3D/3dmodel.model", model)]);
        let m = parse(&data).unwrap();
        assert_eq!(m.triangles(), 2);
        assert_eq!(m.size()[0], 110.0); // two copies, 100 mm apart
        assert_eq!(m.min[1], 5.0); // z = 5 → y after the Z-up conversion
        assert_eq!(m.colors.unwrap()[0], [1.0, 0.0, 0.0, 1.0]);
    }
}
